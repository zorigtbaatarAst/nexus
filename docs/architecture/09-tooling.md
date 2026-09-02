# Tooling

Twelve building blocks, evaluated. The governing rule:

> **Do not duplicate functionality a proven tool already provides.**

The corollary matters as much: *do not adopt a tool for a job it does not actually do*. Both
failures cost the same, and the second is harder to reverse because the dependency comes with
advocates.

Each entry states what the tool provides, Nexus's relationship to it, and — the column usually
omitted — **what Nexus must not build because of it**.

---

## 1. Graphify — structural extraction for languages Nexus cannot parse

**Provides:** deterministic AST extraction across many languages, free, no API key, plus
community detection and a JSON graph. Verified on this repository: `graphify update crates
--no-cluster` produced 1,056 nodes and 2,908 edges in seconds — for Rust, which Nexus cannot
index at all.

**Relationship: conditional input, not a dependency.** Where `graphify-out/graph.json` exists,
the seed and expand stages may consult it **only** for files whose language has no
`LanguageAnalyzer`. Marked `resolution = "external-graph"`, ranked below tree-sitter-derived
edges, and excluded from the resolution denominator.

**Why it is not the index.** Four properties Nexus depends on that Graphify does not provide:

| Nexus needs | Graphify |
|---|---|
| `sig_hash` / `body_hash` per symbol | absent — no change classification, so no `API_CHANGED` vs `BODY_CHANGED` |
| Symbol identity across renames | absent — a rename is a delete plus an add |
| Directed edges by default | undirected by default (`"directed": false` in the output) |
| The cross-stack GraphQL/HTTP seam | absent — it is schema-derived, not AST-derived |

**Must not build because of it:** a general-purpose multi-language AST extractor. When a
language needs *shallow* structure and no one has written an analyzer, Graphify is already there.

**Deletion condition, stated up front:** if Phase 3 lands `nexus-lang-rust`, this path becomes
dead weight for Rust and is removed. A crutch that is never discarded was a dependency all along.

---

## 2. Git — history, and nothing more

**Provides:** the complete change record. Commits, authorship, blame, diffs, churn.

**Relationship: wrapped, never reimplemented.** `nexus-vcs` (156 lines) wraps `git2` for HEAD,
dirty state and changed paths. It grows read-only history primitives — `log_since`, `numstat`,
`blame_lines` — and stops there. Derivation (churn, recency, co-change) is
`nexus-core::history`; persistence is the `commits` table, currently dead.

**Must not build:** a VCS abstraction layer, a "git provider" trait, or support for anything
that is not git. One implementation, one backend, no interface with one implementer.

**The judgement:** git is the highest-value under-used asset in the system. Everything needed for
"this module churns", "this broke before", "these two files always change together" is sitting
in `.git` and is currently read for three facts.

---

## 3. Claude Code — the primary integration, three tiers

**Provides:** MCP client, skills, slash commands, **and hooks** — the last being the one Nexus
does not use and most needs.

**Relationship:** hooks for deterministic invocation, MCP for the pull path, skills and commands
for the explicit path. Detailed in [`07-agent-integration.md`](07-agent-integration.md).

**Must not build:** an agent, a REPL, a chat UI, a session manager, or any orchestration. Claude
Code is the harness; Nexus is a thing the harness calls.

---

## 4. Codex and future agents — served by the CLI, not by a port

**Provides:** a second real integration target, and the reason agent-agnosticism must be
structural rather than aspirational.

**Relationship:** MCP today (`integrations/codex/config.toml` already ships). Hook shims when
Codex exposes them. The genuine portability guarantee is neither: it is
`nexus context --task "…" --json`, which works for an agent that does not exist yet and has never
heard of MCP.

**Must not build:** per-agent code paths inside `crates/`. `if claude_code { … }` anywhere in the
workspace is the smell the boundary tests exist to catch.

---

## 5. SQLite — the machine memory substrate, and it is already correct

**Provides:** embedded, zero-configuration, transactional, single-file storage with real indexes
and real joins. Already the substrate: 24 tables, 5 migrations, append-only ledgers.

**Relationship: keep, unchanged.** No evaluation is needed because it has already been made and
it was right. It is local by default, needs no daemon, survives a `cp`, and — the property that
matters most here — supports the **joins** on which impact, lifecycle and ranking depend. A
document store would make every one of those queries worse.

**Must not build:** an ORM, a second store, a cache layer in front of it, or a "storage backend"
abstraction. `nexus-store` is the only crate with SQL and that stays true.

**Must not add:** a vector extension, until the trigger in [`02-principles.md`](02-principles.md)
§10 fires.

---

## 6. Markdown — the human view, generated

**Provides:** the format humans read, diff, review in a PR, and read on a phone.

**Relationship: an output format, never the source of truth.**
`nexus memory export --markdown docs/knowledge/` renders facts to files. Nexus never parses them
back.

**Why the direction is one-way:** a round trip through Markdown makes an unvalidated text file
authoritative over an evidence-checked row. That inverts the entire memory design, in which a
claim is only worth storing because its evidence was checked. To *add* knowledge a human runs
`nexus fact add`, which enters at `source='human'` and records provenance.

**Must not build:** a Markdown parser, a front-matter schema, or a docs-as-database layer.

---

## 7. Obsidian — optional, and essentially free

**Provides:** a graph view, backlinks and search over a folder of Markdown.

**Relationship: zero code.** Obsidian reads a directory. The Markdown exporter already emits
`[[fact-key]]` wikilinks between related facts, which is the only Obsidian-specific thing here
and it is one string format.

**The honest assessment:** Obsidian is not an integration; it is a viewer someone may point at
the export directory. Treating it as an integration would invite a plugin, a sync layer and a
schema — none of which serve the mission.

**Must not build:** an Obsidian plugin, a vault manager, or any sync.

---

## 8. Matt Pocock Skills — expertise Nexus should feed, not replicate

**Provides** (37 skills, installed globally): `tdd` (red-green-refactor), `diagnosing-bugs` (a
structured diagnosis loop), `codebase-design` (deep-module vocabulary), `domain-modeling`
(builds `CONTEXT.md` and ADRs), `research`, `code-review`, `implement`, `to-spec`, `to-tickets`.

**Relationship: complementary, and the boundary is clean.** These are *methods* — how an
engineer should proceed. Nexus supplies *evidence* — what is true about this project. Neither
substitutes for the other, and the pairing is genuinely productive:

| Skill | What it needs that Nexus has |
|---|---|
| `diagnosing-bugs` | prior findings on the suspect region, the impact set, the regression history |
| `tdd` | what currently covers this code — real coverage, not a filename heuristic |
| `codebase-design` | actual coupling and fan-in, measured, not estimated |
| `domain-modeling` | writes `CONTEXT.md`; Nexus's `arch.*` and `decision.*` facts are exactly its raw material |

**One concrete opportunity, and one boundary.** The opportunity: `nexus memory export --markdown`
can seed `domain-modeling`'s `CONTEXT.md` — evidence-backed facts becoming a human domain model.
The boundary: **Nexus must not ship workflow skills.** The moment a context package starts
containing process instructions, Nexus has crossed from "what is true" into "how you should
work", which is C4 in [`04-future-architecture.md`](04-future-architecture.md) §1.

**Must not build:** a workflow engine, a methodology, or opinions about how to write code.

---

## 9. Superpowers — the same boundary, and one skill that is the spec

**Provides:** `brainstorming`, `systematic-debugging`, `test-driven-development`,
`writing-plans`, `requesting-code-review`, and `verification-before-completion`.

**The notable one.** `verification-before-completion` states: *"Evidence before claims, always…
requires running verification commands and confirming output before making any success claims."*

That is [`08-verification.md`](08-verification.md) written as a prose instruction to a model.
The skill asks the agent to remember and comply; **Nexus's `Stop` hook makes it structural.** A
discipline enforced by a process cannot be forgotten under time pressure, and this is the
clearest case in the whole document of Nexus and a skill doing the same job at different levels
of reliability.

**Relationship:** same as §8. Skills are agent-side; Nexus is evidence-side. Where they overlap,
Nexus makes the skill's instruction executable rather than replacing it.

**Must not build:** a competing skill system, or a skill loader.

---

## 10. Tests — behavioural verification, orchestrated

**Provides:** the only real evidence that behaviour is correct.

**Relationship: run them, record them, never replace them.** `nexus-verify` invokes the
project's own test command from the profile, under the argv allowlist. Results append to
`test_runs`; coverage populates `test_coverage`, retiring `impact::is_test` — a **path-name
string match** currently serving as the sole basis for Review's flagship "nothing tests this"
finding.

**Must not build:** a test framework, a runner, or an assertion library. Test *generation*
(deterministic templates behind `SafeWriter`) is Phase 3 and is generation, not a runner.

---

## 11. Build tools — correctness verification, detected not configured

**Provides:** compilation, the cheapest and strongest correctness signal available.

**Relationship:** `detect.rs` already identifies the build system from evidence (with the
`file:line` that proved it). `nexus-verify` derives the build command from the profile. No
configuration is asked of the developer for the common case.

**Must not build:** a build system, a build abstraction, or a toolchain manager.

---

## 12. Linters and type checkers — quality verification, never reimplemented

**Provides:** style, type and pattern checking that is already solved and already installed.

**Relationship:** invoke and record. This is a standing non-goal: *"replace linters, type
checkers or SAST"* — Nexus **orchestrates them and reasons about what they cannot express.**

The division is the table in [`ai-integration.md`](../ai-integration.md) §5, and it holds
exactly: a linter owns everything mechanically checkable; a model is asked only about properties
a compiler cannot express; and Nexus decides which of the two a given question belongs to,
before either is invoked.

**Must not build:** rules that duplicate a linter. `cap-bughunter` detects Spring proxy
mistakes, orphaned GraphQL fields and committed credentials — three things no linter checks,
because each requires the *project-wide* index that only Nexus has.

---

## Summary: build, use, refuse

| | Tool | Nexus's move |
|---|---|---|
| **Build** | index, dependency graph, the seam, finding lifecycle, context ranking, memory lifecycle | Nothing else provides these |
| **Wrap** | git (`git2`), SQLite (`rusqlite`), tree-sitter | Proven, and the wrapper is thin |
| **Invoke** | build, tests, linters, type checkers | Orchestrate, record, never reimplement |
| **Consume, conditionally** | Graphify | Only for unanalysed languages; deleted when native support lands |
| **Emit to** | Markdown, Obsidian | One-way. A view, never a source |
| **Complement** | Matt Pocock Skills, Superpowers | They own method; Nexus owns evidence. Nexus makes their instructions executable |
| **Refuse** | vector DB, ORM, agent framework, workflow engine, build system, test runner, linter | No demonstrated need, or already solved |
