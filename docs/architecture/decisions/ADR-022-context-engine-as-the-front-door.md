# ADR-022 — The Context Engine is the front door

**Status:** Accepted (2026-09-02)
**Supersedes:** nothing. **Amends:** the `ContextBuilder` sketch in
[`memory-model.md`](../../memory-model.md) §3, which named the component and its formula but was
never built.

---

## Why it is needed

Nexus exposes sixteen MCP tools, each answering one question well. An agent needing context for
a task must decide which to call, in what order, and how to combine the results — then pay for
every partial payload it discards.

Three things follow, all measured in [`current-state.md`](../03-current-state.md):

1. **Nothing assembles context.** `nexus-mcp::budget::fit` truncates a serialized array until
   the bytes fit. Its own note says "showing the N highest-ranked", but nothing ranked them.
   `nexus-core` has no notion of a token budget at all.
2. **Nothing invokes Nexus automatically.** There are no hooks. Every path requires the model to
   choose to call, and `skills/nexus/SKILL.md` is a well-argued plea to remember.
3. **The agent does the assembly**, non-deterministically, at full token price, across many
   round trips — which is the exact cost the product exists to remove.

The mission is *better context, not more context*. Better context is a selection problem.
A selection problem is a ranking problem. Ranking is deterministic computation, which is free.
Nothing in the system currently performs it.

## Decision

Add **one assembly pipeline** at `nexus_core::context`, between the Engine and every adapter:

```
TaskRequest → intent → seeds → expand → signals → rank → budget → package → ContextPackage
```

- **One call** replaces a tool-call conversation.
- **One scoring function** — a weighted sum whose terms are recorded per candidate and whose
  weights are data in `.nexus/policy.toml`, not code.
- **Budget is selection, not truncation.** Candidates are sorted by *density*
  (`score / token_cost`), filled greedily, with a diversity guard and a score floor. An
  unfilled budget is not a problem to solve.
- **The inclusion ledger is mandatory output.** Every candidate carries its decision —
  `included`, or `excluded` with a reason — not only the winners.
- **No stage calls a model.** Intent is a verb table. Ranking is arithmetic. The pipeline is
  reproducible, which is what makes a package explainable.
- Existing tools remain as the primitives and the escape hatch; hooks make the default path
  automatic.

## Alternatives considered

**More tools (status quo, extended).** Add git, verification and memory tools; let the agent
orchestrate. Cheapest to build, and it leaves the primary optimisation target untouched while
adding more things the agent can fail to call. Sixteen well-documented tools requiring correct
orchestration by a model is an API, not an intelligence layer. Rejected: it optimises feature
count, which is not the goal.

**A daemon (`nexusd`) with a filesystem watcher and proactive streaming.** Fastest when warm,
and the natural home for session awareness. Rejected on three independent grounds, any one
sufficient: [ADR-006](../../architecture-decisions.md) defers the daemon until `rescan > 2 s` or
`impact` p95 > 250 ms, and a *full* scan of 880 files currently takes 641 ms — the trigger has
not fired; it violates the standing constraint against unnecessary technology; and it introduces
a stale-index failure mode that is **silent**, where every other failure in this system is loud.
Deferred to Future, gated on its number.

**An LLM-driven context selector.** Ask a model which files matter. Rejected: it is slower, more
expensive and less accurate than a graph query for a question the index can answer exactly, it
makes packages irreproducible, and it puts a probabilistic component inside the one subsystem
whose value depends on being explainable.

## Costs

- **A wrong ranker is worse than no ranker.** Bad context arrives looking like good context and
  the agent has no way to know. This is the real risk of the decision, and it is why the ledger
  is mandatory output rather than a debug flag, and why five golden-package fixtures are part of
  the definition of done.
- **A per-prompt hook is on the developer's critical path.** 150 ms p95 is a hard budget, hooks
  fail open, and they ship off by default until measured on the actual project.
- **`nexus-core` grows.** Mitigated by splitting `engine.rs` first (Phase 0.1), as a
  precondition rather than a follow-up.
- **Weights can become folklore.** Mitigated by keeping them in config and by `--explain`
  decomposing every score into its terms.

## The signal that should make you change it

Change this decision if any of the following turns out to be true:

1. **Golden packages need constant re-baselining** without an accompanying accuracy gain. That
   means the weighted sum is not capturing what makes context useful, and a learned ranker —
   trained on which items agents actually used — is the honest replacement.
2. **Hook p95 exceeds 150 ms** on a real project and cannot be brought back by caching. The
   daemon's trigger has effectively fired, from a direction ADR-006 did not anticipate.
3. **Ledger analysis shows the right item was routinely present but under-budget.** That is a
   budgeting failure, not a ranking one, and the fix is a smarter knapsack rather than more
   signals.
4. **Retrieval misses cluster on semantic similarity** — the task and the code share meaning but
   no token, no edge and no history. That is the *only* evidence that would justify embeddings,
   and it must be measured from ledger data before a vector dependency is added.

Note what is not on this list: "an agent said the context was unhelpful". Ranking is changed on
measurement, not on anecdote.
