# Nexus — architecture

The master plan for Nexus as an **engineering intelligence layer for AI coding agents**.

> Do not give AI more context. Give AI better context.

**Status:** Phase 0 (architecture foundation) complete. No production code has been changed.

## Read in this order

| # | Document | Answers |
|---|---|---|
| [00](00-vision.md) | Vision | What Nexus is, and what success looks like |
| [01](01-problem.md) | Problem | What is actually broken, with the token-cost numbers |
| [02](02-principles.md) | Principles | Ten rules, each with teeth |
| [03](03-current-state.md) | Current state | What exists, measured — modules, boundaries, integrations, debt |
| [04](04-future-architecture.md) | Future architecture | The starting concept criticised; three candidates; the choice |
| [05](05-context-engine.md) | Context Engine | The spine: intent → seeds → expand → rank → budget → package |
| [06](06-memory.md) | Memory | Lifecycle, retrieval, machine memory vs human knowledge |
| [07](07-agent-integration.md) | Agent integration | Hooks, MCP, skills — and the five moments end to end |
| [08](08-verification.md) | Verification | Why "done" is a claim, and how it gets checked |
| [09](09-tooling.md) | Tooling | Twelve building blocks: build, wrap, invoke, or refuse |
| [10](10-roadmap.md) | Roadmap | Phases 0–5, each with a test as its definition of done |
| [11](11-risks.md) | Risks | Twelve risks, ranked, each with a detection signal |
| [12](12-non-goals.md) | Non-goals | What Nexus will not build, and what would reverse each |
| [13](13-evaluation.md) | Evaluation | How we prove it works — or find out it does not |

## Decisions

ADRs continue the existing sequence — see [`decisions/README.md`](decisions/README.md).

- [ADR-022](decisions/ADR-022-context-engine-as-the-front-door.md) — the Context Engine is the front door
- [ADR-023](decisions/ADR-023-sqlite-is-the-substrate-markdown-is-a-view.md) — SQLite is the substrate; Markdown is a one-way view
- [ADR-024](decisions/ADR-024-hooks-are-the-invocation-tier-and-ship-off-by-default.md) — hooks are the invocation tier, off by default
- [ADR-025](decisions/ADR-025-verification-ships-as-a-gate-before-a-reproducer.md) — verification ships as a gate first

## Diagrams

All five render with `mermaid-cli`, verified.

- [`system-overview.mmd`](diagrams/system-overview.mmd) — the whole system, three layers
- [`context-engine.mmd`](diagrams/context-engine.mmd) — the seven-stage pipeline
- [`memory-flow.mmd`](diagrams/memory-flow.mmd) — the memory lifecycle
- [`agent-integration.mmd`](diagrams/agent-integration.mmd) — a real session, end to end
- [`verification-flow.mmd`](diagrams/verification-flow.mmd) — the gate and its judgement matrix

## Relationship to `docs/`

The fifteen documents in [`docs/`](..) are the design of record for **what is built today**.
This directory is the plan for **what it should become**. Where they disagree, the disagreement is
named explicitly in [03-current-state.md](03-current-state.md) — most importantly the
`ContextBuilder` in [`memory-model.md`](../memory-model.md) §3 and the `nexus-verify` crate in
[`verification-engine.md`](../verification-engine.md), both specified and neither built.
