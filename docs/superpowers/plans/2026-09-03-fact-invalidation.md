# Fact Invalidation on Change (roadmap 1.6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fact whose evidence points at a symbol that a scan deleted or changed, or at a file a scan deleted, gets `facts.invalidated_at` set and stops being retrieved — while the row stays on disk.

**Architecture:** Before a scan rewrites the index, the engine resolves every live fact's evidence (`[{file,line,note}]`) to an *anchor*: the file, and — when a symbol spans that line — the symbol's `fqn`, `sig_hash` and `body_hash`. Inside the scan's own transaction, after symbols are written, the store checks each anchor against the live index and sets `invalidated_at` on every fact whose anchor no longer holds. One store method does the check in SQL; one engine helper does the resolution; `scan` and `rescan` each gain two lines. The report and the CLI say how many facts were invalidated.

**Tech Stack:** Rust 1.82+, `rusqlite` (store only), `serde_json` (already a `nexus-core` dependency).

**Spec:** [`docs/architecture/06-memory.md`](../../architecture/06-memory.md) §"The invalidation rule" (the three conditions and "rows are kept"), [`docs/memory-model.md`](../../memory-model.md) §2 rule 3, [`docs/architecture/10-roadmap.md`](../../architecture/10-roadmap.md) task 1.6 and its Phase 1 success criterion: *"A fact whose evidence symbol is edited stops being retrieved, **and the row still exists**."*

## Global Constraints

- **Roadmap id 1.6 is the scope.** No lifecycle states, no `validated_scan_id`, no `durable` column, no retrieval ranking — those are Phase 3. A defect found on the way goes in the summary; it is fixed only if it blocks this task.
- **`make check` must pass after every task** — `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. A warning fails the build.
- **Baseline: 184 passing tests.** No task may reduce that count.
- **Only `nexus-store` contains SQL.** The engine never sees `rusqlite::Transaction` by name; it passes `&tx` to static `Store::` functions exactly as `rescan.rs` already does.
- **Ledger tables are append-only** — `scans`, `changes`, `commits`, `finding_occurrences`, `finding_verifications`, `test_runs`, `audit_events`. `facts` is not a ledger: it is "APPEND + SUPERSEDE" and already takes an `UPDATE` for `superseded_by`. Setting `invalidated_at` is the designed second `UPDATE`. Rows are never deleted.
- **`deny(clippy::unwrap_used, clippy::expect_used)` outside tests in `nexus-core`.** Errors propagate with `?`; unreadable evidence becomes a scan warning, never a panic and never silence.
- **Every `RescanReport` / `ScanReport` constructor must be updated** when a field is added — there are three in `rescan.rs` and one in `scan.rs`.
- **`git add` names files.** Never `git add .` or a directory.
- **Commit after every task**, message naming `roadmap 1.6`, ending with:
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/nexus-store/src/lib.rs` | modify | Three structs, three methods, two tests. The SQL that defines "anchor still holds". |
| `crates/nexus-core/src/engine/memory.rs` | create | `Engine::fact_anchors` — evidence JSON → anchors, via the store. The only place evidence is parsed for this purpose. |
| `crates/nexus-core/src/engine/mod.rs` | modify | `mod memory;` |
| `crates/nexus-core/src/engine/rescan.rs` | modify | Two lines: anchors before the transaction, invalidation before commit. Three report constructors gain a field. |
| `crates/nexus-core/src/engine/scan.rs` | modify | The same two lines and one constructor. |
| `crates/nexus-core/src/report.rs` | modify | `facts_invalidated: usize` on `RescanReport` and `ScanReport`. |
| `crates/nexus-cli/src/render.rs` | modify | One line in each of the scan and rescan renderers, shown only when non-zero. |
| `crates/nexus-core/tests/fact_invalidation.rs` | create | The acceptance test: record, edit, rescan, gone-but-kept; plus the cases that must *not* invalidate. |
| `docs/memory-model.md`, `docs/architecture/10-roadmap.md`, `docs/architecture/11-risks.md` | modify | Status lines that say the rule is unimplemented. |

---

### Task 0: Commit the orient-pass document corrections

The working tree carries two uncommitted files from the orient run (the roadmap's 1.3 void row, the 1.2/1.4 wording, the README status). They must land before this task's own docs commit touches the same file.

**Files:**
- Modify (already modified): `docs/architecture/10-roadmap.md`, `docs/architecture/README.md`

- [ ] **Step 1: Branch from main**

```bash
git checkout -b task/1.6-fact-invalidation main
```

- [ ] **Step 2: Commit the two files by name**

```bash
git add docs/architecture/10-roadmap.md docs/architecture/README.md
git commit -m "docs: roadmap and README reflect what the code shows for Phase 1

1.3 is void: the bug* tables it would drop never exist, migration 0003 renamed
them. Verified against a live database, not a grep. 1.2 landed in
nexus_core::rules, not ::capability. 1.4 kept its N+1 on purpose (ba915d3).
README no longer claims no production code has changed.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

- [ ] **Step 3: Confirm the tree is clean**

Run: `git status --short`
Expected: no output.

---

### Task 1: Store — anchors and invalidation-by-change

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` — structs near `FactRow` (line ~309), methods after `facts()` (line ~1790), tests after `a_fact_is_superseded_rather_than_edited` (line ~2320)

**Interfaces:**
- Produces:
  ```rust
  pub struct LiveFact { pub id: i64, pub evidence_json: Option<String> }
  pub struct AnchorSymbol { pub fqn: String, pub sig_hash: String, pub body_hash: String }
  pub struct FactAnchor { pub fact_id: i64, pub path: String, pub symbol: Option<AnchorSymbol> }
  impl Store {
      pub fn live_facts(&self, project_id: ProjectId) -> Result<Vec<LiveFact>>;
      pub fn symbol_at(&self, project_id: ProjectId, path: &str, line: i64) -> Result<Option<AnchorSymbol>>;
      pub fn invalidate_moved_facts(tx: &Transaction<'_>, project_id: ProjectId, anchors: &[FactAnchor], at: &str) -> Result<Vec<i64>>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block, directly after `a_fact_is_superseded_rather_than_edited`:

```rust
    /// One file with one method spanning lines 3–5, so an anchor at line 4 resolves to it
    /// and an anchor at line 1 resolves to the file alone.
    fn index_pay(s: &mut Store, p: ProjectId, scan: ScanId, body_hash: &str) {
        let tx = s.transaction().expect("tx");
        let file = Store::upsert_file(
            &tx,
            p,
            scan,
            "a.java",
            Some("java"),
            "h1",
            10,
            Some(6),
            None,
            ParseStatus::Ok,
            None,
        )
        .expect("upsert");
        Store::replace_symbols(
            &tx,
            p,
            file,
            scan,
            &[NewSymbol {
                kind: SymbolKind::Method,
                name: "pay".into(),
                fqn: "mn.pay.PaymentService#pay".into(),
                parent_fqn: None,
                signature: None,
                visibility: None,
                start_line: 3,
                end_line: 5,
                sig_hash: "s1".into(),
                body_hash: body_hash.into(),
                annotations: vec![],
            }],
        )
        .expect("symbols");
        tx.commit().expect("commit");
    }

    fn fact_at(s: &mut Store, p: ProjectId, scan: ScanId, key: &str, line: u32) -> i64 {
        s.record_fact(
            p,
            scan,
            &NewFact {
                key: key.into(),
                scope: "symbol".into(),
                subject: Some("mn.pay.PaymentService#pay".into()),
                claim: "pay is idempotent".into(),
                source: "ai".into(),
                evidence_json: Some(format!(
                    r#"[{{"file":"a.java","line":{line},"note":""}}]"#
                )),
                confidence: 0.7,
            },
        )
        .expect("fact")
    }

    #[test]
    fn symbol_at_resolves_a_line_to_the_symbol_spanning_it() {
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/anchor", "a", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");
        index_pay(&mut s, p, scan, "b1");

        let hit = s.symbol_at(p, "a.java", 4).expect("query").expect("a symbol spans line 4");
        assert_eq!(hit.fqn, "mn.pay.PaymentService#pay");
        assert_eq!((hit.sig_hash.as_str(), hit.body_hash.as_str()), ("s1", "b1"));
        assert!(
            s.symbol_at(p, "a.java", 1).expect("query").is_none(),
            "line 1 is outside every symbol"
        );
        assert!(
            s.symbol_at(p, "missing.java", 4).expect("query").is_none(),
            "a file not in the index has no symbols"
        );
    }

    #[test]
    fn a_fact_is_invalidated_when_its_symbol_changes_and_the_row_is_kept() {
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/inv", "i", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");
        index_pay(&mut s, p, scan, "b1");
        let id = fact_at(&mut s, p, scan, "invariant.pay.idempotent", 4);
        let symbol = s.symbol_at(p, "a.java", 4).expect("query");
        let anchors = vec![FactAnchor {
            fact_id: id,
            path: "a.java".into(),
            symbol,
        }];

        // Nothing moved: nothing is invalidated.
        let tx = s.transaction().expect("tx");
        let touched = Store::invalidate_moved_facts(&tx, p, &anchors, "2026-09-03T00:00:00Z")
            .expect("check");
        tx.commit().expect("commit");
        assert!(touched.is_empty(), "an intact anchor must not invalidate: {touched:?}");
        assert_eq!(s.facts(p, None).expect("facts").len(), 1);

        // The body moved: the fact is invalidated, once, and stays on disk.
        let (scan2, _) = s
            .begin_scan(p, ScanKind::Incremental, None, None, "h2", false, "{}")
            .expect("scan2");
        index_pay(&mut s, p, scan2, "b2");
        let tx = s.transaction().expect("tx");
        let touched = Store::invalidate_moved_facts(&tx, p, &anchors, "2026-09-03T00:00:01Z")
            .expect("check");
        let again = Store::invalidate_moved_facts(&tx, p, &anchors, "2026-09-03T00:00:02Z")
            .expect("check");
        tx.commit().expect("commit");
        assert_eq!(touched, vec![id]);
        assert!(again.is_empty(), "already-invalidated rows are not counted twice");
        assert!(
            s.facts(p, None).expect("facts").is_empty(),
            "an invalidated fact must not be retrieved"
        );
        let (count, at): (i64, Option<String>) = s
            .conn
            .query_row(
                "SELECT COUNT(*), MAX(invalidated_at) FROM facts WHERE project_id = ?1",
                params![p],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!(count, 1, "the row is kept — invalidation is not deletion");
        assert_eq!(at.as_deref(), Some("2026-09-03T00:00:01Z"), "the first timestamp stands");
    }

    #[test]
    fn a_fact_anchored_in_a_deleted_file_is_invalidated() {
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/del", "d", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");
        index_pay(&mut s, p, scan, "b1");
        // Line 1 is inside no symbol: the anchor is the file alone.
        let id = fact_at(&mut s, p, scan, "convention.header", 1);
        let anchors = vec![FactAnchor {
            fact_id: id,
            path: "a.java".into(),
            symbol: None,
        }];

        let tx = s.transaction().expect("tx");
        Store::mark_file_deleted(&tx, p, "a.java", scan).expect("delete");
        let touched = Store::invalidate_moved_facts(&tx, p, &anchors, "2026-09-03T00:00:00Z")
            .expect("check");
        tx.commit().expect("commit");
        assert_eq!(touched, vec![id]);
        assert!(s.facts(p, None).expect("facts").is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p nexus-store symbol_at a_fact_is_invalidated a_fact_anchored 2>&1 | tail -20`
Expected: compile error — `FactAnchor`, `symbol_at`, `invalidate_moved_facts` not found.

- [ ] **Step 3: Add the structs**

Directly after `pub struct FactRow { ... }` (line ~317):

```rust
/// A fact that is current: neither superseded nor invalidated.
#[derive(Debug, Clone)]
pub struct LiveFact {
    pub id: i64,
    pub evidence_json: Option<String>,
}

/// The symbol a fact's evidence line falls inside, and the hashes it had when the anchor
/// was taken. Either hash moving means the code the fact describes is not the code it
/// described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorSymbol {
    pub fqn: String,
    pub sig_hash: String,
    pub body_hash: String,
}

/// Where one piece of a fact's evidence points, resolved against the index *before* a scan
/// rewrites it. `symbol` is `None` when no symbol spans the line — a config file, an import,
/// a blank line — and the anchor is then the file alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactAnchor {
    pub fact_id: i64,
    pub path: String,
    pub symbol: Option<AnchorSymbol>,
}
```

- [ ] **Step 4: Add the three methods**

Directly after `pub fn facts(...)` (before `// ── aliases ──`):

```rust
    /// Every fact that would be retrieved right now, with its raw evidence. The engine turns
    /// the evidence into `FactAnchor`s; the store does not know what evidence JSON means.
    pub fn live_facts(&self, project_id: ProjectId) -> Result<Vec<LiveFact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, evidence_json FROM facts
             WHERE project_id = ?1 AND superseded_by IS NULL AND invalidated_at IS NULL",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(LiveFact {
                    id: r.get(0)?,
                    evidence_json: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The innermost live symbol spanning `line` in `path`, with its current hashes.
    /// A method inside a class wins over the class: the narrower span is the one the
    /// evidence is about.
    pub fn symbol_at(
        &self,
        project_id: ProjectId,
        path: &str,
        line: i64,
    ) -> Result<Option<AnchorSymbol>> {
        let hit = self
            .conn
            .query_row(
                "SELECT s.fqn, s.sig_hash, s.body_hash
                 FROM live_symbols s JOIN live_files f ON f.id = s.file_id
                 WHERE f.project_id = ?1 AND f.path = ?2
                   AND ?3 BETWEEN s.start_line AND s.end_line
                 ORDER BY (s.end_line - s.start_line) ASC
                 LIMIT 1",
                params![project_id, path, line],
                |r| {
                    Ok(AnchorSymbol {
                        fqn: r.get(0)?,
                        sig_hash: r.get(1)?,
                        body_hash: r.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(hit)
    }

    /// Invalidate every fact whose anchor no longer holds, and return their ids.
    ///
    /// An anchor holds when its file is live and, if it named a symbol, that symbol is live
    /// in that file with the same `sig_hash` and `body_hash`. Anything else — file deleted or
    /// renamed, symbol deleted or renamed, either hash moved — means the fact describes code
    /// that is not there any more. The row is kept: what Nexus believed at a scan, and what
    /// changed its mind, must stay answerable. A fact already invalidated is not counted
    /// again, so the first timestamp stands.
    pub fn invalidate_moved_facts(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        anchors: &[FactAnchor],
        at: &str,
    ) -> Result<Vec<i64>> {
        let mut invalidated = std::collections::BTreeSet::new();
        for anchor in anchors {
            let intact: bool = match &anchor.symbol {
                Some(symbol) => tx.query_row(
                    "SELECT EXISTS (
                       SELECT 1 FROM live_symbols s JOIN live_files f ON f.id = s.file_id
                       WHERE f.project_id = ?1 AND f.path = ?2 AND s.fqn = ?3
                         AND s.sig_hash = ?4 AND s.body_hash = ?5)",
                    params![
                        project_id,
                        anchor.path,
                        symbol.fqn,
                        symbol.sig_hash,
                        symbol.body_hash
                    ],
                    |r| r.get(0),
                )?,
                None => tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM live_files WHERE project_id = ?1 AND path = ?2)",
                    params![project_id, anchor.path],
                    |r| r.get(0),
                )?,
            };
            if intact {
                continue;
            }
            let changed = tx.execute(
                "UPDATE facts SET invalidated_at = ?2
                 WHERE id = ?1 AND invalidated_at IS NULL",
                params![anchor.fact_id, at],
            )?;
            if changed == 1 {
                invalidated.insert(anchor.fact_id);
            }
        }
        Ok(invalidated.into_iter().collect())
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p nexus-store symbol_at a_fact_is_invalidated a_fact_anchored 2>&1 | tail -8`
Expected: `test result: ok. 3 passed`

- [ ] **Step 6: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 187 tests.

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "store: fact anchors and invalidation by change (roadmap 1.6)

facts.invalidated_at was read in the retrieval query and written nowhere. This
adds the write. An anchor is where a piece of evidence points — the file, and
the symbol spanning that line with its hashes — taken before a scan rewrites
the index. After the scan, an anchor that no longer holds invalidates its fact.
The row is kept, and a second pass does not count it again.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 2: Engine — anchors before the scan, invalidation inside it, on the rescan path

**Files:**
- Create: `crates/nexus-core/src/engine/memory.rs`
- Modify: `crates/nexus-core/src/engine/mod.rs:7-10` (module list)
- Modify: `crates/nexus-core/src/report.rs:82-96` (`RescanReport`)
- Modify: `crates/nexus-core/src/engine/rescan.rs` — constructors at lines ~41, ~140, ~510; the transaction at ~226; the commit at ~500
- Create: `crates/nexus-core/tests/fact_invalidation.rs`

**Interfaces:**
- Consumes: `Store::live_facts`, `Store::symbol_at`, `Store::invalidate_moved_facts`, `nexus_store::now()`, `crate::findings::CodeRef { file: String, line: u32, note: String }`.
- Produces: `Engine::fact_anchors(&self, warnings: &mut Vec<String>) -> Result<Vec<FactAnchor>>` (crate-private), `RescanReport.facts_invalidated: usize`.

- [ ] **Step 1: Write the failing integration tests**

Create `crates/nexus-core/tests/fact_invalidation.rs`:

```rust
//! A fact is invalidated by change, not by age.
//!
//! `facts.invalidated_at` was read in the retrieval query and written nowhere, so a fact
//! anchored at `PaymentService#pay():4` outlived that method's deletion and was served
//! forever as established knowledge. These tests pin the rule from memory-model.md §2:
//! edit the anchored symbol and the fact stops surfacing — while the row stays on disk,
//! which the store's own test asserts, because only the store can run SQL.

use nexus_core::findings::CodeRef;
use nexus_core::{Engine, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";

const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    public void pay(String key) {
        System.out.println("pay " + key);
    }
    public void refund(String key) {
        System.out.println("refund " + key);
    }
}
"#;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

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
    let root = std::env::temp_dir().join(format!("nexus-fact-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    root
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "x"]);
}

/// The 1-based line of `pay`'s signature — inside the method's span.
fn pay_line() -> u32 {
    let idx = SOURCE
        .lines()
        .position(|l| l.contains("public void pay"))
        .expect("pay is in the fixture");
    idx as u32 + 1
}

/// Scanned fixture with one fact anchored on `pay`.
fn scanned_with_fact(name: &str, source: &str, evidence: Vec<CodeRef>) -> (PathBuf, Engine) {
    let root = fixture(name);
    write(&root, SERVICE, SOURCE);
    commit(&root);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");
    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is idempotent on key".into(),
            source: source.into(),
            evidence,
            confidence: 0.7,
        })
        .expect("record");
    assert_eq!(engine.facts(None).expect("facts").len(), 1, "the fact is live");
    (root, engine)
}

fn on_pay() -> Vec<CodeRef> {
    vec![CodeRef {
        file: SERVICE.into(),
        line: pay_line(),
        note: String::new(),
    }]
}

fn edit(root: &Path, from: &str, to: &str) {
    let path = root.join(SERVICE);
    let body = fs::read_to_string(&path).expect("read");
    assert!(body.contains(from), "fixture must contain {from:?}");
    fs::write(&path, body.replace(from, to)).expect("write");
}

#[test]
fn editing_the_anchored_symbol_invalidates_the_fact() {
    let (root, mut engine) = scanned_with_fact("edit", "ai", on_pay());
    edit(&root, r#""pay ""#, r#""paid ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 1, "{report:?}");
    assert!(
        engine.facts(None).expect("facts").is_empty(),
        "a fact about code that changed must not be retrieved"
    );
}

#[test]
fn editing_another_symbol_leaves_the_fact_alone() {
    let (root, mut engine) = scanned_with_fact("other", "ai", on_pay());
    edit(&root, r#""refund ""#, r#""refunded ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.symbols_changed, 1, "refund changed: {report:?}");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn a_reformat_does_not_invalidate() {
    // normalize_body is pinned so that a reformat produces zero symbol changes; a fact
    // must ride on that, not on the raw text.
    let (root, mut engine) = scanned_with_fact("reformat", "ai", on_pay());
    edit(
        &root,
        "        System.out.println(\"pay \" + key);",
        "            System.out.println(\"pay \" + key);",
    );

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.symbols_changed, 0, "{report:?}");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn deleting_the_evidence_file_invalidates_the_fact() {
    let (root, mut engine) = scanned_with_fact("delete", "ai", on_pay());
    fs::remove_file(root.join(SERVICE)).expect("rm");

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 1, "{report:?}");
    assert!(engine.facts(None).expect("facts").is_empty());
}

#[test]
fn a_fact_without_evidence_is_never_invalidated() {
    // A human fact with no anchor is about the project, not a line; nothing a scan sees
    // can contradict it.
    let (root, mut engine) = scanned_with_fact("human", "human", Vec::new());
    edit(&root, r#""pay ""#, r#""paid ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn an_invalidated_fact_can_be_re_established_under_the_same_key() {
    let (root, mut engine) = scanned_with_fact("reestablish", "ai", on_pay());
    edit(&root, r#""pay ""#, r#""paid ""#);
    engine.rescan().expect("rescan");
    assert!(engine.facts(None).expect("facts").is_empty());

    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is still idempotent on key".into(),
            source: "ai".into(),
            evidence: on_pay(),
            confidence: 0.7,
        })
        .expect("record again");
    let facts = engine.facts(None).expect("facts");
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].claim, "pay is still idempotent on key");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p nexus-core --test fact_invalidation 2>&1 | tail -20`
Expected: compile error — no field `facts_invalidated` on `RescanReport`.

- [ ] **Step 3: Add the field to `RescanReport`**

In `crates/nexus-core/src/report.rs`, after `pub symbols_changed: usize,`:

```rust
    /// Facts whose evidence pointed at a symbol or file this scan changed or removed. They
    /// stay on disk and stop being retrieved.
    pub facts_invalidated: usize,
```

- [ ] **Step 4: Create `engine/memory.rs`**

```rust
//! What a scan does to what Nexus remembers.
//!
//! A fact is anchored by its evidence — a file and a line. Before a scan rewrites the index
//! the anchor is resolved to the symbol spanning that line and its hashes; after the scan
//! has written its symbols, an anchor that no longer holds invalidates its fact. The check
//! is SQL in `Store::invalidate_moved_facts`; what lives here is the step the store cannot
//! take, because it does not know what evidence JSON means.
//!
//! Evidence that does not parse becomes a scan warning and the fact is left as it is:
//! silently invalidating on a malformed row would look exactly like the rule working.

use super::*;
use nexus_store::FactAnchor;

impl Engine {
    /// Every live fact's evidence, resolved against the index as it is *now*. Call this
    /// before the scan's transaction opens — afterwards the symbols it would compare
    /// against are the new ones, and nothing would ever look moved.
    pub(super) fn fact_anchors(&self, warnings: &mut Vec<String>) -> Result<Vec<FactAnchor>> {
        let mut anchors = Vec::new();
        for fact in self.store.live_facts(self.project_id)? {
            let Some(json) = fact.evidence_json.as_deref() else {
                continue;
            };
            let refs: Vec<CodeRef> = match serde_json::from_str(json) {
                Ok(refs) => refs,
                Err(e) => {
                    warnings.push(format!(
                        "fact {}: evidence is not readable, so it cannot be checked against this scan: {e}",
                        fact.id
                    ));
                    continue;
                }
            };
            for r in refs {
                let symbol = self
                    .store
                    .symbol_at(self.project_id, &r.file, i64::from(r.line))?;
                anchors.push(FactAnchor {
                    fact_id: fact.id,
                    path: r.file,
                    symbol,
                });
            }
        }
        Ok(anchors)
    }
}
```

In `crates/nexus-core/src/engine/mod.rs`, the module list becomes:

```rust
mod analyze;
mod memory;
mod query;
mod rescan;
mod scan;
```

- [ ] **Step 5: Wire the rescan path**

In `crates/nexus-core/src/engine/rescan.rs`:

The two early-return constructors (Tier 0 gate at ~line 41, no-change return at ~line 140) each gain `facts_invalidated: 0,` directly after `symbols_changed: 0,`.

Immediately before `let tx = self.store.transaction()?;` (~line 226, after the `old_by_path` loop):

```rust
        // Where every fact's evidence points, read against the index this scan is about to
        // rewrite. Resolved here for the same reason `old_by_path` is: the transaction holds
        // the connection.
        let anchors = self.fact_anchors(&mut warnings)?;
```

Immediately before `tx.commit().map_err(nexus_store::StoreError::from)?;` (~line 500, after `Store::resolve_edges`):

```rust
        // A fact about code this scan changed or removed is a trap for the next reader.
        // Inside the transaction, so a crash cannot leave the index new and the memory old.
        let facts_invalidated =
            Store::invalidate_moved_facts(&tx, self.project_id, &anchors, &nexus_store::now())?
                .len();
```

The final constructor (~line 510) gains `facts_invalidated,` directly after `symbols_changed,`.

- [ ] **Step 6: Run the integration tests**

Run: `cargo test -p nexus-core --test fact_invalidation 2>&1 | tail -15`
Expected: `test result: ok. 6 passed`

If `a_reformat_does_not_invalidate` fails on `symbols_changed`, that is the Java `normalize_body` invariant failing, not this task: record it in the summary and remove that one test rather than weakening the assertion.

- [ ] **Step 7: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 193 tests.

```bash
git add crates/nexus-core/src/engine/memory.rs crates/nexus-core/src/engine/mod.rs crates/nexus-core/src/engine/rescan.rs crates/nexus-core/src/report.rs crates/nexus-core/tests/fact_invalidation.rs
git commit -m "core: a rescan invalidates facts whose evidence moved (roadmap 1.6)

Anchors are resolved before the transaction, against the index the scan is
about to rewrite; the check runs inside it, after the symbols are written, so
a crash cannot leave the index new and the memory old. A fact with no evidence
is never touched. A fact whose evidence does not parse is warned about and
left alone — invalidating it would look exactly like the rule working.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 3: The full-scan path, and the CLI says so

**Files:**
- Modify: `crates/nexus-core/src/report.rs:40-59` (`ScanReport`)
- Modify: `crates/nexus-core/src/engine/scan.rs` — before `let tx` (~line 46), before `tx.commit()` (~line 91), constructor (~line 126)
- Modify: `crates/nexus-cli/src/render.rs:148` (scan) and `:224` (rescan)
- Modify: `crates/nexus-core/tests/fact_invalidation.rs` (one more test)

**Interfaces:**
- Produces: `ScanReport.facts_invalidated: usize`.

- [ ] **Step 1: Write the failing test**

Append to `crates/nexus-core/tests/fact_invalidation.rs`:

```rust
#[test]
fn a_full_scan_invalidates_too() {
    // `scan` on an already-indexed project re-parses everything and records no changes
    // ledger, so the rule cannot ride on the ledger. It rides on the anchor's hashes,
    // which both paths have.
    let (root, mut engine) = scanned_with_fact("fullscan", "ai", on_pay());
    edit(&root, r#""pay ""#, r#""paid ""#);

    let report = engine.scan().expect("scan");
    assert_eq!(report.facts_invalidated, 1, "{report:?}");
    assert!(engine.facts(None).expect("facts").is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-core --test fact_invalidation a_full_scan 2>&1 | tail -10`
Expected: compile error — no field `facts_invalidated` on `ScanReport`.

- [ ] **Step 3: Add the field and wire `scan.rs`**

In `report.rs`, after `pub symbols_indexed: usize,` on `ScanReport`:

```rust
    /// Facts whose evidence pointed at a symbol or file this scan changed or removed.
    /// Always zero on a first scan: there is nothing to remember yet.
    pub facts_invalidated: usize,
```

In `scan.rs`, immediately before `let tx = self.store.transaction()?;`:

```rust
        // Where every fact's evidence points, read before the index is rewritten.
        let anchors = self.fact_anchors(&mut warnings)?;
```

Immediately before `tx.commit().map_err(nexus_store::StoreError::from)?;`:

```rust
        let facts_invalidated =
            Store::invalidate_moved_facts(&tx, self.project_id, &anchors, &nexus_store::now())?
                .len();
```

In the `ScanReport { ... }` constructor, after `symbols_indexed,`: `facts_invalidated,`.

- [ ] **Step 4: Render it**

In `crates/nexus-cli/src/render.rs`, after the scan renderer's `writeln!(w, "  symbols      {}", r.symbols_indexed)?;`:

```rust
    if r.facts_invalidated > 0 {
        writeln!(
            w,
            "  facts        {} invalidated — evidence moved",
            r.facts_invalidated
        )?;
    }
```

After the rescan renderer's `writeln!(w, "  {} symbols", r.symbols_changed)?;`:

```rust
    if r.facts_invalidated > 0 {
        writeln!(
            w,
            "  {} facts invalidated — their evidence moved",
            r.facts_invalidated
        )?;
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core --test fact_invalidation 2>&1 | tail -12`
Expected: `test result: ok. 7 passed`

- [ ] **Step 6: See it on the binary**

```bash
T=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/demo
rm -rf $T && mkdir -p $T/src && cd $T && git init -q -b main
printf 'package a;\npublic class S {\n    public void pay(String k) {\n        System.out.println("pay " + k);\n    }\n}\n' > src/S.java
git add -A && git commit -qm x
cargo run -q --bin nexus -- --project $T scan >/dev/null
cargo run -q --bin nexus -- --project $T fact add invariant.pay "pay is idempotent" --evidence src/S.java:3 2>&1 || true
```

If the CLI has no `fact` verb (it is Phase 3.5), record the fact over the store instead and move on — the binary check is the `rescan` line, which the JSON also carries:

```bash
sed -i 's/"pay "/"paid "/' src/S.java
cargo run -q --bin nexus -- --project $T rescan --json | jq .facts_invalidated
```

Expected: `0` if no fact could be recorded from the CLI, `1` otherwise. Either way the key is present, which is the surface this task adds.

- [ ] **Step 7: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 194 tests.

```bash
git add crates/nexus-core/src/report.rs crates/nexus-core/src/engine/scan.rs crates/nexus-cli/src/render.rs crates/nexus-core/tests/fact_invalidation.rs
git commit -m "core, cli: a full scan invalidates too, and both reports say how many (roadmap 1.6)

scan records no changes ledger, so the rule rides on anchor hashes rather than
on changes rows — the same check serves both paths. The count is on ScanReport
and RescanReport, in --json always and in the human output when non-zero.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 4: The documents stop saying it is unimplemented

**Files:**
- Modify: `docs/memory-model.md:3-6` (status block)
- Modify: `docs/architecture/10-roadmap.md` (Phase 1 status paragraph)
- Modify: `docs/architecture/11-risks.md:85-100` (R5)

- [ ] **Step 1: `docs/memory-model.md`**

Replace the last sentence of the status block, `Invalidation-by-change is specified in §2 rule 3 and is not implemented.`, with:

```
> Invalidation-by-change (§2 rule 3) is implemented: `Engine::fact_anchors` resolves each
> fact's evidence before a scan, and `Store::invalidate_moved_facts` sets `invalidated_at`
> inside the scan's transaction when the file is gone or the symbol at the anchor is gone or
> has a different `sig_hash` or `body_hash`. Rows are kept. Pinned by
> `crates/nexus-core/tests/fact_invalidation.rs`.
```

- [ ] **Step 2: `docs/architecture/10-roadmap.md`**

Replace the Phase 1 status paragraph's sentence beginning `1.6, 1.7, 1.8 are not started` through `Next in order: **1.6**.` with:

```
1.6 landed on 2026-09-03 (`crates/nexus-core/src/engine/memory.rs`,
`Store::invalidate_moved_facts`, `tests/fact_invalidation.rs`). 1.7 and 1.8 are not
started — no `ContextPackage` type exists under `crates/`, and the binary has no `context`
command and no `--hooks` flag. Next in order: **1.7**.
```

- [ ] **Step 3: `docs/architecture/11-risks.md`**

After R5's `**Mitigation:**` paragraph, add:

```
**Status (2026-09-03):** mitigated. The detection test above exists as
`crates/nexus-core/tests/fact_invalidation.rs::editing_the_anchored_symbol_invalidates_the_fact`;
the row-kept half is `nexus-store`'s `a_fact_is_invalidated_when_its_symbol_changes_and_the_row_is_kept`.
```

- [ ] **Step 4: Commit**

```bash
git add docs/memory-model.md docs/architecture/10-roadmap.md docs/architecture/11-risks.md docs/superpowers/plans/2026-09-03-fact-invalidation.md
git commit -m "docs: fact invalidation is built; the design of record and roadmap say so (roadmap 1.6)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 5: Acceptance from a fresh worktree

- [ ] **Step 1: `make check` on a clean checkout of the tip**

```bash
W=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/verify
git worktree add -q $W task/1.6-fact-invalidation
make -C $W check 2>&1 | tail -5
git worktree remove --force $W
```

Expected: green, 194 tests. Anything else is a commit that depends on a file it does not contain — find the file with `git status` in the main tree and amend the right commit.

- [ ] **Step 2: The acceptance criterion, stated against the code**

- "A fact whose evidence symbol is edited stops being retrieved" — `editing_the_anchored_symbol_invalidates_the_fact` asserts `engine.facts(None)` is empty after the rescan.
- "and the row still exists" — `a_fact_is_invalidated_when_its_symbol_changes_and_the_row_is_kept` asserts `COUNT(*) = 1` with `invalidated_at` set.

---

## Self-review

**Spec coverage.** 06-memory's three conditions: file deleted → `invalidate_moved_facts` file branch (store test 3, engine `deleting_the_evidence_file`); symbol at anchor deleted → the symbol branch finds no row (covered by the file-deleted engine test, since the file's symbols go with it, and by rename via fqn mismatch); hashes changed → symbol branch with `sig_hash`/`body_hash` equality (store test 2, engine `editing_the_anchored_symbol`). "Rows are kept" → store test 2. "Re-established via `superseded_by`" → `an_invalidated_fact_can_be_re_established_under_the_same_key`. Roadmap 1.6's wording, "when a scan moves a symbol" → both `scan` and `rescan` paths.

**Not in the spec, decided here.** Evidence naming a file that was never in the index resolves to a file-only anchor that is not live, so it is invalidated at the next scan. That is the honest reading — the fact could never be checked — and it is what Phase 3's "refuse at the boundary" will make unreachable. Noted in the summary.

**Placeholders.** None: every code step is complete.

**Type consistency.** `FactAnchor { fact_id: i64, path: String, symbol: Option<AnchorSymbol> }` is used identically in Task 1's tests, Task 2's `memory.rs` and both wiring sites. `invalidate_moved_facts` returns `Vec<i64>`; both callers take `.len()`. `CodeRef.line` is `u32`; `symbol_at` takes `i64`, converted with `i64::from`.
