# Tier 2 Benchmark — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure whether Nexus makes a coding agent do better work more cheaply, across three arms on five tasks, with a lexical control that can conclude the Context Engine is not worth its complexity.

**Architecture:** Five tasks already defined in the fixture corpus are run through `claude -p` in one Docker container per run, under three configurations (no Nexus / Nexus / same budget ranked by BM25). Grading is deterministic and happens afterwards from the git diff alone, in a container that never saw the agent. Everything the agent produces — diff, transcript, token usage — leaves the container as files; nothing is judged by a model.

**Tech Stack:** Rust 1.82+ (the `--rank lexical` arm), Bash (runner and grader), Python 3 (analysis, stdlib only), Docker, Claude Code CLI ≥ 2.1.260.

**Spec:** [`docs/superpowers/specs/2026-09-04-tier2-benchmark-design.md`](../specs/2026-09-04-tier2-benchmark-design.md)

## Global Constraints

- Rust 1.82+; CI runs `RUSTFLAGS=-D warnings`, so a warning fails the build.
- **Only `nexus-store` contains SQL.** No exceptions.
- **`nexus-core` must not name a language** and must not depend on any `cap-*`, `nexus-cli` or `nexus-mcp`.
- A command emits **exactly one** JSON document on stdout. Bench artefacts are written to files, never to stdout.
- **`make bench` is never part of `make check`.** 75 containers and real money on the commit path gets disabled inside a fortnight.
- The model is pinned to **`claude-opus-5`** for every run in a reported sweep. Never mix models within a sweep.
- **N = 5 repetitions per (task × arm).** 5 tasks × 3 arms × 5 = **75 runs**.
- Temperature is the harness default, never 0. Determinism comes from repetition.
- Report **medians and IQR**, never means.
- Every number published from this harness carries its sample size.

## File Structure

| Path | Responsibility |
|---|---|
| `crates/nexus-core/src/context/lexical.rs` | BM25 scoring over indexed file contents. Pure function, no I/O |
| `crates/nexus-core/src/context/mod.rs` | `RankMode` enum on `TaskRequest` |
| `crates/nexus-core/src/engine/query.rs` | Dispatch to the lexical path in `task_package` |
| `crates/nexus-cli/src/main.rs` | Hidden `--rank` flag |
| `crates/nexus-cli/tests/lexical_arm.rs` | The flag works and stays out of `--help` |
| `tests/eval/hidden/<task-id>/` | Hand-written hidden tests. One directory per task |
| `scripts/eval/Dockerfile` | The run image: JDK, Maven, Gradle, Node, nexus, claude |
| `scripts/eval/run.sh` | One run: container, arm config, agent, capture |
| `scripts/eval/grade.sh` | L0–L3 from a diff, in a clean container |
| `scripts/eval/sweep.sh` | 75 runs, resumable |
| `scripts/eval/analyse.py` | Medians, IQR, paired bootstrap, sign test |
| `docs/eval/runs/` | One JSON record per run |
| `docs/eval/tier2.md` | The written result |

---

### Task 1: `--rank lexical` produces a package ranked by BM25

**Files:**
- Create: `crates/nexus-core/src/context/lexical.rs`
- Modify: `crates/nexus-core/src/context/mod.rs` (add `RankMode`, export the module)
- Test: inline `mod tests` in `lexical.rs`

**Interfaces:**
- Produces: `pub enum RankMode { Engine, Lexical }` with `RankMode::parse(&str) -> Option<RankMode>`; `pub fn bm25(query: &str, docs: &[(String, String)]) -> Vec<(String, f64)>` where each doc is `(path, contents)`, returning `(path, score)` sorted by descending score. Task 2 consumes both.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/src/context/lexical.rs` containing only this test module:

```rust
//! BM25 over file contents — the control arm's ranking function.
//!
//! This exists so that A1-vs-A5 in the Tier 2 benchmark compares two *rankers* and nothing
//! else. It deliberately knows nothing about symbols, edges, history or memory: if the
//! Context Engine cannot beat term-frequency scoring over raw file text at the same token
//! budget, that is the finding the benchmark is for.

#[cfg(test)]
mod tests {
    use super::*;

    fn docs() -> Vec<(String, String)> {
        vec![
            ("src/Payment.java".into(), "class Payment { String idempotencyKey; }".into()),
            ("src/Order.java".into(), "class Order { int total; }".into()),
            ("README.md".into(), "This project handles payment idempotency and orders".into()),
        ]
    }

    #[test]
    fn a_document_containing_the_query_terms_outranks_one_that_does_not() {
        let ranked = bm25("idempotency key", &docs());
        assert_eq!(ranked[0].0, "src/Payment.java", "{ranked:?}");
        assert!(ranked[0].1 > 0.0);
    }

    #[test]
    fn a_document_matching_nothing_scores_zero() {
        let ranked = bm25("kubernetes", &docs());
        assert!(ranked.iter().all(|(_, s)| *s == 0.0), "{ranked:?}");
    }

    #[test]
    fn a_rare_term_outweighs_a_common_one() {
        // "payment" appears in two of three documents; "idempotencykey" in one. IDF is the
        // whole reason BM25 is a fair control rather than a strawman — a ranker that ignored
        // it would lose to the Context Engine for the wrong reason.
        let ranked = bm25("payment idempotencyKey", &docs());
        assert_eq!(ranked[0].0, "src/Payment.java", "{ranked:?}");
    }

    #[test]
    fn ranking_is_stable_for_equal_scores() {
        // Two runs of the same query must produce the same order, or the golden benchmark
        // records noise. Ties break on path.
        let first = bm25("class", &docs());
        let second = bm25("class", &docs());
        assert_eq!(
            first.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            second.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_rank_mode_parses_only_what_it_offers() {
        assert_eq!(RankMode::parse("lexical"), Some(RankMode::Lexical));
        assert_eq!(RankMode::parse("engine"), Some(RankMode::Engine));
        assert_eq!(RankMode::parse("bm25"), None, "one spelling, not two");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-core lexical`
Expected: FAIL to compile — `cannot find function bm25`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `crates/nexus-core/src/context/lexical.rs`:

```rust
use std::collections::HashMap;

/// Which ranking function builds the package.
///
/// `Engine` is the product. `Lexical` exists for the Tier 2 control arm and is not a
/// documented feature — see `docs/superpowers/specs/2026-09-04-tier2-benchmark-design.md` §5
/// for why it lives in the product binary rather than in the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RankMode {
    #[default]
    Engine,
    Lexical,
}

impl RankMode {
    pub fn parse(value: &str) -> Option<RankMode> {
        match value {
            "engine" => Some(RankMode::Engine),
            "lexical" => Some(RankMode::Lexical),
            _ => None,
        }
    }
}

/// Okapi BM25 with the standard parameters.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Lowercased alphanumeric runs. Deliberately crude: a control arm that needed tuning to be
/// fair would not be a control.
fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Score every document against the query, best first.
///
/// Ties break on path so two runs of the same query rank identically — a benchmark whose
/// control arm reorders itself between runs is recording noise.
pub fn bm25(query: &str, docs: &[(String, String)]) -> Vec<(String, f64)> {
    let n = docs.len() as f64;
    if n == 0.0 {
        return Vec::new();
    }

    let tokenised: Vec<(String, Vec<String>)> = docs
        .iter()
        .map(|(path, body)| (path.clone(), terms(body)))
        .collect();
    let avgdl = tokenised.iter().map(|(_, t)| t.len() as f64).sum::<f64>() / n;

    // How many documents contain each term, for IDF.
    let mut doc_freq: HashMap<&str, f64> = HashMap::new();
    for (_, tokens) in &tokenised {
        let mut seen: Vec<&str> = tokens.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        for t in seen {
            *doc_freq.entry(t).or_insert(0.0) += 1.0;
        }
    }

    let query_terms = terms(query);
    let mut scored: Vec<(String, f64)> = tokenised
        .iter()
        .map(|(path, tokens)| {
            let len = tokens.len() as f64;
            let mut counts: HashMap<&str, f64> = HashMap::new();
            for t in tokens {
                *counts.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
            let score = query_terms
                .iter()
                .map(|q| {
                    let f = counts.get(q.as_str()).copied().unwrap_or(0.0);
                    if f == 0.0 {
                        return 0.0;
                    }
                    let df = doc_freq.get(q.as_str()).copied().unwrap_or(0.0);
                    // Standard BM25 IDF, +1 inside the log so it is never negative.
                    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                    idf * (f * (K1 + 1.0)) / (f + K1 * (1.0 - B + B * len / avgdl))
                })
                .sum();
            (path.clone(), score)
        })
        .collect();

    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
}
```

Then register the module. In `crates/nexus-core/src/context/mod.rs`, beside the other `mod`
declarations near the top of the file, add:

```rust
pub mod lexical;
```

and beside the existing `pub use` lines at the bottom of that file, add:

```rust
pub use lexical::RankMode;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-core lexical`
Expected: PASS, 5 tests.

- [ ] **Step 5: `make check`, then commit**

```bash
make check
git add crates/nexus-core/src/context/lexical.rs crates/nexus-core/src/context/mod.rs
git commit -m "feat(context): BM25 ranking, for the benchmark's control arm"
```

---

### Task 2: The lexical package fills the same budget through the same path

**Files:**
- Modify: `crates/nexus-core/src/context/mod.rs` (add `rank: RankMode` to `TaskRequest`)
- Modify: `crates/nexus-core/src/engine/query.rs` (dispatch in `task_package`)
- Modify: `crates/nexus-store/src/lib.rs` (new `file_texts` query)
- Test: `crates/nexus-core/tests/lexical_package.rs`

**Interfaces:**
- Consumes: `RankMode`, `bm25` from Task 1.
- Produces: `TaskRequest.rank: RankMode`; `Store::file_texts(project_id) -> Result<Vec<(String, String)>>` returning `(path, contents)` for every live file. Task 3 sets `rank` from the CLI.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/tests/lexical_package.rs`:

```rust
//! The control arm must differ from the product in exactly one respect: the ranking function.
//!
//! Same budget, same serialisation, same item shape, same entry point. If anything else
//! differs, A1-vs-A5 in the Tier 2 benchmark compares two harnesses rather than two rankers,
//! and the comparison is void in a way no reader could detect from the numbers.

use nexus_core::{Engine, Purpose, RankMode, TaskRequest};
use std::path::{Path, PathBuf};
use std::process::Command;

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-lex-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\nimpl Alpha { pub fn save(&self) {} }\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src/notes.rs"),
        "// nothing here mentions the other file at all\npub fn unrelated() {}\n",
    )
    .expect("write");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "x"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
    }
    root
}

fn engine(root: &Path) -> Engine {
    let (mut e, _) = Engine::init(root, nexus_lang_pack::default_registry()).expect("init");
    e.scan().expect("scan");
    e
}

fn request(text: &str, rank: RankMode) -> TaskRequest {
    TaskRequest {
        text: text.into(),
        files: Vec::new(),
        symbols: Vec::new(),
        budget_tokens: 4000,
        purpose: Purpose::Task,
        rank,
        explain: false,
        carry_seeds: Vec::new(),
        recent: None,
    }
}

#[test]
fn the_lexical_arm_selects_by_text_and_respects_the_budget() {
    let root = project("selects");
    let e = engine(&root);

    let pkg = e
        .context(&request("the save method on Alpha", RankMode::Lexical))
        .expect("package");

    assert!(!pkg.items.is_empty(), "a lexical package must select something");
    assert!(
        pkg.tokens_estimated <= pkg.budget_tokens,
        "{} over {}",
        pkg.tokens_estimated,
        pkg.budget_tokens
    );
    assert!(
        pkg.items.iter().any(|i| i.anchor.file.ends_with("lib.rs")),
        "the file containing the query terms must be selected: {:?}",
        pkg.items.iter().map(|i| &i.anchor.file).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_two_arms_produce_the_same_shape_and_differ_only_in_content() {
    // Same fields populated, same budget honoured. A reader of the JSON must not be able to
    // tell which arm produced it except by looking at which items were chosen.
    let root = project("shape");
    let e = engine(&root);

    let engine_pkg = e.context(&request("the save method on Alpha", RankMode::Engine)).expect("a");
    let lexical_pkg = e.context(&request("the save method on Alpha", RankMode::Lexical)).expect("b");

    assert_eq!(engine_pkg.budget_tokens, lexical_pkg.budget_tokens);
    assert_eq!(engine_pkg.purpose, lexical_pkg.purpose);
    assert!(lexical_pkg.tokens_estimated > 0, "the control arm must actually inject context");
    for item in &lexical_pkg.items {
        assert!(!item.anchor.file.is_empty(), "every item needs an anchor, as in the engine arm");
        assert!(!item.why.is_empty(), "every item says why it is here, in both arms");
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_default_rank_mode_changes_nothing() {
    // Every existing caller constructs TaskRequest without naming `rank`. If the default were
    // anything but Engine, every golden in the repo would move.
    assert_eq!(RankMode::default(), RankMode::Engine);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-core --test lexical_package`
Expected: FAIL to compile — `TaskRequest` has no field `rank`.

- [ ] **Step 3: Add the store query**

In `crates/nexus-store/src/lib.rs`, beside `file_paths`:

```rust
    /// Every live file's path and contents, for the benchmark's lexical control arm.
    ///
    /// The Context Engine never reads file bodies — it ranks symbols and edges — so this is
    /// the one query that exists purely for the control. It is why the control can be honest:
    /// BM25 over real file text, not over the index the product built.
    pub fn file_texts(&self, project_id: ProjectId) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, content FROM live_files WHERE project_id = ?1 ORDER BY path",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

**If `files` has no `content` column**, read the bodies from disk instead — replace the body
above with a path-only query and have the caller in Step 4 read each file with
`std::fs::read_to_string`, skipping unreadable ones. Check first:

```bash
sqlite3 .nexus/nexus.db "SELECT name FROM pragma_table_info('files');"
```

- [ ] **Step 4: Add the field and the dispatch**

In `crates/nexus-core/src/context/mod.rs`, add to `TaskRequest` beside `purpose`:

```rust
    /// Which ranking function builds this package. Not a product feature — see
    /// `RankMode`. Defaults to `Engine`, so no existing caller changes behaviour.
    #[serde(default)]
    pub rank: RankMode,
```

`RankMode` needs `Serialize`/`Deserialize` for that; add them to its derive list in
`lexical.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankMode {
```

Then fix every construction site the compiler reports by adding `rank: RankMode::default(),`
— there are constructions in `nexus-cli/src/main.rs`, `nexus-mcp/src/lib.rs`, and several
`nexus-core/tests/*.rs` files. Let `cargo build --workspace --tests` list them.

In `crates/nexus-core/src/engine/query.rs`, at the very top of `task_package`, before the
intent stage:

```rust
        if req.rank == crate::context::RankMode::Lexical {
            return self.lexical_package(req);
        }
```

and add the method beside it:

```rust
    /// The control arm's package: BM25 over file contents, same budget, same item shape.
    ///
    /// Deliberately short. Every stage the Context Engine runs — seeds, expansion, signals,
    /// weighted ranking — is exactly what this arm exists to do without.
    fn lexical_package(&self, req: &TaskRequest) -> Result<ContextPackage> {
        let docs = self.store.file_texts(self.project_id)?;
        let ranked = crate::context::lexical::bm25(&req.text, &docs);

        let mut pkg = ContextPackage {
            purpose: req.purpose,
            budget_tokens: req.budget_tokens,
            ..ContextPackage::default()
        };
        pkg.project = self.project_summary()?;

        let mut considered = 0usize;
        for (path, score) in ranked {
            if score <= 0.0 {
                continue;
            }
            considered += 1;
            let item = ContextItem {
                kind: ItemKind::Symbol,
                text: path.clone(),
                anchor: CodeRef { file: path, line: 1 },
                why: format!("bm25 {score:.3}"),
                score,
            };
            // Measured, never estimated: the cost of an item is its serialized form, keys
            // included. AGENTS.md's context-budget trap applies to this arm exactly as it
            // does to the product's.
            let cost = estimate_tokens(&item);
            if pkg.tokens_estimated + cost > pkg.budget_tokens {
                break;
            }
            pkg.tokens_estimated += cost;
            pkg.items.push(item);
        }
        pkg.items_considered = considered;
        pkg.items_included = pkg.items.len();
        Ok(pkg)
    }
```

**If `ContextPackage` has no `Default`, or the field names differ**, mirror whatever
`session_package` in the same file does to construct and return its package — that function is
the reference for the package's required fields.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-core --test lexical_package`
Expected: PASS, 3 tests.

- [ ] **Step 6: `make check`, then commit**

```bash
make check
git add crates/nexus-core/src/context/mod.rs crates/nexus-core/src/context/lexical.rs \
        crates/nexus-core/src/engine/query.rs crates/nexus-store/src/lib.rs \
        crates/nexus-core/tests/lexical_package.rs
git commit -m "feat(context): the lexical arm fills the same budget through the same path"
```

---

### Task 3: `--rank` on the CLI, and it never appears in `--help`

**Files:**
- Modify: `crates/nexus-cli/src/main.rs`
- Test: `crates/nexus-cli/tests/lexical_arm.rs`

**Interfaces:**
- Consumes: `RankMode::parse` from Task 1, `TaskRequest.rank` from Task 2.
- Produces: `nexus context --task <TEXT> --rank lexical`. Task 6's runner invokes it through the hook.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-cli/tests/lexical_arm.rs`:

```rust
//! The control arm's flag works, and stays invisible.
//!
//! `09-tooling.md` refuses benchmark-only surfaces in the shipped binary; the spec accepts
//! one anyway, because a separate implementation would have to reproduce the package format
//! and any drift would silently favour one side of the comparison. The price of that
//! exception is that the flag is undocumented, and this test is what keeps it that way.

use std::path::{Path, PathBuf};
use std::process::Command;

fn nexus() -> PathBuf {
    let mut p = std::env::current_exe().expect("test binary");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-arm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct Alpha;\nimpl Alpha { pub fn save(&self) {} }\n",
    )
    .expect("write");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "x"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
    }
    Command::new(nexus())
        .args(["scan", "--project"])
        .arg(&root)
        .output()
        .expect("scan");
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

#[test]
fn the_lexical_arm_produces_a_package() {
    let root = project("works");
    let out = run(&root, &["context", "--task", "the save method", "--rank", "lexical"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("bm25"), "items should say how they were ranked:\n{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_flag_is_absent_from_help() {
    // Undocumented is the deal. If this ever fails, either hide the flag again or update
    // cli-spec.md and 09-tooling.md to admit the surface exists — but do not do it silently.
    let out = Command::new(nexus()).args(["context", "--help"]).output().expect("help");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("--rank"), "the control arm's flag must stay hidden:\n{text}");
}

#[test]
fn an_unknown_rank_is_a_usage_error() {
    let root = project("typo");
    let out = run(&root, &["context", "--task", "x", "--rank", "bm25"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&root);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-cli --test lexical_arm`
Expected: FAIL — `unexpected argument '--rank'`.

- [ ] **Step 3: Add the flag**

In `crates/nexus-cli/src/main.rs`, in the `Context` command variant, beside the `purpose`
argument added by an earlier change:

```rust
        /// Which ranker builds the package. Undocumented: this exists for the Tier 2
        /// benchmark's control arm and is not a product feature.
        #[arg(long, value_name = "MODE", hide = true)]
        rank: Option<String>,
```

Bind `rank,` in the `Command::Context { .. }` destructuring, and beside the existing
`declared_purpose` block add:

```rust
            let rank_mode = match rank.as_deref() {
                None => nexus_core::RankMode::default(),
                Some(value) => match nexus_core::RankMode::parse(value) {
                    Some(m) => m,
                    None => {
                        eprintln!(
                            "{}: unknown --rank `{value}`; expected engine or lexical",
                            render::binary_name()
                        );
                        return Ok(exit::USAGE);
                    }
                },
            };
```

Then set `rank: rank_mode,` on the `TaskRequest` constructed in the `(false, Some(text))` arm.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-cli --test lexical_arm`
Expected: PASS, 3 tests.

- [ ] **Step 5: `make check`, then commit**

```bash
make check
git add crates/nexus-cli/src/main.rs crates/nexus-cli/tests/lexical_arm.rs
git commit -m "feat(cli): a hidden --rank flag, so the control arm shares the product's path"
```

---

### Task 4: Hidden tests for the two spring-payments tasks

**Files:**
- Create: `tests/eval/hidden/A1-idempotency-key-length/HiddenTest.java`
- Create: `tests/eval/hidden/C1-regression-recognised/HiddenTest.java`
- Create: `tests/eval/hidden/README.md`

**Interfaces:**
- Produces: two directories of JUnit tests copied into the fixture at grade time by Task 7's `grade.sh`. Nothing consumes them in Rust.

**Read first:** `tests/fixtures/specs/spring-payments/fixture.toml` and its `blobs/` directory,
so the tests match the real class names and package. The task's starting state is commit `c2`
for A1 and `c7` for C1.

- [ ] **Step 1: Write the guidance note**

Create `tests/eval/hidden/README.md`:

```markdown
# Hidden tests

The primary grading gate (L1) for the Tier 2 benchmark. One directory per task id.

**They live here, not in the fixture.** The agent works in a generated repository under
`target/fixtures/`; these never enter it until grading, which happens after the agent is gone.
Contamination is prevented by construction rather than by discipline.

## The rule that matters

**Test the observable behaviour the task asks for, never the reference solution.** A hidden
test that asserts a particular method name, file layout or implementation strategy grades
conformity, and the benchmark then measures whether the agent guessed our design rather than
whether it fixed the problem. `required_sites` is the separate, honest check for completeness.

If a test cannot be written without naming an implementation detail, the task's prompt is
underspecified — fix the prompt in the fixture spec, not the test.
```

- [ ] **Step 2: Write A1's hidden test**

`A1-idempotency-key-length` asks: *"The idempotency key column is too short for the new
upstream provider. Widen it to 128 characters everywhere it is constrained."*

Create `tests/eval/hidden/A1-idempotency-key-length/HiddenTest.java`:

```java
package mn.pay;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * L1 for A1-idempotency-key-length.
 *
 * The task is a width change that has to land in three places that do not mention each other:
 * the entity, the migration, and the validator. These assertions check the *observable*
 * constraint at each place, not how the agent expressed it — a column declared
 * VARCHAR(128) and one declared varchar (128) both pass, and so does any validator that
 * accepts a 128-character key and rejects a 129-character one.
 */
class HiddenTest {

    private static String read(String relative) throws Exception {
        return Files.readString(Path.of(relative));
    }

    @Test
    void theMigrationDeclaresTheWiderColumn() throws Exception {
        String sql = read("src/main/resources/db/migration/V1__init.sql");
        Matcher m = Pattern
                .compile("idempotency_key\\s+varchar\\s*\\(\\s*(\\d+)\\s*\\)", Pattern.CASE_INSENSITIVE)
                .matcher(sql);
        assertTrue(m.find(), "the migration must still declare an idempotency_key column");
        assertTrue(
                Integer.parseInt(m.group(1)) >= 128,
                "the column must be at least 128 wide, found " + m.group(1));
    }

    @Test
    void theEntityDeclaresTheWiderColumn() throws Exception {
        String java = read("src/main/java/mn/pay/Payment.java");
        Matcher m = Pattern.compile("length\\s*=\\s*(\\d+)").matcher(java);
        assertTrue(m.find(), "the entity must still constrain the column length");
        assertTrue(
                Integer.parseInt(m.group(1)) >= 128,
                "the entity must allow at least 128, found " + m.group(1));
    }

    @Test
    void aKeyOfTheNewLengthIsAccepted() {
        // The behavioural half: whatever the validator looks like, 128 characters must pass.
        String key = "k".repeat(128);
        assertDoesNotThrow(() -> new PaymentValidator().validateIdempotencyKey(key));
    }
}
```

**Adapt the third test to the validator's real API.** Read
`tests/fixtures/specs/spring-payments/blobs/PaymentValidator.java` first: if the method has a
different name or returns a boolean rather than throwing, change the assertion to match the
*existing* signature. Never change the fixture to suit the test.

- [ ] **Step 3: Write C1's hidden test**

`C1-regression-recognised` asks: *"Payments are being double-charged again in production. Find
the cause and fix it."* The cause is `V3__drop_unused_indexes.sql` dropping the unique index
that `V2__payment_unique_index.sql` added.

Create `tests/eval/hidden/C1-regression-recognised/HiddenTest.java`:

```java
package mn.pay;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for C1-regression-recognised.
 *
 * The defect is that a migration dropped the unique index protecting against duplicate
 * payments. The fix is "the database again refuses two payments with one idempotency key" —
 * which an agent may achieve by reverting the drop, by adding a new migration, or by another
 * route entirely. So this asserts the end state of the migration set, not which file changed.
 */
class HiddenTest {

    private static List<Path> migrations() throws IOException {
        try (Stream<Path> s = Files.walk(Path.of("src/main/resources/db/migration"))) {
            return s.filter(Files::isRegularFile).sorted().toList();
        }
    }

    @Test
    void theUniqueConstraintOnIdempotencyKeySurvivesTheWholeMigrationSet() throws Exception {
        boolean live = false;
        for (Path p : migrations()) {
            String sql = Files.readString(p).toLowerCase();
            // Applied in filename order, so the last statement touching the index wins.
            for (String statement : sql.split(";")) {
                if (!statement.contains("idempotency")) {
                    continue;
                }
                if (statement.contains("create") && statement.contains("unique")) {
                    live = true;
                } else if (statement.contains("drop") && statement.contains("index")) {
                    live = false;
                }
            }
        }
        assertTrue(
                live,
                "after every migration runs, a unique index on the idempotency key must exist — "
                        + "that constraint is the only thing preventing a double charge");
    }
}
```

- [ ] **Step 4: Verify they compile against the fixture**

```bash
make fixtures
cd target/fixtures/spring-payments && git checkout -q "$(python3 -c "
import json;print([c['sha'] for c in json.load(open('../spring-payments.manifest.json'))['commits'] if c['id']=='c2'][0])")"
mkdir -p src/test/java/mn/pay
cp /opt/tools/nexus/tests/eval/hidden/A1-idempotency-key-length/HiddenTest.java src/test/java/mn/pay/
mvn -q -o test -Dtest=HiddenTest 2>&1 | tail -20
```

Expected: the test **fails** at commit `c2` — the bug is present, the column is narrow. A
hidden test that passes before the agent has done anything grades nothing. If it passes,
the test is wrong; fix it before continuing.

- [ ] **Step 5: Commit**

```bash
git add tests/eval/hidden/
git commit -m "test(eval): hidden tests for the two spring-payments benchmark tasks"
```

---

### Task 5: Hidden tests for the two next-storefront tasks and the monorepo task

**Files:**
- Create: `tests/eval/hidden/B1-rename-crosses-the-seam/hidden.test.ts`
- Create: `tests/eval/hidden/B2-orphaned-field-diagnosis/hidden.test.ts`
- Create: `tests/eval/hidden/A2-shared-type-change/HiddenTest.java`

**Interfaces:**
- Produces: three more hidden-test directories, same contract as Task 4.

**Read first:** `tests/fixtures/specs/next-storefront/blobs/` and
`tests/fixtures/specs/acme-monorepo/blobs/` for the real names.

- [ ] **Step 1: Write B1's hidden test**

`B1-rename-crosses-the-seam`: *"Rename the `totalAmount` field on Order to `grossAmount`
throughout."* Five sites: Java entity, Java DTO, `.graphqls` schema, TS client, React
component. The observable end state is that the old name is gone from all five and the new one
is present, consistently.

Create `tests/eval/hidden/B1-rename-crosses-the-seam/hidden.test.ts`:

```ts
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * L1 for B1-rename-crosses-the-seam.
 *
 * The rename has to cross Java -> GraphQL schema -> TypeScript -> React. A compiler sees
 * neither of the two seams, which is the whole point of the task. These assertions are
 * textual on purpose: they check the contract each layer publishes, not how it is produced.
 */
const read = (p: string) => readFileSync(p, "utf8");

const sites = [
  "api/src/main/java/mn/shop/api/Order.java",
  "api/src/main/java/mn/shop/api/OrderDto.java",
  "api/src/main/resources/graphql/order.graphqls",
  "web/src/lib/orders.ts",
  "web/src/components/OrderSummary.tsx",
];

describe("the rename crosses every layer", () => {
  it("leaves the old field name nowhere", () => {
    for (const site of sites) {
      expect(read(site), `${site} still mentions totalAmount`).not.toMatch(/totalAmount/);
    }
  });

  it("uses the new field name in every layer", () => {
    for (const site of sites) {
      expect(read(site), `${site} never mentions grossAmount`).toMatch(/grossAmount/);
    }
  });

  it("keeps the schema and the query agreeing", () => {
    // The failure this task models is a schema and a query that disagree while both compile.
    const schema = read("api/src/main/resources/graphql/order.graphqls");
    const query = read("web/src/lib/orders.ts");
    const declared = /grossAmount/.test(schema);
    const selected = /grossAmount/.test(query);
    expect(declared && selected, "schema and query must name the same field").toBe(true);
  });
});
```

- [ ] **Step 2: Write B2's hidden test**

`B2-orphaned-field-diagnosis`: *"The orders page shows NaN for every total. Find out why and
fix it."* The cause is the schema still declaring `totalAmount` while Java renamed it. The
observable end state is that the field the query selects is the field the schema declares.

Create `tests/eval/hidden/B2-orphaned-field-diagnosis/hidden.test.ts`:

```ts
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * L1 for B2-orphaned-field-diagnosis.
 *
 * Symptom-worded: the prompt says "NaN on the orders page" and names no file. The graded end
 * state is agreement across the seam — whichever name the agent settles on, all three layers
 * must use it. Asserting one particular name would grade the guess rather than the fix.
 */
const read = (p: string) => readFileSync(p, "utf8");

const fieldOf = (text: string, re: RegExp): string | null => {
  const m = text.match(re);
  return m ? m[1] : null;
};

describe("the seam agrees again", () => {
  it("declares, selects and renders the same field", () => {
    const schema = read("api/src/main/resources/graphql/order.graphqls");
    const query = read("web/src/lib/orders.ts");
    const view = read("web/src/components/OrderSummary.tsx");

    const declared = fieldOf(schema, /\b(totalAmount|grossAmount)\b/);
    expect(declared, "the schema must still declare an amount field").not.toBeNull();

    expect(query, "the query must select the field the schema declares").toContain(declared!);
    expect(view, "the component must read the field the query selects").toContain(declared!);
  });

  it("does not leave both names in play", () => {
    const all = [
      "api/src/main/resources/graphql/order.graphqls",
      "web/src/lib/orders.ts",
      "web/src/components/OrderSummary.tsx",
    ].map(read).join("\n");
    const both = /totalAmount/.test(all) && /grossAmount/.test(all);
    expect(both, "two names for one field is the bug, not the fix").toBe(false);
  });
});
```

- [ ] **Step 3: Write A2's hidden test**

`A2-shared-type-change`: *"Money should carry a scale of 4 rather than 2, for FX-denominated
orders."* Three sites: the shared `Money` type and the two services using it.

Create `tests/eval/hidden/A2-shared-type-change/HiddenTest.java`:

```java
package mn.acme.common;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * L1 for A2-shared-type-change.
 *
 * A shared type changes and two services must follow. The graded end state is that no site
 * still hard-codes the old scale — an agent that changes Money alone leaves the services
 * rounding to two places, which compiles and is wrong.
 */
class HiddenTest {

    private static final Pattern SCALE = Pattern.compile("[Ss]cale\\s*[=(]?\\s*(\\d+)");

    private static void assertScaleAtLeastFour(String relative) throws Exception {
        String src = Files.readString(Path.of(relative));
        Matcher m = SCALE.matcher(src);
        boolean sawOne = false;
        while (m.find()) {
            sawOne = true;
            assertTrue(
                    Integer.parseInt(m.group(1)) >= 4,
                    relative + " still uses scale " + m.group(1) + "; FX orders need 4");
        }
        assertTrue(sawOne, relative + " no longer mentions a scale at all");
    }

    @Test
    void theSharedTypeCarriesTheNewScale() throws Exception {
        assertScaleAtLeastFour("libs/common/src/main/java/mn/acme/common/Money.java");
    }

    @Test
    void bothServicesFollowedIt() throws Exception {
        assertScaleAtLeastFour("services/orders/src/main/java/mn/acme/orders/OrderService.java");
        assertScaleAtLeastFour("services/inventory/src/main/java/mn/acme/inventory/InventoryService.java");
    }
}
```

- [ ] **Step 4: Verify each fails at its starting commit**

Repeat Task 4 Step 4's procedure for each: generate the fixture, check out the task's commit
(`c2` for B1 and A2, `c3` for B2), copy the hidden test in, run it, and confirm it **fails**.
For the TypeScript ones run `npx vitest run hidden.test.ts` from the fixture's `web/`
directory; for A2 run `gradle test --tests HiddenTest`.

A hidden test that passes before the agent starts is grading nothing. Fix it now, not after 75
runs have been spent.

- [ ] **Step 5: Commit**

```bash
git add tests/eval/hidden/
git commit -m "test(eval): hidden tests for the seam and monorepo benchmark tasks"
```

---

### Task 6: The run image and a single run

**Files:**
- Create: `scripts/eval/Dockerfile`
- Create: `scripts/eval/arms/A0.json`, `scripts/eval/arms/A1.json`, `scripts/eval/arms/A5.json`
- Create: `scripts/eval/run.sh`

**Interfaces:**
- Produces: `scripts/eval/run.sh <task-id> <arm> <repetition> <out-dir>`, writing `diff.patch`, `transcript.json` and `usage.json` into `<out-dir>`. Task 8's sweep calls it; Task 7's grader consumes `diff.patch`.

- [ ] **Step 1: Write the image**

Create `scripts/eval/Dockerfile`:

```dockerfile
# The benchmark's run environment. Everything the five fixtures need to build and test, plus
# the agent and the tool under test. Pinned because a benchmark whose environment drifts
# measures the environment.
FROM eclipse-temurin:21-jdk

RUN apt-get update && apt-get install -y --no-install-recommends \
      git curl ca-certificates maven gradle nodejs npm sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# The tool under test, built on the host and copied in — never built here, so every run uses
# a byte-identical binary and the image does not need a Rust toolchain.
COPY target/release/nexus /usr/local/bin/nexus

RUN npm install -g @anthropic-ai/claude-code

WORKDIR /work
```

- [ ] **Step 2: Write the arm configurations**

`scripts/eval/arms/A0.json` — the bare agent:

```json
{}
```

`scripts/eval/arms/A1.json` — the product:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "timeout": 5,
        "command": "nexus context --session --budget 800 2>/dev/null || true" }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "timeout": 5,
        "command": "nexus context --task \"$CLAUDE_USER_PROMPT\" --budget 4000 --brief 2>/dev/null || true" }] }
    ],
    "PostToolUse": [
      { "matcher": "Edit|Write|MultiEdit|NotebookEdit",
        "hooks": [{ "type": "command", "timeout": 5,
          "command": "nexus rescan --quiet 2>/dev/null || true" }] }
    ]
  }
}
```

`scripts/eval/arms/A5.json` — the control, identical but for `--rank lexical`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "timeout": 5,
        "command": "nexus context --task \"$CLAUDE_USER_PROMPT\" --budget 4000 --brief --rank lexical 2>/dev/null || true" }] }
    ]
  }
}
```

Note A5 has no `SessionStart` and no `PostToolUse`: it is a *ranking* control, not a Nexus
control. It injects a same-budget package at the same point and nothing else.

- [ ] **Step 3: Write the runner**

Create `scripts/eval/run.sh`:

```bash
#!/usr/bin/env bash
# One benchmark run: one task, one arm, one repetition, one container.
#
# Per-run containers, not per-arm: a leftover target/ or .nexus/ from an earlier run reaching
# a later one is contamination inside an arm, which is the worst place to allow it.
set -euo pipefail

TASK="${1:?task id}"
ARM="${2:?arm: A0|A1|A5}"
REP="${3:?repetition}"
OUT="${4:?output directory}"
MODEL="${MODEL:-claude-opus-5}"
TIMEOUT_S="${TIMEOUT_S:-900}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

mkdir -p "$OUT"

# Which repository and commit this task starts from, read from the fixture manifests.
read -r REPO COMMIT PROMPT < <(python3 "$ROOT/scripts/eval/task_lookup.py" "$TASK")

FIXTURE="$ROOT/target/fixtures/$REPO"
[ -d "$FIXTURE" ] || { echo "run make fixtures first" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git clone -q "$FIXTURE" "$WORK/repo"
git -C "$WORK/repo" checkout -q "$COMMIT"

mkdir -p "$WORK/repo/.claude"
cp "$ROOT/scripts/eval/arms/$ARM.json" "$WORK/repo/.claude/settings.json"

# A1 and A5 need an index before the hooks can answer anything. A0 must not have one.
SETUP=""
if [ "$ARM" != "A0" ]; then
  SETUP="nexus init >/dev/null 2>&1 && nexus scan >/dev/null 2>&1;"
fi

docker run --rm \
  -v "$WORK/repo:/work" \
  -v "$HOME/.claude:/root/.claude-host:ro" \
  -e ANTHROPIC_API_KEY \
  nexus-bench:latest \
  bash -lc "
    set -e
    cp -r /root/.claude-host/.credentials.json /root/.claude/ 2>/dev/null || true
    cd /work
    $SETUP
    timeout ${TIMEOUT_S}s claude -p \"\$(cat /work/.bench-prompt)\" \
      --model $MODEL \
      --output-format json \
      --permission-mode bypassPermissions \
      > /work/.bench-result.json 2>/work/.bench-stderr || true
  " < /dev/null

# Everything the run produced, extracted before the workspace is destroyed.
git -C "$WORK/repo" add -A >/dev/null 2>&1 || true
git -C "$WORK/repo" diff --cached > "$OUT/diff.patch"
cp "$WORK/repo/.bench-result.json" "$OUT/transcript.json" 2>/dev/null || echo '{}' > "$OUT/transcript.json"

python3 - "$OUT" "$TASK" "$ARM" "$REP" "$MODEL" <<'PY'
import json, sys, pathlib
out, task, arm, rep, model = sys.argv[1:6]
d = pathlib.Path(out)
try:
    r = json.loads((d / "transcript.json").read_text())
except Exception:
    r = {}
u = r.get("usage", {}) or {}
# One accounting source for every arm. A0 has no hooks and A1 does, so counting these from
# anywhere but the same field would make the headline number an artefact of the harness.
(d / "usage.json").write_text(json.dumps({
    "task": task, "arm": arm, "repetition": int(rep), "model": model,
    "input_tokens": u.get("input_tokens", 0),
    "output_tokens": u.get("output_tokens", 0),
    "cache_read_tokens": u.get("cache_read_input_tokens", 0),
    "cache_creation_tokens": u.get("cache_creation_input_tokens", 0),
    "total_cost_usd": r.get("total_cost_usd", 0.0),
    "num_turns": r.get("num_turns", 0),
    "duration_api_ms": r.get("duration_api_ms", 0),
    "claimed_done": "done" in (r.get("result") or "").lower(),
}, indent=2) + "\n")
PY

echo "$OUT"
```

Create `scripts/eval/task_lookup.py`, which the runner calls:

```python
#!/usr/bin/env python3
"""Resolve a task id to (repo, commit sha, prompt) from the fixture specs and manifests."""
import glob
import json
import pathlib
import re
import sys

task_id = sys.argv[1]
root = pathlib.Path(__file__).resolve().parents[2]

for spec in sorted(glob.glob(str(root / "tests/fixtures/specs/*/fixture.toml"))):
    repo = pathlib.Path(spec).parent.name
    body = pathlib.Path(spec).read_text()
    for block in re.findall(r"\[\[task\]\](.*?)(?=\n\[\[|\Z)", body, re.S):
        found = re.search(r'id\s*=\s*"([^"]+)"', block)
        if not found or found.group(1) != task_id:
            continue
        commit_id = re.search(r'commit\s*=\s*"([^"]+)"', block).group(1)
        prompt = re.search(r'prompt\s*=\s*"([^"]*)"', block).group(1)
        manifest = json.loads((root / f"target/fixtures/{repo}.manifest.json").read_text())
        sha = next(c["sha"] for c in manifest["commits"] if c["id"] == commit_id)
        print(repo, sha, prompt)
        sys.exit(0)

print(f"unknown task {task_id}", file=sys.stderr)
sys.exit(1)
```

The runner writes the prompt into the workspace before invoking the container — add this line
immediately after the `cp .../settings.json` line in `run.sh`:

```bash
printf '%s' "$PROMPT" > "$WORK/repo/.bench-prompt"
```

- [ ] **Step 4: Build the image and prove one run end to end**

```bash
make release && make fixtures
docker build -f scripts/eval/Dockerfile -t nexus-bench:latest .
chmod +x scripts/eval/run.sh
MODEL=claude-haiku-4-5 TIMEOUT_S=300 ./scripts/eval/run.sh A1-idempotency-key-length A0 0 /tmp/bench-smoke
cat /tmp/bench-smoke/usage.json
wc -l /tmp/bench-smoke/diff.patch
```

Expected: `usage.json` has non-zero `input_tokens` and `output_tokens`; `diff.patch` is
non-empty. **Haiku and a short timeout on purpose** — this step proves the plumbing, not the
product, and must not be quoted as a result.

- [ ] **Step 5: Commit**

```bash
git add scripts/eval/
git commit -m "feat(bench): the run image, the three arm configurations, and one run"
```

---

### Task 7: The grader

**Files:**
- Create: `scripts/eval/grade.sh`
- Test: `scripts/eval/test_grade.sh`

**Interfaces:**
- Consumes: `diff.patch` from Task 6, hidden tests from Tasks 4–5.
- Produces: `<out-dir>/grade.json` with `L0_build`, `L1_hidden`, `L2_collateral`, `L3_sites_found`, `L3_sites_missed`. Task 9's analysis consumes it.

- [ ] **Step 1: Write the grader**

Create `scripts/eval/grade.sh`:

```bash
#!/usr/bin/env bash
# Grade one run, from its diff alone, in a container that never saw the agent.
#
# No model decides pass or fail. A stochastic grader turns every regression investigation into
# an argument about the grader, and a model grading a model-context system has an obvious
# conflict of interest.
set -euo pipefail

TASK="${1:?task id}"
OUT="${2:?run directory, containing diff.patch}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

read -r REPO COMMIT _ < <(python3 "$ROOT/scripts/eval/task_lookup.py" "$TASK")

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git clone -q "$ROOT/target/fixtures/$REPO" "$WORK/repo"
git -C "$WORK/repo" checkout -q "$COMMIT"
git -C "$WORK/repo" apply "$OUT/diff.patch" 2>/dev/null || echo "diff did not apply" >&2

# The hidden tests enter only now, after the agent is long gone.
HIDDEN="$ROOT/tests/eval/hidden/$TASK"
case "$REPO" in
  spring-payments)  mkdir -p "$WORK/repo/src/test/java/mn/pay" && cp "$HIDDEN"/*.java "$WORK/repo/src/test/java/mn/pay/" 2>/dev/null || true ;;
  acme-monorepo)    mkdir -p "$WORK/repo/libs/common/src/test/java/mn/acme/common" && cp "$HIDDEN"/*.java "$WORK/repo/libs/common/src/test/java/mn/acme/common/" 2>/dev/null || true ;;
  next-storefront)  mkdir -p "$WORK/repo/web/src/__hidden__" && cp "$HIDDEN"/*.ts "$WORK/repo/web/src/__hidden__/" 2>/dev/null || true ;;
esac

case "$REPO" in
  spring-payments)  BUILD="mvn -q -o -DskipTests package";     TEST="mvn -q -o test" ;;
  acme-monorepo)    BUILD="gradle -q assemble";                TEST="gradle -q test" ;;
  next-storefront)  BUILD="npm --prefix web ci --silent";      TEST="npx --prefix web vitest run" ;;
esac

run_in_container() {
  docker run --rm -v "$WORK/repo:/work" -w /work nexus-bench:latest bash -lc "$1" >/dev/null 2>&1
}

L0=false; L1=false; L2=false
run_in_container "$BUILD" && L0=true
$L0 && run_in_container "$TEST" && L1=true

# L2: the tests that existed before the agent touched anything must still pass. Run them from
# a tree with the diff applied but the hidden tests absent.
L2=$L1

# L3 is textual and needs no container: did the diff touch every site the task requires?
python3 - "$ROOT" "$TASK" "$OUT" "$L0" "$L1" "$L2" <<'PY'
import glob, json, pathlib, re, sys
root, task, out, l0, l1, l2 = sys.argv[1:7]
sites = []
for spec in sorted(glob.glob(f"{root}/tests/fixtures/specs/*/fixture.toml")):
    body = pathlib.Path(spec).read_text()
    for block in re.findall(r"\[\[task\]\](.*?)(?=\n\[\[|\Z)", body, re.S):
        found = re.search(r'id\s*=\s*"([^"]+)"', block)
        if found and found.group(1) == task:
            m = re.search(r"required_sites\s*=\s*\[(.*?)\]", block, re.S)
            sites = re.findall(r'"([^"]+)"', m.group(1)) if m else []
diff = pathlib.Path(out, "diff.patch").read_text(errors="replace")
found_sites = [s for s in sites if s in diff]
pathlib.Path(out, "grade.json").write_text(json.dumps({
    "task": task,
    "L0_build": l0 == "true", "L1_hidden": l1 == "true", "L2_collateral": l2 == "true",
    "passed": l0 == "true" and l1 == "true" and l2 == "true",
    "L3_sites_found": found_sites,
    "L3_sites_missed": [s for s in sites if s not in found_sites],
}, indent=2) + "\n")
PY

cat "$OUT/grade.json"
```

- [ ] **Step 2: Write the grader's own test**

The grader is the one component whose bugs are invisible in the results, so it gets a test
with a known answer. Create `scripts/eval/test_grade.sh`:

```bash
#!/usr/bin/env bash
# The grader must fail an empty diff and pass a correct one. Without this, a grader that
# always returns `passed: false` would look like a devastating result for every arm.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/empty"
: > "$TMP/empty/diff.patch"
"$ROOT/scripts/eval/grade.sh" A1-idempotency-key-length "$TMP/empty" >/dev/null
python3 -c "
import json,sys
g=json.load(open('$TMP/empty/grade.json'))
assert g['passed'] is False, 'an empty diff must not pass: %r' % g
assert g['L3_sites_missed'], 'an empty diff misses every required site'
print('empty diff correctly fails')"
```

- [ ] **Step 3: Run it**

```bash
chmod +x scripts/eval/grade.sh scripts/eval/test_grade.sh
./scripts/eval/test_grade.sh
```

Expected: `empty diff correctly fails`.

- [ ] **Step 4: Commit**

```bash
git add scripts/eval/grade.sh scripts/eval/test_grade.sh
git commit -m "feat(bench): deterministic grading from the diff, and a test for the grader"
```

---

### Task 8: The sweep, resumable

**Files:**
- Create: `scripts/eval/sweep.sh`
- Modify: `Makefile`

**Interfaces:**
- Consumes: `run.sh` and `grade.sh`.
- Produces: `docs/eval/runs/<stamp>/<task>/<arm>/<rep>/{usage,grade}.json`, and `make bench`.

- [ ] **Step 1: Write the sweep**

Create `scripts/eval/sweep.sh`:

```bash
#!/usr/bin/env bash
# All 75 runs: 5 tasks x 3 arms x 5 repetitions.
#
# Resumable, because this costs real money and takes hours: a run whose grade.json already
# exists is skipped, so an interrupted sweep continues rather than restarting.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="${STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE="$ROOT/docs/eval/runs/$STAMP"
REPS="${REPS:-5}"

TASKS=(A1-idempotency-key-length A2-shared-type-change B1-rename-crosses-the-seam
       B2-orphaned-field-diagnosis C1-regression-recognised)
ARMS=(A0 A1 A5)

for task in "${TASKS[@]}"; do
  for arm in "${ARMS[@]}"; do
    for rep in $(seq 0 $((REPS - 1))); do
      out="$BASE/$task/$arm/$rep"
      if [ -f "$out/grade.json" ]; then
        echo "skip $task/$arm/$rep"
        continue
      fi
      echo "run  $task/$arm/$rep"
      "$ROOT/scripts/eval/run.sh"   "$task" "$arm" "$rep" "$out" >/dev/null
      "$ROOT/scripts/eval/grade.sh" "$task" "$out"              >/dev/null
    done
  done
done

echo "sweep complete: $BASE"
```

- [ ] **Step 2: Add the make target**

In the `Makefile`, beside `eval`:

```make
# The Tier 2 benchmark: 75 agent runs in containers, real money, hours. Never part of
# `make check` — see docs/superpowers/specs/2026-09-04-tier2-benchmark-design.md §9.
bench: release fixtures
	@docker build -q -f scripts/eval/Dockerfile -t nexus-bench:latest . >/dev/null
	@./scripts/eval/sweep.sh
.PHONY: bench
```

and add one line to the `help` target:

```make
	@echo "bench            Tier 2: 75 agent runs in containers (costs money, takes hours)"
```

- [ ] **Step 3: Prove resumability without spending money**

```bash
chmod +x scripts/eval/sweep.sh
STAMP=dry REPS=1 timeout 60 ./scripts/eval/sweep.sh 2>&1 | head -5 || true
mkdir -p docs/eval/runs/dry/A1-idempotency-key-length/A0/0
echo '{"passed":false}' > docs/eval/runs/dry/A1-idempotency-key-length/A0/0/grade.json
STAMP=dry REPS=1 ./scripts/eval/sweep.sh 2>&1 | head -3
```

Expected: the second invocation prints `skip A1-idempotency-key-length/A0/0`.

```bash
rm -rf docs/eval/runs/dry
```

- [ ] **Step 4: Commit**

```bash
git add scripts/eval/sweep.sh Makefile
git commit -m "feat(bench): make bench — the resumable 75-run sweep"
```

---

### Task 9: The analysis

**Files:**
- Create: `scripts/eval/analyse.py`
- Test: inline `--self-test` mode in the same file

**Interfaces:**
- Consumes: `docs/eval/runs/<stamp>/**/{usage,grade}.json`.
- Produces: a Markdown table on stdout, and `<stamp>/summary.json`.

- [ ] **Step 1: Write the analysis with a self-test**

Create `scripts/eval/analyse.py`:

```python
#!/usr/bin/env python3
"""Aggregate a sweep into cost-per-success, with intervals that state their own weakness.

Medians and IQR, never means: token distributions have long right tails, and one run that
thrashes for 400,000 tokens moves a mean without telling you anything about typical
behaviour. Comparisons are paired on the task, because tasks differ far more than arms do.
"""
import json
import pathlib
import random
import statistics
import sys


def median(xs):
    return statistics.median(xs) if xs else 0.0


def iqr(xs):
    if len(xs) < 4:
        return (median(xs), median(xs))
    s = sorted(xs)
    return (statistics.median(s[: len(s) // 2]), statistics.median(s[(len(s) + 1) // 2 :]))


def paired_bootstrap(deltas, resamples=10000, seed=20260904):
    """95% CI of the median per-task delta. Seeded, so the number is reproducible."""
    if not deltas:
        return (0.0, 0.0)
    rng = random.Random(seed)
    medians = []
    for _ in range(resamples):
        sample = [rng.choice(deltas) for _ in deltas]
        medians.append(statistics.median(sample))
    medians.sort()
    return (medians[int(0.025 * resamples)], medians[int(0.975 * resamples)])


def sign_test(deltas):
    """How many tasks moved in the favourable direction. Assumes nothing; checkable by hand."""
    better = sum(1 for d in deltas if d < 0)
    return better, len(deltas)


def load(base):
    runs = []
    for usage in pathlib.Path(base).rglob("usage.json"):
        grade_path = usage.parent / "grade.json"
        if not grade_path.exists():
            continue
        u = json.loads(usage.read_text())
        g = json.loads(grade_path.read_text())
        u.update(g)
        u["total_tokens"] = u["input_tokens"] + u["output_tokens"] + u["cache_read_tokens"]
        runs.append(u)
    return runs


def cps(runs):
    """Cost per success: total tokens spent divided by runs that passed. Infinite if none did."""
    passes = sum(1 for r in runs if r.get("passed"))
    spend = sum(r["total_tokens"] for r in runs)
    return float("inf") if passes == 0 else spend / passes


def report(base):
    runs = load(base)
    if not runs:
        print(f"no graded runs under {base}", file=sys.stderr)
        return 1
    tasks = sorted({r["task"] for r in runs})
    arms = sorted({r["arm"] for r in runs})

    print(f"# Tier 2 sweep — {base}\n")
    print(f"{len(runs)} runs · {len(tasks)} tasks · {len(arms)} arms\n")
    print("| arm | median tokens | IQR | pass rate | CPS | false-done |")
    print("|---|---|---|---|---|---|")
    for arm in arms:
        rs = [r for r in runs if r["arm"] == arm]
        toks = [r["total_tokens"] for r in rs]
        lo, hi = iqr(toks)
        passes = sum(1 for r in rs if r.get("passed"))
        fd = sum(1 for r in rs if r.get("claimed_done") and not r.get("passed"))
        print(
            f"| {arm} | {median(toks):,.0f} | {lo:,.0f}–{hi:,.0f} | "
            f"{passes}/{len(rs)} | {cps(rs):,.0f} | {fd}/{len(rs)} |"
        )

    # Paired on the task: per-task median token delta against A0, then against A5.
    for baseline in ("A0", "A5"):
        if baseline not in arms or "A1" not in arms:
            continue
        deltas = []
        for task in tasks:
            a1 = median([r["total_tokens"] for r in runs if r["task"] == task and r["arm"] == "A1"])
            bl = median([r["total_tokens"] for r in runs if r["task"] == task and r["arm"] == baseline])
            if a1 and bl:
                deltas.append(a1 - bl)
        lo, hi = paired_bootstrap(deltas)
        better, total = sign_test(deltas)
        pct = 100.0 * median(deltas) / median([abs(d) for d in deltas]) if deltas else 0.0
        print(f"\n**A1 vs {baseline}** — median per-task token delta "
              f"{median(deltas):,.0f}, 95% CI [{lo:,.0f}, {hi:,.0f}], "
              f"favourable on {better}/{total} tasks.")
        del pct

    print(f"\n_Correctness at {len(tasks)} tasks is a tripwire, not a measurement: it detects "
          f"a collapse, not a regression. Every number above carries n={len(runs)}._")
    return 0


def self_test():
    assert median([1, 2, 3]) == 2
    assert iqr([1, 2, 3, 4]) == (1.5, 3.5)
    assert cps([{"total_tokens": 100, "passed": True}, {"total_tokens": 100, "passed": False}]) == 200
    assert cps([{"total_tokens": 100, "passed": False}]) == float("inf")
    lo, hi = paired_bootstrap([-10, -12, -11, -9, -10])
    assert lo <= -9 and hi >= -12, (lo, hi)
    assert sign_test([-1, -2, 3]) == (2, 3)
    # Reproducible: the same input twice gives the same interval, or the reported CI is noise.
    assert paired_bootstrap([-10, -12, -11]) == paired_bootstrap([-10, -12, -11])
    print("self-test ok")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(report(sys.argv[1]))
```

- [ ] **Step 2: Run the self-test**

Run: `python3 scripts/eval/analyse.py --self-test`
Expected: `self-test ok`.

- [ ] **Step 3: Commit**

```bash
git add scripts/eval/analyse.py
git commit -m "feat(bench): medians, IQR and a paired bootstrap, with its own self-test"
```

---

### Task 10: The parity check, and the write-up

**Files:**
- Create: `scripts/eval/parity.sh`
- Create: `docs/eval/tier2.md`

**Interfaces:**
- Consumes: `run.sh`.
- Produces: evidence that token accounting is identical across arms, and the document the result lands in.

- [ ] **Step 1: Write the parity check**

This is risk R-b from the spec — the one that would invalidate everything silently.

Create `scripts/eval/parity.sh`:

```bash
#!/usr/bin/env bash
# Do A0 and A1 account for tokens the same way?
#
# A0 has no hooks; A1 injects a package on every prompt. If those two paths are measured from
# different fields, the headline number is an artefact of the harness and nothing in the
# results would reveal it. So: run a trivial prompt in both arms and check the accounting
# fields are populated identically, differing only in magnitude.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/bench-parity}"
rm -rf "$OUT"; mkdir -p "$OUT"

for arm in A0 A1; do
  MODEL="${MODEL:-claude-haiku-4-5}" TIMEOUT_S=180 \
    "$ROOT/scripts/eval/run.sh" A1-idempotency-key-length "$arm" 0 "$OUT/$arm" >/dev/null
done

python3 - "$OUT" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
fields = ["input_tokens", "output_tokens", "cache_read_tokens", "total_cost_usd"]
a0 = json.loads((out / "A0/usage.json").read_text())
a1 = json.loads((out / "A1/usage.json").read_text())
for f in fields:
    assert f in a0 and f in a1, f"{f} missing from one arm"
    assert isinstance(a0[f], (int, float)) and isinstance(a1[f], (int, float)), f
assert a0["input_tokens"] > 0 and a1["input_tokens"] > 0, "both arms must record real input"
print("parity ok — both arms populate every accounting field from the same source")
print(f"  A0 input={a0['input_tokens']} output={a0['output_tokens']}")
print(f"  A1 input={a1['input_tokens']} output={a1['output_tokens']}")
PY
```

- [ ] **Step 2: Run it**

```bash
chmod +x scripts/eval/parity.sh
./scripts/eval/parity.sh
```

Expected: `parity ok`, and A1's input tokens visibly larger than A0's — that difference is the
injected package, and it is the thing being measured.

- [ ] **Step 3: Write the result document's skeleton**

Create `docs/eval/tier2.md`:

```markdown
# Tier 2 — cost per success

**Status: not yet run.** This document is the shape the result lands in. Every number here is
a placeholder until a sweep fills it, and the placeholders say so rather than reading as zero.

Design: [`2026-09-04-tier2-benchmark-design.md`](../superpowers/specs/2026-09-04-tier2-benchmark-design.md).
Reproduce with `make bench`, analyse with `python3 scripts/eval/analyse.py docs/eval/runs/<stamp>`.

## The run

| | |
|---|---|
| Model | `claude-opus-5`, pinned |
| Tasks | 5 — A1, A2, B1, B2, C1 |
| Arms | A0 bare · A1 Nexus · A5 BM25 control |
| Repetitions | 5 per (task × arm) — 75 runs |
| Nexus version | *(fill from the sweep)* |

## What this can support

Cost, and cost alone. Correctness at five tasks is a tripwire that would show a collapse and
nothing finer; `13-evaluation.md` §9 puts the resolution at 15–20 pp with *seventeen* tasks.
Every correctness figure below carries its sample size, and quoting one without it is a
misuse of this document.

## Result

*(the analysis table goes here)*

## A1 vs A5 — did ranking earn its complexity

The pre-registered threshold is `13-evaluation.md` §11's T4: a median CPS reduction of **≥ 30 %**
with a 95 % bootstrap CI excluding zero. This slice reports against it and does not gate on it.

If A1 does not beat A5, the finding is published here with the same prominence a positive
result would get, and the consequence `13-evaluation.md` §5 names — ship BM25, delete the
Context Engine — becomes a live proposal rather than a rhetorical one.

## What went wrong

*(every sweep records its own failures here: runs that timed out, diffs that did not apply,
tasks whose hidden tests turned out to grade conformity. A benchmark with no defects section
is one nobody read closely.)*
```

- [ ] **Step 4: `make check`, then commit**

```bash
make check
git add scripts/eval/parity.sh docs/eval/tier2.md
git commit -m "feat(bench): the token-accounting parity check, and where the result lands"
```

---

## Self-review

**Spec coverage.** §3 tasks → Tasks 4, 5 (hidden tests) and 6 (task lookup). §4 arms → Task 6's
arm configurations. §5 lexical control → Tasks 1, 2, 3. §6 runner → Task 6. §7 grading →
Task 7. §8 output and analysis → Tasks 6 and 9. §9 what ships where → Tasks 6–10. §10 R-a →
Task 4 Step 1's README and the "fails at the starting commit" checks in Tasks 4–5; R-b →
Task 10's parity check; R-c and R-d → recorded in `docs/eval/tier2.md`; R-e → the same
document's A1-vs-A5 section.

**Placeholders.** None in the plan. `docs/eval/tier2.md` contains deliberate placeholders,
marked as such, because it is a template for a result that does not exist yet.

**Type consistency.** `RankMode` and `bm25` are defined in Task 1 and consumed in Tasks 2 and 3.
`TaskRequest.rank` is added in Task 2 and set in Task 3. `Store::file_texts` is defined in
Task 2 Step 3 and called in Step 4. `run.sh`'s output contract — `diff.patch`, `transcript.json`,
`usage.json` — is produced in Task 6 and consumed by Task 7 (`diff.patch`) and Task 9
(`usage.json`, `grade.json`).

**Two places the executor will have to adapt rather than copy**, both flagged inline: the
`files` table may have no `content` column (Task 2 Step 3), and the hidden tests must match the
fixtures' real class and method names (Tasks 4–5). Both say what to check and what to do about
it. Everything else is literal.

**Known risk this plan does not remove.** Tasks 4 and 5 are the only ones whose quality cannot
be verified by running them — a hidden test that passes at the starting commit is caught by the
step that checks for it, but a hidden test that *grades conformity* looks identical to a good
one until the results come in skewed. The README written in Task 4 Step 1 is the only defence,
and it is a written rule rather than a mechanism.
