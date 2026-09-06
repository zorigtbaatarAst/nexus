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
    container never got far enough to record a status) all come with all-zero token counts.
    Taking a median over those zeros drags every arm's number down by however many of these it
    has, which is not a fact about the arm — it is a fact about the harness that day. These are
    dropped before any statistic below is computed, and the count is reported per arm so a
    systematic pattern (one arm always timing out) stays visible instead of vanishing into a
    lower median.

  * Runs whose grade is not a clean measurement. `grade.json`'s `adjudicate` array (see
    `scripts/eval/grade.sh`) flags a run where L0/L1/L2 answer a question other than "did the
    agent's fix work" — a red baseline that makes L1 unattributable, a hidden test that failed
    to compile rather than to pass, a build failure grade.sh could not explain. These runs did
    spend real tokens, so they are not "infra" in the sense above and their tokens still count
    as spend. But `passed` on a flagged run is not trustworthy, so flagged runs are dropped from
    every *correctness* number (pass rate, the L1-only rate, and the denominator of CPS) and
    reported separately instead. See ADJUDICATE_MEANING below for what each flag means.
"""
import json
import math
import pathlib
import random
import statistics
import sys

# 124 = SIGTERM-then-timeout, 137 = SIGKILL (OOM), -1 = run.sh never wrote a status file at all
# (the container did not even get that far). All three come with all-zero usage.json fields.
INFRA_EXIT_CODES = {-1, 124, 137}

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
    "collateral-unknown-build-failed": "the baseline did not compile for an unrecognised "
        "reason, so L2 (its own tests) was never actually measured",
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


# ---------------------------------------------------------------------------
# Run classification.
# ---------------------------------------------------------------------------


def run_total_tokens(run):
    return sum(
        run.get(k, 0)
        for k in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens")
    )


def is_infra(run):
    """Did not really happen: killed before it could do or record anything."""
    return run.get("claude_exit") in INFRA_EXIT_CODES


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
        run = {**u, **g}
        run["total_tokens"] = run_total_tokens(u)
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


def arm_cps(rs):
    """Cost per success over `rs`: total tokens actually spent (every non-infra run, flagged
    ones included — that spend happened) divided by the count of *clean* passes (flagged runs
    never count as a pass, whatever `passed` says). Infinite if there is no clean pass.
    """
    passes = sum(1 for r in rs if is_clean(r) and r.get("passed"))
    spend = sum(r["total_tokens"] for r in rs)
    return float("inf") if passes == 0 else spend / passes


def arm_stats(rs, n_infra):
    """rs: every non-infra run for one arm (flagged included). n_infra: how many were dropped."""
    clean = [r for r in rs if is_clean(r)]
    flagged = [r for r in rs if is_flagged(r)]
    toks = [r["total_tokens"] for r in rs]
    cache = [r.get("cache_read_tokens", 0) for r in rs]
    lo, hi = iqr(toks)
    passes = sum(1 for r in clean if r.get("passed"))
    l1 = sum(1 for r in clean if r.get("L1_hidden"))
    false_done = sum(1 for r in clean if r.get("claimed_done") and not r.get("passed"))
    return {
        "n_total": len(rs) + n_infra,
        "n_infra": n_infra,
        "n_flagged": len(flagged),
        "n_clean": len(clean),
        "median_tokens": median(toks),
        "iqr_tokens": [lo, hi],
        "median_cache_read_tokens": median(cache),
        "passes": passes,
        "pass_rate": passes / len(clean) if clean else 0.0,
        "l1_only_rate": l1 / len(clean) if clean else 0.0,
        "false_done": false_done,
        "false_done_rate": false_done / len(clean) if clean else 0.0,
        "cps": arm_cps(rs),
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
        "started) have all-zero token counts and did not really happen; they are dropped before any "
        "statistic below, per arm, so a systematic pattern stays visible instead of dragging a "
        "median down. Runs flagged by `adjudicate` spent real tokens but their `passed`/"
        "`L1_hidden` is not trustworthy; their tokens still count as spend, but they are "
        "excluded from every correctness number (pass rate, the L1-only rate, and the pass "
        "count CPS divides by)._\n"
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
        "| arm | n (total/clean/flagged/infra) | median tokens | IQR | median cache-read | "
        "pass rate | L1-only rate | CPS | false-done |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|")
    for arm in arms:
        s = arm_data[arm]
        lo, hi = s["iqr_tokens"]
        cps_str = "inf" if math.isinf(s["cps"]) else f"{s['cps']:,.0f}"
        lines.append(
            f"| {arm} | {s['n_total']}/{s['n_clean']}/{s['n_flagged']}/{s['n_infra']} | "
            f"{s['median_tokens']:,.0f} | {lo:,.0f}–{hi:,.0f} | {s['median_cache_read_tokens']:,.0f} | "
            f"{s['passes']}/{s['n_clean']} | {s['l1_only_rate']*100:,.0f}% (n={s['n_clean']}) | "
            f"{cps_str} | {s['false_done']}/{s['n_clean']} |"
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
        lines.append(
            f"\n**A1 vs {baseline}, token cost** — median per-task delta {median(deltas):,.0f}, "
            f"95% CI [{lo:,.0f}, {hi:,.0f}], favourable (A1 cheaper) on {better}/{total} tasks"
            f"{note}."
        )
        comparisons[f"A1_vs_{baseline}_tokens"] = {
            "deltas": deltas, "skipped_tasks": skipped,
            "median_delta": median(deltas), "ci": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
        }

    threshold = {}
    if "A0" in arms and "A1" in arms:
        deltas, skipped = cps_reduction_deltas(runs, tasks, "A0")
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas, favourable=lambda d: d > 0)
        med = median(deltas)
        meets = bool(deltas) and med >= 30.0 and lo > 0.0
        note = f" ({len(skipped)} task(s) skipped, CPS undefined: {skipped})" if skipped else ""
        lines.append(
            f"\n**T4 (pre-registered): median CPS reduction ≥ 30% (A1 vs A0), 95% CI excluding "
            f"zero.** Observed: {med:,.1f}% median reduction, 95% CI [{lo:,.1f}%, {hi:,.1f}%], "
            f"favourable on {better}/{total} tasks{note}. "
            f"**{'MEETS' if meets else 'does not meet'} T4** at n={total} tasks. "
            f"Reported against the threshold, not gated on it — five tasks cannot carry a "
            f"release gate."
        )
        threshold = {
            "deltas_pct": deltas, "skipped_tasks": skipped,
            "median_reduction_pct": med, "ci_pct": [lo, hi],
            "sign_test": {"favourable": better, "total": total},
            "meets_t4": meets,
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
    }
    (base / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return 0


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

    print("self-test ok")
    return 0


def _write_run(base, task, arm, rep, *, tokens, passed, claude_exit=0, adjudicate=None, l1=None):
    d = base / task / arm / str(rep)
    d.mkdir(parents=True)
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
        assert math.isinf(a0["cps"]), "CPS must be infinite when an arm has zero clean passes"


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    if len(sys.argv) < 2:
        sys.exit(f"usage: {sys.argv[0]} <run-directory> | --self-test")
    sys.exit(report(sys.argv[1]))
