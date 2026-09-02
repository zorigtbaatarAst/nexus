# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the
codebase. This repo is **single-context**: one product, one domain, one set of records.

## Before exploring, read these

- **[`AGENTS.md`](../../AGENTS.md)** at the repo root: the long-form design briefing — invariants,
  deliberate oddities, and the traps that cost real debugging time to find. Read it before
  changing anything in `crates/`. Its table at the end maps questions to documents.
- **[`docs/architecture-decisions.md`](../architecture-decisions.md)**: the ADRs. Twenty-one
  records in one file, headed `## ADR-0NN — <title>`, each stating why it was needed, what else
  was considered, what it costs, and the signal that should make you change it. Read the ones
  that touch the area you're about to work in — grep the file for the subsystem name rather than
  reading all 21.
- **`docs/`**: fifteen documents, the design of record. `architecture.md`, `data-model.md`,
  `cli-spec.md`, `mcp-api.md`, `change-analysis.md`, `capabilities.md` and the rest.
- **`CONTEXT.md`** at the repo root, if it exists.

`CONTEXT.md` does not exist yet, and its absence is not a defect: **proceed silently**. Don't
flag it, don't suggest creating it upfront. `/domain-modeling` (reached via `/grill-with-docs`
and `/improve-codebase-architecture`) creates it lazily, when terms actually get resolved.

## File structure

```
/
├── AGENTS.md                        ← design briefing; read first
├── CLAUDE.md                        ← operating instructions for agents
├── CONTEXT.md                       ← glossary; created lazily, absent today
├── docs/
│   ├── architecture-decisions.md    ← all ADRs, one file, `## ADR-0NN` headings
│   ├── architecture.md
│   ├── data-model.md
│   └── … 12 more design documents
└── crates/                          ← 13-crate Cargo workspace
```

**Note the deviation from the skill default.** ADRs here are *not* one file per record under
`docs/adr/`; they are sections of `docs/architecture-decisions.md`. Cite them as `ADR-021`, the
form the rest of the repo already uses. If you add a record, append a `## ADR-0NN` section to
that file rather than starting a directory.

## Use the glossary's vocabulary

When your output names a domain concept — an issue title, a refactor proposal, a hypothesis, a
test name — use the term the project already uses. Until `CONTEXT.md` exists, `AGENTS.md` and
the `docs/` set are the vocabulary: *capability*, *finding*, *fact*, *impact*, *scope*,
*ledger table*, *seam*, `sig_hash` vs `body_hash`. Don't drift to synonyms.

If the concept you need has no established term, that's a signal: either you're inventing
language the project doesn't use (reconsider) or there's a real gap (note it for
`/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently
overriding:

> _Contradicts ADR-021 (advisory findings are still findings), but worth reopening because…_

Each ADR names the signal that should make you change it. Quote that signal when you argue the
record should be revisited — it's the fastest way to show the reversal is earned.
