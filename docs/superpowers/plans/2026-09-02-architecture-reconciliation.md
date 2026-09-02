# Architecture Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the Nexus codebase back into alignment with its own design of record — correct documents that describe things which do not exist, collapse abstractions duplicated across crates, and move orchestration to the layer the architecture already assigns it to.

**Architecture:** Nothing new is built. Every task either corrects a document, collapses a duplicate, or relocates existing behaviour. The 13-crate layering, the boundary tests, the finding lifecycle and the append-only ledgers are unchanged throughout.

**Tech Stack:** Rust 1.82+, cargo workspace, `rusqlite` (store only), `git2`, tree-sitter, `clap`, `rmcp`.

**Spec:** [`AGENTS.md`](../../../AGENTS.md) and [`CLAUDE.md`](../../../CLAUDE.md) as the design of record, with [`docs/architecture/03-current-state.md`](../../architecture/03-current-state.md) §4 and §6 as the starting inventory — **two of whose claims this reconciliation disproved (see below).**

## Global Constraints

- **The design of record is `AGENTS.md`, `CLAUDE.md` and `docs/*.md` — not `docs/architecture/`.** The latter is a Phase 1–5 plan for work that has not started. Implementing any of it is out of scope.
- **No new features.** No Context Engine, no verification engine, no hooks, no new language analyzer, no new capability, no new MCP tool, no new CLI verb.
- **`make check` must pass after every task** — `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace`. A warning fails the build.
- **Baseline: 173 passing tests.** No task may reduce that count.
- **No existing `Engine` method changes name, signature or behaviour.** Task 5 adds exactly one method, `Engine::ask`, as its explicit deliverable — relocating orchestration that already exists. Task 6 moves code between files and changes nothing a caller can observe.
- **Boundary rules are law.** `nexus-core` must not depend on any `cap-*`, `nexus-mcp` or `nexus-cli`; only `nexus-store` may contain SQL; no `nexus-lang-*` may depend on `nexus-store` or `nexus-core`; `nexus-fixtures` must not depend on any Nexus crate.
- **Ledger tables are append-only** — `scans`, `changes`, `commits`, `finding_occurrences`, `finding_verifications`, `test_runs`, `audit_events` are never `UPDATE`d.
- **`#![forbid(unsafe_code)]` everywhere**, and `deny(clippy::unwrap_used, clippy::expect_used)` outside tests in `nexus-core` and `cap-bughunter`.
- **Commit after every task**, with the task number in the message.

---

## Reconciliation findings

### The headline: there is nothing obsolete in the code

The inspection set out to find dead components. **It found none.** Two claims in
`docs/architecture/03-current-state.md` — written before this inspection — turned out to be
false, and correcting them is Task 1's most important step:

| Claim | Reality | How it was disproved |
|---|---|---|
| "Twelve of twenty-four tables are dead" | **21 tables, 9 unwired, 0 obsolete** | Opened a real database: `nexus init` in a temp repo, then `SELECT name FROM sqlite_master` |
| "`bugs*` (four tables) is legacy … should be dropped, not carried" | **They do not exist.** Migration `0003` renamed them to `findings`, `finding_occurrences`, `finding_verifications`, `finding_relations` and dropped the originals | `tail -30 crates/nexus-store/migrations/0003_findings.sql`: `DROP TABLE findings; ALTER TABLE findings_new RENAME TO findings;` |

The original claim came from grepping `nexus-store/src/lib.rs` for `INTO bugs` / `FROM bugs`
and finding zero. Zero references to a table that was never created reads exactly like zero
references to a dead one. **The lesson is in the method: schema questions get answered against
a database, not against a grep.**

The nine unwired tables — `external_deps`, `commits`, `tests`, `test_coverage`, `test_runs`,
`ui_strings`, `audit_events`, `finding_verifications`, `finding_relations` — are all
designed-but-unbuilt subsystems. `docs/roadmap.md` states the reason: they exist from day one
so adding them later does not mean migrating a populated database. **They stay.**

### Drift — the code is right, the documents are wrong

| | Finding | Evidence |
|---|---|---|
| **D1** | `AGENTS.md:21` claims "**Status: architecture only.** `docs/` is complete; no code exists." | 16.6k lines of Rust, 173 tests |
| **D2** | `AGENTS.md:11` claims the directory and repository are "still called `bughunter`" | Both are `nexus`; only `Engine::migrate_legacy_dir` remains |
| **D3** | `AGENTS.md` constraints 1 and 5 govern `nexus-ai` and `nexus-verify` | Neither crate exists |
| **D4** | `docs/roadmap.md:28` says "sixteen tools"; `:43` says "all 20 tables" | 19 tools; 21 tables. `docs/data-model.md:25` says 21 and is correct |
| **D5** | `docs/testing-strategy.md:99-112` places fixtures at `tests/fixtures/<name>/` | Generated into `target/fixtures/` from specs at `tests/fixtures/specs/` |
| **D6** | `docs/verification-engine.md`, `docs/memory-model.md` §3, `docs/ai-integration.md` describe subsystems with no status marker | `nexus-verify`, `nexus_core::context`, `nexus-ai` do not exist |
| **D7** | `crates/nexus-core/src/project.rs:1` opens "//! Deterministic detectors." | The module defines `ProjectContext`, `Scoped`, `SymbolFacts` |
| **D8** | `docs/architecture/03-current-state.md` P2 and §6 assert dead tables that do not exist | Disproved above |

### Required refactors — the code disagrees with itself

| | Finding | Evidence | Task |
|---|---|---|---|
| **R1** | The capability rule trait is defined three times with the same shape | `cap-architect::rules::Rule`, `cap-bughunter::detectors::Detector`, `cap-review::rules::Rule` | 3 |
| **R2** | The capability list is written out three times and nothing detects drift | `nexus-cli::open:611`, `nexus-cli::open_or_init:619`, `nexus-mcp::with_engine:163` | 2 |
| **R3** | `assert_forbidden` silently passes when its `from` crate is absent | `boundaries.rs`: `if let Some(deps) = graph.get(from)` with no `else` | 2 |
| **R4** | `ask::suggest` issues two `Engine` calls per changed symbol, up to 40 — 80 traversals per question | `ask.rs:144-153` | 5 |
| **R5** | Orchestration lives in an adapter, against the repository's own stated rule | `CLAUDE.md`: "If an MCP handler needs two `Engine` calls, the missing method belongs in `nexus-core`" | 5 |
| **R6** | `nexus-vcs` has zero tests | `grep -c '#\[test\]' crates/nexus-vcs` → 0 | 4 |
| **R7** | `engine.rs` is 2,069 lines: `rescan` 522, `analyze` 239 | Function-offset analysis | 6 |

### Explicitly out of scope

- The nine unwired tables, and `impact::is_test` — wiring either needs a subsystem that does not exist.
- `nexus-core`'s direct dependency on the three `nexus-lang-*` crates (Phase 5.1) and `ProjectContext` materialising before narrowing (Phase 5.4).

---

## File Structure

**Created**

| File | Responsibility |
|---|---|
| `crates/nexus-core/src/rules.rs` | The shared `Rule` trait and the in-memory `Graph`, hoisted from the three capabilities |
| `crates/nexus-core/src/engine/{mod,scan,rescan,analyze,query}.rs` | `engine.rs` split by responsibility; public API unchanged |

**Modified**

| File | Change |
|---|---|
| `AGENTS.md`, `CLAUDE.md`, `docs/roadmap.md`, `docs/testing-strategy.md`, `docs/verification-engine.md`, `docs/memory-model.md`, `docs/ai-integration.md`, `docs/architecture/03-current-state.md` | D1–D8 |
| `crates/nexus-core/src/project.rs` | D7 module doc |
| `crates/nexus-core/src/lib.rs` | Export `rules`; `engine` becomes a directory module |
| `crates/nexus-cli/src/main.rs` | R2 registration helper; R5 `ask` call site |
| `crates/nexus-cli/tests/boundaries.rs` | R3 hardening; capability-parity test |
| `crates/cap-{architect,bughunter,review}/src/{rules,detectors}/mod.rs` and `src/lib.rs` | R1 |
| `crates/nexus-vcs/src/lib.rs` | R6 test module |
| `crates/nexus-cli/src/ask.rs` | R4, R5: becomes a renderer over one `Engine` call |
| `crates/nexus-core/src/report.rs` | R5: `Answer`, `Affected`, `Suggestion` move here |

**Deleted**

| File | Reason |
|---|---|
| `crates/nexus-core/src/engine.rs` | Replaced by `engine/` (Task 6) |

---

## Task 1: Reconcile the documents with the code

`AGENTS.md` is the first thing an agent reads and it opens by saying no code exists. That is the single most expensive inaccuracy in the repository.

**Files:**
- Modify: `AGENTS.md`, `CLAUDE.md`, `docs/roadmap.md`, `docs/testing-strategy.md`, `docs/verification-engine.md`, `docs/memory-model.md`, `docs/ai-integration.md`, `docs/architecture/03-current-state.md`
- Modify: `crates/nexus-core/src/project.rs:1-8`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. Only a module doc comment changes inside `crates/`.

- [ ] **Step 1: Fix the status claim in `AGENTS.md`**

Replace lines 21–23 (`**Status: architecture only.** … are not.`) with:

```markdown
**Status: the MVP ships.** Thirteen crates, ~16.6k lines, 173 tests. `scan`, `rescan`,
`status`, `changes`, `impact`, `graph`, `ask`, `analyze` and `doctor` work; Java, TypeScript
and GraphQL are indexed; three capabilities run. Still absent, and every surface says so
rather than leaving anyone to infer it: the verification engine, Python and Rust analyzers,
and any direct LLM provider. Do not start implementing outside the scope in
[`docs/roadmap.md`](docs/roadmap.md) — the design deliberately defers things that look easy
and are not.
```

- [ ] **Step 2: Fix the rename claim in `AGENTS.md`**

Replace lines 11–14 (`The directory and the GitHub repository are still called … not an oversight.`) with:

```markdown
The rename to `nexus` is complete: the directory, the repository and the crates all carry it.
One thing survives on purpose — `Engine::migrate_legacy_dir` moves a `.bughunter/` directory
to `.nexus/` on first open, so a project indexed before the rename is not silently re-scanned
from nothing.
```

- [ ] **Step 3: Mark the two constraints governing crates that do not exist**

Do **not** delete them: they are the design, and deleting a rule loses it. Append to
`AGENTS.md` constraint 1:

```markdown
   *`nexus-ai` does not exist yet. The rule is enforced today in its stronger form: a
   `cargo metadata` test asserts `nexus-core` depends on no HTTP client at all.*
```

Append to constraint 5:

```markdown
   *`nexus-verify` does not exist yet. `docs/verification-engine.md` is its design; the rule
   binds when the crate lands, and `tests/boundaries.rs` already names it so that it cannot
   land without one.*
```

- [ ] **Step 4: Correct the two counts in `docs/roadmap.md`**

Line 28: `sixteen tools` → `nineteen tools`.
Line 43: `full schema, all 20 tables, migrations` → `full schema, all 21 tables, migrations`.

`docs/data-model.md:25` already says 21 and is correct — this makes the two agree.

- [ ] **Step 5: Correct the fixture location in `docs/testing-strategy.md`**

Replace the paragraph and code block at lines 99–112 with:

```markdown
The backbone. Four small but *real* projects, each a genuine git repository with a scripted
history that plants a specific bug at a known commit.

They are **generated, not committed** — `tests/fixtures/specs/<name>/` holds a declarative
specification, and `make fixtures` builds the repositories into `target/fixtures/`. Generation
is deterministic: the same specification produces the same commit shas on any machine, which
`make fixtures-verify` asserts in CI. See [`tests/fixtures/README.md`](../tests/fixtures/README.md).

    tests/fixtures/specs/spring-payments/   Java 21 · Spring Boot · JPA · GraphQL
    tests/fixtures/specs/next-storefront/   Spring GraphQL API + Next.js, one repository
    tests/fixtures/specs/acme-monorepo/     three Gradle modules over one shared library
    tests/fixtures/specs/legacy-billing/    three plausible invoice calculators, one live
```

Leave the numbered assertion list that follows unchanged — those assertions are still what
the fixtures exist to support.

- [ ] **Step 6: Add a status marker to the three documents describing unbuilt subsystems**

Insert immediately after the H1 of each.

`docs/verification-engine.md`:

```markdown
> **Status: designed, not built.** No `nexus-verify` crate exists. This document is the
> specification the crate must satisfy; nothing described here runs today.
```

`docs/memory-model.md`:

```markdown
> **Status: partly built.** The `facts` table, supersession and evidence-checked recording
> work. §3's `ContextBuilder` in `nexus-core::context` does not exist: retrieval today is
> `ORDER BY source, confidence` with no subject weighting, recency decay or token budget.
> Invalidation-by-change is specified in §2 rule 3 and is not implemented.
```

`docs/ai-integration.md`:

```markdown
> **Status: designed, not built.** No `nexus-ai` crate and no provider exists. The one live
> path is agent-as-provider over MCP — `nexus_record_finding` and `nexus_record_fact` — which
> needs no provider code at all. §5's table is nevertheless in force today: it is what decides
> which questions a rule answers and which are left to a model.
```

- [ ] **Step 7: Correct the two false claims in `docs/architecture/03-current-state.md`**

This document asserted dead tables that do not exist. Replace the P2 heading and its first
paragraph (`### P2 — Twelve of twenty-four tables are dead` through the table list) with:

```markdown
### P2 — Nine of twenty-one tables are unwired

Never inserted into, never read:

`external_deps` · `commits` · `tests` · `test_coverage` · `test_runs` · `ui_strings` ·
`audit_events` · `finding_verifications` · `finding_relations`

**None of them is obsolete.** Each is a designed subsystem with no code yet, and
`docs/roadmap.md` states the reason they exist early: adding them later would mean migrating a
populated database. They stay.

An earlier draft of this document claimed twelve dead tables including a legacy `bugs*` set.
That was wrong. Migration `0003` renamed `bugs` → `findings` and dropped the originals, so
those four tables have never existed in any database this code creates. The error came from
grepping `nexus-store/src/lib.rs` for `FROM bugs` and reading zero hits as "dead" rather than
as "absent" — schema questions are answered against a database, not against a grep.
```

Delete the `| `bugs*` — 4 dead legacy tables |` row from the §6 debt ledger, and change the
row below it to read `| 9 unwired tables … |`.

- [ ] **Step 8: Fix the module doc on `project.rs`**

Replace lines 1–8 of `crates/nexus-core/src/project.rs` — currently a copy of the detectors
module's header — with:

```rust
//! The prepared project snapshot a capability is handed.
//!
//! `ProjectContext` is the whole index as plain data: symbols, edges, files, what changed in
//! the scan under analysis, and the detected profile. A capability reads it and returns
//! findings; it never touches storage, git or the CLI, which is what lets `nexus-core` decide
//! whether a finding is new, recurring, fixed or regressed without every capability
//! re-implementing the answer.
//!
//! `Scoped` is that snapshot narrowed to what was asked for. Narrowing happens once, here,
//! rather than in each rule: a rule that reaches past `scoped` to `ctx` is doing something
//! deliberate, and one that forgets to narrow makes a targeted analysis quietly cost what a
//! full one costs.
```

- [ ] **Step 9: Remove the now-redundant stale-notes paragraph from `CLAUDE.md`**

Steps 1 and 2 fixed both at the source. In `CLAUDE.md:9-13`, delete the sentence beginning
`Two things in it are now stale:` through `Engine::migrate_legacy_dir).`, leaving:

```markdown
[`AGENTS.md`](AGENTS.md) is the long-form design briefing — the invariants, the deliberate
oddities, and the traps that cost real debugging time to find. Read it before changing
anything in `crates/`.
```

- [ ] **Step 10: Verify every claim this task asserts**

Run:

```bash
grep -c '#\[tool' crates/nexus-mcp/src/lib.rs                     # expect 19
grep -rn "architecture only\|no code exists" AGENTS.md CLAUDE.md  # expect no output
ls crates/nexus-ai crates/nexus-verify 2>&1 | grep -c "No such"   # expect 2
ls tests/fixtures/specs                                            # expect the four names
grep -rn "bugs\*\|Twelve of twenty-four" docs/architecture/        # expect no output
```

- [ ] **Step 11: Run the suite**

Run: `make check`
Expected: PASS, 173 tests.

- [ ] **Step 12: Commit**

```bash
git add AGENTS.md CLAUDE.md docs/ crates/nexus-core/src/project.rs
git commit -m "docs: reconcile the design of record with what is built (task 1)"
```

---

## Task 2: One capability list, and a boundary guard that cannot go quiet

Two problems that share a subject. The capability list is written out three times — twice inside `nexus-cli` alone — and nothing notices when the copies drift. Separately, `assert_forbidden` silently passes when the crate it is asked about is not in the graph, so renaming a crate would turn every rule about it into a no-op with no failing test.

**Files:**
- Modify: `crates/nexus-cli/src/main.rs:607-623`
- Modify: `crates/nexus-cli/tests/boundaries.rs:39-47` and append
- Test: `crates/nexus-cli/tests/boundaries.rs`

**Interfaces:**
- Consumes: `nexus_core::Engine::register_capability`, `Engine::capability_list`.
- Produces: `fn register_capabilities(engine: &mut Engine)` in `nexus-cli::main` — private, used by `open` and `open_or_init`.

- [ ] **Step 1: Write the failing parity test**

Append to `crates/nexus-cli/tests/boundaries.rs`:

```rust
/// The CLI and the MCP server are both composition roots, by design — AGENTS.md constraint 0.
/// Two roots means two lists, and a capability added to one and forgotten in the other is
/// invisible: the CLI would run it and an agent would be told it does not exist.
#[test]
fn both_composition_roots_register_the_same_capabilities() {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();

    // Each root names its capabilities in `register_capability` calls. Comparing the sets
    // rather than the text keeps this robust against import aliases: nexus-mcp imports
    // `cap_bughunter::BugHunter as BugHunterCapability`.
    let names = |src: &str| -> std::collections::BTreeSet<String> {
        src.lines()
            .filter(|l| l.contains("register_capability"))
            .filter_map(|l| l.split("Box::new(").nth(1))
            .filter_map(|l| l.split("::new()").next())
            .map(|n| n.trim_end_matches("Capability").to_string())
            .collect()
    };

    let cli = names(&read("src/main.rs"));
    let mcp = names(&read("../nexus-mcp/src/lib.rs"));

    assert!(!cli.is_empty(), "the CLI must register capabilities");
    assert!(!mcp.is_empty(), "the MCP server must register capabilities");
    assert_eq!(
        cli, mcp,
        "the two composition roots disagree. A capability the CLI runs but MCP does not is \
         one an agent cannot reach, and nothing else in the build would catch it."
    );
}
```

- [ ] **Step 2: Run it and watch it pass — then confirm it can fail**

Run: `cargo test -p nexus-cli --test boundaries both_composition_roots`
Expected: PASS (both currently list the same three).

Now prove the test has teeth. Temporarily delete the `ReviewCapability` line from
`crates/nexus-mcp/src/lib.rs:165` and re-run.
Expected: FAIL — `the two composition roots disagree`.
**Restore the line.** A test that cannot fail is not a test.

- [ ] **Step 3: Collapse the CLI's two copies**

In `crates/nexus-cli/src/main.rs`, replace both `open` and `open_or_init` with:

```rust
/// The composition root: this is the one place in the CLI that knows both the platform and
/// which capabilities exist. Nexus never compiles a capability in; it is handed them here.
///
/// `nexus-mcp` is the other root and keeps its own list, because it must — a handler cannot
/// reach into the CLI. `boundaries.rs` asserts the two agree.
fn register_capabilities(engine: &mut Engine) {
    engine.register_capability(Box::new(BugHunter::new()));
    engine.register_capability(Box::new(Architect::new()));
    engine.register_capability(Box::new(Review::new()));
}

fn open(root: &std::path::Path) -> Result<Engine, EngineError> {
    let mut engine = Engine::open(root)?;
    register_capabilities(&mut engine);
    Ok(engine)
}

fn open_or_init(root: &std::path::Path) -> Result<(Engine, bool), EngineError> {
    let (mut engine, fresh) = Engine::open_or_init(root)?;
    register_capabilities(&mut engine);
    Ok((engine, fresh))
}
```

- [ ] **Step 4: Run the suite**

Run: `cargo test -p nexus-cli`
Expected: PASS. The parity test still passes — it counts distinct names, and three lines in
one function read the same as three lines in two.

- [ ] **Step 5: Write the failing test for the silent boundary guard**

Append to `crates/nexus-cli/tests/boundaries.rs`:

```rust
/// Every rule names a `from` crate that must exist.
///
/// `assert_forbidden` skips when `from` is absent from the graph, which is right for a `to`
/// that has not been built yet — `nexus-verify` is named as a forbidden target on purpose.
/// It is wrong for a `from`: rename a crate and every rule about it stops checking anything,
/// with a green build. This test is what makes that impossible.
#[test]
fn every_guarded_crate_is_actually_in_the_workspace() {
    let g = dependency_graph();
    for from in [
        "nexus-core",
        "nexus-mcp",
        "nexus-cli",
        "nexus-store",
        "nexus-fixtures",
        "cap-bughunter",
        "cap-architect",
        "cap-review",
    ] {
        assert!(
            g.contains_key(from),
            "`{from}` is named as the subject of a boundary rule but is not in the workspace. \
             Either it was renamed — in which case the rules about it are silently inert — or \
             the rule is stale and should be deleted."
        );
    }
}
```

- [ ] **Step 6: Run it and prove it can fail**

Run: `cargo test -p nexus-cli --test boundaries every_guarded_crate`
Expected: PASS.

Temporarily add `"nexus-verify"` to the list and re-run.
Expected: FAIL — ``` `nexus-verify` is named as the subject of a boundary rule but is not in the workspace ```
**Remove it again** — `nexus-verify` is a legitimate forbidden *target*, not a subject.

- [ ] **Step 7: Run the full suite**

Run: `make check`
Expected: PASS, 175 tests (173 + 2).

- [ ] **Step 8: Commit**

```bash
git add crates/nexus-cli/src/main.rs crates/nexus-cli/tests/boundaries.rs
git commit -m "cli: one capability list per root, and a boundary guard that cannot go quiet (task 2)"
```

---

## Task 3: Hoist the rule abstraction into the core

Three capabilities, three declarations of the same trait, three `all()` functions, and one `Graph` that `cap-review` owns but every rule asking "who depends on this?" needs. A fourth capability would make it four.

**Files:**
- Create: `crates/nexus-core/src/rules.rs`
- Modify: `crates/nexus-core/src/lib.rs` (add `pub mod rules;`)
- Modify: `crates/cap-review/src/rules/mod.rs` (delete `Rule` and `Graph`), `crates/cap-review/src/lib.rs`
- Modify: `crates/cap-architect/src/rules/mod.rs`, `crates/cap-architect/src/lib.rs`
- Modify: `crates/cap-bughunter/src/detectors/mod.rs`, `crates/cap-bughunter/src/lib.rs`
- Modify: `crates/cap-review/src/rules/{coverage,fanout,seam}.rs`, `crates/cap-architect/src/rules/{scaffolding,scope,tooling}.rs`, `crates/cap-bughunter/src/detectors/{graphql,secrets,spring}.rs` (import path and one signature each)

**Interfaces:**
- Consumes: `nexus_core::project::{ProjectContext, Scoped}`, `nexus_core::findings::Finding`.
- Produces:
  ```rust
  // nexus_core::rules
  pub trait Rule: Send + Sync {
      fn id(&self) -> &'static str;
      fn describe(&self) -> &'static str;
      fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>, graph: &Graph<'_>) -> Vec<Finding>;
  }
  pub struct Graph<'a> { /* … */ }
  impl<'a> Graph<'a> {
      pub fn of(ctx: &'a ProjectContext<'a>) -> Self;
      pub fn dependents_of(&self, fqn: &str) -> &[&'a str];
      pub fn reachable_from(&self, fqn: &'a str, max_depth: usize) -> HashSet<&'a str>;
      pub fn is_changed(&self, fqn: &str) -> bool;
  }
  ```

- [ ] **Step 1: Create the shared module**

Create `crates/nexus-core/src/rules.rs`. Move the `Graph` struct and its `impl` **verbatim**
from `crates/cap-review/src/rules/mod.rs` — do not rewrite it; it is correct and its comments
record why it is bounded.

```rust
//! The rule abstraction every capability shares.
//!
//! A capability owns rules; the platform owns everything else. That split was already the
//! design — it was just declared three times, once per capability, with the same shape and
//! the same doc comment. This is the one declaration.
//!
//! `Graph` lives here for the same reason. Every capability asking "who depends on this?"
//! needs reverse adjacency, and two implementations of that question can disagree about the
//! same symbol. It is the in-memory counterpart to `impact::run`, which answers the same
//! question against the store for callers that have one.

use crate::findings::Finding;
use crate::project::{ProjectContext, Scoped};
use std::collections::{HashMap, HashSet};

pub trait Rule: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new finding.
    fn id(&self) -> &'static str;

    fn describe(&self) -> &'static str;

    /// `scoped` is the narrowed view; `ctx` is the whole project, for the rules that
    /// genuinely need to look past the scope — a self-invocation needs the callee's
    /// annotations even when the callee itself was not asked about. `graph` is reverse
    /// adjacency, built once per analysis rather than once per rule.
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>, graph: &Graph<'_>)
        -> Vec<Finding>;
}

// … `pub struct Graph<'a>` and `impl<'a> Graph<'a>` moved verbatim from
// crates/cap-review/src/rules/mod.rs, including every doc comment.
```

Then move `cap-review`'s two `Graph` unit tests, if it has any, into this file's `mod tests`.

- [ ] **Step 2: Export it**

In `crates/nexus-core/src/lib.rs`, add `pub mod rules;` to the module list, in alphabetical
position between `pub mod report;` and `pub mod walk;`.

- [ ] **Step 3: Build and watch the capabilities fail**

Run: `cargo build --workspace`
Expected: PASS. Nothing uses the new module yet.

Run: `cargo test -p nexus-core rules`
Expected: PASS — the moved `Graph` tests.

- [ ] **Step 4: Convert `cap-review` first — it needs no signature change**

In `crates/cap-review/src/rules/mod.rs`, delete the local `trait Rule` and the whole `Graph`
struct and impl, and replace the imports with:

```rust
pub use nexus_core::rules::{Graph, Rule};
```

`pub use` rather than `use`, so `rules::Graph::of(ctx)` in `lib.rs` keeps working unchanged.

Run: `cargo test -p cap-review`
Expected: PASS, 11 tests. `cap-review`'s rules already take `graph`.

- [ ] **Step 5: Convert `cap-architect`**

In `crates/cap-architect/src/rules/mod.rs`, delete the local `trait Rule` and add:

```rust
pub use nexus_core::rules::{Graph, Rule};
```

In each of `scaffolding.rs`, `scope.rs`, `tooling.rs`, change the `run` signature:

```rust
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>) -> Vec<Finding> {
```

to:

```rust
    fn run(
        &self,
        ctx: &ProjectContext<'_>,
        scoped: &Scoped<'_>,
        _graph: &Graph<'_>,
    ) -> Vec<Finding> {
```

and add `Graph` to each file's `use super::{...}` list.

In `crates/cap-architect/src/lib.rs`, build the graph once before the rule loop:

```rust
        let scoped = ctx.scoped(scope);
        // One reverse-adjacency pass per analysis rather than one per rule. Architect's rules
        // do not use it today; the cost is a single HashMap build over the edge list, paid
        // only after the `Scope::Everything` early return above.
        let graph = rules::Graph::of(ctx);
        let mut out = Vec::new();
        for rule in rules::all() {
            out.extend(rule.run(ctx, &scoped, &graph));
        }
```

Run: `cargo test -p cap-architect`
Expected: PASS, 13 tests.

- [ ] **Step 6: Convert `cap-bughunter`**

Same shape, plus a rename: the trait is called `Detector` there. In
`crates/cap-bughunter/src/detectors/mod.rs`, delete `trait Detector` and add:

```rust
// The trait was called `Detector` here and `Rule` in the other two capabilities, for the
// same shape. One name; the alias keeps `detectors::all()` reading naturally at its call site.
pub use nexus_core::rules::{Graph, Rule};
pub use nexus_core::rules::Rule as Detector;
```

Change `pub fn all() -> Vec<Box<dyn Detector>>` to `Vec<Box<dyn Rule>>`, and give each of
`spring.rs`, `graphql.rs`, `secrets.rs` the `_graph: &Graph<'_>` parameter and the `Graph`
import, exactly as in Step 5. Add the same `let graph = detectors::Graph::of(ctx);` line to
`crates/cap-bughunter/src/lib.rs` before its rule loop.

Note: `cap-bughunter` has no early return, so it pays the graph build on every analysis. On
the measured 40,000-edge monorepo that is one `HashMap` build. If it ever shows up in a
profile, the answer is a lazily-built graph, not three trait declarations.

Run: `cargo test -p cap-bughunter`
Expected: PASS, 21 tests.

- [ ] **Step 7: Prove the abstraction is gone rather than merely unused**

Run:

```bash
grep -rn "trait Rule\|trait Detector" crates/
```

Expected: exactly one line — `crates/nexus-core/src/rules.rs`.

```bash
grep -rn "struct Graph" crates/
```

Expected: exactly one line — `crates/nexus-core/src/rules.rs`.

- [ ] **Step 8: Run the full suite**

Run: `make check`
Expected: PASS, 175 tests. Boundary tests still pass: `nexus-core` gained no dependency, and
no `cap-*` gained one.

- [ ] **Step 9: Commit**

```bash
git add crates/nexus-core/src/rules.rs crates/nexus-core/src/lib.rs crates/cap-*/src
git commit -m "core: one Rule trait and one Graph, hoisted from three capabilities (task 3)"
```

---

## Task 4: Tests for `nexus-vcs`

The one crate in the workspace with zero tests, and the foundation every history question stands on. It is 156 lines, so this is cheap; it is also where a wrong answer is silent — `is_dirty` returning `false` on a dirty tree would make `rescan` short-circuit and report nothing changed.

**Files:**
- Modify: `crates/nexus-vcs/src/lib.rs` (append a `mod tests`)
- Modify: `crates/nexus-vcs/Cargo.toml` (add a `[dev-dependencies]` section)

**Interfaces:**
- Consumes: `git2`, already a dependency.
- Produces: nothing. Tests only.

- [ ] **Step 1: Add the dev-dependency**

`crates/nexus-vcs/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

`tempfile` is already in `Cargo.lock` via `nexus-fixtures`, so this adds nothing to the build
graph and nothing to the release binary.

**Not** `nexus-fixtures`: a boundary test forbids anything but the composition root depending
on it, and a crate that built its test repositories with the fixture generator could not be
tested without it.

- [ ] **Step 2: Write the failing tests**

Append to `crates/nexus-vcs/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A repository with one commit, built with git2 directly.
    ///
    /// Deliberately not the fixture generator: `nexus-vcs` sits below it in the stack, and a
    /// test that reached upward for its input would invert the layering it is testing.
    fn repo_with_one_commit() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().to_path_buf();
        let repo = git2::Repository::init(&path).expect("init");
        std::fs::write(path.join("a.txt"), "one\n").expect("write");

        let mut index = repo.index().expect("index");
        index.add_path(std::path::Path::new("a.txt")).expect("add");
        index.write().expect("write index");
        let tree = repo.find_tree(index.write_tree().expect("tree")).expect("find");
        let sig = git2::Signature::new("T", "t@example.invalid", &git2::Time::new(1_700_000_000, 0))
            .expect("sig");
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "one", &tree, &[])
            .expect("commit");
        (dir, path, oid.to_string())
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_none_not_an_error() {
        let dir = tempfile::tempdir().expect("tmp");
        assert!(
            Repo::discover(dir.path()).is_none(),
            "a project without git is a supported configuration, not a failure"
        );
    }

    #[test]
    fn head_is_the_commit_that_was_just_made() {
        let (_d, path, sha) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        assert_eq!(repo.head_sha().expect("head"), Some(sha));
    }

    #[test]
    fn an_empty_repository_has_no_head_and_that_is_not_a_failure() {
        let dir = tempfile::tempdir().expect("tmp");
        git2::Repository::init(dir.path()).expect("init");
        let repo = Repo::discover(dir.path()).expect("discovered");
        assert_eq!(
            repo.head_sha().expect("an unborn branch is not an error"),
            None
        );
    }

    #[test]
    fn an_untracked_file_makes_the_tree_dirty() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        assert!(!repo.is_dirty().expect("clean"), "nothing has changed yet");

        // Untracked, not modified: this is the case a status check that only looks at
        // tracked files gets wrong, and it is the common one — a new source file.
        std::fs::write(path.join("new.txt"), "x\n").expect("write");
        assert!(
            repo.is_dirty().expect("dirty"),
            "an untracked file means the commit sha alone cannot identify the working state"
        );
    }

    #[test]
    fn a_modified_tracked_file_makes_the_tree_dirty() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        std::fs::write(path.join("a.txt"), "two\n").expect("write");
        assert!(repo.is_dirty().expect("dirty"));
    }

    #[test]
    fn short_sha_is_seven_characters_and_survives_a_shorter_input() {
        let (_d, _p, sha) = repo_with_one_commit();
        assert_eq!(Repo::short_sha(&sha).len(), 7);
        // A truncated sha must not panic on the slice — this is the guard, not a formality.
        assert_eq!(Repo::short_sha("abc"), "abc");
        assert_eq!(Repo::short_sha(""), "");
    }

    #[test]
    fn an_unreachable_baseline_is_an_error_that_names_itself() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        let missing = "0".repeat(40);
        assert!(!repo.is_reachable(&missing));

        let err = repo
            .changed_paths_since(&missing)
            .expect_err("a force-push or a shallow clone must be reported, not guessed around");
        assert!(
            matches!(err, VcsError::Unreachable(_)),
            "the caller has to be able to tell this apart from a git failure: {err}"
        );
    }

    #[test]
    fn changed_paths_reports_a_new_file_and_a_deletion() {
        let (_d, path, base) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        std::fs::write(path.join("b.txt"), "two\n").expect("write");
        std::fs::remove_file(path.join("a.txt")).expect("remove");

        let diff = repo.changed_paths_since(&base).expect("diff");
        assert!(diff.changed.contains("b.txt"), "an added file is a change: {diff:?}");
        assert!(
            diff.deleted.contains("a.txt") || diff.changed.contains("a.txt"),
            "a deleted file must appear somewhere in the diff: {diff:?}"
        );
    }
}
```

- [ ] **Step 3: Run them**

Run: `cargo test -p nexus-vcs`
Expected: PASS, 8 tests. If `changed_paths_since` classifies a deletion differently from the
last assertion's expectation, **read `PathDiff`'s population code and tighten the assertion to
what it actually does** — do not loosen it further.

- [ ] **Step 4: Full suite and commit**

Run: `make check`
Expected: PASS, 183 tests (175 + 8).

```bash
git add crates/nexus-vcs
git commit -m "vcs: tests for the crate every history question stands on (task 4)"
```

---

## Task 5: Move `ask` orchestration into the Engine

`CLAUDE.md` states the rule: *"If an MCP handler needs two `Engine` calls, the missing method belongs in `nexus-core`."* `ask.rs` makes up to eighty. It also lives in an adapter, which is why an agent over MCP cannot ask the most context-shaped question the codebase has — *what should I look at next?*

Moving it removes an N+1 and puts the code where the architecture already says it goes. **It adds no MCP tool and no CLI verb** — exposing it over MCP would be a new feature and is not part of this task.

**Files:**
- Modify: `crates/nexus-core/src/report.rs` (add `Answer`, `Affected`, `Suggestion`, `Question`)
- Modify: `crates/nexus-core/src/engine.rs` (add `Engine::ask`)
- Modify: `crates/nexus-cli/src/ask.rs` (becomes verb parsing plus rendering)
- Modify: `crates/nexus-cli/src/main.rs:540`

**Interfaces:**
- Consumes: `Engine::changes`, `Engine::impact`, `Engine::findings`, `Engine::facts`, `Engine::status`.
- Produces:
  ```rust
  // nexus_core::report
  pub enum Question { Changed, Affected(String), Known(String), Facts, Next }
  // nexus_core::report — Answer, Affected, Suggestion move here unchanged from ask.rs
  // nexus_core::Engine
  pub fn ask(&self, q: &Question) -> Result<Answer>;
  ```

- [ ] **Step 1: Capture the current output as the contract**

Before changing anything, record what `ask next` prints on a real project, so the refactor can
be proved output-identical:

```bash
cargo run -q --bin nexus -- --project . ask next --json > /tmp/ask-before.json
cargo run -q --bin nexus -- --project . ask facts --json >> /tmp/ask-before.json
```

- [ ] **Step 2: Move the types into `nexus-core::report`**

Move `Answer`, `Affected` and `Suggestion` from `crates/nexus-cli/src/ask.rs` into
`crates/nexus-core/src/report.rs` **unchanged** — same fields, same `#[serde]` attributes,
same variant names. The JSON must not move. Add alongside them:

```rust
/// A question an agent or a person actually asks, as a value rather than a string.
///
/// Verb parsing stays in the CLI: `"what-changed"` and `"changed"` are the same question
/// spelled two ways, and which spellings a surface accepts is that surface's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    Changed,
    Affected(String),
    Known(String),
    Facts,
    Next,
}
```

- [ ] **Step 3: Add `Engine::ask`, and fix the N+1 while moving it**

Add to `crates/nexus-core/src/engine.rs`, next to the other query methods:

```rust
    /// The questions a person or an agent actually has, each answered from the index.
    ///
    /// This lives here rather than in an adapter because every answer needs more than one
    /// query, and the rule in CLAUDE.md is that a caller needing two `Engine` calls has found
    /// a missing `Engine` method.
    pub fn ask(&self, q: &Question) -> Result<Answer> {
        match q {
            Question::Changed => Ok(Answer::Changed {
                since: self.status()?.baseline.and_then(|b| b.scan_uid),
                symbols: self
                    .changes(Some("symbol"))?
                    .into_iter()
                    .filter_map(|(_, _, t, _)| t)
                    .collect(),
                files: self.changes(Some("file"))?.len(),
            }),

            // "What is affected by this change?" and "Where is this symbol used?" are the
            // same traversal asked from two directions, so they share an answer.
            Question::Affected(target) => {
                let query = ImpactQuery {
                    target: target.clone(),
                    direction: impact::Direction::Reverse,
                    ..Default::default()
                };
                match self.impact(&query)? {
                    Resolved::One(r) => Ok(Answer::Affected {
                        target: target.clone(),
                        crossed_seam: r.crossed_seam,
                        symbols: r
                            .items
                            .into_iter()
                            .map(|i| Affected {
                                fqn: i.fqn,
                                score: i.score,
                                min_confidence: i.min_confidence,
                            })
                            .collect(),
                    }),
                    _ => Ok(Answer::Affected {
                        target: target.clone(),
                        symbols: Vec::new(),
                        crossed_seam: 0,
                    }),
                }
            }

            Question::Known(target) => Ok(Answer::Known {
                findings: self.findings_for(target)?,
                facts: self.facts(Some(target))?,
                target: target.clone(),
            }),

            Question::Facts => Ok(Answer::Facts {
                facts: self.facts(None)?,
            }),

            Question::Next => Ok(Answer::Next {
                suggestions: self.suggest()?,
            }),
        }
    }

    /// What to look at next: changed symbols, ranked by how much they affect and by whether
    /// anything has gone wrong there before.
    ///
    /// Both halves are already indexed, so this is a ranking rather than an analysis — which
    /// is the point. Nexus does not need to think about what to examine; it already knows.
    fn suggest(&self) -> Result<Vec<Suggestion>> {
        let changed: Vec<String> = self
            .changes(Some("symbol"))?
            .into_iter()
            .filter_map(|(_, _, target, _)| target)
            .take(40)
            .collect();

        // Prior findings for every candidate in one query rather than one per candidate.
        // `findings_for` is a `LIKE` scan; forty of them is forty scans to answer one
        // question, and the ranking only needs a count per component.
        let mut prior: BTreeMap<String, usize> = BTreeMap::new();
        for f in self.findings(None, None, None)? {
            *prior.entry(f.component.clone()).or_default() += 1;
        }

        let mut out = Vec::new();
        for fqn in changed {
            // Reach still costs one traversal per candidate: it is a different graph walk per
            // seed and there is no batched form of it. Forty walks, not eighty round trips.
            let reach = match self.impact(&ImpactQuery {
                target: fqn.clone(),
                ..Default::default()
            })? {
                Resolved::One(r) => r.items.len(),
                _ => 0,
            };
            let component = fqn
                .rsplit_once('#')
                .map(|(owner, _)| owner)
                .unwrap_or(&fqn)
                .rsplit('.')
                .next()
                .unwrap_or(&fqn)
                .to_string();
            let priors = prior.get(&component).copied().unwrap_or(0);

            // Reach is the cost of being wrong; prior findings are evidence that this code
            // has been wrong before. Neither alone is a good reason to look.
            let score = reach as f64 + priors as f64 * 3.0;
            if score <= 0.0 {
                continue;
            }
            out.push(Suggestion {
                why: match (reach, priors) {
                    (r, 0) => format!("changed, and {r} symbols depend on it"),
                    (0, p) => format!("changed, and {p} findings already exist here"),
                    (r, p) => {
                        format!("changed, {r} symbols depend on it, {p} findings already here")
                    }
                },
                target: fqn,
                score,
            });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(10);
        Ok(out)
    }
```

Match `Engine::findings`' real signature when calling it; if its filter parameters differ from
`(None, None, None)`, pass whatever means "no filter".

- [ ] **Step 4: Reduce `ask.rs` to verb parsing and nothing else**

Replace the whole body of `crates/nexus-cli/src/ask.rs` with:

```rust
//! `nexus ask` — the questions an agent actually has.
//!
//! Only the spelling lives here. Every answer is `Engine::ask`, because each one needs
//! several queries and CLAUDE.md's rule is that a caller needing two `Engine` calls has found
//! a missing `Engine` method. What remains is the mapping from the words a person types to
//! the question they mean.

use nexus_core::report::{Answer, Question};
use nexus_core::{Engine, EngineError};

/// The verbs this surface accepts, and what each one means.
pub const UNDERSTOOD: &[&str] = &[
    "changed",
    "affected <target>",
    "uses <target>",
    "known <target>",
    "facts",
    "next",
];

pub fn answer(engine: &Engine, question: &[String]) -> Result<Answer, EngineError> {
    let verb = question.first().map(String::as_str).unwrap_or("");
    let target = question.get(1..).map(|r| r.join(" ")).unwrap_or_default();

    let q = match verb {
        "changed" | "what-changed" => Question::Changed,
        "affected" | "uses" | "affects" => Question::Affected(target),
        "known" | "about" | "seen" => Question::Known(target),
        "facts" | "remember" => Question::Facts,
        "next" | "what-next" => Question::Next,
        _ => {
            return Ok(Answer::Unknown {
                asked: question.join(" "),
                understood: UNDERSTOOD.to_vec(),
            })
        }
    };
    engine.ask(&q)
}
```

`Answer::Unknown` stays in `report.rs` with the other variants; its `understood` field keeps
its existing type.

- [ ] **Step 5: Prove the output did not change**

Run:

```bash
cargo run -q --bin nexus -- --project . ask next  --json >  /tmp/ask-after.json
cargo run -q --bin nexus -- --project . ask facts --json >> /tmp/ask-after.json
diff /tmp/ask-before.json /tmp/ask-after.json && echo "IDENTICAL"
```

Expected: `IDENTICAL`. This is the whole safety argument for the task — a refactor that
changes what a user sees is not a refactor.

If they differ, the likely cause is the component derivation in `suggest`: `findings_for`
matched on a `LIKE` over the target, and the batched form matches on `component`. Compare the
two on a project with findings and reconcile before continuing.

- [ ] **Step 6: Add a regression test for the batched ranking**

Append to `crates/cap-bughunter/tests/finding_lifecycle.rs`, which already has the helpers this
needs — `fixture`, `write` and `analyze`:

```rust
/// `ask next` ranks a changed symbol that already carries a finding above one that does not.
///
/// The performance claim — one findings query rather than forty — is not observable from
/// outside the Engine, so it is a code-review property. What this test protects is that
/// batching did not change the *ranking*, which is the only part a user sees.
#[test]
fn ask_next_ranks_a_symbol_with_prior_findings_above_a_clean_one() {
    use nexus_core::report::{Answer, Question};

    let root = fixture("asknext");
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.register_capability(Box::new(BugHunter::new()));
    engine.scan().expect("scan");
    let found = analyze(&mut engine);
    assert_eq!(found.new, 1, "the fixture plants exactly one finding");

    // Change both classes, so both are in the changed set and only the finding separates them.
    write(
        &root,
        "src/mn/pay/PaymentService.java",
        &SELF_INVOCATION.replace("repo.save(key)", "repo.save(key.trim())"),
    );
    write(
        &root,
        "src/mn/pay/PaymentRepository.java",
        "package mn.pay;\n@Repository\npublic class PaymentRepository { public Payment save(String k) { return null; } }\n// touched\n",
    );
    engine.rescan().expect("rescan");

    let Answer::Next { suggestions } = engine.ask(&Question::Next).expect("ask next") else {
        panic!("Question::Next must answer with Answer::Next");
    };
    assert!(!suggestions.is_empty(), "two symbols changed; something should be suggested");

    let service = suggestions
        .iter()
        .position(|s| s.target.contains("PaymentService"))
        .expect("the class carrying the finding must be suggested");
    let repository = suggestions
        .iter()
        .position(|s| s.target.contains("PaymentRepository"));

    if let Some(repository) = repository {
        assert!(
            service < repository,
            "a changed symbol with a finding on it outranks a changed symbol without one: {suggestions:?}"
        );
    }
    assert!(
        suggestions[service].why.contains("findings already"),
        "the reason must say why it ranked, not just that it did: {:?}",
        suggestions[service].why
    );
}
```

- [ ] **Step 7: Run it, and prove it can fail**

Run: `cargo test -p cap-bughunter ask_next_ranks`
Expected: PASS.

Temporarily change the `* 3.0` weight in `suggest` to `* 0.0` and re-run.
Expected: FAIL — the ordering assertion. **Restore the weight.**

- [ ] **Step 7: Full suite and commit**

Run: `make check`
Expected: PASS, 184 tests.

```bash
git add crates/nexus-core/src crates/nexus-cli/src crates/cap-bughunter/tests
git commit -m "core: ask orchestration moves to the Engine, and loses its N+1 (task 5)"
```

---

## Task 6: Split `engine.rs`

2,069 lines, a 522-line `rescan`, a 239-line `analyze`. `Engine` as the single public API is a good decision and stays; its implementation living in one file is what will resist the next four subsystems.

**This task moves code and changes nothing.** No signature, no behaviour, no test.

**Files:**
- Create: `crates/nexus-core/src/engine/mod.rs`, `scan.rs`, `rescan.rs`, `analyze.rs`, `query.rs`
- Delete: `crates/nexus-core/src/engine.rs`

**Interfaces:**
- Consumes: everything `engine.rs` consumes today.
- Produces: the identical public surface. `nexus_core::Engine`, `EngineError`, `NEXUS_DIR`, `DB_FILE`, `SIBLING_WARN_FLOOR`, `MODEL_CONFIDENCE_CAP` all resolve to the same paths.

- [ ] **Step 1: Record the public surface before touching anything**

```bash
grep -n "^    pub fn \|^pub const \|^pub enum \|^pub struct " crates/nexus-core/src/engine.rs \
  | sed 's/ *{$//' | sed 's/^[0-9]*://' | sort > /tmp/engine-api-before.txt
wc -l /tmp/engine-api-before.txt
```

- [ ] **Step 2: Create the directory and move the file**

```bash
mkdir crates/nexus-core/src/engine
git mv crates/nexus-core/src/engine.rs crates/nexus-core/src/engine/mod.rs
```

Run: `make check`
Expected: PASS. Rust resolves `mod engine;` to `engine/mod.rs` identically. **This step alone
must be green before any code moves** — it separates "the module system still works" from "the
split is correct".

- [ ] **Step 3: Move the scan path into `scan.rs`**

Create `crates/nexus-core/src/engine/scan.rs` with a second `impl Engine` block containing
`pub fn scan` (line ~284) and the free functions only it uses: `classify`, `to_new_edge`,
`to_new_symbol`, `parse_all`.

Header:

```rust
//! `scan` — the full index: walk, hash, parse, resolve, and set the baseline.
//!
//! Split out of `engine.rs` by responsibility. `Engine`'s public API is unchanged: an `impl`
//! block in another file of the same module is the same type, and callers cannot tell.
use super::*;
```

Add `mod scan;` to `engine/mod.rs`. Delete the moved items from `mod.rs`. Make any function
the other files still need `pub(super)` rather than private.

Run: `cargo test -p nexus-core`
Expected: PASS.

- [ ] **Step 4: Move the rescan path into `rescan.rs`**

Same shape. Move `pub fn rescan` (line ~421, the 522-line one), `fn candidates` (~943), and
the rename machinery: `symbol_change` (~1831), `resolve_symbol_renames` (~1875),
`detect_renames` (~1904), plus the `#[derive]`'d helper struct at ~1854 that they share.

Header:

```rust
//! `rescan` — the incremental cascade, and the rename resolution that makes it honest.
//!
//! Renames are resolved after every changed file has been seen, never per file: the two
//! halves of a package move live in different files. That is why the buffering lives here
//! rather than inside the per-file loop.
use super::*;
```

Run: `cargo test -p nexus-core renames`
Expected: PASS — the rename fixtures are the most sensitive thing in this move.

Run: `cargo test -p nexus-core`
Expected: PASS.

- [ ] **Step 5: Move the analysis path into `analyze.rs`**

Move `pub fn analyze` (~1047), `pub fn record_finding` (~1286), and `to_summary` (~1935).

Header:

```rust
//! `analyze` — capability dispatch and the finding lifecycle.
//!
//! Nexus owns identity, recurrence, fixed and regressed; a capability owns only rules. This
//! file is that division: everything here is the platform's half.
use super::*;
```

Run: `cargo test -p cap-bughunter && cargo test -p cap-architect && cargo test -p cap-review`
Expected: PASS — these exercise the lifecycle through `analyze`.

- [ ] **Step 6: Move the read-only queries into `query.rs`**

Move `status`, `changes`, `findings_for`, `record_fact`, `facts`, `previous_scan_id`,
`capability_list`, `findings`, `finding`, `ignore_finding`, `impact`, `symbol`, `graph`,
`doctor`, `read_lines`, and `Engine::ask` and `suggest` from Task 5.

Header:

```rust
//! Read-only queries: what is here, what it reaches, what is known about it.
//!
//! Nothing in this file writes, except `record_fact` and `ignore_finding` — both of which are
//! a single row and neither of which touches the index. They are here because they are what a
//! caller asking a question does next.
use super::*;
```

Run: `cargo test -p nexus-core && cargo test -p nexus-cli`
Expected: PASS.

- [ ] **Step 7: What stays in `mod.rs`**

`EngineError`, the six constants, `LEGACY_DIR`/`LEGACY_DB`, `struct Engine`, and the lifecycle
and helpers: `init`, `migrate_legacy_dir`, `open`, `open_or_init`, `open_at`,
`register_capability`, `root`, `name`, `detect`, `save_profile`, `load_profile`,
`tool_versions`, `head`, `canonical`, `write_if_absent`, `dir_size`, `human_bytes`,
`DEFAULT_CONFIG`, `DEFAULT_POLICY`.

Add the module list at the top:

```rust
mod analyze;
mod query;
mod rescan;
mod scan;
```

- [ ] **Step 8: Prove the public surface is byte-identical**

```bash
grep -hn "^    pub fn \|^pub const \|^pub enum \|^pub struct " crates/nexus-core/src/engine/*.rs \
  | sed 's/ *{$//' | sed 's/^[0-9]*://' | sort > /tmp/engine-api-after.txt
diff /tmp/engine-api-before.txt /tmp/engine-api-after.txt && echo "API IDENTICAL"
```

Expected: `API IDENTICAL`, except for the `ask`/`suggest` lines Task 5 added. If anything else
differs, a method changed visibility during the move — fix it before continuing.

- [ ] **Step 9: Check the file sizes actually improved**

```bash
wc -l crates/nexus-core/src/engine/*.rs
```

Expected: no file over ~900 lines. `rescan.rs` will be the largest; that is fine — the 522-line
function is a separate problem and splitting it changes behaviour, which this task must not.

- [ ] **Step 10: Full suite and commit**

Run: `make check`
Expected: PASS, 184 tests. Same count as before the split: this task adds no tests because it
adds no behaviour.

```bash
git add crates/nexus-core/src/engine
git commit -m "core: split engine.rs by responsibility, public API unchanged (task 6)"
```

---

## Done when

- [ ] `make check` passes: 184 tests, zero clippy warnings.
- [ ] `grep -rn "trait Rule\|trait Detector\|struct Graph" crates/` returns exactly two lines, both in `nexus-core/src/rules.rs`.
- [ ] `grep -rn "architecture only\|no code exists\|still called \`bughunter\`" AGENTS.md CLAUDE.md` returns nothing.
- [ ] No file in `crates/nexus-core/src/engine/` exceeds ~900 lines.
- [ ] `nexus ask next --json` output is byte-identical to the recording taken before Task 5.
- [ ] Every document that describes an unbuilt subsystem says so in its first paragraph.
- [ ] `docs/architecture/03-current-state.md` no longer claims tables that do not exist.
