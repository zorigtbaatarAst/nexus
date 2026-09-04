# ADR-026 — Coverage is not accuracy, and the product measures only one of them

**Status:** Accepted (2026-09-03)

## Why it is needed

`nexus graph` reports the share of in-project call sites that bound a destination. Nothing in
the product compares a bound destination against a ground truth — no test, no assertion, no
report. The number says how much of the graph *exists*.

It has been read as saying how much of it is *right*, including by this project's own
documents. [`docs/architecture/12-non-goals.md`](../12-non-goals.md) sets the trigger for
building LSP sidecars at *"measured impact **recall** below 85 % for a language"* and then
satisfies it with *"Current measurement: 96 % of in-project edges resolved."* Recall and
coverage are different quantities. An architectural commitment was resting on a metric that
never measured the thing its own trigger named.

Two properties of the metric made the confusion easy to keep. It was computed over edge rows,
and the ambiguous tiers write one row per candidate, so it **rose as the resolver grew less
certain** — repaired in [ADR-017](../../architecture-decisions.md#adr-017-external-is-a-resolution-outcome-not-a-failure)'s
2026-09-03 revision. And it was published without provenance, so three different values for
Rust resolution — 48 %, 23 % and 13 % — coexisted across the READMEs, `AGENTS.md` and the
roadmap, with no way to tell stale from wrong.

## Decision

**The in-product metric is named `coverage` on every surface that prints it, and every such
surface states that it is not accuracy.** That includes `nexus scan`, `nexus graph`, the
`nexus_get_graph` MCP tool description, and the agent-facing command docs.

**Accuracy is measured out-of-band and never on a user's machine.** It is computed against a
compiler-grade oracle — SCIP indexes from `rust-analyzer`, `scip-java`, `scip-typescript` and
`scip-python` — by a development-only crate that is not compiled into the shipped binary. The
design is
[the resolution-accuracy harness spec](../../superpowers/specs/2026-09-03-resolution-accuracy-harness-design.md).

**Every published resolution figure carries the commit it was measured on.** The root cause of
the three contradictory Rust numbers was not arithmetic; it was that none of them said when
they were true.

## Consequences

The published figure cannot change meaning silently, and a future ranking or trust decision
that needs accuracy must cite an eval run rather than a scan.

The cost is that the product cannot answer "is my graph correct?" on its own. That is honest:
it never could. It previously answered a different question and let the reader assume
otherwise, which is worse than declining.

The trigger in `12-non-goals.md` stays fired for Rust — 46 % coverage is far below its 85 %
threshold under any reading — so no architectural decision reverses because of this ADR. What
changes is that the next reading of that trigger will be against a quantity that matches its
own wording.
