# ADR-023 — SQLite is the memory substrate; Markdown is a one-way view

**Status:** Accepted (2026-09-02)

## Why it is needed

Memory has two audiences with incompatible requirements. The Context Engine needs ranked,
filtered, joined retrieval in under 150 ms on a per-prompt path. A developer needs something
reviewable in a pull request and readable on a phone.

A single representation cannot serve both, and picking the wrong one as authoritative is
expensive to reverse — every consumer written against it has to move.

## Decision

**SQLite is the source of truth. Markdown is generated, and never read back.**

- Facts, findings and ledgers live in `.nexus/nexus.db`, as they already do.
- `nexus memory export --markdown docs/knowledge/` renders them, one file per `fact_key`
  namespace, with `[[wikilink]]` cross-references.
- **Nexus never parses `docs/knowledge/`.** The direction is one-way, permanently.
- Humans add knowledge through `nexus fact add`, entering at `source='human'` with provenance
  recorded — not by editing the export.
- Obsidian is a *viewer* pointed at that directory. The wikilink format is the entire
  integration; no plugin, no sync, no schema.

## Alternatives considered

**Markdown as the source of truth (docs-as-database).** Attractive: greppable, diffable, no
schema migrations, works with every editor. Rejected because the Context Engine's queries are
joins — facts by subject prefix, filtered on `invalidated_at IS NULL`, ranked by source and
state, intersected with the impact set, under a token budget. Serving that from Markdown means
building an index over the Markdown, which is SQLite again with a worse schema and a parser in
front of it. It also loses transactions: a half-written scan could leave the knowledge base
inconsistent with no way to detect it.

**Bidirectional sync between rows and files.** Rejected on a single argument: it makes an
unvalidated text file authoritative over an evidence-checked row. The entire memory design rests
on a claim being storable only because its evidence was checked, and a round trip through a text
editor discards that check while keeping the appearance of authority. It also introduces merge
conflicts between a database and a working tree, which has no good resolution.

**A vector store for facts.** Rejected, and deferred behind a trigger (see
[`12-non-goals.md`](../12-non-goals.md) N9). Facts are retrieved by *subject* — an FQN, a module
prefix — which is an exact structural match, not a semantic one. Embeddings would answer a
question nobody is asking, at the cost of a dependency and an index that can silently drift.

## Costs

- Knowledge is not editable in place. Correcting a fact means `nexus fact add` with the same key,
  which supersedes — more ceremony than editing a file, and deliberately so.
- The export can drift from the database between runs. It is regenerable in one command, and the
  generated header says which scan produced it.
- SQLite is a binary file: not diffable, not mergeable. Mitigated by `export`/`import` over JSON
  for the sharing case, which is the only case where diffing matters.

## The signal that should make you change it

1. **Developers routinely edit the export and expect it to stick.** That is evidence the human
   entry point is too awkward, and the fix is a better `fact add` — a TUI, an `$EDITOR` flow —
   not a parse-back path.
2. **Retrieval latency is dominated by SQLite** rather than by ranking. Unlikely at this scale,
   but it would justify a cache layer, not a different store.
3. **A team needs to review knowledge changes in pull requests as the primary workflow.** Then
   the export becomes the review artefact and the database becomes a build product — a genuine
   inversion, and it should be argued as one rather than arrived at by drift.
