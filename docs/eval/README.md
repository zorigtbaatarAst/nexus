# Resolution accuracy — the first measurement

`nexus graph` has always reported **coverage**: the share of call sites that found any
destination. Nothing in the product asked whether the destination was the *right* one, and
the confidence on every edge was a probability claim nobody had tested. This is the first
run that tests both, against an index produced by a real compiler frontend.

Reproduce it with `make eval`. The committed run is [`baseline.json`](baseline.json).

## The run

| | |
|---|---|
| Oracle | `rust-analyzer 1.98.0` (`rust-analyzer scip`) |
| Repository | this one, at `0ded6b3` |
| Language | Rust only — one SCIP indexer speaks one language |
| Oracle coverage | **102 of 102** Rust files indexed; not partial |
| Comparable set | **1,368 of 4,232** call sites; 1,528 edges judged |
| Excluded | 391 edges whose site the oracle recorded no in-project reference at; 3,218 of an edge type or tier SCIP cannot judge |

## What it found

| Metric | Value | 95 % interval | Unit |
|---|---|---|---|
| Precision | **0.723** | 0.700 – 0.745 | per edge — a fan-out of three with one right scores 1/3 |
| Recall | **0.803** | 0.781 – 0.824 | per site — did the truth appear among the candidates |
| Strict | **0.715** | 0.690 – 0.738 | per site — exactly one candidate, and it is right |
| F1 | 0.761 | | |
| Brier | 0.187 | | mean squared error of the confidence claims |
| ECE | 0.113 | | expected calibration error, weighted by tier size |

### Calibration, by claimed confidence

| Claimed | Measured | 95 % interval | n | Verdict | Jeffreys estimate |
|---|---|---|---|---|---|
| 0.70 | 0.865 | 0.838 – 0.888 | 719 | **miscalibrated — under-claims** | 0.86 |
| 0.60 | 0.657 | 0.616 – 0.696 | 545 | **miscalibrated — under-claims** | 0.66 |
| 0.30 | 0.385 | 0.323 – 0.451 | 218 | **miscalibrated — under-claims** | 0.39 |
| 1.00 | 0.891 | 0.770 – 0.953 | 46 | under-powered — no proposal | — |

**Bins are claimed values, not mechanisms.** `symbol_edges.resolution` records the tier as
`heuristic` for six different mechanisms, so the confidence constant is the only thing
separating them: 0.70 is the unique-simple-name arm, 0.60 the bare-member arm, 0.30 a
three-way overload fan-out (`0.9 / n`). That mapping is read off the constants in
`docs/architecture.md` §3, not off the data.

**No constant is changed by this commit.** A measurement and a behaviour change in one diff
cannot be reviewed independently, and three of the four verdicts point the same way — the
heuristic tiers are *pessimistic*, not optimistic, which is the safer direction to be wrong
in and the less urgent one to fix.

**The lead worth following is `exact`.** It claims 1.00 and measured 0.891 over 46 edges. The
sample is too small to act on — that is what `under-powered` means, and why no proposal is
offered — but a tier named "exact" being wrong five times in forty-six is either a real
defect in FQN matching or an artefact of the oracle, and only a second repository can say
which.

## What this run does *not* say

- **One repository, one language.** Java resolves at 96 % coverage here and was not measured
  at all; `scip-java` needs a full project compile, which `make eval LANG_KIND=java` supports
  and nothing has yet run.
- **Coverage is not accuracy.** The 46 % figure quoted for Rust elsewhere is the share of
  call sites that resolved to *something*. This page is about whether that something was
  right, over the 1,368 sites where both tools had an opinion.
- **32 % of sites are outside the comparable set**, mostly edge types no compiler frontend
  models. Widening that set would change what is being measured, which is why
  `COMPARABLE_EDGE_TYPES` is a named constant rather than an inlined filter.

## How the first attempt went wrong

Recorded because the failure mode is the one this harness exists to prevent, and it happened
to the harness itself.

The first run reported precision **0.001** with `exact` scoring zero — a number that reads as
a catastrophic resolver and was a catastrophic ruler. SCIP counts lines from zero and Nexus
counts from one, so every site lookup missed by one and 3,462 of 4,208 sites silently fell
out of the comparable set. Every unit test passed throughout: both sides of them are
hand-built, so they agreed with each other in whichever convention they were written in.

Only running it against a real index disagreed. A harness that is never pointed at real data
is not yet an instrument.
