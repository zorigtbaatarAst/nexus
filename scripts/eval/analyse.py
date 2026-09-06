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

  * Two task sets, one corpus. `E1-untested-change` and `N1-null-task` run A1 and A5 only —
    A0 contributes nothing to a ranking comparison and running it would cost five runs a task
    for no statistical power — so T4 (efficiency, A1 vs A0) rests on five tasks at three arms
    while T7 (ranking, A1 vs A5) rests on seven at two. Every comparison below is computed over
    the tasks its two arms were both actually run on (`comparison_task_set`), and states that
    set and its size on its own line. Reporting all seven under T4 would have listed the two
    ranking-only tasks as "skipped for missing data", which reads as data loss rather than as
    the deliberate shape of the corpus; hard-coding the five here would have been a second
    place for the corpus to be declared, and the two would eventually disagree.

  * What the injected package was, not just how big it was. `n_zero_injection` used to exist to
    expose that A1's per-prompt package was empty on two of five tasks. The lexical fallback
    (see `crates/nexus-core/src/engine/query.rs`) means A1 now almost always injects something,
    so that counter reads ~0 while the condition it was watching for — the graph anchored
    nothing and BM25 over file contents was substituted — is exactly as frequent as before. A
    counter that reads zero for the wrong reason looks like good news, so packages are
    classified from `injected.log` instead: `empty` (nothing was sent at all — the fallback's
    two-corroborating-term gate refused too, so the empty package is still the answer),
    `lexical` (every item is a BM25 hit) and `graph`. Both counts are reported per arm. For A1
    a lexical package is the fallback firing; for A5 every package is lexical by construction,
    which is what makes `A5 lexical == all of them` the arm's own sanity check.
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

# One rendered item inside a package body, as `render::context_items` writes it (the only
# renderer the arms' `--brief` hooks reach):
#
#     "  com.acme.OrderService.cancel        src/main/java/OrderService.java:42 · seed: names …"
#     "  db/migration/V1__init.sql           db/migration/V1__init.sql:1 · bm25 4.312"
#
# The `why` is the whole point: it is the only place in `injected.log` that says how the item
# was chosen. `basis.selection` — which names the fallback outright — is printed by `--explain`
# and never by `--brief`, so it is not in the log the sweep collects and cannot be parsed from
# it. The `file:line` anchor is required by the pattern so that a footer line
# ("considered 8 · included 5 · …") cannot be mistaken for an item.
ITEM_WHY_RE = re.compile(r"^\s+\S.* \S+:\d+ · (\S+)")

# The prefix every lexically-ranked item's `why` carries (`format!("bm25 {score:.3}")`). Both
# lexical paths emit it: the fallback (the graph anchored nothing) and A5's `--rank lexical`
# control arm. Which of the two it was is not in the item — it is in the arm.
LEXICAL_WHY_PREFIX = "bm25"

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


def package_kind(size_bytes, whys):
    """What the hook injected, from the item explanations it rendered.

      * `empty`   — nothing was sent at all. Still a real condition after the lexical fallback
                    shipped: the fallback demands two corroborating terms between the prompt and
                    the corpus before it will guess, and below that bar the empty package
                    remains the honest answer (see the `query_overlap(...) >= 2` gate in
                    `crates/nexus-core/src/engine/query.rs`). A dead hook, a missing baseline
                    and a timeout land here too, which is why it is reported and not assumed.
      * `lexical` — every item is a BM25 hit over file contents. In A1 that is the fallback: the
                    graph anchored nothing and a guess was substituted. In A5 it is the arm.
      * `graph`   — anything else: at least one item the engine path selected.

    `whys and all(...)` rather than `all(...)`: `all([])` is True, so a package with no item
    lines at all would otherwise be classified as a lexical guess, which is the specific
    false-good-news this counter exists to avoid.
    """
    if size_bytes == 0:
        return "empty"
    if whys and all(w.startswith(LEXICAL_WHY_PREFIX) for w in whys):
        return "lexical"
    return "graph"


def injected_packages(run_dir):
    """Every per-prompt package the arm's hooks injected in this run, as (bytes, kind) pairs.

    Returns None when there is no `injected.log` at all — A0 has no hooks by design, and "no
    hooks" must never read as "the hooks selected nothing", which is a finding about the ranker.
    SessionStart records are excluded: they are a fixed project summary that does not depend on
    the prompt, and folding them in would mask exactly what these counters exist to expose. Their
    bodies are dropped with them, so a SessionStart summary's items can never be attributed to
    the prompt package that follows it.
    """
    log = run_dir / "injected.log"
    if not log.is_file():
        return None
    packages, current = [], None
    for line in log.read_text(errors="replace").splitlines():
        header = INJECTED_RE.match(line)
        if header:
            if header.group(1) == "SessionStart":
                current = None
                continue
            current = {"bytes": int(header.group(2)), "whys": []}
            packages.append(current)
            continue
        if current is None:
            continue
        item = ITEM_WHY_RE.match(line)
        if item:
            current["whys"].append(item.group(1))
    return [(p["bytes"], package_kind(p["bytes"], p["whys"])) for p in packages]


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
        packages = injected_packages(usage_path.parent)
        run["injected_packages"] = packages
        # Kept as the per-run total the byte median has always been over: one `claude -p` fires
        # UserPromptSubmit once, so this is normally that single package's size, and summing is
        # what keeps it right if a run ever carries more than one.
        run["injected_prompt_bytes"] = None if packages is None else sum(b for b, _ in packages)
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


def arm_stats(rs, n_infra, tasks=None):
    """rs: every non-infra run for one arm (flagged included). n_infra: how many were dropped.

    `tasks`: the arm's full task set, infra runs included — the corpus is asymmetric (A0 does
    not run the two ranking-only tasks) and the arm table has to say so, which it cannot do
    from `rs` alone if an arm's only run at some task was killed by the harness.
    """
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
    packages = [p for r in rs if r.get("injected_packages") for p in r["injected_packages"]]
    task_set = sorted(tasks) if tasks is not None else sorted({r["task"] for r in rs if r.get("task")})
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
        "n_with_injection_log": len(injected),
        # Per prompt package, not per run: "how many packages came from the fallback" is a
        # question about packages. One `claude -p` fires UserPromptSubmit once, so these are
        # normally also per-run counts.
        "n_prompt_packages": len(packages),
        "n_empty_packages": sum(1 for _, k in packages if k == "empty"),
        "n_lexical_packages": sum(1 for _, k in packages if k == "lexical"),
        "tasks": task_set,
        "n_tasks": len(task_set),
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


def comparison_task_set(runs, arms):
    """The tasks a comparison between `arms` is over: those every one of those arms was run on.

    The corpus is asymmetric on purpose. `E1-untested-change` and `N1-null-task` run A1 and A5
    only, so T4 (A1 vs A0) rests on five tasks at three arms and T7 (A1 vs A5) on seven at two.
    Derived from the run tree rather than listed here, because a task list in this file would be
    a second declaration of the corpus and the two would eventually disagree.

    Membership is "has any run at that arm", infra runs included, and that distinction carries
    weight: a task whose A0 runs all timed out IS missing data and must keep being reported as a
    skipped task, while a task that never ran A0 was never in this comparison's corpus at all
    and reporting it as skipped would read as data loss.
    """
    by_arm = {}
    for r in runs:
        by_arm.setdefault(r["arm"], set()).add(r["task"])
    sets = [by_arm.get(a, set()) for a in arms]
    return sorted(set.intersection(*sets)) if sets else []


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
    task_sets = {
        "A1_vs_A0": comparison_task_set(runs, ("A1", "A0")),
        "A1_vs_A5": comparison_task_set(runs, ("A1", "A5")),
    }

    lines = []
    lines.append(f"# Tier 2 sweep — {base}\n")
    lines.append(f"{len(runs)} graded runs · {len(tasks)} tasks · {len(arms)} arms · model {model}\n")
    # First thing under the headline, because every threshold below is read against it: the two
    # pre-registered numbers rest on different task sets, and a reader who assumes one corpus
    # will over-read whichever number came from the larger one.
    t4_set, t7_set = task_sets["A1_vs_A0"], task_sets["A1_vs_A5"]
    ranking_only = [t for t in t7_set if t not in t4_set]
    lines.append(
        "_**Two task sets, one corpus.** **T4** (efficiency, A1 vs A0) is computed over the "
        f"**{len(t4_set)}** task(s) that ran both A0 and A1: {', '.join(t4_set) or '—'}. "
        f"**T7** (ranking, A1 vs A5) is computed over the **{len(t7_set)}** task(s) that ran "
        f"both A1 and A5: {', '.join(t7_set) or '—'}. "
        + (
            f"{', '.join(ranking_only)} joined for the ranking comparison only and never ran "
            "A0 — A0 contributes nothing to a ranking comparison and running it would cost five "
            "runs a task for no statistical power. "
            if ranking_only else ""
        )
        + "The `tasks` column below is each arm's own count. Two sample sizes over one corpus, "
        "not two corpora._\n"
    )
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
        arm_data[arm] = arm_stats(rs, len(infra), tasks={r["task"] for r in all_rs})
        for r in rs:
            for flag in r.get("adjudicate") or []:
                flag_counts[flag] = flag_counts.get(flag, 0) + 1

    lines.append(
        "_`injected` is what the arm's hooks put into the turn at the prompt, from "
        "`injected.log`: the median package size, and then what those packages were. **empty** "
        "counts packages where nothing was sent at all — still a real condition after the "
        "lexical fallback shipped, because the fallback demands two corroborating terms before "
        "it will guess and the empty package remains the answer below that bar. **lexical** "
        "counts packages ranked by BM25 over file contents: **for A1 that is the fallback "
        "firing — the graph anchored nothing and a guess was substituted** — while for A5 every "
        "package is lexical by construction, so `A5 lexical == all` is that arm's sanity check "
        "rather than a finding. Both are per prompt package. A0 has no hooks and no log, so it "
        "reads `— (no hooks)` rather than 0: \"no hooks\" and \"the ranker selected nothing\" "
        "are different facts. An empty or lexical count above zero means no comparison over "
        "those runs is a contrast between two graph rankings. `tasks` is how many tasks the arm "
        "ran — it is not the same number for every arm, and which threshold rests on which set "
        "is stated under the headline above and again on each threshold's own line._\n"
    )
    lines.append(
        "| arm | tasks | n (total/clean/flagged/infra) | median tokens | IQR | median cache-read | "
        "median injected | empty pkgs | lexical pkgs | pass rate (clean) | "
        "pass rate (flagged=fail) | L1-only rate | false-done |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for arm in arms:
        s = arm_data[arm]
        lo, hi = s["iqr_tokens"]
        if s["n_with_injection_log"]:
            inj = f"{s['median_injected_bytes']:,.0f} B"
            empty = f"{s['n_empty_packages']}/{s['n_prompt_packages']}"
            lexical = f"{s['n_lexical_packages']}/{s['n_prompt_packages']}"
        else:
            inj = empty = lexical = "— (no hooks)"
        lines.append(
            f"| {arm} | {s['n_tasks']} | "
            f"{s['n_total']}/{s['n_clean']}/{s['n_flagged']}/{s['n_infra']} | "
            f"{s['median_tokens']:,.0f} | {lo:,.0f}–{hi:,.0f} | {s['median_cache_read_tokens']:,.0f} | "
            f"{inj} | {empty} | {lexical} | "
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
        cmp_tasks = task_sets[f"A1_vs_{baseline}"]
        deltas, skipped = token_deltas(runs, cmp_tasks, baseline)
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas)
        note = f" ({len(skipped)} task(s) skipped for missing data: {skipped})" if skipped else ""
        outside = [t for t in tasks if t not in cmp_tasks]
        set_note = (
            f" Task set: the {len(cmp_tasks)} task(s) run at both arms ({', '.join(cmp_tasks) or '—'})"
            + (
                f"; {', '.join(outside)} never ran {baseline} and "
                f"{'is' if len(outside) == 1 else 'are'} not in this comparison."
                if outside else "."
            )
        )
        # Carried on the line itself, not left to the arm table: a comparison where one arm was
        # handed an empty package, or a guess in place of a graph ranking, is not a contrast
        # between two graph rankings there, and a reader quoting this delta has to see that in
        # the same sentence.
        inj_note = "".join(
            f" {a}: {arm_data[a]['n_empty_packages']} empty and "
            f"{arm_data[a]['n_lexical_packages']} lexical of "
            f"{arm_data[a]['n_prompt_packages']} injected package(s)."
            for a in ("A1", baseline)
            if arm_data.get(a, {}).get("n_prompt_packages")
        )
        lines.append(
            f"\n**A1 vs {baseline}, token cost** — median per-task delta {median(deltas):,.0f}, "
            f"95% CI [{lo:,.0f}, {hi:,.0f}], favourable (A1 cheaper) on {better}/{total} tasks"
            f"{note}.{set_note}{inj_note}"
        )
        comparisons[f"A1_vs_{baseline}_tokens"] = {
            "deltas": deltas, "skipped_tasks": skipped,
            "task_set": cmp_tasks, "n_task_set": len(cmp_tasks),
            "median_delta": median(deltas), "ci": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
            "injection": {
                a: {
                    "n_empty_packages": arm_data[a]["n_empty_packages"],
                    "n_lexical_packages": arm_data[a]["n_lexical_packages"],
                    "n_prompt_packages": arm_data[a]["n_prompt_packages"],
                    "n_with_injection_log": arm_data[a]["n_with_injection_log"],
                }
                for a in ("A1", baseline)
                if a in arm_data
            },
        }

    threshold = {}
    if "A0" in arms and "A1" in arms:
        t4_tasks = task_sets["A1_vs_A0"]
        deltas, skipped = cps_reduction_deltas(runs, t4_tasks, "A0")
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
            f"zero.** **Task set: the {len(t4_tasks)} task(s) run at three arms "
            f"({', '.join(t4_tasks) or '—'}) — not the T7 set below.** "
            f"Observed: {med:,.1f}% median reduction, 95% CI [{lo:,.1f}%, {hi:,.1f}%], "
            f"favourable on {better}/{total} tasks{note}. "
            f"**{'MEETS' if meets else 'does not meet'} T4**{gate_note} at n={total} of "
            f"{len(t4_tasks)} tasks (minimum {MIN_T4_TASKS} required). "
            f"Reported against the threshold, not gated on it — five tasks cannot carry a "
            f"release gate."
        )
        threshold = {
            "deltas_pct": deltas, "skipped_tasks": skipped,
            # `task_set`/`n_task_set` is the corpus this threshold is over; `n_tasks` is how many
            # of those survived a defined CPS at both arms. They are different numbers and both
            # belong in the record — collapsing them is how "5 tasks" and "7 tasks" become one.
            "task_set": t4_tasks, "n_task_set": len(t4_tasks),
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
        t7_tasks = task_sets["A1_vs_A5"]
        deltas, skipped = cps_reduction_deltas(runs, t7_tasks, "A5")
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
        a1 = arm_data.get("A1", {})
        a5 = arm_data.get("A5", {})
        inj_note = (
            f" **Injection:** of A1's {a1.get('n_prompt_packages', 0)} injected package(s), "
            f"{a1.get('n_empty_packages', 0)} were empty and "
            f"{a1.get('n_lexical_packages', 0)} came from the lexical fallback — the graph "
            f"anchored nothing there and BM25 over file contents was substituted, which is the "
            f"same ranking A5 uses, so those tasks are a tie by construction and not a contrast "
            f"between two rankings. A5: {a5.get('n_empty_packages', 0)} empty of "
            f"{a5.get('n_prompt_packages', 0)} (all of A5's packages are lexical by design)."
            if (a1.get("n_prompt_packages") or a5.get("n_prompt_packages")) else ""
        )
        lines.append(
            f"\n**T7 (pre-registered): A1 CPS < A5 CPS, sign test p < 0.10 across tasks.** "
            f"**Task set: the {len(t7_tasks)} task(s) run at A1 and A5 "
            f"({', '.join(t7_tasks) or '—'}) — a different, larger set than T4's above.** "
            f"Observed: {med:,.1f}% median CPS reduction vs A5, 95% CI [{lo:,.1f}%, {hi:,.1f}%], "
            f"favourable on {better}/{total} tasks (p = {p:.4f}, one-sided exact binomial over "
            f"{n_effective} non-tied task(s)){note}. "
            f"**{'MEETS' if meets else 'does not meet'} T7**{gate_note} at n={total} of "
            f"{len(t7_tasks)} tasks (minimum {MIN_T4_TASKS} required).{inj_note} "
            f"Reported against the threshold, not gated on it — but this is the comparison whose "
            f"pre-registered consequence is shipping BM25 and deleting the Context Engine."
        )
        t7 = {
            "deltas_pct": deltas, "skipped_tasks": skipped,
            "task_set": t7_tasks, "n_task_set": len(t7_tasks),
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
        "task_sets": {k: {"tasks": v, "n": len(v)} for k, v in task_sets.items()},
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

    # -- C-1: what the hooks actually injected — the empty package, the lexical fallback, and ----
    # -- the arm that has no log at all. ----------------------------------------------------------
    _self_test_injection()

    # -- #38: two thresholds, two task sets, each stated with its own sample size. ---------------
    _self_test_task_sets()

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
    """What each arm's hooks put into the turn, and *what kind of package it was*, through
    report() itself.

    The old counter here counted zero-byte packages, to expose that the engine seeds from
    identifiers and a prompt naming none got nothing. The lexical fallback changed what that
    number means without changing the condition: A1 now almost always injects something, so a
    zero count reads as good news while the graph is anchoring exactly as little as before. So
    packages are classified, from the only evidence `injected.log` actually carries under the
    arms' `--brief` hooks — each item's rendered `why`. (`basis.selection`, which names the
    fallback outright, is a `--explain` line and is not in the log at all.)

    Seven A1 packages are built by hand across six reps, each rep's log preceded by a
    SessionStart record:

      rep | injected  | body                                   | kind    | why
      ----|-----------|----------------------------------------|---------|--------------------
       0  |    300 B  | two items, `seed:` and `downstream:`    | graph   | the engine path
       1  |    200 B  | two items, both `bm25`, plus a footer   | lexical | the fallback fired
       2  |      0 B  | nothing                                 | empty   | nothing was sent
       3  |    250 B  | one `bm25` item and one `seed:` item    | graph   | a mixed package
       4  |    120 B  | a scope warning, no items at all        | graph   | not a lexical guess
       5  | 0 + 300 B | two prompt packages in one run          | empty,  | the counters are per
          |           |                                         | graph   | package, not per run

    Hand-computed: per-run bytes [300, 200, 0, 250, 120, 300] -> sorted
    [0, 120, 200, 250, 300, 300] -> median 225 (rep 5's two packages sum, as they always have).
    7 packages, 2 empty, 1 lexical. A5: 2 packages, both lexical, 400 B each -> median 400,
    0 empty, 2 lexical. A0: no log at all -> None / 0 / 0.

    Every SessionStart body here is written to *look* lexical (a `bm25` item at 50 B), which no
    real SessionStart package contains — it is there so that a parser which fails to drop those
    records is caught by value rather than by inspection.

    The wrong implementations this discriminates against, each by an exact assertion:

      * counting SessionStart records as packages     -> 13 packages, 7 lexical, median 275
      * counting per run rather than per package (what the old zero-byte counter did) -> rep 5's
        empty package hides behind its sibling's 300 bytes and the empty count reads 1, not 2
      * `any(bm25)` instead of `all(bm25)`            -> 2 lexical (rep 3 wrongly included)
      * dropping the `whys and` guard in package_kind -> 2 lexical (rep 4's empty item list
        makes `all([])` True, so a package with no items at all reads as a lexical guess —
        precisely the false good news this counter replaced)
      * a loose item pattern that does not require the `file:line` anchor -> rep 1's footer line
        ("considered 8 · included 2 · …") yields a non-`bm25` why, so rep 1 reads graph and the
        lexical count drops to 0
      * classifying from the byte count alone         -> 0 lexical
      * keeping only the old zero-byte counter        -> KeyError / no lexical column at all
    """
    import io
    import tempfile
    from contextlib import redirect_stdout

    session = (
        "=== SessionStart injected=50 bytes\n"
        "Code (1)\n"
        "  crates/nexus-core/src/lib.rs      crates/nexus-core/src/lib.rs:1 · bm25 9.999\n"
    )

    def log(body):
        return session + body

    graph_pkg = (
        "=== UserPromptSubmit prompt=93 injected=300 bytes\n"
        "Code (2)\n"
        "  com.acme.OrderService.cancel      src/main/java/OrderService.java:42 · seed: names OrderService\n"
        "  com.acme.OrderRepo.find           src/main/java/OrderRepo.java:11 · downstream: via calls\n"
    )
    lexical_pkg = (
        "=== UserPromptSubmit prompt=93 injected=200 bytes\n"
        "Code (2)\n"
        "  db/migration/V1__init.sql         db/migration/V1__init.sql:1 · bm25 4.312\n"
        "  db/migration/V2__add.sql          db/migration/V2__add.sql:1 · bm25 2.008\n"
        "  considered 8 · included 2 · excluded 6 · 100 of 4000 tokens\n"
    )
    empty_pkg = "=== UserPromptSubmit prompt=93 injected=0 bytes\n"
    mixed_pkg = (
        "=== UserPromptSubmit prompt=93 injected=250 bytes\n"
        "Code (2)\n"
        "  db/migration/V1__init.sql         db/migration/V1__init.sql:1 · bm25 4.312\n"
        "  com.acme.OrderService.cancel      src/main/java/OrderService.java:42 · seed: names OrderService\n"
    )
    itemless_pkg = (
        "=== UserPromptSubmit prompt=93 injected=120 bytes\n"
        "Scope warning: the index covers 3 of 9 files\n"
    )
    a5_pkg = (
        "=== UserPromptSubmit prompt=93 injected=400 bytes\n"
        "Code (2)\n"
        "  db/migration/V1__init.sql         db/migration/V1__init.sql:1 · bm25 4.312\n"
        "  db/migration/V2__add.sql          db/migration/V2__add.sql:1 · bm25 2.008\n"
    )

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))
        # Rep 5's log carries two prompt packages, an empty one and a graph one. Nothing in the
        # harness produces that today — `claude -p` fires UserPromptSubmit exactly once — but it
        # is the only fixture that separates "how many packages were empty" from "how many runs
        # injected zero bytes in total", and the counter claims to be the first.
        two_packages = empty_pkg + graph_pkg
        bodies = (graph_pkg, lexical_pkg, empty_pkg, mixed_pkg, itemless_pkg, two_packages)
        for rep, body in enumerate(bodies):
            _write_run(base, "T", "A1", rep, tokens=(1000, 0, 0, 0), passed=True,
                       injected_log=log(body))
        for rep in range(2):
            _write_run(base, "T", "A5", rep, tokens=(1500, 0, 0, 0), passed=True,
                       injected_log=a5_pkg)
        for rep in range(3):
            _write_run(base, "T", "A0", rep, tokens=(2000, 0, 0, 0), passed=False)

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        a1 = summary["arms_stats"]["A1"]
        assert a1["median_injected_bytes"] == 225, a1["median_injected_bytes"]
        assert a1["n_with_injection_log"] == 6, a1["n_with_injection_log"]
        assert a1["n_prompt_packages"] == 7, a1["n_prompt_packages"]
        assert a1["n_empty_packages"] == 2, a1["n_empty_packages"]
        assert a1["n_lexical_packages"] == 1, a1["n_lexical_packages"]
        # 7 packages over 6 runs: the unit is the package, not the run.
        assert a1["n_prompt_packages"] > a1["n_with_injection_log"]
        # The old counter is gone, not renamed alongside a stale twin: two counters for one
        # condition is how a report comes to disagree with itself.
        assert "n_zero_injection" not in a1, a1

        a5 = summary["arms_stats"]["A5"]
        assert a5["n_prompt_packages"] == 2, a5["n_prompt_packages"]
        assert a5["n_lexical_packages"] == 2, "every A5 package is lexical by construction"
        assert a5["n_empty_packages"] == 0, a5["n_empty_packages"]

        a0 = summary["arms_stats"]["A0"]
        assert a0["median_injected_bytes"] is None, a0["median_injected_bytes"]
        assert a0["n_with_injection_log"] == 0, a0["n_with_injection_log"]
        assert a0["n_prompt_packages"] == 0, "an arm with no hooks has no packages at all"
        assert a0["n_empty_packages"] == 0, "no hooks is not an empty package"
        assert a0["n_lexical_packages"] == 0, a0["n_lexical_packages"]

        # The numbers have to reach the page, not just summary.json: an operator reads the table.
        stdout = buf.getvalue()
        assert "| 2/7 | 1/7 |" in stdout, stdout          # A1: empty pkgs, lexical pkgs
        assert "| 0/2 | 2/2 |" in stdout, stdout          # A5: none empty, all lexical
        assert "lexical fallback" in stdout, stdout       # the T7 line names it
        # And the A0 column must say "no hooks", never a bare 0 that reads as a ranker failure.
        assert "— (no hooks)" in stdout, stdout

    # The classifier itself, on the cases the tree above exercises through report(). Same
    # expectations, stated where a reader can check them by eye.
    assert package_kind(0, []) == "empty"
    assert package_kind(0, ["bm25 1.0"]) == "empty"      # bytes decide "nothing was sent"
    assert package_kind(200, ["bm25 4.3", "bm25 2.0"]) == "lexical"
    assert package_kind(250, ["bm25 4.3", "seed:"]) == "graph"
    assert package_kind(120, []) == "graph", "no items at all is not a lexical guess"
    assert package_kind(300, ["seed:", "downstream:"]) == "graph"


def _self_test_task_sets():
    """The two thresholds rest on two task sets, and each must be computed over — and report —
    its own.

    Six tasks, deliberately asymmetric, exactly as the corpus is: P, Q, R and S run all three
    arms; E1 and N1 run A1 and A5 only, because A0 contributes nothing to a ranking comparison.
    Every cell is one rep and one clean pass unless the table says otherwise, so a cell's CPS is
    its token count.

        task | A0    | A1  | A5   | T4 reduction (A0-A1)/A0 | T7 reduction (A5-A1)/A5
        -----|-------|-----|------|-------------------------|------------------------
        E1   |  —    | 800 | 1000 | not in the T4 set       | 20%
        N1   |  —    | 400 | 1000 | not in the T4 set       | 60%
        P    | 1000  | 500 | 1000 | 50%                     | 50%
        Q    | 1000  | 250 | 1000 | 75%                     | 75%
        R    | fail  | 500 | 1000 | skipped: A0 CPS is inf  | 50%
        S    | infra | 500 | 1000 | skipped: no A0 data     | 50%

    Three shapes of "A0 has no usable number at this task", which must not collapse into each
    other: E1/N1 (**never ran A0** — not in the corpus, not missing data), R (**ran and failed**
    — CPS infinite), S (**ran and was killed by the harness** — its only A0 cell is infra, so it
    IS in the corpus and IS missing data). S is the case the `tasks=` kwarg and
    `comparison_task_set`'s "any run, infra included" rule both exist for, and the only fixture
    where an arm's *entire* presence at a task is an infra run.

    So T4's task set is 4 (P, Q, R, S) of which 2 survive. Its deltas are [50, 75], median 62.5,
    and it does NOT meet T4: 2 surviving tasks is below MIN_T4_TASKS, however good 62.5% looks.
    T7's task set is 6, all surviving, deltas in sorted-task order [20, 60, 50, 75, 50, 50] ->
    sorted [20, 50, 50, 50, 60, 75] -> median 50, favourable 6 of 6, p = 1/64 = 0.015625 < 0.10
    -> MEETS.

    The wrong implementations this discriminates against:

      * computing both thresholds over every task in the tree (what this file did before the
        corpus went asymmetric) -> T4's task set is 6 and E1/N1 appear in its `skipped_tasks`
        as missing data, which is the specific misreading — corpus shape reported as data loss
      * a union instead of an intersection                 -> same, T4 n_task_set 6
      * the two sets swapped                               -> T4 n_task_set 6, T7 4
      * `n_task_set` collapsed onto `n_tasks` (surviving)  -> T4 n_task_set 2, not 4; R and S
        exist in the corpus and were paid for, they just have no defined CPS
      * `comparison_task_set` skipping infra runs when building membership -> S drops out of
        T4's task set entirely and out of its `skipped_tasks`, so a task that was paid for and
        lost to the harness reads as a task that was never in the corpus
      * `arm_stats` deriving its task count from non-infra runs (i.e. dropping the `tasks=`
        kwarg report() passes) -> A0's `n_tasks` reads 3, not 4, and the arm table under-counts
        exactly the tasks the harness took away
    """
    import io
    import tempfile
    from contextlib import redirect_stdout

    with tempfile.TemporaryDirectory() as d:
        base = pathlib.Path(d)
        (base / "meta.json").write_text(json.dumps({"model": "claude-opus-5"}))

        for task, a1_tokens in (
            ("P", 500), ("Q", 250), ("R", 500), ("S", 500), ("E1", 800), ("N1", 400)
        ):
            _write_run(base, task, "A1", 0, tokens=(a1_tokens, 0, 0, 0), passed=True)
            _write_run(base, task, "A5", 0, tokens=(1000, 0, 0, 0), passed=True)
        _write_run(base, "P", "A0", 0, tokens=(1000, 0, 0, 0), passed=True)
        _write_run(base, "Q", "A0", 0, tokens=(1000, 0, 0, 0), passed=True)
        # R's A0 cell ran and failed: CPS is infinite there, so R is genuinely skipped — a
        # different fact from E1/N1, which never ran A0 at all.
        _write_run(base, "R", "A0", 0, tokens=(1000, 0, 0, 0), passed=False)
        # S's ONLY A0 cell was killed by the harness. That run was still paid for and S is still
        # in T4's corpus; it belongs in the task set and in `skipped_tasks`, and in A0's arm-table
        # task count. Deriving either from non-infra runs erases it.
        _write_run(base, "S", "A0", 0, tokens=(0, 0, 0, 0), passed=False, claude_exit=124)

        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = report(base)
        assert rc == 0, buf.getvalue()
        stdout = buf.getvalue()

        summary = json.loads((base / "summary.json").read_text())
        assert summary["task_sets"] == {
            "A1_vs_A0": {"tasks": ["P", "Q", "R", "S"], "n": 4},
            "A1_vs_A5": {"tasks": ["E1", "N1", "P", "Q", "R", "S"], "n": 6},
        }, summary["task_sets"]

        t4 = summary["t4_threshold"]
        assert t4["task_set"] == ["P", "Q", "R", "S"], t4["task_set"]
        assert t4["n_task_set"] == 4, t4["n_task_set"]
        # E1 and N1 are not in this comparison's corpus, so they are not "missing data" either.
        # R and S are: one arm ran and could not pass, the other was killed mid-run.
        assert t4["skipped_tasks"] == ["R", "S"], t4["skipped_tasks"]
        assert t4["deltas_pct"] == [50.0, 75.0], t4["deltas_pct"]
        assert t4["median_reduction_pct"] == 62.5, t4["median_reduction_pct"]
        assert t4["n_tasks"] == 2, t4["n_tasks"]
        assert t4["meets_t4"] is False, "2 surviving tasks is below MIN_T4_TASKS whatever the median"

        t7 = summary["t7_threshold"]
        assert t7["task_set"] == ["E1", "N1", "P", "Q", "R", "S"], t7["task_set"]
        assert t7["n_task_set"] == 6, t7["n_task_set"]
        assert t7["skipped_tasks"] == [], t7["skipped_tasks"]
        assert t7["deltas_pct"] == [20.0, 60.0, 50.0, 75.0, 50.0, 50.0], t7["deltas_pct"]
        assert t7["median_reduction_pct"] == 50.0, t7["median_reduction_pct"]
        assert t7["n_tasks"] == 6 and t7["n_non_tied"] == 6, t7
        assert t7["p_value"] == 1 / 64, t7["p_value"]
        assert t7["meets_t7"] is True, t7

        # The token comparisons are on their own task sets too, not on all six.
        assert summary["comparisons"]["A1_vs_A0_tokens"]["n_task_set"] == 4
        assert summary["comparisons"]["A1_vs_A5_tokens"]["n_task_set"] == 6

        # Per-arm task counts, so the asymmetry is visible in the table itself and not only in
        # the prose above it. A0's 4 includes S, whose only A0 run was infra: the harness took
        # the measurement away, it did not take the task out of the corpus.
        assert summary["arms_stats"]["A0"]["n_tasks"] == 4, summary["arms_stats"]["A0"]
        assert summary["arms_stats"]["A0"]["n_infra"] == 1, summary["arms_stats"]["A0"]
        assert summary["arms_stats"]["A1"]["n_tasks"] == 6
        assert summary["arms_stats"]["A5"]["n_tasks"] == 6

        # And a reader of the page — not of summary.json — must see both sizes stated beside the
        # thresholds, or the whole change bought nothing.
        assert "Task set: the 4 task(s) run at three arms (P, Q, R, S)" in stdout, stdout
        assert "Task set: the 6 task(s) run at A1 and A5 (E1, N1, P, Q, R, S)" in stdout, stdout
        assert "| A0 | 4 |" in stdout, stdout
        assert "Two task sets, one corpus" in stdout, stdout
        assert "E1, N1 joined for the ranking comparison only" in stdout, stdout
        assert "not the T7 set below" in stdout and "a different, larger set than T4's" in stdout


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
