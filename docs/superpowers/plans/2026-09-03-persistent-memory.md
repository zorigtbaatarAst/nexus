# Persistent Engineering Memory (roadmap 3.1 – 3.6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Memory with a lifecycle. A fact is validated by surviving scans, ages honestly, is ranked by one formula, is readable by a human as Markdown, and can be moved between machines without a server.

**Architecture:** No new table. `facts` gains three lifecycle columns; the per-scan pass that already invalidates a moved fact now also validates an intact one, using the same anchors it already computes. Retrieval moves out of SQL into one scoring function in `nexus-core::memory`, which both the ask path and the Context Engine call. Markdown and the export file are views: generated, never read back.

**Tech Stack:** Rust 1.82+, `rusqlite` (store only), `serde_json`.

**Spec:** [`06-memory.md`](../../architecture/06-memory.md) §2 (namespaces), §3 (lifecycle), §4 (retrieval formula), §5–6 (Markdown as a view), §7 (portability); [`memory-model.md`](../../memory-model.md); [ADR-023](../../architecture/decisions/ADR-023-sqlite-is-the-substrate-markdown-is-a-view.md).

## Note on this plan's form

Six tasks, condensed like the Phase 2 completion plan: design decisions and an acceptance criterion per task, code only where the obvious implementation is wrong. Each task ends with `make check` green and one commit naming its roadmap id.

## Global Constraints

- **Roadmap 3.1 through 3.6 is the scope.** Out: verification (Phase 4), any language analyzer (Phase 5), embeddings, a team server, an Obsidian plugin, any parse-back path from Markdown.
- **Nexus never reads `docs/knowledge/`.** §6's rule. A round trip through Markdown would make an unvalidated text file authoritative over an evidence-checked row, which inverts the whole design. The exporter writes; nothing reads.
- **Facts are never edited.** A new fact supersedes; the old row stays. Setting a lifecycle column is not an edit to a belief, it is recording what a scan observed about one — the same category as `invalidated_at`.
- **Invalidated rows are kept.** "What did Nexus believe at scan 12, and what changed its mind" must stay answerable.
- **One retrieval formula.** §4's five factors, in one function, called by every consumer. Two ranking functions over facts would disagree, and the one in the Context Engine is the one that would be wrong.
- **`make check` after every task.** Baseline: **272 passing tests**. No task may reduce it.
- **Only `nexus-store` contains SQL.**
- **Ledger tables stay append-only.**
- **`git add` names files**, and `Cargo.lock` is committed with any manifest change — the omission that broke a clean checkout last time.
- Commit per task, message naming the roadmap id, ending with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## Decisions taken before writing code

**1. Validation reuses the anchors invalidation already computes.** `Engine::fact_anchors` resolves every live fact's evidence before a scan rewrites the index. That same list answers both questions: an anchor that no longer holds invalidates, and one that still holds validates. Computing them twice would be two definitions of "the evidence still means what it did", and they would drift.

**2. A fact with no evidence is never validated and never invalidated.** It has no anchor, so a scan observes nothing about it. Human facts are durable on arrival, which is what §3 says, so nothing is lost. This keeps the two passes symmetrical: they act on anchors, and a fact without one is simply not their business.

**3. Durability has two independent routes, and the count is of scans, not of passes.** `source='human'` is durable at insert. Everything else needs three *distinct* scans. The pass therefore guards on `validated_scan_id <> :this_scan`, or a re-run of the same scan would promote a fact three times over.

**4. Retrieval leaves SQL.** §4's formula needs `subject_match` against the caller's seeds, which the store cannot know. `Store::facts` returns rows in a stable order; `nexus_core::memory::relevance` scores them. The ask path scores with no seeds, the Context Engine with its own. `ORDER BY source, confidence` is replaced by the formula, which is what 3.2 asks for — it just does not stay in the database.

**5. The namespace list is closed, and an unknown prefix is refused.** §2 lists ten and says `task.` is *deliberately* absent, which only means something if the list is enforced. Every fact this repository has ever written uses one of the ten, so enforcing costs nothing today and prevents the namespace becoming folklore. The error names the valid prefixes.

**6. `nexus fact` keeps its spelling and gains `--evidence`.** The roadmap writes the task as `nexus fact add`, but `nexus fact KEY CLAIM` already exists and CLI verbs are interface. Renaming buys nothing and breaks a documented one. What it *does* gain is the evidence flag, whose absence has now cost twice: a terminal-recorded fact has no anchor, so 1.6 never invalidates it and 1.7 excludes it from a session package with `no file:line anchor`. That is the actual deliverable of 3.5.

**7. Export is a file, not a protocol.** §7 and N13: `export`/`import` over a committed file is the first answer, and a status conflict is reported, never silently resolved. Evidence travels as paths and lines, never as source text, so the file is safe to commit.

---

## Tasks

### 3.1 — Lifecycle states and the per-scan validation pass

**Files:** create `crates/nexus-store/migrations/0007_fact_lifecycle.sql`; modify `nexus-store/src/lib.rs` (schema 7, `FactRow`, `validate_facts`), `nexus-core/src/engine/memory.rs`, `engine/scan.rs`, `engine/rescan.rs`, `report.rs`; tests in `nexus-core/tests/fact_invalidation.rs`.

**Deliverable.** `facts` gains `validated_scan_id`, `validated_count`, `durable`. `Store::validate_facts(tx, project_id, anchors, scan_id)` promotes intact anchors; three distinct scans, or `source='human'`, sets `durable`. Both scan paths call it beside the existing invalidation. `ScanReport`/`RescanReport` gain `facts_validated`.

**Acceptance.** A fact recorded by an agent and left alone is validated by the next scan and durable after three. A human fact is durable at insert. A fact whose evidence moved is invalidated and is *not* validated in the same pass. Re-running one scan does not promote anything twice.

### 3.2 — The full retrieval formula

**Files:** create `crates/nexus-core/src/memory.rs`; modify `nexus-store/src/lib.rs` (`facts` query and `FactRow`), `engine/query.rs`, `context/signals.rs`.

**Deliverable.** `memory::relevance(fact, seeds) -> f64` implementing §4: `subject_match × source_weight × state_weight × confidence × recency_decay`, excluding invalidated rows. State weights: durable 1.0, validated 0.85, candidate 0.6. `Engine::facts` and the Context Engine both call it; `signals.rs` uses the same `state_weight`.

**Acceptance.** A durable human fact about the exact symbol outranks a candidate AI fact about the module. Zeroing a fact's confidence removes it from the top. The same fact scores identically through both call paths — asserted, because two rankings over one table is the failure this task exists to prevent.

### 3.3 — The `fact_key` namespaces

**Files:** modify `crates/nexus-core/src/memory.rs` (the list and the check), `engine/query.rs` (`record_fact`), `engine/mod.rs` (an error variant).

**Deliverable.** The ten namespaces of §2 as a constant. `record_fact` refuses a key outside them, naming what is allowed. `task.` is refused explicitly, with the reason: task history is already `finding_occurrences`, `changes` and `scans`, and a parallel narrative log is a transcript by another name.

**Acceptance.** Each of the ten prefixes is accepted. `task.something` and `nonsense.x` are refused with a message listing the ten. No existing test's fact key breaks.

### 3.4 — `nexus memory export --markdown`

**Files:** create `crates/nexus-cli/src/memory.rs`; modify `main.rs`, `render.rs`; test in `crates/nexus-cli/tests/memory_export.rs`.

**Deliverable.** One file per namespace under a target directory, each fact a section with claim, evidence links, source, state and the scan it was learned in, plus `[[fact-key]]` wikilinks to related facts. A generated header saying the file is generated and will be overwritten.

**Acceptance.** Exporting twice is byte-identical. Every emitted file carries the header. An invalidated fact does not appear. Nothing in `crates/` ever reads the output directory — asserted by a boundary-style test that greps the workspace for a read of the export path.

### 3.5 — `nexus fact` gains evidence

**Files:** modify `crates/nexus-cli/src/main.rs`; test in `crates/nexus-cli/tests/hooks.rs` or a new `facts.rs`.

**Deliverable.** `--evidence PATH:LINE`, repeatable. A human fact still enters at `source='human'` and durable. With evidence it now participates in invalidation and appears in a session package.

**Acceptance.** A fact recorded with evidence appears in `nexus context --session` and is invalidated when its anchor moves. Without evidence it is still accepted (a human fact needs no anchor) and still excluded from the package with the stated reason.

### 3.6 — `nexus export` / `nexus import`

**Files:** create `crates/nexus-core/src/portable.rs`; modify `nexus-store/src/lib.rs`, `engine/query.rs`, `crates/nexus-cli/src/main.rs`; test in `crates/nexus-core/tests/portable.rs`.

**Deliverable.** `nexus export --facts --findings` writes one JSON document: facts by key with state and evidence references, findings by fingerprint with status and history. `nexus import <file>` merges by fact key and finding fingerprint. A status conflict is **reported and skipped**, never resolved.

**Acceptance.** Export then import into an empty project reproduces the facts and findings. Importing a fact whose key exists with a different claim reports a conflict and changes nothing. No source text appears anywhere in the file — asserted by scanning the output for a line of the fixture's code.

---

## Self-review

**Spec coverage.** §2's ten namespaces: 3.3. §3's five states and both durability routes: 3.1. §4's formula and its three weight tables: 3.2. §5–6's Markdown-as-a-view and the never-read-back rule: 3.4. §7's export/import with reported conflicts: 3.6. The human entry point of §6: 3.5.

**Deliberately not covered.** "What did Nexus believe at scan 12, and what changed its mind?" is a Phase 3 success criterion and is *answerable from the database* once 3.1 lands — the rows and their scan ids are all present. A CLI verb to ask it is not in the task list, and adding one would be scope this phase did not buy.

**Risk.** Enforcing the namespace list is the only change here that can reject input that used to be accepted. Every fact in this repository and its tests uses one of the ten, so the blast radius is a project that invented its own prefix. The error names the ten, which is the difference between a wall and a door.
