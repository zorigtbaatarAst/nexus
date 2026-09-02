# Non-goals

What Nexus will not build, and — for each — **the trigger that would reverse the decision**.

A non-goal without a reversal condition is a prejudice. Every entry here names the evidence that
would change the answer, written down *now*, while nobody wants the feature, so that the decision
later is made by a measurement rather than by whoever is most enthusiastic.

---

## Permanent — these do not have triggers

These are not deferrals. Building any of them would make Nexus a different product.

### N1 — A new LLM, or any model training

Nexus has no model, fine-tunes nothing, and evaluates no checkpoints. **The deterministic build
carries no HTTP client in its dependency tree at all**, asserted by a `cargo metadata` test.
That is not a limitation to lift; it is what makes Nexus something an agent can trust, because
it is structurally incapable of guessing.

### N2 — A coding agent

Nexus does not write code, edit files, or decide what to do next. *Nexus owns evidence, history
and verification; the agent owns reasoning.* A tool that both diagnoses and treats has no
independent check on itself — which is the entire reason the split exists.

### N3 — A generic multi-agent framework, or an agent swarm

No agent spawns another agent. No orchestration, no supervisor, no message bus, no swarm. The
brief's own shape is `Developer → Claude Code → Nexus capabilities when useful`, and the inverse
— Nexus launching agents — is explicitly rejected.

The failure mode is not theoretical: autonomous agent fan-out multiplies token cost by the
branching factor while the accuracy of any individual branch stays flat. That is the exact
opposite of *maximum useful information per token*.

### N4 — An IDE, an editor, or a UI

Nexus is a CLI and an MCP server. `stdout` is results, `stderr` is everything else, and
`--json | jq` works. That is the interface.

### N5 — Telemetry

No usage reporting, no update checks, no crash reporting, no analytics. Ever. Memory is local by
default and never leaves the machine.

### N6 — Automatic fixes

Nexus reports and proves. Applying the fix is the developer's or the agent's job, with the
evidence in hand. See N2 — this is the same principle at a smaller scale.

### N7 — Style, taste, or "this could be cleaner"

Nothing comments on naming, formatting, or how code ought to be written, because none of it is
checkable and a tool that guesses at it gets switched off. `cap-review` reports only what the
index can *prove* about a change: that nothing tests it, that it crosses the seam, that a
signature moved while its callers did not.

### N8 — Arbitrary command execution

There is no such tool over MCP and there will not be one. The allowlist is the entire execution
surface, entries are argv templates with typed holes, and **`sh -c` is never used, anywhere**.

---

## Deferred — each has a trigger, and none has fired

### N9 — A vector database or embeddings

**Not building.** Dependency, recall opacity, index staleness, and a second retrieval path that
disagrees with the first.

**Why the shape is wrong for the problem:** dependency, impact and regression are *structural*
facts, not semantic ones. A caller frequently shares no vocabulary with its callee. Embedding
similarity retrieves things that *read* alike, which is not the question being asked.

**Trigger:** ledger analysis showing retrieval misses that cluster on **semantic** similarity —
the task and the correct code share meaning but no token, no edge, and no history. The
[inclusion ledger](05-context-engine.md) §8 is what makes this measurable, and **no evidence has
been gathered yet**. Gathering it is a prerequisite, not a formality.

### N10 — A daemon (`nexusd`) or filesystem watcher

**Not building.** [ADR-006](../architecture-decisions.md) already decided this, and the
redesign upholds it.

**Trigger:** no-op `rescan` > 2 s, or `impact` p95 > 250 ms. **Measured today: 641 ms for a
*full* scan of 880 files.** The trigger has not fired, and building the daemon now would be
deciding by enthusiasm.

**The specific hazard:** every other failure in this system is loud. A daemon serving a stale
index is silent, and a wrong answer that looks right is the most expensive kind there is.

### N11 — A second code indexer

**Not building.** One index, one graph, one source of truth about the code.

Graphify is consumed as an optional *signal* for languages with no analyzer — ranked below
tree-sitter edges, excluded from the resolution denominator, and **deleted when native support
lands** ([`09-tooling.md`](09-tooling.md) §1).

**Trigger:** none. Two indexes that can disagree about the same symbol is a defect, not a
feature.

### N12 — LSP sidecars

**Not building yet**, though this is the highest-quality deferred item: `jdtls` and
`rust-analyzer` resolve symbols exactly where tree-sitter heuristics approximate.

**Trigger:** measured impact recall below 85 % for a language. Current measurement: **96 % of
in-project edges resolved** on a real 880-file project. Not fired.

### N13 — Distributed infrastructure, a server, or any cloud dependency

**Not building.** Nexus is a single binary and a single SQLite file. It works on a plane.

**Trigger for a team-shared store specifically:** more than one developer maintaining the same
findings on the same repository — and even then the first answer is
`nexus export` / `nexus import` over a committed JSON file, which requires no infrastructure at
all. A server is the second answer, and only if the file fails.

### N14 — Storing conversations, tool calls, or agent reasoning

**Not building.** Memory is rows, not transcripts. A chat log is unqueryable, unbounded,
unverifiable and provider-specific, and its signal-to-noise ratio *falls* with every session.

**Trigger:** none foreseen. If a genuine need appears, the answer is to extract structured facts
from the conversation, which is the mechanism that already exists.

### N15 — Test generation, in the near term

Deferred to Phase 5, and it arrives **with** the `SafeWriter` jail and the Docker sandbox, never
before. Writing files into a project is the highest-consequence thing Nexus could do, and the
jail is the precondition, not the follow-up.

---

## Not-yet, per phase

Each phase in [`10-roadmap.md`](10-roadmap.md) carries its own "do not build yet" list. The
recurring temptations, named so they can be recognised:

| Temptation | Why it is wrong *now* |
|---|---|
| Tune ranking weights before there is ledger data | Tuning without measurement is folklore. Ship the ledger, gather evidence, then tune |
| Add a language because it is easy | Each analyzer is permanent maintenance surface. Rust first, because it makes Nexus able to index itself |
| Make hooks on by default | A per-prompt hook on the developer's critical path, before its latency is measured on their project, is exactly the "change how you work" the mission forbids |
| Generalise `Capability` into a plugin system | One extension point, no loader, no manifest, no dynamic libraries. There is no second implementer asking |
| Build the "explain why" UI | `--explain` on the CLI is the whole feature. A UI is N4 |

---

## The test for any future proposal

Three questions, in order. A "no" at any step ends it:

1. **Does a proven tool already do this?** If yes, invoke it — do not rebuild it.
2. **Has its trigger fired, with a number?** If no, it is not needed yet, however good the
   argument.
3. **Does it raise useful information per token?** If it only raises *volume*, it is the
   problem, not the solution.
