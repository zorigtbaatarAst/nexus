# Vision

## What Nexus is

An **engineering intelligence layer for AI coding agents**.

Not a coding agent. Not a multi-agent framework. Not a linter, an indexer, or a RAG system.

Nexus is the thing that already knows the project, so the agent working in it does not have to
rediscover it every session, one `Read` at a time, at full token price.

## The sentence the whole design serves

> Give Claude Code, Codex, and future AI coding agents the right context, project knowledge,
> workflow, and verification at the right time — without requiring the developer to change how
> they work.

The developer still types:

```bash
cd project
claude
```

Nothing about that changes. Nexus works behind the agent.

## The core principle

> **Do not give AI more context. Give AI better context.**

More context is easy and actively harmful: it costs tokens linearly, dilutes attention, and
buries the three lines that mattered under four hundred that did not. Better context is a
*selection problem* — and a selection problem is a ranking problem, which is deterministic
computation, which is free.

The operational form of the principle, and the number every design decision here is judged
against:

> **Maximum useful information per token.**

## What "better" means, concretely

An experienced senior engineer joining your project does four things a fresh agent cannot:

| The senior engineer | Today's agent | What Nexus supplies |
|---|---|---|
| Knows what this system *is* before reading a file | Infers it from whatever file it opened first | `detect` profile, persisted |
| Knows this module broke last quarter, and how | Has no memory across sessions | Findings with history, facts with evidence |
| Knows a change here reaches the frontend | Cannot see it — nothing in the text connects `fetch('/api/x')` to `@QueryMapping` | The GraphQL/HTTP seam in the dependency graph |
| Does not believe "done" until it compiles and passes | Says "done" | The verification gate |

Nexus is those four things, made queryable, persistent, and cheap.

## The division of labour

> **Nexus owns evidence, history and verification. The agent owns reasoning.**

This is inherited from the existing design and it is not up for revision. It is what keeps the
system honest: the component that gathers the evidence is not the component that draws the
conclusion, so there is an independent check. A tool that both diagnoses and treats has none.

The corollary that shapes every crate boundary: **identity, lifecycle and storage belong to the
platform; only rules belong to a capability.**

## Deterministic before probabilistic

Every question is asked of a query first and a model second, and most questions never reach a
model at all.

This is not asceticism. A model asked "what calls this method?" is slower, more expensive, and
*less accurate* than an index. Spending tokens where a join would do is spending money to
become worse. `docs/ai-integration.md` §5 draws the line and it holds: the model is for
business-logic errors, races, data-consistency violations and behavioural regressions — the
properties a compiler cannot express. Everything else is a query.

## What success looks like

Nexus has succeeded when, on a real project:

1. An agent's first action in a session is *not* a `Read` of a file it will discard.
2. "What does this change affect?" is answered in one call, across the frontend/backend seam,
   with the edge chain that proves it.
3. A conclusion an agent reached expensively last month is retrieved this month for free.
4. An agent saying "done" is followed by a compile, a test run, and a verdict — not by trust.
5. The token cost of all of the above is a number the developer can print, and it is smaller
   than the cost of the agent working it out itself.

Point 5 is the one that matters. Everything else is a feature; that one is the product.
