# Session Context Package and SessionStart Hook (roadmap 1.7, 1.8) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `nexus context --session` returns the project profile, the open findings and the durable facts as one budgeted, self-explaining package in at most 800 estimated tokens; `nexus init --hooks` installs a fail-open SessionStart hook that runs it. Phase 1 is then complete.

**Architecture:** A new `nexus-core::context` module owns the package types from [`05-context-engine.md`](../../architecture/05-context-engine.md) §2 and one builder. Phase 1 fills them with a **fixed query, not a ranked selection** — three existing Engine queries, greedy fill to a token ceiling, every candidate recorded in an inclusion ledger. Phase 2 replaces the builder's body behind the same signature. The hook is a Claude Code settings file written by the CLI, containing no logic beyond a command string and a timeout.

**Tech Stack:** Rust 1.82+, `serde_json` (already in both crates), `clap`.

**Spec:** [`05-context-engine.md`](../../architecture/05-context-engine.md) §2 (types), §7 (budget), §8 (ledger), §10 (freshness), §12 (prohibitions); [`07-agent-integration.md`](../../architecture/07-agent-integration.md) §2 Tier 1 and §4 (the concrete output); [ADR-024](../../architecture/decisions/ADR-024-hooks-are-the-invocation-tier-and-ship-off-by-default.md); [`10-roadmap.md`](../../architecture/10-roadmap.md) tasks 1.7 and 1.8 with the Phase 1 success criteria.

## Global Constraints

- **Roadmap 1.7 and 1.8 are the scope.** Explicitly **do not build**: ranking or scoring logic (Phase 2.6), seeds, intent classification, graph expansion, the `--task` flag, the `--explain` flag (2.8), the `UserPromptSubmit`/`PostToolUse`/`Stop` hooks, any MCP tool, any package cache, any new language analyzer.
- **The Context Engine must never call a model, return a whole file, include an item with no `file:line` anchor, pad the budget with weak items, or truncate silently.** Every omission is a ledger row. ([`05`](../../architecture/05-context-engine.md) §12.)
- **`make check` must pass after every task.** Baseline: **194 passing tests**. No task may reduce that count.
- **Only `nexus-store` contains SQL.** The context builder is inside `nexus-core` and reaches storage through existing `Store` methods only. No new SQL.
- **`nexus-core` must not gain a dependency.** The module uses `serde`, `serde_json` and the crate's own types.
- **`deny(clippy::unwrap_used, clippy::expect_used)` outside tests in `nexus-core`.**
- **`stdout` is results, `stderr` is everything else.** `--json | jq` must work with `-v` on.
- **Exit codes are interface**: 0 ok, 1 runtime, 2 usage, 3 findings, 5 no baseline, 6 ambiguous.
- **`nexus init` without `--hooks` must write nothing outside `.nexus/`.** Hooks are opt-in (ADR-024).
- **`nexus init --hooks` must never clobber an existing `.claude/settings.json`.** Merge, preserve every unrelated key, and be idempotent.
- **`git add` names files.** Never a directory.
- **Commit after every task**, message naming the roadmap id, ending with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## Decisions taken before writing code

Four questions the spec leaves open at Phase 1. Each is settled here so the implementer does not re-litigate them.

**1. "Durable facts" with no lifecycle column.** [`06-memory.md`](../../architecture/06-memory.md) defines durable as *validated across ≥ 3 scans, or `source='human'`*. The lifecycle states are Phase 3.1 and the `durable` column does not exist. Phase 1 therefore approximates durable as **the order `Store::facts` already returns** — human, then deterministic, then AI, each by confidence descending — and takes them until the budget runs out. The package says so in its `basis` note. This is an approximation that gets *better*, not one that has to be unwound.

**2. Facts with no evidence are excluded.** §2 says the anchor is `file:line`, "always present, no exceptions", and §12 forbids an item without one. A fact whose `evidence_json` is empty or unreadable therefore becomes an **excluded ledger row reading `no file:line anchor`**, not an item. This is visible rather than silent, which is the point of the ledger. **Consequence to report:** the CLI `nexus fact` verb accepts no evidence, so every fact recorded from a terminal is unanchored and will never appear in a session package. That is the same gap that keeps CLI-recorded facts out of the 1.6 invalidation rule. It is named in the summary, not fixed here.

**3. `context --session` never writes.** [`07`](../../architecture/07-agent-integration.md) §6.1 sketches the hook scanning when no baseline exists. Phase 1 does not: a hook that writes to the database on session start is a side effect the developer did not ask for, and a first scan cannot fit a 400 ms budget. With no baseline the command prints one line naming `nexus scan` and exits 5. The hook swallows the code.

**4. The hook file is a CLI concern.** [`07`](../../architecture/07-agent-integration.md) §3 forbids agent-specific logic in the binary's analysis path and requires adding an agent to be a shim. `.claude/settings.json` is Claude Code's format, so the writer lives in `nexus-cli`, touches no Engine method, and holds one command string that the fail-open test also reads.

---

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/nexus-core/src/context.rs` | create | The package types and the Phase 1 fixed-query builder. The only place a budget is spent. |
| `crates/nexus-core/src/lib.rs` | modify | `pub mod context;` |
| `crates/nexus-core/src/engine/query.rs` | modify | `Engine::context` — one method, the public entry point. |
| `crates/nexus-core/src/engine/mod.rs` | modify | One `EngineError` variant for a purpose Phase 1 does not serve. |
| `crates/nexus-core/tests/session_context.rs` | create | The acceptance test: budget, anchors, ledger completeness. |
| `crates/nexus-cli/src/main.rs` | modify | The `context` subcommand, its envelope name, and `init --hooks`. |
| `crates/nexus-cli/src/render.rs` | modify | The human rendering of a package. |
| `crates/nexus-cli/src/hooks.rs` | create | The Claude Code settings merge, and the hook command constant. |
| `crates/nexus-cli/tests/hooks.rs` | create | Opt-in, idempotence, preservation, and the fail-open property. |
| `docs/architecture/10-roadmap.md`, `README.md`, `docs/architecture/03-current-state.md` | modify | Phase 1 status. |

---

### Task 1: The context module and its types

**Files:**
- Create: `crates/nexus-core/src/context.rs`
- Modify: `crates/nexus-core/src/lib.rs` (module list and re-export)

**Interfaces:**
- Consumes: `crate::findings::CodeRef`, `crate::report::{Profile, FindingSummary}`, `nexus_store::FactRow`.
- Produces: every type below, plus `estimate_tokens`.

- [ ] **Step 1: Write the module**

Create `crates/nexus-core/src/context.rs`:

```rust
//! The context package: what an agent is handed, and why each thing is in it.
//!
//! The types here are the Context Engine's contract ([`05-context-engine.md`] §2). Phase 1
//! ships the contract and one fixed query behind it — the session package, which is profile
//! plus open findings plus durable facts under a token ceiling. Phase 2 replaces the body of
//! [`Engine::context`](crate::Engine::context) with the seven-stage ranked pipeline; nothing
//! here changes shape when it does, which is the reason the types land first.
//!
//! Two rules from §12 are enforced in this file rather than trusted to callers:
//!
//!   * **Every item carries a `file:line` anchor.** A candidate without one is an *excluded*
//!     ledger row, never a silent omission and never an anchorless item.
//!   * **Remaining budget is never padded.** Selection stops when the next candidate does not
//!     fit; it does not go looking for a smaller one to fill the gap.

use crate::findings::CodeRef;
use crate::report::{FindingSummary, Profile};
use serde::Serialize;

/// The default ceiling for a session package. The `SessionStart` hook's budget in ADR-024.
pub const SESSION_BUDGET_TOKENS: usize = 800;

/// Bytes per token. The estimator `budget::fit` already uses, and an estimate on purpose:
/// a real tokenizer is a dependency bought for a rounding error ([`05`] §6).
const BYTES_PER_TOKEN: f64 = 3.5;

/// Estimated tokens for a rendered string. Always at least 1 for non-empty input, so that
/// nothing is ever free and the greedy fill cannot loop.
pub fn estimate_tokens(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    ((s.len() as f64 / BYTES_PER_TOKEN).ceil() as usize).max(1)
}

/// Why a package was asked for. Phase 1 serves `Session`; the rest are the Phase 2 surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Session,
    Task,
    Review,
    Debug,
    Verify,
}

/// What was asked for. `text`, `files` and `symbols` are the Phase 2 seed inputs and are
/// accepted now so the signature does not move when ranking lands.
#[derive(Debug, Clone)]
pub struct TaskRequest {
    pub text: String,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub budget_tokens: usize,
    pub purpose: Purpose,
}

impl TaskRequest {
    /// The session package: no text, no explicit anchors, 800 tokens.
    pub fn session(budget_tokens: usize) -> Self {
        Self {
            text: String::new(),
            files: Vec::new(),
            symbols: Vec::new(),
            budget_tokens,
            purpose: Purpose::Session,
        }
    }
}

/// What the project is. Answered from the stored profile, never inferred at query time.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectSummary {
    pub name: String,
    pub profile: Option<Profile>,
    pub files: i64,
    pub symbols: i64,
    /// Present when the scan looks like one module of something larger. Silence here is what
    /// lets an impact answer report a small blast radius with total confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Symbol,
    File,
    Finding,
    Fact,
    Change,
    Test,
    Decision,
}

/// Every weighted term that produced a score, individually. Phase 1 records zeros and says so
/// in the package's `basis`: a fixed query has no ranking to decompose. The struct exists now
/// because Phase 2.6's contract is that *every* term is recorded, and a field added later is a
/// field some caller has already learned to live without.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ScoreTerms {
    pub seed: f64,
    pub graph: f64,
    pub churn: f64,
    pub recency: f64,
    pub history: f64,
    pub fact: f64,
    pub test: f64,
    pub arch: f64,
    pub cost: f64,
}

/// One thing in the package. Anchors, not contents: Nexus says where, and the agent has
/// `Read` ([`05`] §2, principle 3).
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    pub kind: ItemKind,
    pub anchor: CodeRef,
    /// At most a few lines. Never a whole file. Phase 1 emits none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    pub score: f64,
    pub terms: ScoreTerms,
    /// One clause, human-readable, saying why this is here.
    pub why: String,
    /// The rendered line. Carried so the token estimate and the output cannot disagree.
    pub text: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Included,
    Excluded,
}

/// One candidate's fate. Recorded for winners *and* losers: a ranker that only explains its
/// inclusions cannot be debugged for the failure that matters most, which is the right item
/// that never made it in ([`05`] §8).
#[derive(Debug, Clone, Serialize)]
pub struct LedgerRow {
    pub kind: ItemKind,
    pub label: String,
    pub decision: Decision,
    pub reason: String,
    pub score: f64,
    pub tokens: usize,
}

/// Every candidate considered, in the order they were considered.
#[derive(Debug, Clone, Default, Serialize)]
pub struct InclusionLedger {
    pub rows: Vec<LedgerRow>,
}

impl InclusionLedger {
    pub fn included(&mut self, kind: ItemKind, label: String, score: f64, tokens: usize) {
        self.rows.push(LedgerRow {
            kind,
            label,
            decision: Decision::Included,
            reason: "selected".into(),
            score,
            tokens,
        });
    }
    pub fn excluded(
        &mut self,
        kind: ItemKind,
        label: String,
        reason: String,
        score: f64,
        tokens: usize,
    ) {
        self.rows.push(LedgerRow {
            kind,
            label,
            decision: Decision::Excluded,
            reason,
            score,
            tokens,
        });
    }
    pub fn count(&self, decision: Decision) -> usize {
        self.rows.iter().filter(|r| r.decision == decision).count()
    }
}

/// What the package describes and what it was built from.
///
/// A package that does not state its basis implies a clean tree it may not have been built
/// from ([`05`] §10). All four fields are also the Phase 2.9 cache key.
#[derive(Debug, Clone, Serialize)]
pub struct PackageBasis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub dirty: bool,
    /// What produced the selection. Phase 1 says so plainly, because a caller cannot tell a
    /// fixed query from a ranked one by looking at the output.
    pub selection: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextPackage {
    pub purpose: Purpose,
    pub project: ProjectSummary,
    pub items: Vec<ContextItem>,
    pub ledger: InclusionLedger,
    pub basis: PackageBasis,
    pub budget_tokens: usize,
    pub tokens_estimated: usize,
    pub items_considered: usize,
    pub items_included: usize,
}

/// A candidate before the budget has had its say.
pub(crate) struct Candidate {
    pub kind: ItemKind,
    pub label: String,
    pub anchor: Option<CodeRef>,
    pub why: String,
    pub text: String,
}

impl Candidate {
    pub(crate) fn tokens(&self) -> usize {
        estimate_tokens(&self.text)
    }
}

/// Greedy fill in the order given, recording every candidate's fate.
///
/// Order *is* the selection in Phase 1 — there is no score to sort by, and inventing one
/// would be the folklore [`05`] §6 warns against. When a candidate does not fit, it is
/// excluded and the fill continues: a later, smaller item may still belong, and skipping the
/// rest would be truncation rather than selection.
pub(crate) fn fill(
    candidates: Vec<Candidate>,
    budget_tokens: usize,
    spent: usize,
    ledger: &mut InclusionLedger,
) -> (Vec<ContextItem>, usize) {
    let mut items = Vec::new();
    let mut used = spent;
    for c in candidates {
        let tokens = c.tokens();
        let Some(anchor) = c.anchor else {
            // §12: no item without a `file:line` anchor. Recorded, never dropped quietly.
            ledger.excluded(
                c.kind,
                c.label,
                "no file:line anchor".into(),
                0.0,
                tokens,
            );
            continue;
        };
        if used + tokens > budget_tokens {
            ledger.excluded(
                c.kind,
                c.label,
                format!("budget exhausted at {budget_tokens} tokens"),
                0.0,
                tokens,
            );
            continue;
        }
        used += tokens;
        ledger.included(c.kind, c.label, 0.0, tokens);
        items.push(ContextItem {
            kind: c.kind,
            anchor,
            window: None,
            score: 0.0,
            terms: ScoreTerms::default(),
            why: c.why,
            text: c.text,
            tokens,
        });
    }
    (items, used)
}
```

In `crates/nexus-core/src/lib.rs`, add `pub mod context;` to the module list (alphabetical, after `pub mod capability;`) and this re-export beside the others:

```rust
pub use context::{ContextPackage, Purpose, TaskRequest};
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p nexus-core 2>&1 | tail -5`
Expected: no errors. Warnings about unused `pub(crate)` items are impossible here because `fill` and `Candidate` are used in Task 2; if the build is run before Task 2 lands, `cargo clippy` may flag them as unused — that is expected and resolved by the next task, so run only `cargo build` at this step.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-core/src/context.rs crates/nexus-core/src/lib.rs
git commit -m "core: the context package types (roadmap 1.7)

The Context Engine's contract from 05-context-engine.md §2, landed before the
pipeline that fills it, so Phase 2 replaces a body rather than a signature.
Two of §12's rules live in this file rather than in its callers: an item
without a file:line anchor becomes an excluded ledger row, and a budget with
room left is not padded.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 2: `Engine::context` and the session package

**Files:**
- Modify: `crates/nexus-core/src/engine/mod.rs` (one `EngineError` variant)
- Modify: `crates/nexus-core/src/engine/query.rs` (the method, appended after `facts`)
- Create: `crates/nexus-core/tests/session_context.rs`

**Interfaces:**
- Consumes: `Engine::status`, `Engine::findings`, `Engine::graph`, `Store::facts`, `SIBLING_WARN_FLOOR`, everything from Task 1.
- Produces: `Engine::context(&self, req: &TaskRequest) -> Result<ContextPackage>`.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/tests/session_context.rs`:

```rust
//! The session package: what an agent knows before it reads a file.
//!
//! Phase 1 selection is a fixed query, so these tests pin the *contract* rather than a
//! ranking — the budget holds, every item is anchored, and every candidate that did not make
//! it says why. A ranked Phase 2 must keep all three true.

use nexus_core::context::{Decision, ItemKind, Purpose, TaskRequest, SESSION_BUDGET_TOKENS};
use nexus_core::{Engine, EngineError, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";

const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    public void pay(String key) {
        System.out.println("pay " + key);
    }
}
"#;

fn git(root: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-ctx-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let path = root.join(SERVICE);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, SOURCE).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    root
}

fn scanned(name: &str) -> Engine {
    let root = fixture(name);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");
    engine
}

fn session(engine: &Engine) -> nexus_core::ContextPackage {
    engine
        .context(&TaskRequest::session(SESSION_BUDGET_TOKENS))
        .expect("context")
}

#[test]
fn a_session_package_says_what_the_project_is_and_what_it_was_built_from() {
    let engine = scanned("profile");
    let pkg = session(&engine);

    assert_eq!(pkg.purpose, Purpose::Session);
    assert!(pkg.project.symbols > 0, "the index is not empty");
    let profile = pkg.project.profile.as_ref().expect("a detected profile");
    assert!(
        profile.languages.iter().any(|l| l.lang == "java"),
        "the fixture is Java: {:?}",
        profile.languages
    );
    // §10: a package that does not state its basis implies a clean tree it may not describe.
    assert!(pkg.basis.scan_uid.is_some(), "the package names its scan");
    assert!(pkg.basis.commit.is_some(), "and its commit");
    assert!(
        !pkg.basis.selection.is_empty(),
        "and says how it selected: a caller cannot tell a fixed query from a ranked one"
    );
}

#[test]
fn an_anchored_fact_is_included_and_an_unanchored_one_is_excluded_with_a_reason() {
    let mut engine = scanned("facts");
    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is idempotent on key".into(),
            source: "human".into(),
            evidence: vec![nexus_core::findings::CodeRef {
                file: SERVICE.into(),
                line: 3,
                note: String::new(),
            }],
            confidence: 0.9,
        })
        .expect("anchored");
    engine
        .record_fact(FactInput {
            key: "convention.error-handling".into(),
            scope: "project".into(),
            subject: None,
            claim: "errors carry context".into(),
            source: "human".into(),
            evidence: Vec::new(),
            confidence: 0.9,
        })
        .expect("unanchored");

    let pkg = session(&engine);
    let facts: Vec<_> = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .collect();
    assert_eq!(facts.len(), 1, "only the anchored fact is an item: {facts:?}");
    assert!(facts[0].text.contains("idempotent"));
    assert_eq!(facts[0].anchor.file, SERVICE);

    // §12 forbids an anchorless item; §8 forbids a silent omission. Both, together.
    let row = pkg
        .ledger
        .rows
        .iter()
        .find(|r| r.label.contains("convention.error-handling"))
        .expect("the unanchored fact is in the ledger");
    assert_eq!(row.decision, Decision::Excluded);
    assert!(
        row.reason.contains("anchor"),
        "the reason names the missing anchor: {row:?}"
    );
}

#[test]
fn the_package_stays_within_its_budget_and_accounts_for_every_candidate() {
    let mut engine = scanned("budget");
    // Enough anchored facts that the 800-token ceiling has to refuse some.
    for i in 0..200 {
        engine
            .record_fact(FactInput {
                key: format!("invariant.pay.rule-{i:03}"),
                scope: "symbol".into(),
                subject: Some("mn.pay.PaymentService#pay".into()),
                claim: format!(
                    "rule {i:03}: a payment is settled exactly once, and the ledger row proves it"
                ),
                source: "human".into(),
                evidence: vec![nexus_core::findings::CodeRef {
                    file: SERVICE.into(),
                    line: 3,
                    note: String::new(),
                }],
                confidence: 0.9,
            })
            .expect("fact");
    }

    let pkg = session(&engine);
    assert!(
        pkg.tokens_estimated <= SESSION_BUDGET_TOKENS,
        "{} tokens exceeds the {SESSION_BUDGET_TOKENS} ceiling",
        pkg.tokens_estimated
    );
    assert!(pkg.items_included > 0, "the budget bought something");
    assert!(
        pkg.ledger.count(Decision::Excluded) > 0,
        "200 facts do not fit in 800 tokens, so something was refused"
    );
    // §8: considered = included + excluded. An unexplained omission fails here.
    assert_eq!(
        pkg.items_considered,
        pkg.ledger.rows.len(),
        "every candidate is a ledger row"
    );
    assert_eq!(pkg.items_included, pkg.items.len());
    assert_eq!(
        pkg.items_included,
        pkg.ledger.count(Decision::Included),
        "the ledger and the item list agree"
    );
    for row in pkg.ledger.rows.iter().filter(|r| r.decision == Decision::Excluded) {
        assert!(!row.reason.is_empty(), "an unexplained exclusion: {row:?}");
    }
}

#[test]
fn every_item_carries_an_anchor() {
    let engine = scanned("anchors");
    for item in session(&engine).items {
        assert!(
            !item.anchor.file.is_empty(),
            "§12: no item without a file:line anchor: {item:?}"
        );
    }
}

#[test]
fn without_a_baseline_there_is_no_package() {
    let root = fixture("nobaseline");
    let (engine, _) = Engine::init(&root).expect("init");
    match engine.context(&TaskRequest::session(SESSION_BUDGET_TOKENS)) {
        Err(EngineError::NoBaseline) => {}
        other => panic!("a package built from nothing is worse than none: {other:?}"),
    }
}

#[test]
fn a_purpose_phase_one_does_not_serve_is_refused_rather_than_faked() {
    let engine = scanned("purpose");
    let mut req = TaskRequest::session(SESSION_BUDGET_TOKENS);
    req.purpose = Purpose::Task;
    match engine.context(&req) {
        Err(EngineError::Unsupported(m)) => assert!(
            m.contains("task"),
            "the error names what was asked for: {m}"
        ),
        other => panic!("a session package answering a task request is a lie: {other:?}"),
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-core --test session_context 2>&1 | grep -E "^error" -A 2 | head -8`
Expected: `no method named 'context' found for struct 'Engine'`.

- [ ] **Step 3: Add the error variant**

In `crates/nexus-core/src/engine/mod.rs`, inside `enum EngineError`, after the `NoBaseline` variant:

```rust
    #[error("{0}")]
    Unsupported(String),
```

- [ ] **Step 4: Implement `Engine::context`**

Append to `crates/nexus-core/src/engine/query.rs`, inside the existing `impl Engine` block, after `pub fn facts`:

```rust
    /// The context package for a request.
    ///
    /// Phase 1 serves `Purpose::Session` with a **fixed query** — profile, open findings,
    /// durable facts, greedy fill to the budget. There is no ranking here on purpose: a
    /// scoring function invented before the ledger has any data to justify its weights is
    /// folklore, and Phase 2.6 replaces this body with the real one behind this signature.
    ///
    /// Reads only. A hook that writes to the database when a session opens is a side effect
    /// nobody asked for, so a project with no baseline gets `NoBaseline` and the advice to
    /// scan, not an implicit scan.
    pub fn context(&self, req: &TaskRequest) -> Result<ContextPackage> {
        if req.purpose != Purpose::Session {
            return Err(EngineError::Unsupported(format!(
                "context for purpose '{}' is the Phase 2 context engine; only --session is built",
                match req.purpose {
                    Purpose::Session => "session",
                    Purpose::Task => "task",
                    Purpose::Review => "review",
                    Purpose::Debug => "debug",
                    Purpose::Verify => "verify",
                }
            )));
        }
        let status = self.status()?;
        let Some(baseline) = status.baseline.clone() else {
            return Err(EngineError::NoBaseline);
        };

        // A scan that covers one module of something larger answers impact questions with a
        // confidently small blast radius. Saying so costs one query and is the single most
        // useful correction an agent can be handed at session start.
        let graph = self.graph()?;
        let scope_warning = (graph.edges_sibling >= SIBLING_WARN_FLOOR as i64).then(|| {
            format!(
                "{} edges point at code this project owns that was not scanned — impact \
                 answers here are understated; scan from the repository root",
                graph.edges_sibling
            )
        });

        let project = ProjectSummary {
            name: status.project.clone(),
            profile: status.profile.clone(),
            files: status.files,
            symbols: status.symbols,
            scope_warning,
        };

        let mut candidates = Vec::new();

        // Open findings: what is broken now. FIXED and IGNORED are history, not news.
        for f in self.findings(None, None, None)? {
            if matches!(f.status.as_str(), "FIXED" | "IGNORED") {
                continue;
            }
            let anchor = match (&f.file, f.line) {
                (Some(file), Some(line)) => Some(CodeRef {
                    file: file.clone(),
                    line: line.max(0) as u32,
                    note: String::new(),
                }),
                _ => None,
            };
            candidates.push(Candidate {
                kind: ItemKind::Finding,
                label: f.uid.clone(),
                anchor,
                why: format!("open finding, {}", f.status.to_lowercase()),
                text: format!(
                    "{}  {}  {}  {}",
                    f.uid,
                    f.status,
                    f.component.as_deref().unwrap_or("-"),
                    f.title
                ),
            });
        }

        // Durable facts: what previous sessions worked out.
        //
        // The lifecycle states are Phase 3.1, so "durable" is approximated by the order the
        // store already returns — human, then deterministic, then AI, each by confidence.
        // The approximation gets better when the lifecycle lands; it does not get unwound.
        for row in self.store.facts(self.project_id, None)? {
            let anchor = row
                .evidence_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<Vec<CodeRef>>(j).ok())
                .and_then(|refs| refs.into_iter().next());
            candidates.push(Candidate {
                kind: ItemKind::Fact,
                label: row.key.clone(),
                anchor,
                why: format!("{} fact about {}", row.source, row.subject.as_deref().unwrap_or("the project")),
                text: format!("{}  {}  [{}]", row.key, row.claim, row.source),
            });
        }

        let mut ledger = InclusionLedger::default();
        let considered = candidates.len();
        // The summary is not a candidate — it is what the package is *about*, and a package
        // that dropped it under budget pressure would describe findings in an unnamed project.
        let spent = estimate_tokens(&project.name)
            + project
                .scope_warning
                .as_deref()
                .map(estimate_tokens)
                .unwrap_or(0);
        let (items, tokens_estimated) =
            context::fill(candidates, req.budget_tokens, spent, &mut ledger);

        Ok(ContextPackage {
            purpose: req.purpose,
            project,
            items_included: items.len(),
            items,
            ledger,
            basis: PackageBasis {
                scan_uid: baseline.scan_uid,
                commit: status.current.commit.clone(),
                dirty: status.current.dirty,
                selection: "phase-1 fixed query: open findings then durable facts, in store order",
            },
            budget_tokens: req.budget_tokens,
            tokens_estimated,
            items_considered: considered,
        })
    }
```

Add to the `use` list at the top of `crates/nexus-core/src/engine/query.rs`:

```rust
use crate::context::{
    self, estimate_tokens, Candidate, ContextPackage, InclusionLedger, ItemKind, PackageBasis,
    ProjectSummary, Purpose, TaskRequest,
};
```

`CodeRef` is already imported by `engine/mod.rs`'s glob into this module; if the compiler disagrees, add `use crate::findings::CodeRef;`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core --test session_context 2>&1 | grep -E "^test |test result" `
Expected: `test result: ok. 6 passed`

- [ ] **Step 6: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 200 tests.

```bash
git add crates/nexus-core/src/engine/query.rs crates/nexus-core/src/engine/mod.rs crates/nexus-core/tests/session_context.rs
git commit -m "core: Engine::context builds the session package (roadmap 1.7)

Profile, open findings and durable facts under a token ceiling, with every
candidate's fate recorded. Selection is a fixed query and the package says so:
inventing weights before the ledger has data to justify them is the folklore
05-context-engine.md §6 warns against, and Phase 2.6 replaces this body behind
the same signature.

Reads only. No baseline yields NoBaseline and the advice to scan, because a
hook that writes to the database when a session opens is a side effect nobody
asked for. A purpose Phase 1 does not serve is refused rather than answered
with a session package.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 3: `nexus context --session` on the command line

**Files:**
- Modify: `crates/nexus-cli/src/main.rs` (the `Command` enum, the dispatch arm, the envelope name)
- Modify: `crates/nexus-cli/src/render.rs` (one renderer)

**Interfaces:**
- Consumes: `Engine::context`, `TaskRequest::session`, `SESSION_BUDGET_TOKENS`.
- Produces: the `context` subcommand.

- [ ] **Step 1: Add the subcommand**

In `crates/nexus-cli/src/main.rs`, in `enum Command`, after the `Ask` variant:

```rust
    /// What an agent should know before it reads a file
    Context {
        /// The session package: what this project is, what is open, what is known
        #[arg(long)]
        session: bool,
        /// Token ceiling. The package is selected to fit, never truncated to fit.
        #[arg(long, value_name = "TOKENS")]
        budget: Option<usize>,
    },
```

In `fn envelope`, add to the `command` match: `Command::Context { .. } => "context",`.

In the dispatch `match`, after the `Command::Ask` arm:

```rust
        Command::Context { session, budget } => {
            // One shape today. The flag is required rather than defaulted so that adding
            // `--task` in Phase 2 does not silently change what a bare `nexus context` means.
            if !session {
                eprintln!("nexus context: --session is required (--task lands with the context engine)");
                return Ok(exit::USAGE);
            }
            let engine = Engine::open(&root)?;
            let budget = budget.unwrap_or(nexus_core::context::SESSION_BUDGET_TOKENS);
            match engine.context(&nexus_core::TaskRequest::session(budget)) {
                Ok(pkg) => {
                    emit!(&pkg, {
                        render::context(&mut out, &st, &pkg)?;
                    });
                }
                Err(nexus_core::EngineError::NoBaseline) => {
                    // Not an error worth a stack trace: the project simply has not been
                    // scanned. The exit code carries it; the hook ignores the code.
                    if !cli.quiet && !cli.json {
                        writeln!(
                            out,
                            "No baseline — run `{} scan`.",
                            render::binary_name()
                        )?;
                    }
                    return Ok(exit::NO_BASELINE);
                }
                Err(e) => return Err(e.into()),
            }
        }
```

- [ ] **Step 2: Add the renderer**

In `crates/nexus-cli/src/render.rs`, after `pub fn profile`:

```rust
/// The session package, in the shape `07-agent-integration.md` §4 specifies.
///
/// Every line here is a query result. Nothing is inferred and no token was spent producing
/// it, which is the whole claim the package makes.
pub fn context(w: &mut impl Write, st: &Style, p: &ContextPackage) -> std::io::Result<()> {
    if let Some(prof) = &p.project.profile {
        profile(w, st, prof)?;
    } else {
        writeln!(w, "Project: {}", st.head(&p.project.name))?;
    }

    let findings: Vec<&ContextItem> = p
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Finding)
        .collect();
    if !findings.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head(&format!("Open findings ({})", findings.len())))?;
        for i in &findings {
            writeln!(w, "  {}", i.text)?;
        }
    }

    let facts: Vec<&ContextItem> = p.items.iter().filter(|i| i.kind == ItemKind::Fact).collect();
    if !facts.is_empty() {
        writeln!(w)?;
        writeln!(w, "{}", st.head(&format!("Known ({})", facts.len())))?;
        for i in &facts {
            writeln!(w, "  {}", i.text)?;
        }
    }

    if let Some(warning) = &p.project.scope_warning {
        writeln!(w)?;
        writeln!(w, "{} {}", st.warn("Scope warning:"), warning)?;
    }

    writeln!(w)?;
    let excluded = p.items_considered.saturating_sub(p.items_included);
    writeln!(
        w,
        "{}",
        st.dim(&format!(
            "considered {} · included {} · excluded {} · {} of {} tokens",
            p.items_considered, p.items_included, excluded, p.tokens_estimated, p.budget_tokens
        ))
    )?;
    Ok(())
}
```

Add `ContextItem`, `ContextPackage` and `ItemKind` to `render.rs`'s imports from `nexus_core` (it already glob-imports the report types; add `use nexus_core::context::{ContextItem, ContextPackage, ItemKind};`).

- [ ] **Step 3: See it work**

```bash
cargo build -q --bin nexus
T=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/ctx
rm -rf $T && mkdir -p $T/src && cd $T && git init -q -b main
printf 'package a;\npublic class S {\n    public void pay(String k) {\n        System.out.println("pay " + k);\n    }\n}\n' > src/S.java
git add -A && git -c user.name=t -c user.email=t@t commit -qm x
/opt/tools/nexus/target/debug/nexus --project $T scan >/dev/null
/opt/tools/nexus/target/debug/nexus --project $T context --session
/opt/tools/nexus/target/debug/nexus --project $T context --session --json | jq '.result | {tokens_estimated, items_considered, items_included, basis}'
```

Expected: the profile block, a token accounting line, and JSON whose `tokens_estimated` is at most 800.

- [ ] **Step 4: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 200 tests.

```bash
git add crates/nexus-cli/src/main.rs crates/nexus-cli/src/render.rs
git commit -m "cli: nexus context --session (roadmap 1.7)

--session is required rather than defaulted, so that adding --task in Phase 2
does not silently change what a bare 'nexus context' means. No baseline prints
one line and exits 5 rather than raising: the project has simply not been
scanned, and the hook ignores the code.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 4: `nexus init --hooks` and the SessionStart hook

**Files:**
- Create: `crates/nexus-cli/src/hooks.rs`
- Modify: `crates/nexus-cli/src/main.rs` (`mod hooks;`, the `Init` variant, its dispatch arm)
- Create: `crates/nexus-cli/tests/hooks.rs`

**Interfaces:**
- Produces: `hooks::SESSION_START_COMMAND`, `hooks::install(root: &Path) -> std::io::Result<Outcome>`, `hooks::Outcome { Installed, AlreadyPresent }`.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-cli/tests/hooks.rs`:

```rust
//! Hooks are the deterministic invocation tier, and they ship off by default (ADR-024).
//!
//! The property that decides whether they survive contact with a real developer is
//! fail-open: a tool that occasionally hangs or breaks a session is uninstalled once and
//! never reinstalled. That is asserted here by running the hook's own command string with
//! `nexus` absent from `PATH`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn nexus() -> PathBuf {
    // target/debug/deps/<test binary> -> target/debug/nexus
    let mut p = std::env::current_exe().expect("test binary path");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-hooks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run nexus")
}

fn settings(root: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(root.join(".claude/settings.json")).ok()?;
    Some(serde_json::from_str(&raw).expect("settings.json is valid JSON"))
}

fn session_hooks(v: &Value) -> &Vec<Value> {
    v["hooks"]["SessionStart"]
        .as_array()
        .expect("a SessionStart array")
}

#[test]
fn init_writes_no_hooks_by_default() {
    let root = fixture("default");
    let out = run(&root, &["init"]);
    assert!(out.status.success(), "{:?}", out);
    assert!(
        settings(&root).is_none(),
        "hooks are opt-in: plain init must write nothing outside .nexus/ (ADR-024)"
    );
}

#[test]
fn init_with_hooks_installs_the_session_start_hook() {
    let root = fixture("install");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{:?}", out);
    let v = settings(&root).expect("settings.json written");
    let entries = session_hooks(&v);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let cmd = entries[0]["hooks"][0]["command"]
        .as_str()
        .expect("a command string");
    assert!(cmd.contains("context --session"), "{cmd}");
    assert!(
        entries[0]["hooks"][0]["timeout"].is_number(),
        "a hook without a timeout can hang a session: {entries:?}"
    );
}

#[test]
fn installing_twice_changes_nothing() {
    let root = fixture("idempotent");
    run(&root, &["init", "--hooks"]);
    let first = settings(&root).expect("written");
    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{:?}", out);
    let second = settings(&root).expect("still there");
    assert_eq!(first, second, "a second install must not duplicate the hook");
    assert_eq!(session_hooks(&second).len(), 1);
}

#[test]
fn an_existing_settings_file_is_merged_never_clobbered() {
    let root = fixture("merge");
    std::fs::create_dir_all(root.join(".claude")).expect("mkdir");
    std::fs::write(
        root.join(".claude/settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo bye"}]}]}}"#,
    )
    .expect("seed");

    let out = run(&root, &["init", "--hooks"]);
    assert!(out.status.success(), "{:?}", out);
    let v = settings(&root).expect("written");
    assert_eq!(v["model"], "opus", "an unrelated key was destroyed");
    assert_eq!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"], "echo bye",
        "another hook was destroyed"
    );
    assert_eq!(session_hooks(&v).len(), 1, "ours was still added");
}

#[test]
fn the_hook_command_fails_open_when_nexus_is_not_on_path() {
    // The acceptance criterion for 1.8: removing nexus from PATH mid-session must leave the
    // harness fully working. The hook is a shell string, so this runs the real one.
    let root = fixture("failopen");
    run(&root, &["init", "--hooks"]);
    let v = settings(&root).expect("written");
    let cmd = session_hooks(&v)[0]["hooks"][0]["command"]
        .as_str()
        .expect("command")
        .to_string();

    let out = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .env("PATH", "/nonexistent")
        .current_dir(&root)
        .output()
        .expect("sh");
    assert!(
        out.status.success(),
        "the hook must exit 0 with nexus absent: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "and print nothing, or the agent reads an error as context: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-cli --test hooks 2>&1 | grep -E "^test |test result|error:" | head`
Expected: failures — `init --hooks` is an unrecognized argument.

- [ ] **Step 3: Write the hooks module**

Create `crates/nexus-cli/src/hooks.rs`:

```rust
//! Installing the deterministic invocation tier (ADR-024).
//!
//! This lives in the CLI, not in `nexus-core`, and that is deliberate. `.claude/settings.json`
//! is one agent's format; `07-agent-integration.md` §3 says adding an agent is a shim and
//! never a change under `crates/` core. Keeping the format here means the binary's analysis
//! path stays agent-agnostic, which is the property the boundary tests exist to protect.
//!
//! A hook contains no logic. It is one command and one timeout, so a hook regression costs
//! the automatic path and nothing else.

use serde_json::{json, Map, Value};
use std::path::Path;

/// The `SessionStart` command, budget 800 tokens (ADR-024).
///
/// Fail-open is in the string itself rather than in a wrapper script: `|| true` survives a
/// missing binary, a missing baseline (exit 5) and any runtime error, and `2>/dev/null`
/// keeps a diagnostic from being read as context. Removing `nexus` from `PATH` mid-session
/// must leave the harness fully working, and a test asserts exactly this string does.
pub const SESSION_START_COMMAND: &str =
    "nexus context --session --budget 800 2>/dev/null || true";

/// Seconds. A ceiling, not a target: the budget is 400 ms.
const TIMEOUT_SECONDS: u64 = 5;

pub enum Outcome {
    Installed,
    AlreadyPresent,
}

/// Add the `SessionStart` hook to `<root>/.claude/settings.json`, preserving everything else.
///
/// Never clobbers: an existing file is parsed, added to, and written back. A file that is not
/// valid JSON is an error rather than a thing to overwrite — someone's configuration is not
/// ours to discard because we could not read it.
pub fn install(root: &Path) -> std::io::Result<Outcome> {
    let dir = root.join(".claude");
    let path = dir.join("settings.json");

    let mut settings: Value = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not valid JSON ({e}) — fix or move it first", path.display()),
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(e),
    };

    if !settings.is_object() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a JSON object", path.display()),
        ));
    }

    let entries = settings
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("settings is not an object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("hooks is not an object"))?
        .entry("SessionStart")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| std::io::Error::other("SessionStart is not an array"))?;

    // Idempotent on the command, not on exact equality: someone may have edited the timeout.
    let present = entries.iter().any(|e| {
        e["hooks"]
            .as_array()
            .is_some_and(|hs| hs.iter().any(|h| h["command"] == json!(SESSION_START_COMMAND)))
    });
    if present {
        return Ok(Outcome::AlreadyPresent);
    }

    entries.push(json!({
        "hooks": [{
            "type": "command",
            "command": SESSION_START_COMMAND,
            "timeout": TIMEOUT_SECONDS,
        }]
    }));

    std::fs::create_dir_all(&dir)?;
    let mut body = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    body.push('\n');
    std::fs::write(&path, body)?;
    Ok(Outcome::Installed)
}
```

- [ ] **Step 4: Wire it into `init`**

In `crates/nexus-cli/src/main.rs`, add `mod hooks;` beside the other module declarations. Change the `Init` variant to:

```rust
    /// Detect the project, create .nexus/, migrate the database
    Init {
        /// Also install the SessionStart hook for Claude Code. Off by default: a hook whose
        /// latency has not been measured on this project is not turned on uninvited.
        #[arg(long)]
        hooks: bool,
    },
```

Update `fn envelope`'s match arm to `Command::Init { .. } => "init",`, and the dispatch arm to:

```rust
        Command::Init { hooks } => {
            let (_engine, profile) = Engine::init(&root)?;
            let installed = if *hooks {
                Some(hooks::install(&root)?)
            } else {
                None
            };
            emit!(&profile, {
                render::banner(&mut out, &st)?;
                render::profile(&mut out, &st, &profile)?;
                writeln!(out)?;
                // The directory is named once, in nexus-core. Spelling it here again is how
                // it came to report a directory the tool has not created since the rename.
                writeln!(
                    out,
                    "Initialized {}/{}",
                    root.display(),
                    nexus_core::NEXUS_DIR
                )?;
                match installed {
                    Some(hooks::Outcome::Installed) => {
                        writeln!(out, "Installed the SessionStart hook in .claude/settings.json")?;
                    }
                    Some(hooks::Outcome::AlreadyPresent) => {
                        writeln!(out, "The SessionStart hook was already installed")?;
                    }
                    None => {}
                }
                writeln!(out, "  next: {} scan", render::binary_name())?;
            });
        }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-cli --test hooks 2>&1 | grep -E "^test |test result"`
Expected: `test result: ok. 5 passed`

- [ ] **Step 6: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 205 tests.

```bash
git add crates/nexus-cli/src/hooks.rs crates/nexus-cli/src/main.rs crates/nexus-cli/tests/hooks.rs
git commit -m "cli: nexus init --hooks installs the SessionStart hook (roadmap 1.8)

Off by default, because a per-prompt hook whose latency nobody has measured on
this project is the change to how a developer works that the mission forbids.
Fail-open lives in the command string itself, and a test runs that exact string
with nexus absent from PATH: exit 0, nothing on stdout.

An existing settings.json is merged, never clobbered, and one that will not
parse is an error rather than something to overwrite — a configuration is not
ours to discard because we could not read it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 5: Phase 1 is complete, and the documents say so

**Files:**
- Modify: `docs/architecture/10-roadmap.md` (Phase 1 status paragraph)
- Modify: `docs/architecture/README.md` (status line)
- Modify: `docs/architecture/03-current-state.md` (the debt ledger rows Phase 1 paid off)

- [ ] **Step 1: Roadmap**

Replace the Phase 1 status paragraph with:

```
**Status (2026-09-03): complete.** 1.1, 1.2, 1.4, 1.5 landed 2026-09-02; 1.6, 1.7 and 1.8 on
2026-09-03; 1.3 is void, see the row. Success criteria met: `make check` green at 205 tests;
a fact whose evidence symbol is edited stops being retrieved while the row stays on disk
(`nexus-core/tests/fact_invalidation.rs`); `nexus context --session` returns profile, open
findings and durable facts inside 800 estimated tokens
(`nexus-core/tests/session_context.rs`); the `SessionStart` hook exits 0 and prints nothing
with `nexus` absent from `PATH` (`nexus-cli/tests/hooks.rs`). Next: **Phase 2**, starting at
2.1.
```

- [ ] **Step 2: README**

Replace the status line with:

```
**Status:** Phase 0 and Phase 1 complete (1.3 void). Phase 2, the Context Engine, is next.
See [10-roadmap.md](10-roadmap.md).
```

- [ ] **Step 3: Current state**

In the technical-debt ledger table, mark the rows Phase 1 paid off. Append to each of the five rows for `engine.rs`, fact invalidation, `Rule`/`Detector` triplication, the two graph implementations, and `ask.rs` the prefix `**Paid, 2026-09-03.** ` in the Interest column, and change the `nexus-vcs has zero tests` row likewise. Leave the Phase 2/4/5 rows untouched. Add below the table:

```
**Phase 1 closed this ledger's top six rows.** What remains is scheduled for the phase named
in each row; nothing above is unowned.
```

- [ ] **Step 4: Commit**

```bash
git add docs/architecture/10-roadmap.md docs/architecture/README.md docs/architecture/03-current-state.md docs/superpowers/plans/2026-09-03-session-context-and-hook.md
git commit -m "docs: Phase 1 is complete (roadmap 1.7, 1.8)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 6: Acceptance

- [ ] **Step 1: `make check` from a detached worktree of the tip**

```bash
W=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/verify-p1
rm -rf $W; git worktree prune
git worktree add -q --detach $W HEAD && make -C $W check 2>&1 | tail -5
git worktree remove --force $W
```

Expected: green, 205 tests. A failure here is a commit that depends on a file it does not contain.

- [ ] **Step 2: Measure the hook's latency**

ADR-024 budgets `SessionStart` at 400 ms and says the budgets are assertions, not hopes. Phase 1 does not gate CI on it (that is 2.13's harness), but the number must be *known* before the hook is recommended:

```bash
T=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/ctx
for i in 1 2 3 4 5; do
  /usr/bin/time -f %e /opt/tools/nexus/target/release/nexus --project $T context --session > /dev/null
done
```

Build with `make release` first. Record the numbers in the summary. If any run exceeds 400 ms on a small fixture, say so plainly rather than rounding it away.

- [ ] **Step 3: State the acceptance criteria against the code**

- "`nexus context --session` returns profile + open findings + durable facts in ≤ 800 tokens" — `the_package_stays_within_its_budget_and_accounts_for_every_candidate`.
- "The `SessionStart` hook fails open: removing `nexus` from `PATH` mid-session leaves Claude Code fully working" — `the_hook_command_fails_open_when_nexus_is_not_on_path`.
- "`make check` passes; no behavioural surface moved" — Task 6 Step 1.

---

## Self-review

**Spec coverage.** [`05`](../../architecture/05-context-engine.md) §2's five types all exist (`TaskRequest`, `ContextPackage`, `ContextItem`, plus `InclusionLedger` and `ProjectSummary` named in the roadmap row). §7's budget: greedy fill, no padding, exclusion recorded. §8's ledger: every candidate a row, inclusions and exclusions both. §10's basis: `scan_uid`, commit, dirty, and what selected. §12's five prohibitions: no model call anywhere in the module, no file contents (`window` is `None`), no anchorless item, no padding, no silent truncation. [`07`](../../architecture/07-agent-integration.md) §4's output shape drives the renderer. ADR-024: off by default, fail-open, timeout, no logic in the hook.

**Deliberately not covered, and why.** §3 intent, §4 seeds, §5 expand, §6 ranking, §9 compression and §11 caching are Phase 2 by the roadmap's own "do not build yet" list. `ScoreTerms` records zeros and the package's `basis.selection` says the selection was a fixed query, so a caller cannot mistake one for the other.

**Placeholders.** None. Every code step is complete.

**Type consistency.** `Candidate` and `fill` are `pub(crate)` in `context` and used only from `engine/query.rs`, which is the same crate. `ContextItem.kind` is `ItemKind` in the module, the test and the renderer. `estimate_tokens` is the single estimator, used by `Candidate::tokens` and by the project-summary charge. `hooks::SESSION_START_COMMAND` is read by both the installer and the fail-open test, so they cannot drift.

**Risk this plan carries.** The greedy fill is order-dependent, and Phase 1's order is "findings, then facts, in store order". On a project with many open findings the facts can be squeezed out entirely. That is a real limitation of a fixed query, it is visible in the ledger rather than silent, and it is exactly what Phase 2.7's density sort exists to fix. It is named here so that nobody later reads it as a bug in the budgeter.
