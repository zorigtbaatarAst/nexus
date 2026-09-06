#!/usr/bin/env python3
"""Aggregate a Tier-2 sweep into cost-per-success, with intervals that state their own weakness.

Medians and IQR, never means: token distributions have long right tails, and one run that
thrashes for 400,000 tokens moves a mean without telling you anything about typical behaviour.
Comparisons are paired on the task, because tasks differ far more than arms do — everything
that compares A1 against a baseline resamples per-task deltas, never pooled per-run values.

Three things this file has to get right that are easy to get wrong without the table looking
any different:

  * Cache reads. `usage.json` reports `input_tokens`, `output_tokens`, `cache_read_tokens` and
    `cache_creation_tokens` as four separate fields, on purpose — a cache read costs a fraction
    of a fresh input token, and A1 injects a stable, cacheable prefix every turn. Silently
    folding cache reads into "input tokens" (or silently dropping them because they are "just"
    cache) hides exactly the arm-dependent quantity the spec calls out by name. This script
    keeps all four visible: `total_tokens` is the full sum (nothing is invisibly free), and
    cache-read volume is reported as its own median so a reader can see how much of an arm's
    total is cheap re-reads rather than fresh spend.

  * Runs that did not happen. `claude_exit` 124 (timeout), 137 (OOM-killed) or -1 (the
    container never got far enough to record a status) all come with all-zero token counts —
    and so does any other nonzero exit that still spent nothing ("anything else a crash", per
    run.sh's own comment). Taking a median over those zeros drags every arm's number down by
    however many of these it has, which is not a fact about the arm — it is a fact about the
    harness that day. These are dropped before any statistic below is computed, and the count
    is reported per arm so a systematic pattern (one arm always timing out) stays visible
    instead of vanishing into a lower median.

  * Runs whose grade is not a clean measurement. `grade.json`'s `adjudicate` array (see
    `scripts/eval/grade.sh`) flags a run where L0/L1/L2 answer a question other than "did the
    agent's fix work" — a red baseline that makes L1 unattributable, a hidden test that failed
    to compile rather than to pass, a build failure grade.sh could not explain (or, for
    `collateral-unknown-build-failed` specifically, one it explained perfectly well: the agent's
    own diff doesn't compile, a trustworthy False that just leaves L2 unmeasured). These runs
    did spend real tokens, so they are not "infra" in the sense above. Charging that spend
    against zero credit is not free of judgment either — a `hidden-test-compile-error` is a
    legitimate fix the grader can't score — so this script reports both bounds rather than
    picking one: `cps` counts the spend against clean passes, `cps_clean_only` excludes flagged
    runs' spend entirely, and both `pass_rate` (flagged excluded) and
    `pass_rate_with_flagged_as_fail` (flagged counted as failures) are reported side by side.
    See ADJUDICATE_MEANING below for what each flag means.
"""
import json
import math
import pathlib
import random
import re
import statistics
import sys

# One record per hook firing in `injected.log`, written by scripts/eval/nexus-hook.sh:
#
#     === SessionStart injected=259 bytes
#     === UserPromptSubmit prompt=93 injected=0 bytes
#
# The package itself follows on the next lines, so the label must be anchored at the start of
# the line. Everything that is not SessionStart is a per-prompt package: an arm's task package
# is the thing A1-vs-A5 is a contrast between, and a hook renamed one day must keep being
# counted rather than silently reading as "injected nothing".
INJECTED_RE = re.compile(r"^=== (\S+)[^\n]*?\binjected=(\d+) bytes\s*$")

# 124 = SIGTERM-then-timeout, 137 = SIGKILL (OOM), -1 = run.sh never wrote a status file at all
# (the container did not even get that far). All three come with all-zero usage.json fields.
INFRA_EXIT_CODES = {-1, 124, 137}

# A grade.json missing any of these is not "a run that failed" — it is a field grade.sh no
# longer writes, and every .get() downstream would default it falsy, silently converting a
# whole sweep into failures across every arm the moment grade.sh's schema drifts. Checked in
# load(), which fails loudly and names the path rather than letting that happen quietly.
REQUIRED_GRADE_KEYS = {"passed", "L0_build", "L1_hidden", "L2_collateral", "adjudicate"}

# Five real tasks total. A single surviving task after the others are skipped for undefined CPS
# has a bootstrap CI that is a single point by construction (every resample draws that one
# value) — trivially "excludes zero" and proves nothing about the arm. T4 requires at least this
# many surviving tasks before "meets" can be True at all.
MIN_T4_TASKS = 3

# What each adjudicate flag means, so the report can explain itself rather than just listing
# strings. Kept in sync with scripts/eval/grade.sh, which is the only writer of these values.
ADJUDICATE_MEANING = {
    "baseline-failure-unrecognised": "baseline build failed for a reason grade.sh could not "
        "attribute to a compiler or a test runner — an infrastructure failure, not a verdict",
    "graded-failure-unrecognised": "graded build failed for a reason grade.sh could not "
        "attribute to a compiler or a test runner — an infrastructure failure, not a verdict",
    "l1-not-isolated": "the baseline was already red, so a red graded run cannot be attributed "
        "to the hidden tests alone",
    "hidden-test-compile-error": "the baseline compiled and the graded tree did not — a "
        "legitimate fix that broke the hidden test's own compilation",
    "collateral-unknown-build-failed": "the baseline (start commit + the agent's own diff) "
        "failed a *recognised* compiler check — L0 and L2 are both False, and L0 is a "
        "trustworthy False (the agent's code does not compile). L2 specifically never got to "
        "run any tests, so it is not a measurement, but `passed` is correctly False either way",
    "diff-did-not-apply": "graded against the unpatched tree; L1 fails by construction and "
        "says nothing about the diff the agent actually produced",
}


# ---------------------------------------------------------------------------
# Pure statistics — no knowledge of runs, arms or tasks below this line.
# ---------------------------------------------------------------------------


def median(xs):
    return statistics.median(xs) if xs else 0.0


def iqr(xs):
    """Tukey hinges: median of the lower half, median of the upper half. Needs >= 4 points to
    say anything sharper than "the median twice"."""
    if len(xs) < 4:
        m = median(xs)
        return (m, m)
    s = sorted(xs)
    lower = s[: len(s) // 2]
    upper = s[(len(s) + 1) // 2 :]
    return (statistics.median(lower), statistics.median(upper))


def paired_bootstrap(deltas, resamples=10000, seed=20260904):
    """95% CI of the median of `deltas`, by resampling `deltas` itself with replacement.

    `deltas` must already be one number per task (or whatever the pairing unit is) — this
    function does not know what a task is and does not enforce pairing; the caller must hand it
    already-paired deltas, never raw per-run values from two different arms pooled together.
    Seeded, so the number is reproducible.
    """
    if not deltas:
        return (0.0, 0.0)
    rng = random.Random(seed)
    medians = []
    for _ in range(resamples):
        sample = [rng.choice(deltas) for _ in deltas]
        medians.append(statistics.median(sample))
    medians.sort()
    lo = medians[int(0.025 * resamples)]
    hi = medians[min(int(0.975 * resamples), resamples - 1)]
    return (lo, hi)


def sign_test(deltas, favourable=lambda d: d < 0):
    """How many paired deltas moved in the favourable direction. Assumes nothing about the
    distribution and can be checked by counting on your fingers."""
    better = sum(1 for d in deltas if favourable(d))
    return better, len(deltas)


def sign_test_p(better, total):
    """Exact one-sided binomial p for a sign test: P(X >= better), X ~ Binomial(total, 0.5).

    `total` must already exclude ties — a zero delta favours neither arm, and dropping it is
    what a sign test does with it. One-sided because T7 is a directional claim ("A1 CPS < A5
    CPS"), not "the two differ"; a two-sided p would be twice this and is the wrong test for
    the pre-registered wording. No data (total == 0) is p = 1.0: no evidence, not proof.
    """
    if total <= 0:
        return 1.0
    better = max(0, min(better, total))
    return sum(math.comb(total, k) for k in range(better, total + 1)) / 2 ** total


# ---------------------------------------------------------------------------
# Run classification.
# ---------------------------------------------------------------------------


def run_total_tokens(run):
    return sum(
        run.get(k, 0)
        for k in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens")
    )


def is_infra(run):
    """Did not really happen: killed before it could do or record anything.

    124/137/-1 are the named cases, but run.sh's own comment is blunter: "anything else a
    crash" also produces zero tokens. A `claude_exit` of 1, 2, or anything nonzero that still
    left every token counter at zero is the same non-event under a different number — it must
    not be folded into medians as though it were a genuine zero-cost success. A nonzero exit
    that *did* spend tokens (a real, if failed, attempt) is not infra; that run happened and its
    cost is real.
    """
    if run.get("claude_exit") in INFRA_EXIT_CODES:
        return True
    return run.get("claude_exit", 0) != 0 and run.get("total_tokens", 0) == 0


def injected_prompt_bytes(run_dir):
    """Bytes the arm's hook injected at the prompt, summed over that run's `injected.log`.

    Returns None when there is no log at all — A0 has no hooks by design, and "no hooks" must
    never read as "the hooks selected nothing", which is a finding about the ranker. SessionStart
    records are excluded: they are a fixed project summary that does not depend on the prompt,
    and folding them in would mask exactly the case this field exists to expose (A1's per-prompt
    package is empty on the tasks whose prompt names no identifier — see docs/eval/tier2.md).
    """
    log = run_dir / "injected.log"
    if not log.is_file():
        return None
    total = 0
    for line in log.read_text(errors="replace").splitlines():
        m = INJECTED_RE.match(line)
        if m and m.group(1) != "SessionStart":
            total += int(m.group(2))
    return total


def is_flagged(run):
    """Happened, spent real tokens, but grade.sh says the verdict is not a clean measurement."""
    return bool(run.get("adjudicate"))


def is_clean(run):
    """The only runs whose `passed` / `L1_hidden` may be trusted at face value."""
    return not is_infra(run) and not is_flagged(run)


# ---------------------------------------------------------------------------
# Loading.
# ---------------------------------------------------------------------------


def load(base):
    """Every run under `base` with both usage.json and a non-empty grade.json.

    A grade.json that exists but is 0 bytes is what a kill mid-write leaves behind (sweep.sh's
    own resume check treats it the same way, with `-s` not `-f`) — it must read as "not graded
    yet", never as a completed grade with every field missing.
    """
    base = pathlib.Path(base)
    runs, ungraded = [], []
    for usage_path in sorted(base.rglob("usage.json")):
        grade_path = usage_path.parent / "grade.json"
        if not grade_path.is_file() or grade_path.stat().st_size == 0:
            ungraded.append(str(usage_path.parent.relative_to(base)))
            continue
        u = json.loads(usage_path.read_text())
        g = json.loads(grade_path.read_text())
        missing = REQUIRED_GRADE_KEYS - g.keys()
        if missing:
            sys.exit(
                f"analyse.py: {grade_path} is missing required field(s) {sorted(missing)} — "
                f"refusing to guess. A grade.sh field rename must fail loudly here, not turn "
                f"every run into a silent clean failure downstream."
            )
        run = {**u, **g}
        run["total_tokens"] = run_total_tokens(u)
        run["cost_usd"] = u.get("total_cost_usd", 0.0)
        run["injected_prompt_bytes"] = injected_prompt_bytes(usage_path.parent)
        run["path"] = str(usage_path.parent.relative_to(base))
        runs.append(run)
    return runs, ungraded


def check_model(base, runs):
    """meta.json pins one model for the whole stamp. A run tree that disagrees with itself
    (or with meta.json) must be refused loudly, not averaged across — see requirement 5.

    Returns (ok, model_or_message): model_or_message is the single agreed model on success,
    or a human-readable explanation of the disagreement on failure.
    """
    models = sorted({r.get("model") for r in runs if r.get("model")})
    meta_path = base / "meta.json"
    if meta_path.is_file():
        meta_model = json.loads(meta_path.read_text()).get("model")
        bad = [m for m in models if m != meta_model]
        if bad:
            return False, f"meta.json declares model={meta_model!r} but usage.json files use {models!r}"
        return True, meta_model
    if len(models) > 1:
        return False, f"no meta.json to pin a model, and runs disagree: {models!r}"
    return True, (models[0] if models else None)


# ---------------------------------------------------------------------------
# Per-arm / per-task aggregation.
# ---------------------------------------------------------------------------


def _cps(spend, passes):
    return float("inf") if passes == 0 else spend / passes


def arm_cps(rs, field="total_tokens"):
    """Cost per success over `rs`: `field` actually spent (every non-infra run, flagged ones
    included — that spend happened) divided by the count of *clean* passes (flagged runs never
    count as a pass, whatever `passed` says). Infinite if there is no clean pass.

    This bounds one edge rather than resolving it: charging a flagged run's spend against zero
    credit penalises an arm for the grader's own limitation (a `hidden-test-compile-error` is
    documented as a legitimate fix the grader simply can't score). `arm_cps_clean_only` is the
    other bound — spend and passes both restricted to clean runs — so a reader gets both ends
    rather than one number presented as settled.
    """
    passes = sum(1 for r in rs if is_clean(r) and r.get("passed"))
    spend = sum(r.get(field, 0) for r in rs)
    return _cps(spend, passes)


def arm_cps_clean_only(rs, field="total_tokens"):
    """The other bound: flagged runs' spend excluded entirely, not just their pass credit."""
    clean = [r for r in rs if is_clean(r)]
    passes = sum(1 for r in clean if r.get("passed"))
    spend = sum(r.get(field, 0) for r in clean)
    return _cps(spend, passes)


def arm_stats(rs, n_infra):
    """rs: every non-infra run for one arm (flagged included). n_infra: how many were dropped."""
    clean = [r for r in rs if is_clean(r)]
    flagged = [r for r in rs if is_flagged(r)]
    toks = [r["total_tokens"] for r in rs]
    cache = [r.get("cache_read_tokens", 0) for r in rs]
    costs = [r.get("cost_usd", 0.0) for r in rs]
    lo, hi = iqr(toks)
    # What the arm's hooks actually put into the turn. Without this, "A1 was no better than A5"
    # has two readings — the ranking was no better, or the ranker selected nothing at all — and
    # only the second is a fact about the product. Runs with no injected.log (A0) are not in the
    # denominator at all; a run whose log records a zero-byte package is.
    injected = [r["injected_prompt_bytes"] for r in rs if r.get("injected_prompt_bytes") is not None]
    passes = sum(1 for r in clean if r.get("passed"))
    l1 = sum(1 for r in clean if r.get("L1_hidden"))
    false_done = sum(1 for r in clean if r.get("claimed_done") and not r.get("passed"))
    correctness_n = len(clean) + len(flagged)  # flagged counted as failures in the second rate
    return {
        "n_total": len(rs) + n_infra,
        "n_infra": n_infra,
        "n_flagged": len(flagged),
        "n_clean": len(clean),
        "median_tokens": median(toks),
        "iqr_tokens": [lo, hi],
        "max_run_tokens": max(toks) if toks else 0,
        "median_cache_read_tokens": median(cache),
        "median_cost_usd": median(costs),
        "median_injected_bytes": median(injected) if injected else None,
        "n_zero_injection": sum(1 for b in injected if b == 0),
        "n_with_injection_log": len(injected),
        "passes": passes,
        "pass_rate": passes / len(clean) if clean else 0.0,
        # The mirror image of excluding flagged runs from the denominator: what the pass rate
        # looks like if every flagged run is instead charged as a failure. Neither is "the" pass
        # rate; printing both is how a flagged run avoids silently reading as a clean anything.
        "pass_rate_with_flagged_as_fail": passes / correctness_n if correctness_n else 0.0,
        "l1_only_rate": l1 / len(clean) if clean else 0.0,
        "false_done": false_done,
        "false_done_rate": false_done / len(clean) if clean else 0.0,
        "cps": arm_cps(rs),
        "cps_clean_only": arm_cps_clean_only(rs),
        "cps_usd": arm_cps(rs, field="cost_usd"),
    }


def task_arm_tokens(runs, task, arm):
    """Median total tokens for one (task, arm) cell, over its non-infra runs. Flagged runs'
    tokens still count here — they are real spend, just not a trustworthy verdict."""
    vals = [r["total_tokens"] for r in runs if r["task"] == task and r["arm"] == arm and not is_infra(r)]
    return median(vals) if vals else None


def task_arm_cps(runs, task, arm):
    rs = [r for r in runs if r["task"] == task and r["arm"] == arm and not is_infra(r)]
    return arm_cps(rs) if rs else None


def token_deltas(runs, tasks, baseline_arm, treatment_arm="A1"):
    """One delta per task: treatment median tokens minus baseline median tokens. Negative is
    favourable (the treatment is cheaper). Tasks with no data for either arm are skipped and
    reported, never silently treated as a zero delta."""
    deltas, skipped = [], []
    for task in tasks:
        t = task_arm_tokens(runs, task, treatment_arm)
        b = task_arm_tokens(runs, task, baseline_arm)
        if t is None or b is None:
            skipped.append(task)
            continue
        deltas.append(t - b)
    return deltas, skipped


def cps_reduction_deltas(runs, tasks, baseline_arm, treatment_arm="A1"):
    """One delta per task: percent CPS reduction, (baseline - treatment) / baseline * 100.
    Positive is favourable (the treatment costs less per success). A task is skipped, not
    zeroed, when either arm has no clean pass at that task — CPS is infinite there and a
    percentage reduction against infinity is not a number, it is a different kind of result
    that belongs in the flagged/infra counts, not folded into this median."""
    deltas, skipped = [], []
    for task in tasks:
        b = task_arm_cps(runs, task, baseline_arm)
        t = task_arm_cps(runs, task, treatment_arm)
        if b is None or t is None or not math.isfinite(b) or b == 0 or not math.isfinite(t):
            skipped.append(task)
            continue
        deltas.append((b - t) / b * 100.0)
    return deltas, skipped


# ---------------------------------------------------------------------------
# Reporting.
# ---------------------------------------------------------------------------


def report(base):
    base = pathlib.Path(base)
    runs, ungraded = load(base)
    if not runs:
        print(f"no graded runs under {base}", file=sys.stderr)
        return 1

    ok, model_or_msg = check_model(base, runs)
    if not ok:
        print(f"analyse.py: {model_or_msg} — refusing to average across models", file=sys.stderr)
        return 1
    model = model_or_msg

    tasks = sorted({r["task"] for r in runs})
    arms = sorted({r["arm"] for r in runs})

    lines = []
    lines.append(f"# Tier 2 sweep — {base}\n")
    lines.append(f"{len(runs)} graded runs · {len(tasks)} tasks · {len(arms)} arms · model {model}\n")
    if ungraded:
        lines.append(
            f"_{len(ungraded)} run(s) have no grade.json yet and are not in any number below: "
            + ", ".join(ungraded) + "_\n"
        )

    lines.append(
        f"_Runs with `claude_exit` in {sorted(INFRA_EXIT_CODES)} (timeout / OOM-killed / never "
        "started), or any other non-zero exit that still left every token counter at zero (a "
        "crash — run.sh's own words), did not really happen; they are dropped before any "
        "statistic below, per arm, so a systematic pattern stays visible instead of dragging a "
        "median down. Runs flagged by `adjudicate` spent real tokens but their `passed`/"
        "`L1_hidden` is not trustworthy; their tokens still count toward the main CPS's spend "
        "(see `CPS (clean-only)` for the bound that excludes them entirely), and both a "
        "pass rate that excludes them and one that counts them as failures are reported side "
        "by side so neither reading is silently the only one offered._\n"
    )
    lines.append(
        "_`L1_hidden` is `L0 ∧ L2 ∧ hidden`, not an independent measurement — the \"L1-only "
        "rate\" below is computed over clean runs only and is a tripwire, not a purity claim. "
        "`L2_collateral` means only \"the tests in the tree the agent left all pass\", not that "
        "every test green at the start commit is still green — nothing here enumerates the "
        "start-commit tests._\n"
    )

    arm_data = {}
    infra_by_arm = {}
    flag_counts = {}
    for arm in arms:
        all_rs = [r for r in runs if r["arm"] == arm]
        infra = [r for r in all_rs if is_infra(r)]
        rs = [r for r in all_rs if not is_infra(r)]
        infra_by_arm[arm] = infra
        arm_data[arm] = arm_stats(rs, len(infra))
        for r in rs:
            for flag in r.get("adjudicate") or []:
                flag_counts[flag] = flag_counts.get(flag, 0) + 1

    lines.append(
        "_`injected` is what the arm's hooks put into the turn at the prompt, from "
        "`injected.log`: the median package size and how many runs got a **zero-byte** package. "
        "A0 has no hooks and no log, so it reads `—` rather than 0 — \"no hooks\" and \"the "
        "ranker selected nothing\" are different facts. A zero-injection count above zero means "
        "the arm was untreated on those runs, and no comparison involving it is a contrast "
        "between two rankings._\n"
    )
    lines.append(
        "| arm | n (total/clean/flagged/infra) | median tokens | IQR | median cache-read | "
        "median injected | zero-injection | pass rate (clean) | pass rate (flagged=fail) | "
        "L1-only rate | false-done |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|")
    for arm in arms:
        s = arm_data[arm]
        lo, hi = s["iqr_tokens"]
        if s["n_with_injection_log"]:
            inj = f"{s['median_injected_bytes']:,.0f} B"
            zero = f"{s['n_zero_injection']}/{s['n_with_injection_log']}"
        else:
            inj = zero = "— (no hooks)"
        lines.append(
            f"| {arm} | {s['n_total']}/{s['n_clean']}/{s['n_flagged']}/{s['n_infra']} | "
            f"{s['median_tokens']:,.0f} | {lo:,.0f}–{hi:,.0f} | {s['median_cache_read_tokens']:,.0f} | "
            f"{inj} | {zero} | "
            f"{s['passes']}/{s['n_clean']} | {s['passes']}/{s['n_clean']+s['n_flagged']} | "
            f"{s['l1_only_rate']*100:,.0f}% (n={s['n_clean']}) | {s['false_done']}/{s['n_clean']} |"
        )

    lines.append(
        "\n**Cost detail** — CPS brackets the flagged-spend question with two numbers rather "
        "than settling it (see the caveat above); `max run tokens` names the single largest "
        "contributor to the median-tokens column, since CPS is a ratio of sums (a mean in "
        "disguise) and one thrashing run can move it while the median holds steady; the two "
        "dollar columns come straight from `total_cost_usd` in `usage.json` — the API's own "
        "price-weighted figure, which already weights a cache read at its real ~0.1x — so they "
        "read $0.00 rather than erroring under subscription auth, where the API reports no "
        "per-call price.\n"
    )
    lines.append(
        "| arm | CPS (all spend) | CPS (clean-only) | max run tokens | median $/run | CPS ($) |"
    )
    lines.append("|---|---|---|---|---|---|")
    for arm in arms:
        s = arm_data[arm]
        cps_str = "inf" if math.isinf(s["cps"]) else f"{s['cps']:,.0f}"
        cps_clean_str = "inf" if math.isinf(s["cps_clean_only"]) else f"{s['cps_clean_only']:,.0f}"
        cps_usd_str = "inf" if math.isinf(s["cps_usd"]) else f"${s['cps_usd']:,.4f}"
        lines.append(
            f"| {arm} | {cps_str} | {cps_clean_str} | {s['max_run_tokens']:,.0f} | "
            f"${s['median_cost_usd']:,.4f} | {cps_usd_str} |"
        )

    if flag_counts:
        lines.append("\n**Flagged runs, by reason** (excluded from correctness numbers above):\n")
        lines.append("| flag | count | means |")
        lines.append("|---|---|---|")
        for flag, count in sorted(flag_counts.items()):
            lines.append(f"| {flag} | {count} | {ADJUDICATE_MEANING.get(flag, '(undocumented flag)')} |")

    if any(infra_by_arm.values()):
        lines.append("\n**Infra failures, by arm** (all-zero tokens, excluded from every statistic above):\n")
        lines.append("| arm | count | exit codes |")
        lines.append("|---|---|---|")
        for arm in arms:
            if infra_by_arm[arm]:
                codes = sorted({r.get("claude_exit") for r in infra_by_arm[arm]})
                lines.append(f"| {arm} | {len(infra_by_arm[arm])} | {codes} |")

    comparisons = {}
    for baseline in ("A0", "A5"):
        if baseline not in arms or "A1" not in arms:
            continue
        deltas, skipped = token_deltas(runs, tasks, baseline)
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas)
        note = f" ({len(skipped)} task(s) skipped for missing data: {skipped})" if skipped else ""
        # Carried on the line itself, not left to the arm table: a comparison where one arm was
        # handed a zero-byte package on some runs is not a contrast between two rankings there,
        # and a reader quoting this delta has to see that in the same sentence.
        inj_note = "".join(
            f" {a}: {arm_data[a]['n_zero_injection']}/{arm_data[a]['n_with_injection_log']} run(s) "
            f"with a zero-byte injected package."
            for a in ("A1", baseline)
            if arm_data.get(a, {}).get("n_with_injection_log")
        )
        lines.append(
            f"\n**A1 vs {baseline}, token cost** — median per-task delta {median(deltas):,.0f}, "
            f"95% CI [{lo:,.0f}, {hi:,.0f}], favourable (A1 cheaper) on {better}/{total} tasks"
            f"{note}.{inj_note}"
        )
        comparisons[f"A1_vs_{baseline}_tokens"] = {
            "deltas": deltas, "skipped_tasks": skipped,
            "median_delta": median(deltas), "ci": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
            "zero_injection": {
                a: {
                    "n_zero_injection": arm_data[a]["n_zero_injection"],
                    "n_with_injection_log": arm_data[a]["n_with_injection_log"],
                }
                for a in ("A1", baseline)
                if a in arm_data
            },
        }

    threshold = {}
    if "A0" in arms and "A1" in arms:
        deltas, skipped = cps_reduction_deltas(runs, tasks, "A0")
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas, favourable=lambda d: d > 0)
        med = median(deltas)
        # A task is skipped here precisely when one arm's CPS was infinite at it — often the
        # tasks the arms differ *most* on. Skipping them and then declaring victory on whatever
        # is left is selection on outcome, and with few enough tasks left the CI collapses to a
        # single point (every bootstrap resample draws the same one value) that trivially
        # "excludes zero" without meaning anything. MIN_T4_TASKS blocks that regardless of how
        # good the surviving numbers look.
        enough_tasks = len(deltas) >= MIN_T4_TASKS
        meets = enough_tasks and med >= 30.0 and lo > 0.0
        note = f" ({len(skipped)} task(s) skipped, CPS undefined: {skipped})" if skipped else ""
        if not enough_tasks:
            gate_note = f" (fewer than {MIN_T4_TASKS} surviving tasks — not evaluated)"
        else:
            gate_note = ""
        lines.append(
            f"\n**T4 (pre-registered): median CPS reduction ≥ 30% (A1 vs A0), 95% CI excluding "
            f"zero.** Observed: {med:,.1f}% median reduction, 95% CI [{lo:,.1f}%, {hi:,.1f}%], "
            f"favourable on {better}/{total} tasks{note}. "
            f"**{'MEETS' if meets else 'does not meet'} T4**{gate_note} at n={total} tasks "
            f"(minimum {MIN_T4_TASKS} required). "
            f"Reported against the threshold, not gated on it — five tasks cannot carry a "
            f"release gate."
        )
        threshold = {
            "deltas_pct": deltas, "skipped_tasks": skipped,
            "median_reduction_pct": med, "ci_pct": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
            "n_tasks": total,
            "min_tasks_required": MIN_T4_TASKS,
            "meets_t4": meets,
        }

    # T7 — the comparison this whole branch exists to make, and the one with a pre-registered
    # consequence: if A1 does not beat A5, the Context Engine has not earned its complexity and
    # BM25 ships instead. Computed here rather than left to a reader with a calculator, and
    # computed BEFORE any data exists, so the rule is fixed in code rather than chosen once the
    # numbers are on the table. Same MIN_T4_TASKS guard as T4, for the same reason: a CPS-defined
    # subset of one or two tasks is selection on outcome whatever it says.
    t7 = {}
    if "A5" in arms and "A1" in arms:
        deltas, skipped = cps_reduction_deltas(runs, tasks, "A5")
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas, favourable=lambda d: d > 0)
        # Ties are dropped from the sign test, not counted against either side.
        n_effective = sum(1 for d in deltas if d != 0)
        p = sign_test_p(better, n_effective)
        med = median(deltas)
        # Both extra terms are currently subsumed by the p-value and are kept anyway: an exact
        # one-sided binomial cannot reach 0.10 with fewer than 4 non-tied tasks (3 of 3 is 0.125),
        # and clearing it needs a strong enough majority favourable that the median is positive
        # too. They are here so that loosening the threshold later cannot quietly re-enable a
        # two-task verdict or a "significant" result pointing the wrong way. A mutant that drops
        # them therefore survives the self-test, which is a fact about the arithmetic, not a hole.
        enough_tasks = len(deltas) >= MIN_T4_TASKS
        meets = enough_tasks and med > 0.0 and p < 0.10
        note = f" ({len(skipped)} task(s) skipped, CPS undefined: {skipped})" if skipped else ""
        gate_note = "" if enough_tasks else f" (fewer than {MIN_T4_TASKS} surviving tasks — not evaluated)"
        a1_zero = arm_data.get("A1", {}).get("n_zero_injection", 0)
        a5_zero = arm_data.get("A5", {}).get("n_zero_injection", 0)
        inj_note = (
            f" **Injection:** A1 got a zero-byte package on {a1_zero} run(s), A5 on {a5_zero}. "
            "A task where one arm was handed nothing is not a contrast between two rankings."
            if (a1_zero or a5_zero) else ""
        )
        lines.append(
            f"\n**T7 (pre-registered): A1 CPS < A5 CPS, sign test p < 0.10 across tasks.** "
            f"Observed: {med:,.1f}% median CPS reduction vs A5, 95% CI [{lo:,.1f}%, {hi:,.1f}%], "
            f"favourable on {better}/{total} tasks (p = {p:.4f}, one-sided exact binomial over "
            f"{n_effective} non-tied task(s)){note}. "
            f"**{'MEETS' if meets else 'does not meet'} T7**{gate_note} at n={total} tasks "
            f"(minimum {MIN_T4_TASKS} required).{inj_note} "
            f"Reported against the threshold, not gated on it — but this is the comparison whose "
            f"pre-registered consequence is shipping BM25 and deleting the Context Engine."
        )
        t7 = {
            "deltas_pct": deltas, "skipped_tasks": skipped,
            "median_reduction_pct": med, "ci_pct": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
            "n_tasks": total,
            "n_non_tied": n_effective,
            "p_value": p,
            "min_tasks_required": MIN_T4_TASKS,
            "meets_t7": meets,
        }

    lines.append(
        f"\n_Correctness at {len(tasks)} tasks is a tripwire, not a measurement: it detects a "
        f"collapse, not a regression. Every number above carries its own n; quoting a "
        f"correctness delta from this slice past what a tripwire supports is a misuse of it._"
    )

    text = "\n".join(lines)
    print(text)

    summary = {
        "base": str(base),
        "model": model,
        "n_runs": len(runs),
        "n_ungraded": len(ungraded),
        "ungraded_paths": ungraded,
        "tasks": tasks,
        "arms": arms,
        "flag_counts": flag_counts,
        "arms_stats": arm_data,
        "comparisons": comparisons,
        "t4_threshold": threshold,
        "t7_threshold": t7,
    }
    # An infinite CPS (no clean pass in an arm) is the *designed* representation of "nothing
    # passed", and Python's json module happily emits the bare token `Infinity` for it by
    # default — not valid JSON per RFC 8259. `jq` reads it back as a float silently; a strict
    # parser (JSON.parse, most typed languages) throws. allow_nan=False turns that into a loud
    # ValueError here instead of a downstream parse failure days later, and _json_safe removes
    # the cause by writing `null` (the adjacent `passes`/`n_clean` fields explain why).
    (base / "summary.json").write_text(json.dumps(_json_safe(summary), indent=2, allow_nan=False) + "\n")
    return 0


def _json_safe(value):
    """Recursively replace non-finite floats (inf, -inf, nan) with None, so the emitted JSON
    is valid RFC 8259 rather than the `Infinity`/`NaN` tokens Python's json module allows by
    default. Everything else passes through unchanged."""
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, dict):
        return {k: _json_safe(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_json_safe(v) for v in value]
    return value


# ---------------------------------------------------------------------------
# Self-test: hand-computable fixtures with known answers. Not "does it run" —
# "does a subtly wrong implementation (mean instead of median, unpaired instead of paired,
# infra/flagged runs left in) give a different, detectably wrong answer."
# ---------------------------------------------------------------------------


def self_test():
    # -- median is not mean: an outlier that would move a mean must not move the median. -------
    skewed = [100, 100, 100, 100, 10000]
    assert median(skewed) == 100, median(skewed)
    naive_mean = statistics.mean(skewed)
    assert naive_mean == 2080, naive_mean
    assert median(skewed) != naive_mean  # the two are meant to disagree here

    # -- iqr, hand-computed. --------------------------------------------------------------------
    assert iqr([1, 2, 3, 4]) == (1.5, 3.5)
    assert iqr([1, 2, 3]) == (2, 2)  # < 4 points: median twice, not a real spread

    # -- sign test, hand-counted. ----------------------------------------------------------------
    assert sign_test([-1, -2, 3]) == (2, 3)
    assert sign_test([0, -1, 2]) == (1, 3)  # a zero delta favours no one

    # -- sign-test p-value, hand-computed from 2^-n (I-3). ---------------------------------------
    # 5 of 5: only one of the 32 outcomes is this extreme -> 1/32. 4 of 5: that outcome plus the
    # five with exactly 4 -> 6/32. 3 of 3: 1/8. A two-sided p (twice each of these) is the
    # mutant this discriminates against — T7's wording is directional.
    assert sign_test_p(5, 5) == 1 / 32, sign_test_p(5, 5)
    assert sign_test_p(4, 5) == 6 / 32, sign_test_p(4, 5)
    assert sign_test_p(3, 3) == 1 / 8, sign_test_p(3, 3)
    assert sign_test_p(0, 5) == 1.0  # every outcome is at least this extreme
    assert sign_test_p(0, 0) == 1.0  # no data is no evidence, not proof
    assert sign_test_p(4, 4) == 1 / 16 and sign_test_p(4, 4) < 0.10 <= 2 * sign_test_p(4, 4)

    # -- cps: numerator is total spend, denominator is passes, inf when there are none. ----------
    assert arm_cps([{"total_tokens": 100, "passed": True, "adjudicate": []},
                     {"total_tokens": 100, "passed": False, "adjudicate": []}]) == 200
    assert arm_cps([{"total_tokens": 100, "passed": False, "adjudicate": []}]) == float("inf")

    # -- paired bootstrap: reproducible, and correctly collapses when every task-level delta ----
    # -- happens to be identical (all resamples draw the same value, so the CI has zero width). --
    lo, hi = paired_bootstrap([-10, -12, -11, -9, -10])
    assert lo <= -9 and hi >= -12, (lo, hi)
    assert paired_bootstrap([-10, -12, -11]) == paired_bootstrap([-10, -12, -11])
    lo, hi = paired_bootstrap([-100, -100, -100, -100, -100])
    assert lo == hi == -100, (lo, hi)

    # -- paired vs unpaired: the axis the bootstrap resamples over is the whole result. ----------
    # Five tasks. A1's per-task median tokens and the baseline's per-task median tokens vary
    # hugely task to task (100 vs 5000), but *within every task* A1 is exactly 100 tokens
    # cheaper. Paired on the task, that is a dead-certain effect: every one of the 5 per-task
    # deltas is -100, so the true paired CI is [-100, -100] (checked above). A wrong
    # implementation that pools all ten numbers into two unpaired samples and bootstraps the
    # difference of independently-resampled medians destroys the pairing — it can draw five
    # values from {100,100,100,5000,5000} for one side and {200,5100,5100,5100,200} for the
    # other, so its resampled "delta" ranges over thousands of tokens in either direction. Their
    # CIs must not agree: if they did, the axis would not matter, and it does.
    a1_by_task = [100, 5000, 100, 5000, 100]
    baseline_by_task = [200, 5100, 200, 5100, 200]
    paired = [a - b for a, b in zip(a1_by_task, baseline_by_task)]
    assert paired == [-100] * 5
    paired_lo, paired_hi = paired_bootstrap(paired)
    assert paired_lo == paired_hi == -100, (paired_lo, paired_hi)

    def unpaired_bootstrap_WRONG(pool_a, pool_b, resamples=10000, seed=20260904):
        """The mistake this proves is possible: resample each arm's values independently,
        losing which value came from which task. Exists only so the self-test can show its
        answer disagrees with the correct one — never call this from report()."""
        rng = random.Random(seed)
        medians = []
        for _ in range(resamples):
            sa = [rng.choice(pool_a) for _ in pool_a]
            sb = [rng.choice(pool_b) for _ in pool_b]
            medians.append(statistics.median(sa) - statistics.median(sb))
        medians.sort()
        return medians[int(0.025 * resamples)], medians[min(int(0.975 * resamples), resamples - 1)]

    wrong_lo, wrong_hi = unpaired_bootstrap_WRONG(a1_by_task, baseline_by_task)
    # The correct, paired CI has zero width at exactly -100. An unpaired shuffle over the same
    # numbers cannot reproduce that — it sees two multi-modal pools (100s and 5000s on one side,
    # 200s and 5100s on the other) and its resampled medians land on the mode boundaries
    # (roughly -100, -5000 or +4800 depending on which mode each side resamples into), so its
    # interval is wide where the correct one is a point. Detectably different, as required.
    assert (wrong_lo, wrong_hi) != (paired_lo, paired_hi)
    assert (wrong_hi - wrong_lo) > 500, "unpaired bootstrap should be far wider than the paired one"

    # -- infra runs: all-zero tokens, must not enter medians or counts as though real. ----------
    clean_pass = {"task": "T", "arm": "A1", "total_tokens": 1000, "passed": True,
                  "adjudicate": [], "claude_exit": 0, "L1_hidden": True, "claimed_done": True}
    clean_fail = {"task": "T", "arm": "A1", "total_tokens": 1200, "passed": False,
                  "adjudicate": [], "claude_exit": 0, "L1_hidden": False, "claimed_done": False}
    flagged_run = {"task": "T", "arm": "A1", "total_tokens": 900, "passed": False,
                   "adjudicate": ["l1-not-isolated"], "claude_exit": 0, "L1_hidden": False,
                   "claimed_done": False}
    # Corrupt-but-plausible: a timeout that somehow left passed=True. is_infra must be decided
    # by claude_exit alone, never by trusting `passed` on a run that did not really happen.
    infra_run = {"task": "T", "arm": "A1", "total_tokens": 0, "passed": True,
                 "adjudicate": [], "claude_exit": 124, "L1_hidden": True, "claimed_done": True}

    assert is_clean(clean_pass) and is_clean(clean_fail)
    assert is_flagged(flagged_run) and not is_clean(flagged_run)
    assert is_infra(infra_run) and not is_clean(infra_run)

    all_four = [clean_pass, clean_fail, flagged_run, infra_run]
    non_infra = [r for r in all_four if not is_infra(r)]
    stats = arm_stats(non_infra, n_infra=1)
    # Token median/IQR pool: clean_pass, clean_fail, flagged_run (infra's zero excluded).
    assert stats["n_infra"] == 1
    assert stats["n_flagged"] == 1
    assert stats["n_clean"] == 2
    assert median([1000, 1200, 900]) == 1000
    assert stats["median_tokens"] == 1000
    # Pass rate is over clean runs only: 1 of 2, not 2 of 4 (which is what trusting the
    # corrupted infra run's passed=True, or counting the flagged run as a fail, would give).
    assert stats["pass_rate"] == 0.5, stats["pass_rate"]
    naive_pass_rate = sum(1 for r in all_four if r.get("passed")) / len(all_four)
    assert naive_pass_rate == 0.5  # coincidentally equal here...
    naive_pass_rate_over_all_non_infra = sum(1 for r in non_infra if r.get("passed")) / len(non_infra)
    assert naive_pass_rate_over_all_non_infra != stats["pass_rate"], "counting flagged as a fail should differ"
    # CPS: spend includes the flagged run's real tokens (1000+1200+900=3100), passes counts only
    # the one clean pass. A naive cps() over the whole non-infra list without excluding the
    # flagged run's *pass* status would still give 1 pass here since it's not passed anyway, but
    # the numerator must not silently drop the flagged run's spend.
    assert stats["cps"] == 3100 / 1, stats["cps"]
    # Important 5: the other bound. Excluding the flagged run's spend entirely (not just its
    # pass credit) drops the numerator to clean_pass + clean_fail = 2200, same 1 pass.
    assert stats["cps_clean_only"] == 2200 / 1, stats["cps_clean_only"]
    # Important 1: the mirror image of excluding a flagged run from the denominator is printing
    # what the rate looks like if it's counted as a failure instead — neither is "the" rate.
    # clean-only: 1 pass / 2 clean = 0.5 (asserted above as stats["pass_rate"]). Counting the
    # flagged run as a failure too: 1 pass / 3 (2 clean + 1 flagged).
    assert abs(stats["pass_rate_with_flagged_as_fail"] - (1 / 3)) < 1e-9, stats["pass_rate_with_flagged_as_fail"]
    assert stats["pass_rate_with_flagged_as_fail"] != stats["pass_rate"]
    # Important 4: CPS is a ratio of sums (a mean in disguise) — the largest single contributor
    # to the numerator must be visible next to it. Of {1000, 1200, 900}, 1200 is largest.
    assert stats["max_run_tokens"] == 1200, stats["max_run_tokens"]

    # -- dollar CPS (Important 6): usage.json's own total_cost_usd, price-weighted by the API, --
    # -- reported alongside the token-based numbers rather than only implied by them. ------------
    usd_a = {"total_tokens": 1000, "cost_usd": 0.30, "passed": True, "adjudicate": [], "claude_exit": 0}
    usd_b = {"total_tokens": 1000, "cost_usd": 0.50, "passed": False, "adjudicate": [], "claude_exit": 0}
    usd_stats = arm_stats([usd_a, usd_b], n_infra=0)
    assert usd_stats["median_cost_usd"] == 0.4, usd_stats["median_cost_usd"]  # median(0.30, 0.50)
    assert usd_stats["cps_usd"] == 0.8, usd_stats["cps_usd"]  # (0.30+0.50) spend / 1 pass

    # -- is_infra, broadened (Important 2): run.sh's own words are "anything else a crash" — ----
    # -- a nonzero exit that still spent zero tokens is the same non-event as 124/137, whatever --
    # -- the actual number is. A nonzero exit that DID spend tokens is a real, if failed, run. ---
    assert is_infra({"claude_exit": 1, "total_tokens": 0})
    assert is_infra({"claude_exit": 2, "total_tokens": 0})
    assert not is_infra({"claude_exit": 1, "total_tokens": 500})  # spent something; not infra
    assert not is_infra({"claude_exit": 0, "total_tokens": 0})  # a genuine (if odd) success

    # -- grade.json shape (Important 7): a missing required field must refuse loudly, never ----
    # -- default-falsy its way into a silent clean failure across an entire sweep. ---------------
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        run_dir = base / "T" / "A1" / "0"
        run_dir.mkdir(parents=True)
        (run_dir / "usage.json").write_text(json.dumps({
            "task": "T", "arm": "A1", "repetition": 0, "model": "claude-opus-5",
            "input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 0,
            "cache_creation_tokens": 0, "claude_exit": 0, "claimed_done": False,
        }))
        (run_dir / "grade.json").write_text(json.dumps({"task": "T"}))  # missing everything else
        try:
            load(base)
            assert False, "load() must refuse a grade.json missing required fields"
        except SystemExit as e:
            assert "missing required field" in str(e), e

    # -- JSON safety (Critical 2): an infinite CPS must round-trip as valid JSON, never the -----
    # -- bare `Infinity` token RFC 8259 forbids and a strict parser would choke on. --------------
    unsafe = {"cps": float("inf"), "nested": {"cps_clean_only": float("inf")}, "fine": 42}
    safe = _json_safe(unsafe)
    dumped = json.dumps(safe, allow_nan=False)  # must not raise
    reloaded = json.loads(dumped)
    assert reloaded["cps"] is None and reloaded["nested"]["cps_clean_only"] is None and reloaded["fine"] == 42

    # -- model consistency: a tree that disagrees with meta.json must be refused, not averaged. -
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))
        ok, msg = check_model(base, [{"model": "claude-opus-5"}, {"model": "claude-opus-5"}])
        assert ok and msg == "claude-opus-5"
        ok, msg = check_model(base, [{"model": "claude-opus-5"}, {"model": "claude-haiku-5"}])
        assert not ok, "a run tree using a second model must not be silently averaged in"

    # -- end-to-end: a synthetic run tree, exercised through report() itself. --------------------
    _self_test_synthetic_tree()

    # -- Critical 1: the per-task token-delta pipeline, through report(), against a tree whose --
    # -- deltas are computed by hand — not just the isolated paired_bootstrap/sign_test helpers. -
    _self_test_per_task_deltas()

    # -- Important 3: T4 must not declare victory off a single surviving task's point CI. --------
    _self_test_t4_min_tasks()

    # -- C-1: what the hooks actually injected, including the zero-byte package and the arm ------
    # -- that has no log at all. -----------------------------------------------------------------
    _self_test_injection()

    # -- I-3: T7, the A1-vs-A5 threshold the ship-or-delete decision is read from. ---------------
    _self_test_t7()

    print("self-test ok")
    return 0


def _self_test_per_task_deltas():
    """Three tasks, hand-computed medians and deltas, run through report() itself.

    This is what Critical 1 demanded: the earlier self-test proved paired_bootstrap/sign_test
    are correct in isolation on a literal list, but nothing exercised token_deltas() itself —
    the function that actually builds that list from per-run data in report()'s pipeline. Two
    mutants slipped past the old self-test with `self-test ok, rc=0`: task_arm_tokens() using
    mean instead of median, and token_deltas() pooling every (A1 run, baseline run) cross-product
    within a task instead of one delta per task. Both are caught here by construction:

      * Task P has 3 asymmetric A1 reps (100, 100, 400) — median 100, mean 200. A mean
        substitution changes P's delta from -200 to -100, which the exact-value assertion below
        catches directly.
      * Tasks Q and R give A1 and the baseline *different* rep counts (2 vs 3, 4 vs 2). A
        cross-product mutant would emit 3x3 + 2x3 + 4x2 = 9+6+8 = 23 "deltas" instead of 3 (one
        per real task) — caught by the exact length/list assertion, not just a count that could
        coincidentally match.

    Hand computation (median() is statistics.median: the average of the middle two for an even
    count):
        P: A1 median(100,100,400)=100, baseline median(300,300,300)=300  -> delta -200
        Q: A1 median(50,150)=100,       baseline median(500,500,500)=500 -> delta -400
        R: A1 median(10,20,30,40)=25,   baseline median(100,200)=150     -> delta -125
    """
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))

        def put(task, arm, values):
            for rep, total in enumerate(values):
                _write_run(base, task, arm, rep, tokens=(total, 0, 0, 0), passed=True)

        put("P", "A1", [100, 100, 400])
        put("P", "A0", [300, 300, 300])
        put("Q", "A1", [50, 150])
        put("Q", "A0", [500, 500, 500])
        put("R", "A1", [10, 20, 30, 40])
        put("R", "A0", [100, 200])

        import io
        from contextlib import redirect_stdout

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        cmp = summary["comparisons"]["A1_vs_A0_tokens"]

        expected_deltas = [-200.0, -400.0, -125.0]  # tasks sorted P, Q, R
        assert cmp["deltas"] == expected_deltas, cmp["deltas"]
        assert cmp["median_delta"] == -200.0, cmp["median_delta"]  # median of [-200,-400,-125]
        assert cmp["sign_test"] == {"favourable": 3, "total": 3}, cmp["sign_test"]
        # The bootstrap resamples *from these three values themselves* (paired_bootstrap's own
        # contract), so any resampled median is necessarily one of the three original deltas —
        # never a value in between and never outside [-400, -125]. A wrong axis (the
        # cross-product mutant) would hand the bootstrap a completely different, much longer
        # list, whose CI would not respect this bound.
        lo, hi = cmp["ci"]
        assert lo in expected_deltas and hi in expected_deltas and lo <= hi, (lo, hi)


def _self_test_t4_min_tasks():
    """One task where A1's real advantage is large (93% CPS reduction) and two tasks skipped
    because one arm had zero clean passes there — exactly the tasks the arms differ most on,
    which is why skipping them and then judging what's left is selection on outcome. With one
    surviving delta, every bootstrap resample draws that same value, so the CI is a trivial
    point that "excludes zero" no matter what. meets_t4 must be False anyway: T4 requires at
    least MIN_T4_TASKS surviving tasks, not just a threshold-clearing number."""
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))

        # S1: real signal on both arms. A0 3 reps @ 1000 tokens, 1 passes -> cps 3000/1=3000.
        # A1 3 reps @ 200 tokens, all pass -> cps 600/3=200. Reduction (3000-200)/3000*100=93.3%.
        for rep in range(3):
            _write_run(base, "S1", "A0", rep, tokens=(1000, 0, 0, 0), passed=(rep == 0))
            _write_run(base, "S1", "A1", rep, tokens=(200, 0, 0, 0), passed=True)
        # S2: A0 has zero clean passes -> its CPS is infinite -> skipped (not zeroed).
        _write_run(base, "S2", "A0", 0, tokens=(1000, 0, 0, 0), passed=False)
        _write_run(base, "S2", "A1", 0, tokens=(500, 0, 0, 0), passed=True)
        # S3: A1 has zero clean passes -> its CPS is infinite -> skipped.
        _write_run(base, "S3", "A0", 0, tokens=(1000, 0, 0, 0), passed=True)
        _write_run(base, "S3", "A1", 0, tokens=(800, 0, 0, 0), passed=False)

        import io
        from contextlib import redirect_stdout

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        t4 = summary["t4_threshold"]
        assert t4["skipped_tasks"] == ["S2", "S3"], t4["skipped_tasks"]
        assert t4["n_tasks"] == 1, t4  # only S1 survives
        assert abs(t4["median_reduction_pct"] - 93.333333) < 1e-3, t4["median_reduction_pct"]
        # A single-value bootstrap is a point at that same value — genuinely >=30 and >0, and
        # that is exactly why n alone, not the CI shape, has to be the gate.
        assert t4["ci_pct"][0] > 0 and t4["median_reduction_pct"] >= 30.0
        assert t4["meets_t4"] is False, "one surviving task must never be enough to meet T4"


def _self_test_injection():
    """What each arm's hooks put into the turn, through report() itself.

    The finding this exists to make unmissable (C-1): the engine seeds from identifiers, so a
    prompt naming none produces a zero-byte package while the arm looks, from every other column,
    like a treated arm. Three shapes are built by hand:

      * A1 rep 0: SessionStart 50 B + prompt package 100 B  -> 100
      * A1 rep 1: SessionStart 50 B + prompt package   0 B  ->   0   <- the finding
      * A1 rep 2: SessionStart 50 B + prompt package 200 B  -> 200

    median(100, 0, 200) = 100, one zero-injection run of three. Two mutants are caught by exact
    value: counting SessionStart records too gives 150/50/250 -> median 150 and NO zero-injection
    run at all (the finding erased); ignoring the log gives None.

      * A0: no injected.log at all. Must not crash, must not read as a zero-injection run, and
        must not enter the median as a 0 — "no hooks" and "the ranker selected nothing" are
        different facts and the second is the one about the product.
    """
    import io
    import tempfile
    from contextlib import redirect_stdout

    def log(prompt_bytes):
        return (
            "=== SessionStart injected=50 bytes\n"
            "Project: repo\n"
            f"=== UserPromptSubmit prompt=93 injected={prompt_bytes} bytes\n"
            "Code (0)\n"
        )

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))
        for rep, prompt_bytes in enumerate((100, 0, 200)):
            _write_run(base, "T", "A1", rep, tokens=(1000, 0, 0, 0), passed=True,
                       injected_log=log(prompt_bytes))
        for rep in range(3):
            _write_run(base, "T", "A0", rep, tokens=(2000, 0, 0, 0), passed=False)

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        a1 = summary["arms_stats"]["A1"]
        assert a1["median_injected_bytes"] == 100, a1["median_injected_bytes"]
        assert a1["n_zero_injection"] == 1, a1["n_zero_injection"]
        assert a1["n_with_injection_log"] == 3, a1["n_with_injection_log"]

        a0 = summary["arms_stats"]["A0"]
        assert a0["median_injected_bytes"] is None, a0["median_injected_bytes"]
        assert a0["n_zero_injection"] == 0, "an arm with no hooks has no zero-injection runs"
        assert a0["n_with_injection_log"] == 0, a0["n_with_injection_log"]

        # The number has to reach the page, not just summary.json: an operator reads the table.
        stdout = buf.getvalue()
        assert "1/3" in stdout, stdout
        assert "zero-byte injected package" in stdout, stdout
        # And the A0 column must say "no hooks", never a bare 0 that reads as a ranker failure.
        assert "— (no hooks)" in stdout, stdout


def _self_test_t7():
    """T7 end to end: four tasks, A1 cheaper per success on all four.

    Hand computation. Every cell is one rep, one clean pass, so a cell's CPS is its token count:

        task | A5 CPS | A1 CPS | reduction (A5-A1)/A5
        W    |  1000  |   500  | 50%
        X    |  1000  |   400  | 60%
        Y    |  1000  |   600  | 40%
        Z    |  1000  |   200  | 80%

    median(50, 60, 40, 80) = (50+60)/2 = 55. Favourable on 4 of 4, no ties, so the one-sided
    exact binomial p is 1/16 = 0.0625 — under 0.10, so T7 is met.

    n=4 is chosen deliberately: a two-sided p over the same counts is 0.125, which does NOT clear
    0.10. So the mutant that computes a two-sided p flips meets_t7 to False here and is caught by
    an exact assertion, not by inspection. The baseline mutant (A0 instead of A5) is caught too —
    this tree has no A0 arm, so a T7 computed against A0 produces no deltas at all.
    """
    import io
    import tempfile
    from contextlib import redirect_stdout

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))
        for task, a1_tokens in (("W", 500), ("X", 400), ("Y", 600), ("Z", 200)):
            _write_run(base, task, "A5", 0, tokens=(1000, 0, 0, 0), passed=True)
            _write_run(base, task, "A1", 0, tokens=(a1_tokens, 0, 0, 0), passed=True)

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        t7 = summary["t7_threshold"]
        assert t7["deltas_pct"] == [50.0, 60.0, 40.0, 80.0], t7["deltas_pct"]  # tasks sorted W,X,Y,Z
        assert t7["median_reduction_pct"] == 55.0, t7["median_reduction_pct"]
        assert t7["sign_test"] == {"favourable": 4, "total": 4}, t7["sign_test"]
        assert t7["n_non_tied"] == 4, t7["n_non_tied"]
        assert t7["p_value"] == 1 / 16, t7["p_value"]
        assert t7["meets_t7"] is True, t7
        assert "MEETS T7" in buf.getvalue(), buf.getvalue()

    # And the other direction: A1 worse on every task must not meet T7, however small n is.
    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))
        for task in ("W", "X", "Y", "Z"):
            _write_run(base, task, "A5", 0, tokens=(500, 0, 0, 0), passed=True)
            _write_run(base, task, "A1", 0, tokens=(1000, 0, 0, 0), passed=True)

        buf = io.StringIO()
        with redirect_stdout(buf):
            assert report(base) == 0
        t7 = json.loads((base / "summary.json").read_text())["t7_threshold"]
        assert t7["deltas_pct"] == [-100.0] * 4, t7["deltas_pct"]
        assert t7["sign_test"] == {"favourable": 0, "total": 4}, t7["sign_test"]
        assert t7["p_value"] == 1.0, t7["p_value"]
        assert t7["meets_t7"] is False, t7
        assert "does not meet T7" in buf.getvalue()


def _write_run(base, task, arm, rep, *, tokens, passed, claude_exit=0, adjudicate=None, l1=None,
               injected_log=None):
    d = base / task / arm / str(rep)
    d.mkdir(parents=True)
    # None means no injected.log at all — what an arm with no hooks (A0) leaves behind.
    if injected_log is not None:
        (d / "injected.log").write_text(injected_log)
    input_t, output_t, cache_read, cache_creation = tokens
    (d / "usage.json").write_text(json.dumps({
        "task": task, "arm": arm, "repetition": rep, "model": "claude-opus-5",
        "input_tokens": input_t, "output_tokens": output_t,
        "cache_read_tokens": cache_read, "cache_creation_tokens": cache_creation,
        "num_turns": 3, "claude_exit": claude_exit, "claimed_done": passed,
    }))
    (d / "grade.json").write_text(json.dumps({
        "task": task, "L0_build": True, "L1_hidden": l1 if l1 is not None else passed,
        "L2_collateral": True, "passed": passed,
        "L3_sites_found": [], "L3_sites_missed": [], "adjudicate": adjudicate or [],
    }))


def _self_test_synthetic_tree():
    """Build a tiny run tree by hand — two tasks, A0 and A1, with one infra and one flagged run
    mixed in — and check that report() (the real entry point, not a helper) produces the
    exclusions this module promises, end to end."""
    import io
    import tempfile
    from contextlib import redirect_stdout

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        base.mkdir(exist_ok=True)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))

        # Task X: A0 costs ~2000 tokens and fails every rep; A1 costs ~1000 and passes every rep.
        for rep in range(3):
            _write_run(base, "X", "A0", rep, tokens=(1500, 400, 0, 0), passed=False)
            _write_run(base, "X", "A1", rep, tokens=(700, 200, 100, 0), passed=True)
        # Task Y: same shape, plus one A1 rep that timed out (all zero, must not pull the
        # median down) and one A1 rep that is flagged (real tokens, untrustworthy verdict).
        for rep in range(2):
            _write_run(base, "Y", "A0", rep, tokens=(1600, 500, 0, 0), passed=False)
            _write_run(base, "Y", "A1", rep, tokens=(800, 300, 200, 0), passed=True)
        _write_run(base, "Y", "A1", 2, tokens=(0, 0, 0, 0), passed=False, claude_exit=124)
        _write_run(base, "Y", "A1", 3, tokens=(900, 250, 50, 0), passed=False,
                   adjudicate=["l1-not-isolated"], l1=False)

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()
        stdout = buf.getvalue()
        assert "T4" in stdout
        assert "l1-not-isolated" in stdout  # the flag reason must surface, per requirement 1

        summary = json.loads((base / "summary.json").read_text())
        a1 = summary["arms_stats"]["A1"]
        # 7 A1 runs total (3 for X, 4 for Y), 1 infra (Y rep 2), 1 flagged (Y rep 3), 5 clean.
        assert a1["n_total"] == 7, a1
        assert a1["n_infra"] == 1, a1
        assert a1["n_flagged"] == 1, a1
        assert a1["n_clean"] == 5, a1
        # All 5 clean A1 runs passed.
        assert a1["passes"] == 5 and a1["pass_rate"] == 1.0, a1
        # A1's median tokens must come from the 6 non-infra runs (3 clean-X + 2 clean-Y +
        # 1 flagged-Y = 6), never the 7th (infra) run's zero.
        assert a1["n_total"] - a1["n_infra"] == 6
        a0 = summary["arms_stats"]["A0"]
        assert a0["n_total"] == 5 and a0["n_infra"] == 0 and a0["n_flagged"] == 0
        assert a0["passes"] == 0  # A0 fails every rep by construction
        # summary.json is parsed JSON here: an infinite CPS must have round-tripped as `null`
        # (Critical 2), never as the raw Python float or the bare `Infinity` token that RFC 8259
        # forbids and a strict parser would have choked on before this assertion even ran.
        assert a0["cps"] is None, "CPS must serialise as JSON null when an arm has zero clean passes"
        assert a0["cps_clean_only"] is None


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    if len(sys.argv) < 2:
        sys.exit(f"usage: {sys.argv[0]} <run-directory> | --self-test")
    sys.exit(report(sys.argv[1]))
