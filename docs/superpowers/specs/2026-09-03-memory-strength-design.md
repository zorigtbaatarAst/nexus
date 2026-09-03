# Memory Strength — Design

**Status:** approved, not yet implemented
**Date:** 2026-09-03
**Prerequisite:** [`2026-09-03-retrieval-design.md`](2026-09-03-retrieval-design.md). Ranking
memory better is pointless while every request still loads all of it — 274 ms at 200,000
facts. That spec is the floor this one stands on and is implemented first.

## Why

### Memory currently gets *worse* the more the project is worked on

`memory::relevance` ends in `recency_decay(created_scan_id, current_scan_id)`:

```rust
(-age / 40.0).exp().clamp(0.05, 1.0)
```

Measured: 25 edit-and-rescan cycles advance `scan_id` by exactly 25, because the `PostToolUse`
hook runs `nexus rescan` after every edit.

| Scans since a fact was recorded | Its weight |
|---:|---:|
| 25 | 0.535 |
| 100 | 0.082 |
| 200 and forever after | **0.050 — the clamped floor** |

A developer makes 200 edits in a couple of days. Every fact is therefore worth a twentieth of
a fresh one within about a week of active work, however hard-won, and can never recover. The
function's own comment claims *"halving every 40 scans keeps a year-old invariant near full
weight"*; it halves at ~28 and reaches the floor at ~120.

**The unit is not the bug.** An earlier reading of this called the scan clock wrong and wall
time right. That is backwards: a project idle for a year has done no work, its code has not
moved, and its facts should not have aged. Scans measure work done, which is the right proxy
for "has the world moved under this claim." Two things are wrong, and neither is the clock:

1. **The curve.** Exponential decay has a tail that collapses, and the `0.05` clamp makes the
   collapse permanent.
2. **The half-life is fixed.** Every fact decays at the same rate whether it has proved itself
   a hundred times or never once.

### Nothing measures whether a fact was ever useful

```
relevance = subject_match × source_weight × state_weight × confidence × recency_decay
```

Structural, provenance, provenance, provenance, time. `validated_count` counts scans a fact
*survived*, which is persistence, not value. No table records that a fact was served, read, or
helped. The system ages; it does not learn.

## What the research says

**The field's open problem is one Nexus already solved.** Surveys of agent memory name
staleness — high-relevance memories becoming confidently wrong over time — as unsolved beyond
decay heuristics ([Anatomy of Agentic Memory](https://arxiv.org/html/2602.19320v1),
[State of AI Agent Memory 2026](https://mem0.ai/blog/state-of-ai-agent-memory-2026)). Nexus
answers it structurally: a fact is anchored to `file:line` and a scan that moves the anchor
invalidates it. Decay is therefore *not* needed to do staleness work here, which frees it to
do the job it is actually suited to.

**The precedent is the DSR model** — Difficulty, Stability, Retrievability — behind FSRS and
Duolingo's half-life regression ([The Algorithm](https://github.com/open-spaced-repetition/awesome-fsrs/wiki/The-Algorithm),
[FSRS scheduler](https://github.com/open-spaced-repetition/free-spaced-repetition-scheduler),
[Adaptive Forgetting Curves](https://arxiv.org/pdf/2004.11327)). Two results transfer:

1. **Power-law, not exponential.** FSRS moved from `R = 0.9^(t/S)` to
   `R = (1 + t/(9S))^(-1)`. The power-law tail does not collapse.
2. **Stability grows with each successful recall**, so the half-life extends with use, and
   growth is largest when retrievability had fallen — the spacing effect.

The mapping is close to exact:

| DSR | Nexus |
|---|---|
| Retrievability | the decay term in `relevance` |
| Stability | grows when a fact was served and the next scan changed its subject |
| Difficulty | already present as `source_weight × confidence` |
| A review | a scan that re-checks the fact's evidence anchor |

## Design

### 1 — Retrievability replaces recency decay

One term of the product changes shape. Nothing else about `relevance` moves.

```rust
/// §4's decay, as a power law over a per-fact half-life.
///
/// `t` is scans since the fact was recorded or last confirmed — work done, not days elapsed.
/// A project nobody touches has not aged its knowledge.
pub fn retrievability(t: i64, stability: f64) -> f64 {
    let t = t.max(0) as f64;
    (1.0 + t / (9.0 * stability)).recip()
}
```

No clamp. The power-law tail reaches small values without ever pinning every fact to one
number, which is what made the current floor destroy ordering among old facts.

```
relevance = subject_match × source_weight × state_weight × confidence
          × retrievability(scans_since_confirmed, stability)
```

### 2 — Stability, and how it grows

`facts` gains two columns:

| Column | Meaning | Initial |
|---|---|---|
| `stability REAL NOT NULL` | half-life in scans, divided by 9 | `20.0` |
| `confirmed_scan_id INTEGER` | the scan that last confirmed it | `NULL`, falls back to `created_scan_id` |

Base stability 20 gives an unconfirmed fact a half-life of **180 scans** — generous room to
earn its first confirmation before it fades.

On a confirmation:

```rust
/// Growth is large only for a fact that had faded — the spacing effect.
///
/// Re-confirming something served a moment ago teaches nothing and grows stability by ~5%;
/// confirming one that had decayed to half strength doubles it. That asymmetry is also the
/// anti-gaming property: serving a fact repeatedly in quick succession cannot inflate it.
fn grow(stability: f64, t: i64) -> f64 {
    stability * (1.0 + 2.0 * (1.0 - retrievability(t, stability)))
}
```

**No cap is needed, because the rule is self-limiting.** As stability rises, retrievability
stays high for longer, so each further confirmation grows it less. Confirmed every 150 scans,
stability goes 20 → 38 → 61 → 88 → 116 → 145 — additive in practice, not exponential.

| | Current | Unconfirmed | Confirmed 3× | Confirmed 6× |
|---:|---:|---:|---:|---:|
| 50 scans | 0.287 | 0.783 | 0.940 | 0.969 |
| 200 scans | **0.050 floor** | 0.474 | 0.798 | 0.887 |
| 1,000 scans | 0.050 | 0.153 | 0.441 | 0.611 |
| 20,000 scans | 0.050 | 0.009 | 0.038 | 0.073 |

Proven knowledge stops decaying. Unproven knowledge fades gently instead of falling off a
cliff, and keeps its ordering relative to other unproven knowledge all the way down.

### 3 — The confirmation signal

**A fact is confirmed when it was served in a package and the next scan reports a change to
its subject.** It was in front of someone while they worked on exactly what it describes.

Both halves already flow through the system. `changes.fqn` records what each scan saw move;
the only missing piece is a record of what was served.

```sql
CREATE TABLE fact_serves (
  fact_id  INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
  at       TEXT    NOT NULL,
  PRIMARY KEY (fact_id)
);
```

One row per fact, not per serve — a fact served five times between two scans is one pending
confirmation, which is also what stops a chatty hook from inflating anything.

**Learning happens at scan boundaries, never at query time.** This is not a preference; it is
required. `05-context-engine.md` pins Nexus as a pure function of (request, index, memory),
which is what makes a golden package meaningful. If serving changed rank immediately, the same
request would answer differently the second time. So `fact_serves` feeds nothing. The scan
transaction reads it, updates `stability` and `confirmed_scan_id` for facts whose subject
appears in this scan's `changes`, and deletes every row it consumed. Between scans, memory is
frozen.

The table is bounded by facts-served-since-last-scan, and the hook scans after every edit, so
it holds a handful of rows.

Golden packages scan once and then query, so nothing reconciles and they do not move.

### 4 — What is deliberately not taken from FSRS

**Its 21 fitted parameters.** They come from millions of labelled reviews. Nexus has no labels
and no training data, so the structure transfers and the constants do not. `9`, `20.0` and
`2.0` above are chosen for the behaviour tabulated here, and a test pins that behaviour so a
later change to them is visible rather than silent.

**Its failed-review penalty.** In FSRS a lapse cuts stability. Here, "served and the subject
did not change" often means the developer read the fact and correctly decided to change
nothing — a good outcome. The signal cannot distinguish that from the fact being useless, so
stability grows on confirmation and is never reduced. Asymmetric, and honest about what the
evidence can show.

**Difficulty as a fitted variable.** `source_weight × confidence` already occupies that slot
in the product.

## Consequences

| | Now | After |
|---|---|---|
| Fact weight after 200 edits | 0.050, floored, forever | 0.474 unconfirmed · 0.798 confirmed 3× |
| Ordering among facts older than ~120 scans | all identical at the floor | preserved, by proof |
| Storage per fact | — | one `REAL`, one nullable `INTEGER` |
| Per-request work | unchanged | unchanged — one term swapped in the same product |
| Tables | — | one, emptied every scan |

`state_weight` and `validated_count` stay. They describe the lifecycle — has this been checked,
did a person write it — which is orthogonal to how strongly it is held.

## Acceptance criteria

1. `retrievability(200, 20.0)` is within 0.01 of 0.474; `retrievability(200, S)` for a fact
   confirmed three times at 150-scan gaps is within 0.01 of 0.798. The table above is a test.
2. Retrievability is strictly decreasing in `t` and never reaches a floor: for any stability,
   `retrievability(1_000_000, s) < retrievability(20_000, s)`.
3. `grow` applied to a fact confirmed immediately after being served raises stability by less
   than 10%; applied at half retrievability it doubles it.
4. Eight successive confirmations at a fixed gap produce a strictly increasing stability
   whose *growth multiplier* strictly decreases — 1.909, 1.608, 1.427, 1.320, 1.252, 1.207,
   1.174, 1.150 at a 150-scan gap. This is the self-limiting property; a test that only
   checked "stability increases" would not catch runaway.
5. A fact served, followed by a scan that changes its subject, has a higher `stability` and a
   later `confirmed_scan_id` afterwards. One served with no such change has neither changed.
6. Serving a fact does not change any package: the same request answered twice between two
   scans returns byte-identical output.
7. `fact_serves` is empty after a scan completes.
8. Golden packages do not move.
9. `make check` green.

## Out of scope, and named

**Position bias.** A fact that ranks high is served more, so serving is not independent of
rank, and stability compounds that. The literature's answer is inverse-propensity weighting
([doubly-robust correction](https://arxiv.org/pdf/2203.17118)), and Nexus already logs the
propensity — the `InclusionLedger` records every candidate with its score and decision. Not
built here: base stability gives a new fact a 180-scan half-life to earn its first
confirmation, and `subject_match` dominates the product regardless of stability. Recorded as a
known limitation rather than solved speculatively.

**Forgetting.** Facts are never deleted, so storage grows without bound even though retrieval
cost will not after the retrieval spec lands. Consolidating duplicates and retiring facts that
have decayed below usefulness is a real piece of work and a separate one.

**Explicit feedback.** An agent could mark a fact as having helped. It would be a truer signal
than the coincidence used here, but nothing else in this project asks a model to grade itself,
and making memory quality depend on model cooperation is a larger decision than this design
should make on its own.
