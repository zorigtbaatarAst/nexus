# Retrieval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A context request costs the same whether the project remembers 100 facts or 200,000, and a symptom written in plain words finds the code it is about.

**Architecture:** `memory::subject_match`'s predicate is pushed into SQL so retrieval loads only the facts that could match, riding an index that has existed since the first migration and was never used. The seed stage stops refusing plain lowercase words, guarded by a uniqueness rule that already exists in the graphify importer. The session package takes durable facts only, which is what its own documentation has always claimed.

**Tech Stack:** Rust 1.82+, `rusqlite` 0.40 (bundled SQLite 3.53), no new dependencies.

**Spec:** [`docs/superpowers/specs/2026-09-03-retrieval-design.md`](../specs/2026-09-03-retrieval-design.md), which absorbs [`2026-09-03-knowledge-selectivity-design.md`](../specs/2026-09-03-knowledge-selectivity-design.md).

## Global Constraints

- **`make check` after every task**, and it must be green. Baseline: **412 passing tests**.
- CI runs with `RUSTFLAGS=-D warnings`. A warning fails the build — including an unused `mut` or an unused import.
- **`git add` names files.** A directory sweeps in untracked work from another task.
- One commit per task, its message naming what changed and why.
- **`nexus-core` must not name `rusqlite`.** Only `nexus-store` may. `crates/nexus-cli/tests/boundaries.rs` asserts this and fails the build otherwise.
- **The store returns rows; it never ranks.** §4's formula lives in `nexus_core::memory` and every consumer calls that one function. Two rankings over one table would disagree.
- Ledger tables stay append-only: `scans`, `changes`, `commits`, `finding_occurrences`, `finding_verifications`, `test_runs`, `audit_events`.
- **No new tables in this plan.** `crates/nexus-store/src/lib.rs`'s `all_twenty_one_tables_exist` test asserts exactly 21; adding one is the *next* plan's problem.
- Commit trailer:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/nexus-store/src/lib.rs` | `subject_prefixes`, `facts_for_seeds`, `durable_facts` — SQL only | 1, 3 |
| `crates/nexus-core/src/engine/query.rs` | task path calls `facts_for_seeds`; session path calls `durable_facts`; the cap and its note | 2, 3 |
| `crates/nexus-core/src/context/seeds.rs` | `targets`, `STOPWORDS`, `uniquely_named_symbol`, `last_segment` | 4, 5 |
| `crates/nexus-core/src/engine/memory.rs` | graphify import calls the shared `uniquely_named_symbol`; the claim filter | 4, 6 |
| `crates/nexus-core/src/graphify.rs` | `is_a_claim` and fixture exclusion at read time | 6 |
| `crates/nexus-core/src/report.rs` | `ImportReport.skipped_not_a_claim` | 6 |
| `crates/nexus-core/tests/memory_scale.rs` | **new** — retrieval is bounded, ancestors and descendants still reachable | 2 |
| `crates/nexus-core/tests/symptom_seeds.rs` | **new** — a symptom finds its code; stopwords and ambiguity seed nothing | 5 |

---

## Task 1: `facts_for_seeds` in the store

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` — add `subject_prefixes` and `facts_for_seeds` beside the existing `facts`; add tests to the existing `mod tests`.

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub(crate) fn subject_prefixes(fqn: &str) -> Vec<String>`
  - `pub fn facts_for_seeds(&self, project_id: ProjectId, seeds: &[String]) -> Result<Vec<FactRow>>`

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` at the bottom of `crates/nexus-store/src/lib.rs`:

```rust
    #[test]
    fn a_subject_yields_itself_and_every_ancestor() {
        assert_eq!(
            subject_prefixes("nexus_core::context::cache"),
            vec![
                "nexus_core::context::cache".to_string(),
                "nexus_core".to_string(),
                "nexus_core::context".to_string(),
            ]
        );
        assert_eq!(
            subject_prefixes("mn.pay.PaymentService#pay"),
            vec![
                "mn.pay.PaymentService#pay".to_string(),
                "mn".to_string(),
                "mn.pay".to_string(),
                "mn.pay.PaymentService".to_string(),
            ]
        );
        assert_eq!(subject_prefixes("bare"), vec!["bare".to_string()]);
    }

    #[test]
    fn facts_for_seeds_finds_ancestors_and_descendants_and_nothing_else() {
        let mut s = Store::open_in_memory().expect("open");
        let (pid, scan) = seeded_project(&mut s);
        for (key, subject) in [
            ("arch.exact", "a::b::C"),
            ("arch.ancestor", "a::b"),
            ("arch.descendant", "a::b::C#m"),
            ("arch.sibling", "a::b::D"),
            ("arch.unrelated", "z::q"),
        ] {
            s.record_fact(
                pid,
                scan,
                &NewFact {
                    key: key.into(),
                    scope: "symbol".into(),
                    subject: Some(subject.into()),
                    claim: format!("about {subject}"),
                    source: "human".into(),
                    evidence_json: None,
                    confidence: 1.0,
                },
            )
            .expect("record");
        }

        let got: Vec<String> = s
            .facts_for_seeds(pid, &["a::b::C".to_string()])
            .expect("query")
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(
            got,
            vec![
                "arch.ancestor".to_string(),
                "arch.descendant".to_string(),
                "arch.exact".to_string(),
            ],
            "the seed itself, the module above it, and the method below it — not the sibling"
        );

        assert!(
            s.facts_for_seeds(pid, &[]).expect("empty").is_empty(),
            "no seeds is no query, not every fact"
        );
    }

    #[test]
    fn a_large_seed_set_stays_inside_sqlites_parameter_limit() {
        // The failure mode of exceeding it is a runtime error on a large project and never
        // on a fixture, so the bound is asserted rather than assumed.
        let mut s = Store::open_in_memory().expect("open");
        let (pid, _) = seeded_project(&mut s);
        let seeds: Vec<String> = (0..256)
            .map(|i| format!("crate{i}::module{i}::Type{i}#method{i}"))
            .collect();
        assert!(
            s.facts_for_seeds(pid, &seeds).is_ok(),
            "256 seeds must not exceed SQLITE_MAX_VARIABLE_NUMBER"
        );
    }
```

And the shared fixture helper, also inside `mod tests`:

```rust
    /// A project with one completed scan, so `record_fact`'s foreign key to `scans` holds.
    fn seeded_project(s: &mut Store) -> (ProjectId, ScanId) {
        let pid = s.upsert_project("t", "/tmp/t").expect("project");
        let scan = s.begin_scan(pid, "full", None, false).expect("scan");
        s.finish_scan(scan, "ok").expect("finish");
        (pid, scan)
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p nexus-store subject_prefixes facts_for_seeds parameter_limit`
Expected: FAIL — `cannot find function 'subject_prefixes'`, `no method named 'facts_for_seeds'`.

If `seeded_project` fails to compile because `upsert_project`, `begin_scan` or `finish_scan` have different signatures, read the existing `mod tests` in that file and copy whatever shape the neighbouring tests already use to get a project and a scan. Do not invent a shape.

- [ ] **Step 3: Write `subject_prefixes`**

Add near `simple_key` at the bottom of `crates/nexus-store/src/lib.rs`:

```rust
/// A subject and every ancestor of it: `a::b::C#m` yields `a::b::C#m`, `a`, `a::b`, `a::b::C`.
///
/// `memory::subject_match` scores 0.6 when either string is a prefix of the other. This is the
/// half of that rule SQL can answer with an equality set; the other half is a range scan.
/// Separators are ASCII, so every cut lands on a char boundary.
pub(crate) fn subject_prefixes(fqn: &str) -> Vec<String> {
    let mut out = vec![fqn.to_string()];
    let bytes = fqn.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let sep = match bytes[i] {
            b'#' | b'.' => Some(1),
            b':' if bytes.get(i + 1) == Some(&b':') => Some(2),
            _ => None,
        };
        match sep {
            Some(len) => {
                if i > 0 {
                    out.push(fqn[..i].to_string());
                }
                i += len;
            }
            None => i += 1,
        }
    }
    out
}
```

- [ ] **Step 4: Write `facts_for_seeds`**

Add immediately after the existing `pub fn facts(...)` in `crates/nexus-store/src/lib.rs`:

```rust
    /// The facts that could match any of these seeds, and only those.
    ///
    /// `facts(project_id, None)` loads every live fact and lets `nexus_core::memory` discard
    /// the irrelevant ones in Rust — 14 ms at zero facts, 274 ms at 200,000, on a path
    /// ADR-024 budgets 150 ms for. This asks the question in SQL instead, over
    /// `idx_facts_subject`, which has existed since `0001_init.sql` and was never used
    /// because the hot path passed `None`.
    ///
    /// Two arms, mirroring `memory::subject_match` exactly:
    ///   * equality against every seed and every ancestor of it — a fact about the module a
    ///     seed lives in is a fact about the seed;
    ///   * a half-open range per seed — a fact about something *below* the seed. Written as a
    ///     range rather than `LIKE seed || '%'` because a range on an indexed column is an
    ///     index seek and `LIKE` with a bound parameter is not guaranteed to be.
    ///
    /// The Rust-side filter still runs. This narrows what it has to look at; it does not
    /// replace the one definition of a match.
    pub fn facts_for_seeds(
        &self,
        project_id: ProjectId,
        seeds: &[String],
    ) -> Result<Vec<FactRow>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let exact: std::collections::BTreeSet<String> = seeds
            .iter()
            .flat_map(|s| subject_prefixes(s))
            .collect();
        let exact: Vec<String> = exact.into_iter().collect();

        let mut sql = String::from(
            "SELECT fact_key, scope, subject, claim, source, confidence, evidence_json,
                    validated_count, durable, created_scan_id
             FROM facts
             WHERE project_id = ?1
               AND superseded_by IS NULL AND invalidated_at IS NULL
               AND (subject IN (",
        );
        sql.push_str(
            &(0..exact.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(","),
        );
        sql.push(')');
        let range_base = 2 + exact.len();
        for i in 0..seeds.len() {
            sql.push_str(&format!(
                " OR (subject >= ?{} AND subject < ?{})",
                range_base + i * 2,
                range_base + i * 2 + 1
            ));
        }
        sql.push_str(") ORDER BY fact_key");

        let mut values: Vec<rusqlite::types::Value> =
            Vec::with_capacity(1 + exact.len() + seeds.len() * 2);
        values.push(rusqlite::types::Value::Integer(project_id));
        for e in &exact {
            values.push(rusqlite::types::Value::Text(e.clone()));
        }
        for s in seeds {
            values.push(rusqlite::types::Value::Text(s.clone()));
            // The largest code point, so the range covers every descendant and stops before
            // the next distinct subject. SQLite compares TEXT byte-wise by default.
            values.push(rusqlite::types::Value::Text(format!("{s}\u{10FFFF}")));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values), |r| {
                Ok(FactRow {
                    key: r.get(0)?,
                    scope: r.get(1)?,
                    subject: r.get(2)?,
                    claim: r.get(3)?,
                    source: r.get(4)?,
                    confidence: r.get(5)?,
                    evidence_json: r.get(6)?,
                    validated_count: r.get(7)?,
                    durable: r.get::<_, i64>(8)? == 1,
                    created_scan_id: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

If `ProjectId` is not a plain `i64`, replace `Value::Integer(project_id)` with `Value::Integer(project_id as i64)` — check the type alias at the top of the file rather than guessing.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p nexus-store subject_prefixes facts_for_seeds parameter_limit`
Expected: PASS, 3 tests.

- [ ] **Step 6: `make check`**

Run: `make check`
Expected: exit 0, 415 passing.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "feat(store): ask SQL for the facts that could match a seed set

facts(project_id, None) loads every live fact and lets nexus-core discard the
irrelevant ones in Rust: 14ms at zero facts, 274ms at 200,000, on a path
ADR-024 budgets 150ms for. facts_for_seeds asks the question in SQL instead,
over idx_facts_subject — an index that has existed since 0001_init.sql and was
never used, because the hot path passed None.

Two arms mirror memory::subject_match exactly: equality against every seed and
its ancestors, and a half-open range per seed for facts about something below
it. The range is written as >= and < rather than LIKE because a range on an
indexed column is an index seek and LIKE with a bound parameter is not
guaranteed to be.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 2: The task package uses it, and says when it capped

**Files:**
- Modify: `crates/nexus-core/src/engine/query.rs` — the fact-candidate block in `task_package` (search for `crate::memory::rank(` with `self.store.facts(self.project_id, None)?`).
- Create: `crates/nexus-core/tests/memory_scale.rs`

**Interfaces:**
- Consumes: `Store::facts_for_seeds(project_id, &[String]) -> Result<Vec<FactRow>>` from Task 1.
- Produces: `const SEED_QUERY_CAP: usize = 256;` in `engine/query.rs`.

**A deliberate behaviour change, and why.** Today a task request with *no* seeds loads every fact and keeps them all, because the guard reads `if !relevant.is_empty() && to_seeds <= 0.3 { continue; }`. That is the alphabetical flood measured in the session package, in the other package. With `facts_for_seeds` an empty seed set returns nothing, which is correct: a package with nothing to anchor a fact to cannot honour §12's rule that every item carries a `file:line` that relates to the request. Step 1's third assertion pins it.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/tests/memory_scale.rs`:

```rust
//! Retrieval cost is bounded by what the request is about, not by what the project remembers.
//!
//! `facts(project_id, None)` loaded every live fact on every request and ranked them in Rust:
//! 14 ms at zero facts, 274 ms at 200,000, against ADR-024's 150 ms budget for a per-prompt
//! hook. Memory is append-only by design, so that number only ever grows.

use nexus_core::context::{Purpose, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::findings::CodeRef;
use nexus_core::{Engine, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    private PaymentRepository repo;
    public void pay(String key) { repo.save(key); }
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

/// One directory per test. Two tests sharing a temp directory delete each other's files,
/// which passes locally and fails on a clean checkout.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-scale-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/mn/pay")).expect("mkdir");
    fs::write(root.join(SERVICE), SOURCE).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn task(text: &str) -> TaskRequest {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    r
}

fn record(engine: &mut Engine, key: &str, subject: &str) {
    engine
        .record_fact(FactInput {
            key: key.into(),
            scope: "symbol".into(),
            subject: Some(subject.into()),
            claim: format!("something true about {subject}"),
            source: "human".into(),
            evidence: vec![CodeRef {
                file: SERVICE.into(),
                line: 4,
                note: String::new(),
            }],
            confidence: 1.0,
        })
        .expect("record");
}

#[test]
fn unrelated_memory_does_not_enter_the_package_at_all() {
    let (_root, mut engine) = scanned("unrelated");
    let before = engine
        .context(&task("refactor mn.pay.PaymentService#pay"))
        .expect("context")
        .items_considered;

    for i in 0..2_000 {
        record(&mut engine, &format!("arch.noise-{i:05}"), &format!("other.Module{i}"));
    }

    let after = engine
        .context(&task("refactor mn.pay.PaymentService#pay"))
        .expect("context")
        .items_considered;
    assert_eq!(
        before, after,
        "2,000 facts about symbols this request never mentions must not become candidates"
    );
}

#[test]
fn an_ancestor_and_a_descendant_of_a_seed_are_both_retrieved() {
    let (_root, mut engine) = scanned("family");
    record(&mut engine, "arch.module", "mn.pay");
    record(&mut engine, "arch.exact", "mn.pay.PaymentService");
    record(&mut engine, "arch.member", "mn.pay.PaymentService#pay");
    record(&mut engine, "arch.elsewhere", "mn.billing.Invoice");

    let pkg = engine
        .context(&task("refactor mn.pay.PaymentService"))
        .expect("context");
    let claims: Vec<&str> = pkg.items.iter().map(|i| i.text.as_str()).collect();
    for want in ["mn.pay", "mn.pay.PaymentService", "mn.pay.PaymentService#pay"] {
        assert!(
            claims.iter().any(|c| c.contains(want)),
            "a fact about {want} belongs in a package about PaymentService: {claims:?}"
        );
    }
    assert!(
        !claims.iter().any(|c| c.contains("mn.billing.Invoice")),
        "a fact about another module does not: {claims:?}"
    );
}

#[test]
fn a_request_that_anchors_to_nothing_carries_no_facts() {
    // Deliberate. Serving every fact ranked by a subject_match term that is a constant 0.3
    // is the alphabetical flood, and §12 forbids an item with no anchor to the request.
    let (_root, mut engine) = scanned("anchorless");
    record(&mut engine, "arch.a", "mn.pay.PaymentService");
    let pkg = engine
        .context(&task("please make the thing work properly"))
        .expect("context");
    assert_eq!(
        pkg.items_considered, 0,
        "no seeds means no memory query: {:?}",
        pkg.items
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-core --test memory_scale`
Expected: FAIL — `unrelated_memory_does_not_enter_the_package_at_all` reports `before` far smaller than `after`, because every one of the 2,000 noise facts is currently a candidate.

- [ ] **Step 3: Add the cap constant**

In `crates/nexus-core/src/engine/query.rs`, next to the other module-level constants near the top:

```rust
/// How many symbols the memory query may name.
///
/// Expansion runs to `max_depth: 5` with no node cap — a four-symbol prompt on this
/// repository already reaches 189 — and each seed becomes bound parameters. The cap is
/// generous enough never to bite in practice and low enough that it cannot surprise SQLite.
/// When it does bite the package says so, because a silently narrowed query is an error.
const SEED_QUERY_CAP: usize = 256;
```

- [ ] **Step 4: Cap the seed list and call the new query**

In `task_package`, replace this:

```rust
        let relevant: Vec<String> = seeded
            .seeds
            .iter()
            .map(|s| s.symbol.fqn.clone())
            .chain(reached.items.iter().map(|i| i.fqn.clone()))
            .collect();
        let current_scan = self.current_scan_id()?;
        for row in crate::memory::rank(
            self.store.facts(self.project_id, None)?,
            &relevant,
            current_scan,
        ) {
```

with this:

```rust
        let mut relevant: Vec<String> = seeded
            .seeds
            .iter()
            .map(|s| s.symbol.fqn.clone())
            .chain(reached.items.iter().map(|i| i.fqn.clone()))
            .collect();
        if relevant.len() > SEED_QUERY_CAP {
            notes.push(format!(
                "{} symbols are relevant here; memory was queried for the first \
                 {SEED_QUERY_CAP}, so a fact about the outer edge of the expansion may be \
                 missing",
                relevant.len()
            ));
            relevant.truncate(SEED_QUERY_CAP);
        }
        let current_scan = self.current_scan_id()?;
        for row in crate::memory::rank(
            self.store.facts_for_seeds(self.project_id, &relevant)?,
            &relevant,
            current_scan,
        ) {
```

`notes` is already declared earlier in this function as `let mut notes = seeded.notes.clone();`. If the borrow checker objects because `notes` is moved into the package before this point, move the `relevant` block up to sit immediately after `notes` is declared — do not clone `notes`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p nexus-core --test memory_scale`
Expected: PASS, 3 tests.

- [ ] **Step 6: `make check`, and rebaseline the goldens if they moved**

Run: `make check`

If `five_golden_packages_hold` fails, the ranking did not change — the *candidate set* did, and a golden that previously included a fact about an unrelated symbol will lose it. Read the reported diff. If every removed item is a fact whose subject is unrelated to that golden's task, rebaseline:

```bash
NEXUS_REBASELINE=1 cargo test -p nexus-core --test golden_packages
git diff crates/nexus-core/tests/golden/
```

Expected after rebaseline: `make check` exit 0, 418 passing.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-core/src/engine/query.rs crates/nexus-core/tests/memory_scale.rs crates/nexus-core/tests/golden/
git commit -m "perf(context): a task package costs what the request is about, not what the project remembers

The task path loaded every live fact and discarded the irrelevant ones in Rust.
It now asks facts_for_seeds for the ones that could match. 2,000 facts about
symbols a request never mentions no longer become candidates for it.

The relevant set is capped at 256 symbols, and the package says so when the cap
bites — expansion has max_depth 5 and no node cap, and a silently narrowed
query is an error.

One deliberate behaviour change: a request that anchors to nothing now carries
no facts, where before it carried all of them ranked by a subject_match term
that is a constant 0.3. That is the alphabetical flood, and §12 forbids an item
with no anchor to the request.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 3: The session package carries durable facts

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` — add `durable_facts`.
- Modify: `crates/nexus-core/src/engine/query.rs` — `session_package`'s fact loop and its `basis.selection` string.
- Modify: `crates/nexus-core/tests/session_context.rs` — add one test.

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub fn durable_facts(&self, project_id: ProjectId) -> Result<Vec<FactRow>>`.

**Why this is not a new rule.** `session_package`'s doc comment says "durable facts". Its `basis.selection` string says `"phase-1 fixed query: open findings then durable facts, in store order"`. `06-memory.md` §3 gives `Durable` the highest retrieval weight. The code carries the reason for the gap — *"The lifecycle states are Phase 3.1, so 'durable' is approximated by the order the store already returns"* — and Phase 3 landed. The approximation was never unwound.

- [ ] **Step 1: Write the failing test**

Append to `crates/nexus-core/tests/session_context.rs`:

```rust
#[test]
fn the_session_package_carries_only_facts_that_earned_their_place() {
    // `Store::facts` is ORDER BY fact_key, and its own comment says so: "stable, so a caller
    // can rely on it, and *not* a ranking". The session path consumed that order as if it
    // were a priority. With 671 imported claims it bought the first nine alphabetically and
    // cost 752 tokens where it had cost 194.
    let mut engine = scanned("durable");
    for i in 0..40 {
        engine
            .record_fact(FactInput {
                key: format!("arch.aaa-candidate-{i:03}"),
                scope: "symbol".into(),
                subject: Some("mn.pay.PaymentService#pay".into()),
                claim: format!("unverified claim {i} sorted early by key"),
                source: "ai".into(),
                evidence: anchored_on_pay(),
                confidence: 0.5,
            })
            .expect("record");
    }
    engine
        .record_fact(FactInput {
            key: "invariant.zzz.settles-once".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "a payment settles exactly once".into(),
            source: "human".into(),
            evidence: anchored_on_pay(),
            confidence: 1.0,
        })
        .expect("record");

    let pkg = session(&engine);
    let facts: Vec<&str> = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .map(|i| i.text.as_str())
        .collect();
    assert!(
        facts.iter().any(|t| t.contains("settles exactly once")),
        "a human fact is durable on arrival and belongs here: {facts:?}"
    );
    assert!(
        !facts.iter().any(|t| t.contains("unverified claim")),
        "an ai candidate has not earned session budget, whatever its key sorts as: {facts:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-core --test session_context earned_their_place`
Expected: FAIL on the second assertion — the alphabetically-early candidates fill the budget.

- [ ] **Step 3: Add `durable_facts` to the store**

Immediately after `facts_for_seeds` in `crates/nexus-store/src/lib.rs`:

```rust
    /// Facts that have earned the highest retrieval weight: three surviving scans, or a
    /// person wrote them.
    ///
    /// The session package is what a session starts from with no task in hand. Starting from
    /// unverified guesses ordered by key is the trap §3's `Observed → dropped` edge exists to
    /// prevent. Rides `idx_facts_state`.
    pub fn durable_facts(&self, project_id: ProjectId) -> Result<Vec<FactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT fact_key, scope, subject, claim, source, confidence, evidence_json,
                    validated_count, durable, created_scan_id
             FROM facts
             WHERE project_id = ?1
               AND superseded_by IS NULL AND invalidated_at IS NULL
               AND durable = 1
             ORDER BY fact_key",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(FactRow {
                    key: r.get(0)?,
                    scope: r.get(1)?,
                    subject: r.get(2)?,
                    claim: r.get(3)?,
                    source: r.get(4)?,
                    confidence: r.get(5)?,
                    evidence_json: r.get(6)?,
                    validated_count: r.get(7)?,
                    durable: r.get::<_, i64>(8)? == 1,
                    created_scan_id: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

- [ ] **Step 4: Call it, and correct the comment that admitted the gap**

In `session_package` in `crates/nexus-core/src/engine/query.rs`, replace:

```rust
        // Durable facts: what previous sessions worked out.
        //
        // The lifecycle states are Phase 3.1, so "durable" is approximated by the order the
        // store already returns — human, then deterministic, then AI, each by confidence.
        // The approximation gets better when the lifecycle lands; it does not get unwound.
        for row in self.store.facts(self.project_id, None)? {
```

with:

```rust
        // Durable facts: what previous sessions worked out and the project kept.
        //
        // Durability is now asked for rather than approximated. It was approximated by store
        // order while the lifecycle was Phase 3.1; the lifecycle landed and the approximation
        // outlived it, which is how 671 imported claims came to buy the session budget nine
        // at a time in alphabetical order.
        for row in self.store.durable_facts(self.project_id)? {
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p nexus-core --test session_context`
Expected: PASS, all tests in the file. The three existing fact-recording tests there use `source: "human"`, which is durable on arrival, so they are unaffected.

- [ ] **Step 6: `make check`**

Run: `make check`
Expected: exit 0, 419 passing.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/lib.rs crates/nexus-core/src/engine/query.rs crates/nexus-core/tests/session_context.rs
git commit -m "fix(context): the session package delivers what it always promised

session_package's doc comment says 'durable facts'. Its basis string says
'open findings then durable facts'. 06-memory.md §3 gives Durable the highest
retrieval weight. The code took Store::facts, which is ORDER BY fact_key and
whose own comment says that is 'stable ... and *not* a ranking'.

With a handful of hand-written facts the distinction never showed. With 671
imported claims it was the whole behaviour: the session package bought the
first nine alphabetically and grew from 194 tokens to 752.

durable_facts asks for durability instead of approximating it, riding
idx_facts_state. The approximation was documented as temporary while the
lifecycle was Phase 3.1; the lifecycle landed and the approximation outlived it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 4: One definition of "this word names exactly one symbol"

**Files:**
- Modify: `crates/nexus-core/src/context/seeds.rs` — add `last_segment` and `uniquely_named_symbol`.
- Modify: `crates/nexus-core/src/engine/memory.rs` — delete the private `last_segment`, `looks_like_an_identifier` and `symbol_named_in`; call the shared function.

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces, both in `crates/nexus-core/src/context/seeds.rs`:
  - `pub(crate) fn last_segment(fqn: &str) -> &str`
  - `pub(crate) fn uniquely_named_symbol(store: &Store, project_id: i64, word: &str) -> Result<Option<SymbolRef>, StoreError>`

This task changes no behaviour. It is a refactor that Task 5 needs, and it is separate so that a reviewer can reject Task 5's widening without also rejecting the deduplication.

- [ ] **Step 1: Move the two helpers into `seeds.rs`**

Add to `crates/nexus-core/src/context/seeds.rs`, below `targets`:

```rust
/// The last name in a qualified path, whichever separator wrote it.
pub(crate) fn last_segment(fqn: &str) -> &str {
    let after_member = fqn.rsplit('#').next().unwrap_or(fqn);
    let after_member = after_member.split('(').next().unwrap_or(after_member);
    let after_colons = after_member.rsplit("::").next().unwrap_or(after_member);
    after_colons.rsplit('.').next().unwrap_or(after_colons)
}

/// The one indexed symbol a word names, if it names exactly one.
///
/// `find_symbols` matches by suffix, which is right for a name a person typed and wrong
/// wherever the word came out of prose: without the last-segment check, "integration" once
/// anchored six imported design claims on `NoContinuousIntegration`.
///
/// Two callers, one rule: the seed stage reading a request, and the graphify import reading a
/// claim's label. Two copies of this would drift, and the copy further from the failure would
/// be the one still wrong.
pub(crate) fn uniquely_named_symbol(
    store: &Store,
    project_id: i64,
    word: &str,
) -> Result<Option<SymbolRef>, StoreError> {
    let hits = store.find_symbols(project_id, word, 2)?;
    let [only] = hits.as_slice() else {
        return Ok(None);
    };
    if last_segment(&only.fqn) == word {
        Ok(Some(only.clone()))
    } else {
        Ok(None)
    }
}
```

- [ ] **Step 2: Point the graphify import at it**

In `crates/nexus-core/src/engine/memory.rs`, delete the private `last_segment` and `looks_like_an_identifier` functions and replace the whole `symbol_named_in` method body with:

```rust
    fn symbol_named_in(&self, label: &str) -> Result<Option<nexus_store::SymbolRef>> {
        for target in crate::context::seeds::targets(label) {
            // A word out of prose is only worth looking up when it is shaped like an
            // identifier: `Integration`, `Agent` and `Hooks` are sentence words, and looking
            // them up anchored design claims on whatever symbol happened to end with them.
            let distinctive = target.len() >= 4
                && (target.contains("::")
                    || target.contains('#')
                    || target.contains('_')
                    || target.contains('/')
                    || target.contains('.')
                    || target.chars().filter(|c| c.is_uppercase()).count() >= 2);
            if !distinctive {
                continue;
            }
            if let Some(sym) =
                crate::context::seeds::uniquely_named_symbol(&self.store, self.project_id, &target)?
            {
                return Ok(Some(sym));
            }
        }
        Ok(None)
    }
```

- [ ] **Step 3: Run the graphify import tests to verify nothing moved**

Run: `cargo test -p nexus-core --test graphify_import`
Expected: PASS, 6 tests — identical behaviour, one definition.

If the compiler reports unused imports in `engine/memory.rs` after the deletions, remove them. `-D warnings` makes an unused import a build failure.

- [ ] **Step 4: `make check`**

Run: `make check`
Expected: exit 0, 419 passing.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/src/engine/memory.rs
git commit -m "refactor(seeds): one definition of 'this word names exactly one symbol'

The graphify import needed it to stop anchoring design claims on whatever
symbol happened to end with an English word. The seed stage is about to need
the same rule for the same reason. Two copies would drift, and the copy further
from the failure would be the one still wrong.

No behaviour change.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 5: A symptom can seed a package

**Files:**
- Modify: `crates/nexus-core/src/context/seeds.rs` — `STOPWORDS`, `targets`, and the name-match loop in `resolve`.
- Create: `crates/nexus-core/tests/symptom_seeds.rs`

**Interfaces:**
- Consumes: `seeds::uniquely_named_symbol` from Task 4.
- Produces: `const STOPWORDS: &[&str]`.

**The measured problem.** Four defects fixed in this repository on 2026-09-03, each given to `nexus context --task` as its symptom, produced zero hits and three empty packages. `cache` is indexed as `nexus_core::context::cache`; `targets` refused it because it carries no capital, underscore or path separator.

**The measured cost.** 3 target words 12 ms, 40 target words 23 ms — about 0.3 ms per indexed lookup. A 25-word symptom adds roughly 7 ms against ADR-024's 150 ms budget.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/tests/symptom_seeds.rs`:

```rust
//! A symptom is written in plain words, and plain words used to seed nothing.
//!
//! `seeds::targets` accepted a word only if it carried a capital, an underscore, or a path
//! separator — a rule that suited "refactor PaymentService" and refused "the cache serves a
//! stale package". Four real defects fixed in this repository, given to the context engine as
//! their symptoms, produced zero hits and three empty packages, while the code they named sat
//! in the index the whole time.

use nexus_core::context::{Purpose, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

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

/// A fixture shaped like the code the symptoms were about: a lowercase module name that
/// occurs exactly once, another that occurs twice, and a stopword that is also a symbol.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-symptom-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, body) in [
        ("src/context/cache.rs", "pub fn put() {}\npub fn get() {}\n"),
        ("src/store/ledger.rs", "pub fn append() {}\n"),
        ("src/a/handler.rs", "pub fn handler() {}\n"),
        ("src/b/handler.rs", "pub fn handler() {}\n"),
        ("src/util/error.rs", "pub fn error() {}\n"),
        ("src/lib.rs", "pub mod context;\npub mod store;\npub mod util;\n"),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn files_in(engine: &Engine, text: &str) -> Vec<String> {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    engine
        .context(&r)
        .expect("context")
        .items
        .iter()
        .map(|i| i.anchor.file.clone())
        .collect()
}

#[test]
fn a_symptom_in_plain_words_reaches_the_code_it_names() {
    let (_root, engine) = scanned("plain");
    let files = files_in(&engine, "the context cache serves a package from before a fact was recorded");
    assert!(
        files.iter().any(|f| f.contains("context/cache.rs")),
        "`cache` is one indexed symbol and the symptom names it: {files:?}"
    );
}

#[test]
fn a_word_naming_two_symbols_seeds_nothing() {
    let (_root, engine) = scanned("ambiguous");
    let files = files_in(&engine, "the handler drops the second request");
    assert!(
        !files.iter().any(|f| f.contains("handler.rs")),
        "`handler` names two symbols, so it identifies neither: {files:?}"
    );
}

#[test]
fn a_stopword_seeds_nothing_even_when_it_is_a_symbol() {
    // `error` is in the index here. It is also the word every symptom in the world contains.
    let (_root, engine) = scanned("stopword");
    let files = files_in(&engine, "there is an error when the ledger appends");
    assert!(
        !files.iter().any(|f| f.contains("util/error.rs")),
        "a stopword is not a seed however well it matches: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.contains("store/ledger.rs")),
        "the distinctive word in the same sentence still seeds: {files:?}"
    );
}

#[test]
fn a_short_word_seeds_nothing() {
    let (_root, engine) = scanned("short");
    let files = files_in(&engine, "get put now");
    assert!(
        files.is_empty(),
        "three-letter words are not evidence, however many symbols they match: {files:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-core --test symptom_seeds`
Expected: FAIL on `a_symptom_in_plain_words_reaches_the_code_it_names` — the package is empty, because `cache` never becomes a target.

- [ ] **Step 3: Add the stopword list**

At the top of `crates/nexus-core/src/context/seeds.rs`, below the imports:

```rust
/// Words a request uses *about* code rather than *as* code.
///
/// The Rust analyzer's PRELUDE deny-list solved this exact shape of problem the same way, and
/// for the same reason: a hint that matches everything produces a *wrong* seed rather than a
/// missing one. Deliberately short and boring — English function words, and the handful of
/// code words that appear in almost every sentence about a defect.
const STOPWORDS: &[&str] = &[
    // English.
    "that", "this", "with", "from", "when", "then", "than", "them", "they", "there", "these",
    "those", "have", "does", "done", "into", "over", "only", "some", "same", "such", "were",
    "will", "what", "which", "while", "would", "should", "could", "after", "before", "about",
    "because", "returns", "return", "still", "just", "make", "made", "much", "more", "most",
    "less", "very", "also", "even", "never", "always", "again",
    // Code words a prompt uses about code.
    "test", "tests", "error", "errors", "value", "values", "result", "results", "file",
    "files", "line", "lines", "call", "calls", "type", "types", "data", "code", "name",
    "names", "case", "cases", "item", "items", "list", "lists", "null", "none", "true",
    "false", "class", "method", "function", "field", "module", "package", "project", "symbol",
];
```

- [ ] **Step 4: Widen `targets`**

In `crates/nexus-core/src/context/seeds.rs`, replace the doc comment and filter of `targets`. The current doc comment ends with the sentence beginning *"Filtering here rather than querying every word keeps the stage at a handful of indexed lookups…"* — replace that whole paragraph with the measurement, and add the plain-word arm:

```rust
/// Candidate words from the prompt that could name a symbol: anything containing a dot,
/// slash, hash or `::` (an FQN or a path), an underscore (a `snake_case` identifier), a
/// capital (a type name), or a plain lowercase word of four characters or more that is not a
/// stopword.
///
/// The plain-word arm is what lets a *symptom* find code. `cache` is indexed as
/// `nexus_core::context::cache`, and refusing it because it carries no capital meant four
/// real defects, handed to the context engine as their symptoms, produced zero hits and three
/// empty packages.
///
/// It is affordable, measured rather than assumed: one word that passes this filter is one
/// indexed lookup, and 3 target words cost 12 ms against 40 target words at 23 ms — about
/// 0.3 ms each. A 25-word symptom adds roughly 7 ms to ADR-024's 150 ms budget. The previous
/// comment here justified the narrow filter with that budget and was over-cautious by an
/// order of magnitude.
///
/// Noise control is not this function's job: `resolve` accepts a plain word only when it
/// names exactly one symbol whose own last segment *is* the word.
pub(crate) fn targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '(' | ')'))
        .map(|w| w.trim_end_matches(['.', '?', '!', ':']))
        .filter(|w| w.len() > 2)
        .filter(|w| {
            w.contains('.')
                || w.contains('/')
                || w.contains('#')
                || w.contains("::")
                // An underscore inside a word is an identifier, not English prose. Leading
                // and trailing ones are stripped first so `_private` and a markdown `_word_`
                // do not both arrive as targets.
                || w.trim_matches('_').contains('_')
                || w.chars().next().is_some_and(char::is_uppercase)
                || is_plain_candidate(w)
        })
        .map(str::to_string)
        .collect();
    // A member is stored as `Owner#name` in every language, because the platform needs one
    // separator. A Rust or C++ developer writes `Engine::context`, so the last `::` is also
    // offered as a `#` — otherwise the most natural way to name a method finds nothing.
    let aliases: Vec<String> = out
        .iter()
        .filter_map(|w| w.rsplit_once("::").map(|(o, n)| format!("{o}#{n}")))
        .collect();
    out.extend(aliases);
    out.sort();
    out.dedup();
    out
}

/// A lowercase word worth one indexed lookup: long enough to be distinctive, and not a word
/// every sentence about a defect contains.
fn is_plain_candidate(w: &str) -> bool {
    w.len() >= 4 && !STOPWORDS.contains(&w.to_ascii_lowercase().as_str())
}
```

- [ ] **Step 5: Require uniqueness for the plain-word arm in `resolve`**

In `resolve`, replace the name-match loop:

```rust
    for target in targets(&req.text) {
        let exact_shape = target.contains('.') || target.contains('/') || target.contains('#');
        for s in store.find_symbols(project_id, &target, 10)? {
            let source = if exact_shape {
                SeedSource::Exact
            } else {
                SeedSource::NameMatch
            };
            offer(&mut found, s, source, format!("'{target}' in the request"));
        }
    }
```

with:

```rust
    for target in targets(&req.text) {
        let exact_shape = target.contains('.') || target.contains('/') || target.contains('#');
        // A plain lowercase word is weaker evidence than a name someone qualified, so it is
        // accepted only when it identifies one symbol outright. Without that rule the word
        // "integration" reaches `NoContinuousIntegration`, and a symptom seeds the wrong file
        // with confidence.
        let plain = !exact_shape
            && !target.contains("::")
            && !target.trim_matches('_').contains('_')
            && !target.chars().next().is_some_and(char::is_uppercase);
        if plain {
            if let Some(s) = uniquely_named_symbol(store, project_id, &target)? {
                offer(
                    &mut found,
                    s,
                    SeedSource::NameMatch,
                    format!("'{target}' in the request names exactly one symbol"),
                );
            }
            continue;
        }
        for s in store.find_symbols(project_id, &target, 10)? {
            let source = if exact_shape {
                SeedSource::Exact
            } else {
                SeedSource::NameMatch
            };
            offer(&mut found, s, source, format!("'{target}' in the request"));
        }
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p nexus-core --test symptom_seeds`
Expected: PASS, 4 tests.

- [ ] **Step 7: `make check`, and rebaseline the goldens**

Run: `make check`

`five_golden_packages_hold` is expected to fail here. The widening applies to every request, so a golden whose task text contains a plain lowercase word naming exactly one symbol gains a seed. **Read the diff before accepting it** — every new item must trace to a word actually present in that golden's task text. If one does not, the uniqueness rule is not being applied and Step 5 is wrong.

```bash
NEXUS_REBASELINE=1 cargo test -p nexus-core --test golden_packages
git diff crates/nexus-core/tests/golden/
make check
```

Expected: exit 0, 423 passing.

- [ ] **Step 8: Verify against the four real symptoms**

This is the acceptance criterion the fixture cannot express, because it needs this repository's own index. Run it by hand:

```bash
cargo build --release
W=$(mktemp -d) && git clone -q . "$W" && ./target/release/nexus --project "$W" scan >/dev/null
for t in "the context cache serves a package from before a fact was recorded" \
         "tokens_estimated says 253 but the package ships 11113 tokens" \
         "resolved call edges fell from 551 to 104 after a rename"; do
  echo "== $t"
  ./target/release/nexus --project "$W" context --task "$t" --json \
    | python3 -c "import json,sys; d=json.load(sys.stdin)['result']; print(' ', d['items_included'], 'items:', sorted({i['anchor']['file'] for i in d['items']}))"
done
```

Expected: the first names `crates/nexus-core/src/context/cache.rs`. At least two of the three return a non-empty package. If all three are still empty, stop and report it — the widening is not reaching `resolve`.

- [ ] **Step 9: Commit**

```bash
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/tests/symptom_seeds.rs crates/nexus-core/tests/golden/
git commit -m "feat(seeds): let a symptom find the code it is about

seeds::targets accepted a word only if it carried a capital, an underscore or a
path separator. That suits 'refactor PaymentService' and refuses 'the cache
serves a stale package'. Four defects fixed in this repository, handed to the
context engine as their symptoms, produced zero hits and three empty packages —
while `cache`, indexed as nexus_core::context::cache, sat there the whole time.

Plain lowercase words of four characters or more are now candidates, minus a
stopword list, and a plain word is accepted only when it names exactly one
symbol whose last segment is the word — the rule the graphify import already
uses, for the same reason.

No intent gate. Gating on the verb table adds a branch that misclassifies, and
a widening that silently fails to happen is the hardest kind of defect to
notice. Uniqueness is the noise control and it does not care what the verb was.

Cost measured, not assumed: 0.3 ms per lookup, so a 25-word symptom adds ~7 ms
to ADR-024's 150 ms budget. The comment that justified the narrow filter with
that budget was over-cautious by an order of magnitude.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 6: Import only what is a claim

**Files:**
- Modify: `crates/nexus-core/src/graphify.rs` — `is_a_claim`, fixture exclusion, and the `justified` set at read time.
- Modify: `crates/nexus-core/src/report.rs` — `ImportReport.skipped_not_a_claim`.
- Modify: `crates/nexus-core/src/engine/memory.rs` — count the skips.
- Modify: `crates/nexus-cli/src/main.rs` — print the count.
- Modify: `crates/nexus-core/tests/graphify_import.rs` — add tests.

**Interfaces:**
- Consumes: `Engine::symbol_named_in` (Task 4's shared version).
- Produces: `ExternalConcept.names_a_symbol: bool` is **not** added — the fourth keep-rule is applied in `import_graphify`, where the store is reachable, not in `graphify.rs`, which must stay free of the index.

- [ ] **Step 1: Write the failing tests**

Append to `crates/nexus-core/tests/graphify_import.rs`. Also replace the `GRAPH` constant at the top of that file with this longer one, and update `imported()`'s two assertions from `3` to `2` — the new fixture has one node that must now be filtered:

```rust
const GRAPH: &str = r#"{
  "nodes": [
    {"id": "n1", "label": "PaymentService settles a payment exactly once",
     "file_type": "concept", "source_file": "docs/design.md", "source_location": "L12"},
    {"id": "n2", "label": "PaymentService retries are the caller's job",
     "file_type": "rationale", "source_file": "docs/design.md", "source_location": null},
    {"id": "n3", "label": "Golden Fixture Repositories",
     "file_type": "concept", "source_file": "docs/design.md", "source_location": null},
    {"id": "n4", "label": "next", "file_type": "concept",
     "source_file": "tests/fixtures/specs/blobs/package.json", "source_location": null},
    {"id": "n5", "label": "Structural", "file_type": "code", "source_file": "src/x.java"}
  ],
  "links": []
}"#;
```

```rust
#[test]
fn a_heading_is_not_knowledge() {
    let (root, mut engine) = scanned("claims");
    let r = engine
        .import_graphify(&root.join("graph.json"))
        .expect("import");
    let claims: Vec<String> = engine
        .facts(None)
        .expect("facts")
        .into_iter()
        .map(|f| f.claim)
        .collect();
    assert!(
        claims.iter().any(|c| c.contains("settles a payment exactly once")),
        "a sentence is a claim: {claims:?}"
    );
    assert!(
        claims.iter().any(|c| c.contains("retries are the caller's job")),
        "a rationale node is a claim by construction: {claims:?}"
    );
    assert!(
        !claims.iter().any(|c| c == "Golden Fixture Repositories"),
        "a title-case heading names a thing, it does not assert one: {claims:?}"
    );
    assert!(
        !claims.iter().any(|c| c == "next"),
        "a dependency name out of a fixture's package.json is not project knowledge: {claims:?}"
    );
    assert_eq!(
        r.skipped_not_a_claim, 2,
        "the heading and the fixture node, counted where a person can see them"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p nexus-core --test graphify_import`
Expected: FAIL — `no field 'skipped_not_a_claim'`, and once that compiles, the heading and the fixture node are both imported.

- [ ] **Step 3: Add the field**

In `crates/nexus-core/src/report.rs`, inside `ImportReport`, after `skipped`:

```rust
    /// Nodes that were prose but not a claim — a heading, a label, a dependency name out of a
    /// fixture. graphify's `concept` nodes are mostly *names of things*, and importing them
    /// put `next`, `react` and `Golden Fixture Repositories` in project memory.
    pub skipped_not_a_claim: usize,
```

- [ ] **Step 4: Filter at read time**

In `crates/nexus-core/src/graphify.rs`, inside `read`, replace the concept-collecting branch:

```rust
        if matches!(n.file_type.as_str(), "concept" | "rationale") && !n.label.trim().is_empty() {
            // A claim with nowhere to point cannot become a fact: §12 refuses an item with no
            // `file:line`, and a fact that can never be shown is a row nobody reads.
            if src.is_empty() {
                continue;
            }
```

with:

```rust
        if matches!(n.file_type.as_str(), "concept" | "rationale") && !n.label.trim().is_empty() {
            // A claim with nowhere to point cannot become a fact: §12 refuses an item with no
            // `file:line`, and a fact that can never be shown is a row nobody reads.
            if src.is_empty() {
                continue;
            }
            // Test data is not this project's design.
            if src.starts_with("tests/") || src.contains("/blobs/") {
                continue;
            }
```

and add, beside `line_of`:

```rust
/// Whether a prose node asserts something, rather than naming something.
///
/// graphify's `concept` nodes are mostly titles: of 681 prose nodes on this repository the
/// imported facts included `next`, `react` and `Golden Fixture Repositories`. A `rationale`
/// is an assertion by construction; a `concept` has to earn it, by being the thing some node
/// is a `rationale_for`, by reading as a sentence, or — checked by the caller, which is the
/// only place the index is reachable — by naming a symbol.
///
/// The sentence test is a guess about English and will be wrong sometimes. It is kept because
/// without it the import loses the claims identifiable only as prose, and those are the ones
/// with the most to say.
fn is_a_claim(n: &NodeRow, justified: &std::collections::HashSet<&str>) -> bool {
    if n.file_type == "rationale" || justified.contains(n.id.as_str()) {
        return true;
    }
    let words: Vec<&str> = n.label.split_whitespace().collect();
    words.len() >= 4
        && words
            .iter()
            .filter(|w| w.chars().next().is_some_and(char::is_lowercase))
            .count()
            >= 2
}
```

Build the `justified` set before the node loop in `read`:

```rust
    let justified: std::collections::HashSet<&str> = file
        .links
        .iter()
        .filter(|l| l.relation.as_deref() == Some("rationale_for"))
        .map(|l| l.target.as_str())
        .collect();
```

and mark the node rather than dropping it, so the caller can apply the fourth rule. Add to `ExternalConcept`:

```rust
    /// Whether this asserts something on its own. A node that does not may still be imported
    /// when its label names an indexed symbol — a check only the engine can make.
    pub is_claim: bool,
```

setting it in the push: `is_claim: is_a_claim(n, &justified),`.

- [ ] **Step 5: Apply the fourth rule where the index is reachable**

In `crates/nexus-core/src/engine/memory.rs`, inside `import_graphify`'s loop, immediately after the `key.is_empty()` guard:

```rust
            // Rule four, and it is not redundant: rules one to three drop
            // `nexus-cli::main composition root`, a heading by shape that names code, which
            // is the entire reason to keep a claim.
            let anchor = self.symbol_named_in(&c.label)?;
            if !c.is_claim && anchor.is_none() {
                report.skipped_not_a_claim += 1;
                continue;
            }
```

and delete the later `let anchor = self.symbol_named_in(&c.label)?;` so it is computed once. Initialise the new field where `ImportReport` is constructed: `skipped_not_a_claim: 0,`.

- [ ] **Step 6: Print it**

In `crates/nexus-cli/src/main.rs`, in the `MemoryCommand::Import` arm, extend the human line:

```rust
                writeln!(
                    out,
                    "  {} claim(s) read, {} recorded, {} anchored on code, {} not a claim, {} skipped",
                    r.concepts_read, r.facts_recorded, r.anchored_on_code, r.skipped_not_a_claim, r.skipped
                )?;
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p nexus-core --test graphify_import`
Expected: PASS, 7 tests.

- [ ] **Step 8: `make check`**

Run: `make check`
Expected: exit 0, 424 passing.

- [ ] **Step 9: Commit**

```bash
git add crates/nexus-core/src/graphify.rs crates/nexus-core/src/report.rs crates/nexus-core/src/engine/memory.rs crates/nexus-cli/src/main.rs crates/nexus-core/tests/graphify_import.rs
git commit -m "fix(memory): a heading is not knowledge

graphify's concept nodes are mostly names of things, not assertions. Importing
all of them put 'next', 'react' and 'Golden Fixture Repositories' into project
memory: of 681 prose nodes on this repository, 641 named no symbol at all.

A prose node is imported when it is a rationale, when something is a
rationale_for it, when its label reads as a sentence, or when it names an
indexed symbol — and never from tests/ or a /blobs/ path. The fourth rule is
applied in the engine because it is the only place the index is reachable, and
it is not redundant: rules one to three drop 'nexus-cli::main composition root',
a heading by shape that names code.

The count is reported, so the filter's effect is visible in the command rather
than only in a test.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Task 7: Record what changed, and measure that it worked

**Files:**
- Modify: `AGENTS.md` — the retrieval trap.
- Modify: `docs/architecture/05-context-engine.md` — the seed rule.
- Modify: `docs/superpowers/specs/2026-09-03-retrieval-design.md` — status line.

- [ ] **Step 1: Measure the result on this repository**

```bash
cargo build --release
W=$(mktemp -d) && git clone -q . "$W" && ./target/release/nexus --project "$W" scan >/dev/null
python3 - "$W" <<'PY'
import sqlite3, subprocess, sys, time, os
W = sys.argv[1]; B = "./target/release/nexus"
c = sqlite3.connect(f"{W}/.nexus/nexus.db")
pid, = next(c.execute("SELECT id FROM projects LIMIT 1"))
scan, = next(c.execute("SELECT MAX(id) FROM scans"))
def t():
    xs = []
    for _ in range(5):
        os.system(f"rm -rf {W}/.nexus/cache")
        s = time.perf_counter()
        subprocess.run([B, "--project", W, "context", "--task",
                        "change SafeWriter to allow a symlink", "--json"], capture_output=True)
        xs.append((time.perf_counter() - s) * 1000)
    xs.sort(); return xs[2]
print(f"{'facts':>8} {'ms':>7}")
for target in (0, 10_000, 200_000):
    have, = next(c.execute("SELECT COUNT(*) FROM facts"))
    if target > have:
        c.executemany(
            "INSERT OR IGNORE INTO facts (project_id, fact_key, scope, subject, claim, source,"
            " confidence, created_scan_id, durable) VALUES (?,?,?,?,?,?,?,?,0)",
            [(pid, f"arch.noise-{i:07}", "symbol", f"unrelated::Mod{i}", "noise", "ai", 0.5, scan)
             for i in range(have, target)])
        c.commit()
    print(f"{target:8} {t():7.0f}")
PY
```

Expected: all three rows within 20% of each other. Record the numbers — they go in the commit message. If 200,000 is materially slower than 0, `facts_for_seeds` is not using the index; run `EXPLAIN QUERY PLAN` on the generated SQL before going further.

- [ ] **Step 2: Add the trap to `AGENTS.md`**

Under `## Traps`, above the cache-key entry:

```markdown
- **Memory is queried by subject, never scanned.** `Store::facts(project_id, None)` loads
  every live fact; only `memory_export` may, because its job is everything. Every request
  path calls `facts_for_seeds` or `durable_facts`. The unscoped form cost 274 ms at 200,000
  facts on a path ADR-024 budgets 150 ms for, and memory is append-only by design.
```

- [ ] **Step 3: Correct the seed rule in the spec of record**

In `docs/architecture/05-context-engine.md`, find the paragraph describing stage 2's candidate words and replace the "starts with a capital" rule with the widened one, naming the uniqueness guard. Search for `capital` in that file; if the rule is not documented there, skip this step rather than inventing a place for it.

- [ ] **Step 4: Mark the specs implemented**

In `docs/superpowers/specs/2026-09-03-retrieval-design.md` change `**Status:** approved, not yet implemented` to `**Status:** implemented`. Do the same in `2026-09-03-knowledge-selectivity-design.md`, whose status line currently reads `approved; **absorbed into** …` — append `; implemented`.

- [ ] **Step 5: `make check` and commit**

```bash
make check
git add AGENTS.md docs/architecture/05-context-engine.md docs/superpowers/specs/
git commit -m "docs: retrieval is bounded by the request, and a symptom can seed

Records the trap so it is not reintroduced: memory is queried by subject, never
scanned, and only memory_export may load everything.

Measured on this repository after the change: <fill in the three numbers from
Task 7 Step 1>.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

## Self-Review

**Spec coverage.**

| Spec requirement | Task |
|---|---|
| §1 `facts_for_seeds`, two arms, both indexed | 1 |
| §1 prefixes split on `::` `.` `#`, deduplicated | 1 |
| §1 parameter count inside SQLite's limit, asserted | 1 |
| §1 `SEED_QUERY_CAP` = 256, note when it bites | 2 |
| §1 Rust-side `to_seeds` filter stays | 2 (untouched by construction) |
| §1 `session_package` takes durable facts | 3 |
| §1 `memory_export` keeps loading everything | 3 (untouched by construction) |
| §2 plain lowercase words ≥ 4 chars, not a stopword | 5 |
| §2 accepted only if exactly one symbol, last segment equal | 4, 5 |
| §2 no intent gate | 5 |
| §2 `STOPWORDS` a const in `seeds.rs` | 5 |
| §2 `symbol_named_in` has one definition | 4 |
| §3 import filter, four keep-rules, no fixtures | 6 |
| §3 durable session package | 3 |
| §3 task packages keep candidates | 2 (untouched by construction) |
| Acceptance 1 — 10,000 unrelated facts change nothing | 2, 7 |
| Acceptance 2 — ancestor and descendant retrieved, real index | 2 |
| Acceptance 3 — cap reported in `notes` | 2 |
| Acceptance 4 — the four symptoms | 5 |
| Acceptance 5 — ambiguity and stopwords seed nothing | 5 |
| Acceptance 6 — one definition | 4 |
| Acceptance 7 — session package is durable-only | 3 |
| Acceptance 8 — `next` out, `Budget is selection` in | 6 |
| Acceptance 9 — `make check` green, goldens rebaselined with the diff read | 2, 5 |

**Gap found and closed.** Acceptance 3 said the cap must be reported but no task tested it. It is asserted in Task 2 Step 4's implementation and visible in `notes`; a fixture cannot reach 256 relevant symbols without a large corpus, so it is verified by the code path and the note text rather than by a test. This is stated rather than papered over.

**Placeholder scan.** One deliberate placeholder remains: Task 7 Step 5's commit message says `<fill in the three numbers from Task 7 Step 1>`, because those numbers do not exist until the measurement runs.

**Type consistency.** `facts_for_seeds(&self, ProjectId, &[String]) -> Result<Vec<FactRow>>` is defined in Task 1 and called in Task 2 with `&relevant: &Vec<String>`, which derefs. `durable_facts(&self, ProjectId) -> Result<Vec<FactRow>>` defined in Task 3, called there. `uniquely_named_symbol(&Store, i64, &str) -> Result<Option<SymbolRef>, StoreError>` defined in Task 4, called in Task 4 (`memory.rs`, where `?` converts `StoreError` into the engine's error) and Task 5 (`seeds.rs`, whose `resolve` already returns `Result<SeedResult, StoreError>`). `ExternalConcept.is_claim: bool` defined in Task 6 Step 4, read in Task 6 Step 5. `ImportReport.skipped_not_a_claim: usize` defined in Task 6 Step 3, written in Step 5, printed in Step 6, asserted in Step 1.
