---
name: nexus
description: "Use when working in a codebase that Nexus has indexed (a .nexus/ directory exists), and especially before changing code you have not read: it answers what changed since the last scan, what a change touches across the frontend/backend seam, what is already known about a file or symbol, and which Spring proxy mistakes, orphaned GraphQL fields or committed credentials exist. NOT for writing code, running tests, or general Java/TypeScript questions."
metadata:
  version: "0.2.0"
  user-invocable: "true"
---

# Nexus

Persistent code intelligence. Nexus indexed this project once and remembers its structure,
its git history, its dependency graph, and every finding recorded in it. **It knows things
this session does not.** Ask before deriving.

## When it earns its keep

**Before editing code you have not read.** `nexus_get_known` answers "what has gone wrong
here before, and what did a previous session work out about it" — a `REGRESSED` finding means
this exact thing broke, was fixed, and broke again, which is worth knowing before you touch it.

**When asked what a change affects.** `nexus_get_impact` traverses the real dependency graph
and crosses the GraphQL seam, so a change to a backend service method reaches the React
components that render it. Reading files cannot answer this: nothing in the source text
connects `fetch('/api/x')` to `@QueryMapping`.

**When you need to know what moved.** `nexus_rescan` reports changes down to the symbol and
distinguishes `API_CHANGED` from `BODY_CHANGED` from `CONTRACT_CHANGED` — the last being an
annotation change a compiler never notices and that, in Spring, often matters most.

## A normal sequence

```
nexus_get_project_context     what is this, and has the baseline drifted
nexus_rescan                  what changed since it was last indexed
nexus_get_impact <symbol>     what breaks, including on the other side of the stack
nexus_get_known <file>        what is already known about this code
bughunter_analyze             run the deterministic rules over it
```

## Reading the answers honestly

**Confidence is not decoration.** Every impact result carries the edge chain that produced it
and `min_confidence`, the weakest link along that chain. Below 0.7 the path went through a
heuristic hop: report it as a lead, not a fact.

**A deterministic finding is not a model estimate.** `bughunter_analyze` runs rules — both
sides of every claim are in the index and comparing them is a query. Do not hedge those the
way you would hedge your own inference.

**Truncation is stated.** A result carrying `truncated: true` gives the true total and a way
to narrow. Use it rather than reasoning from a partial answer.

## Contributing what you work out

Nexus finds what rules can express. Business-logic errors, races, transaction boundaries and
data-consistency problems are yours — and `nexus_record_finding` is how they survive the
session. A recorded finding gets the same identity and history a rule's does, so the same
observation next week is recognized rather than repeated.

Two rules, and both are enforced rather than advisory:

- **Every finding needs `file:line` evidence.** Evidence pointing at a file outside the index
  is rejected, not stored. A claim nobody can check is not a finding.
- **Your confidence is capped at 0.75.** Nothing here runs tests, so nothing is proven by
  reproduction. Do not present a recorded finding as certain.

`nexus_record_fact` is for what is true about the project but is not a symbol, an edge or a
finding — an invariant, a convention, a decision and its reason. Expensive conclusions should
be reached once.

## What it does not do

It does not run tests, so no finding is verified by reproduction. It does not analyze Python
or Rust yet. And it does not reason: the index, the graph and the history are evidence, and
what they mean is your job.
