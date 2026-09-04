# Resolution Accuracy Harness — Implementation Plan (Plan B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure whether a resolved edge points at the *right* symbol, against a compiler-grade oracle, and turn nine chosen confidence constants into estimated ones.

**Architecture:** Nexus gains one output — `graph --edges <path>`, NDJSON. A new development-only crate `nexus-eval` reads that plus a SCIP index produced by a real compiler frontend, matches them **positionally** (never by name), and reports precision, recall, strict site accuracy and per-tier calibration with Wilson intervals. The crate is never compiled into the shipped binary.

**Tech Stack:** Rust 1.82+, `scip` 0.9 + `protobuf` =3.7.2 (rust-protobuf, not prost), `rust-analyzer scip` / `scip-java` / `scip-typescript` / `scip-python`.

**Spec:** [`docs/superpowers/specs/2026-09-03-resolution-accuracy-harness-design.md`](../specs/2026-09-03-resolution-accuracy-harness-design.md)

**Prerequisite:** [Plan A](2026-09-03-resolution-metric-fixes.md), complete as of `bc7c3cf`. Tier labels are trustworthy only after `65d4acc`; per-tier calibration on the old labels would bin `sibling` and `external-graph` edges into `heuristic`.

## Global Constraints

- Rust 1.82+; CI runs `RUSTFLAGS=-D warnings`, so a warning fails the build.
- **Only `nexus-store` contains SQL.** The new crate reads NDJSON, never the database.
- **Nothing in the workspace may depend on `nexus-eval`**, enforced in `crates/nexus-cli/tests/boundaries.rs`. It is the mirror of `nexus-fixtures`: fixtures generate and never mark; eval marks and never generates.
- `protobuf` must be pinned `=3.7.2` — `scip` pins it exactly and the generated types must unify.
- A command emits **exactly one** JSON document on stdout; the edge dump therefore goes to a file, never stdout.
- **Branch discipline on `task/retrieval`: never rebase, amend across, or force-push.** Stage explicit paths; a parallel session shares this working tree.
- `make check` must pass before every commit. `make eval` is **not** part of `make check`.

---

### Task 1: `graph --edges` writes the edge dump

**Files:**
- Modify: `crates/nexus-store/src/lib.rs` (new `all_edges` method, near `edge_counts` at ~L1265)
- Modify: `crates/nexus-core/src/report.rs` (new `EdgeRecord`)
- Modify: `crates/nexus-core/src/engine/query.rs` (new `Engine::edge_records`)
- Modify: `crates/nexus-cli/src/main.rs` (the `Graph` command gains `--edges`)
- Test: `crates/nexus-cli/tests/edge_dump.rs`

**Interfaces:**
- Produces: `nexus_core::report::EdgeRecord { src_fqn: String, src_file: String, site_line: Option<i64>, edge_type: String, dst_fqn: Option<String>, dst_file: Option<String>, dst_start_line: Option<i64>, dst_end_line: Option<i64>, resolution: String, confidence: f64 }`, serialized one per line. Task 4 consumes it.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-cli/tests/edge_dump.rs`. Copy `nexus()`, `git()` and the Rust fixture from `crates/nexus-cli/tests/metric_agreement.rs` verbatim — the executor may be reading these tasks out of order, and that fixture is the one proven to produce a fan-out (3 rows, 1 site):

```rust
#[test]
fn the_edge_dump_has_one_line_per_edge_row_not_per_site() {
    // The dump is the harness's input and must be the *un*collapsed truth: precision is an
    // edge-level metric, and a fan-out of three candidates is three chances to be wrong.
    // `graph`'s summary counts sites; this file must not.
    let root = project("dump");
    run(&root, &["scan"]);
    let out = root.join("edges.ndjson");
    run(&root, &["graph", "--edges", out.to_str().expect("path")]);

    let body = std::fs::read_to_string(&out).expect("dump written");
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "three candidate rows for one call site:\n{body}");

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is one object");
        assert_eq!(v["resolution"], "heuristic");
        assert_eq!(v["site_line"].as_i64().is_some(), true, "a site needs a line");
        assert!(v["dst_file"].as_str().is_some(), "a bound edge names its destination file");
    }
}

#[test]
fn stdout_still_holds_exactly_one_json_document_when_edges_are_dumped() {
    // json_contract.rs pins this for every command; --edges must not break it by streaming
    // the dump to stdout.
    let root = project("dump-json");
    run(&root, &["scan"]);
    let out = root.join("e.ndjson");
    let stdout = run(
        &root,
        &["graph", "--json", "--edges", out.to_str().expect("path")],
    );
    let n = serde_json::Deserializer::from_str(&stdout)
        .into_iter::<serde_json::Value>()
        .count();
    assert_eq!(n, 1, "stdout must stay one document:\n{stdout}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-cli --test edge_dump`
Expected: FAIL — `error: unexpected argument '--edges'`.

- [ ] **Step 3: Add the store query**

In `crates/nexus-store/src/lib.rs`, next to `edge_counts`:

```rust
/// Every edge row, uncollapsed, with both endpoints' positions.
///
/// Deliberately *not* the call-site unit `edge_counts` uses: this feeds accuracy
/// measurement, where a fan-out of three candidates is three separate chances to be wrong
/// and precision must be able to see all three.
pub fn all_edges(&self, project_id: ProjectId) -> Result<Vec<EdgeRow>> {
    let mut stmt = self.conn.prepare(
        "SELECT s.fqn, sf.path, e.site_line, e.edge_type,
                d.fqn, df.path, d.start_line, d.end_line,
                e.resolution, e.confidence
         FROM symbol_edges e
         JOIN symbols s  ON s.id = e.src_symbol_id AND s.deleted = 0
         JOIN files   sf ON sf.id = s.file_id
         LEFT JOIN symbols d  ON d.id = e.dst_symbol_id AND d.deleted = 0
         LEFT JOIN files   df ON df.id = d.file_id
         WHERE e.project_id = ?1
         ORDER BY sf.path, e.site_line",
    )?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(EdgeRow {
                src_fqn: r.get(0)?,
                src_file: r.get(1)?,
                site_line: r.get(2)?,
                edge_type: r.get(3)?,
                dst_fqn: r.get(4)?,
                dst_file: r.get(5)?,
                dst_start_line: r.get(6)?,
                dst_end_line: r.get(7)?,
                resolution: r.get(8)?,
                confidence: r.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
```

and the row type next to `EdgeCounts`:

```rust
#[derive(Debug, Clone)]
pub struct EdgeRow {
    pub src_fqn: String,
    pub src_file: String,
    pub site_line: Option<i64>,
    pub edge_type: String,
    pub dst_fqn: Option<String>,
    pub dst_file: Option<String>,
    pub dst_start_line: Option<i64>,
    pub dst_end_line: Option<i64>,
    pub resolution: String,
    pub confidence: f64,
}
```

- [ ] **Step 4: Add the report type and the engine method**

In `crates/nexus-core/src/report.rs`, beside `GraphReport`:

```rust
/// One edge row as the accuracy harness consumes it. Serialized one per line as NDJSON,
/// never into the single-document stdout contract.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeRecord {
    pub src_fqn: String,
    pub src_file: String,
    pub site_line: Option<i64>,
    pub edge_type: String,
    pub dst_fqn: Option<String>,
    pub dst_file: Option<String>,
    pub dst_start_line: Option<i64>,
    pub dst_end_line: Option<i64>,
    pub resolution: String,
    pub confidence: f64,
}
```

In `crates/nexus-core/src/engine/query.rs`, beside `Engine::graph`:

```rust
/// The uncollapsed edge list, for out-of-band accuracy measurement.
pub fn edge_records(&self) -> Result<Vec<EdgeRecord>> {
    Ok(self
        .store
        .all_edges(self.project_id)?
        .into_iter()
        .map(|e| EdgeRecord {
            src_fqn: e.src_fqn,
            src_file: e.src_file,
            site_line: e.site_line,
            edge_type: e.edge_type,
            dst_fqn: e.dst_fqn,
            dst_file: e.dst_file,
            dst_start_line: e.dst_start_line,
            dst_end_line: e.dst_end_line,
            resolution: e.resolution,
            confidence: e.confidence,
        })
        .collect())
}
```

- [ ] **Step 5: Wire the flag**

In `crates/nexus-cli/src/main.rs`, change the `Graph` variant to carry a path and write the file before rendering the summary:

```rust
    /// Dependency graph size and how much of it resolved
    Graph {
        /// Also write every edge row to this path as NDJSON, for `nexus-eval`.
        /// Not stdout: `--json` is exactly one document, and an edge list is not it.
        #[arg(long, value_name = "PATH")]
        edges: Option<PathBuf>,
    },
```

and in the `Command::Graph` arm, before rendering:

```rust
        Command::Graph { edges } => {
            let engine = open(&project)?;
            if let Some(path) = edges {
                use std::io::Write as _;
                let file = std::fs::File::create(&path)?;
                let mut w = std::io::BufWriter::new(file);
                for rec in engine.edge_records()? {
                    // One object per line. Streamed rather than collected into a Vec and
                    // serialized: this is the one output whose size is proportional to the
                    // repository rather than to the answer.
                    writeln!(w, "{}", serde_json::to_string(&rec)?)?;
                }
                w.flush()?;
            }
            let report = engine.graph()?;
            // ... existing rendering, unchanged
        }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p nexus-cli --test edge_dump && cargo test -p nexus-cli --test json_contract`
Expected: PASS, 2 + 2 tests.

- [ ] **Step 7: `make check`, then commit**

```bash
make check
git add crates/nexus-store/src/lib.rs crates/nexus-core/src/report.rs \
        crates/nexus-core/src/engine/query.rs crates/nexus-cli/src/main.rs \
        crates/nexus-cli/tests/edge_dump.rs
git commit -m "feat(graph): dump the uncollapsed edge list for accuracy measurement"
```

---

### Task 2: The `nexus-eval` crate exists and nothing depends on it

**Files:**
- Create: `crates/nexus-eval/Cargo.toml`, `crates/nexus-eval/src/lib.rs`, `crates/nexus-eval/src/main.rs`
- Modify: `Cargo.toml` (workspace `members`)
- Modify: `crates/nexus-cli/tests/boundaries.rs`

**Interfaces:**
- Produces: the crate `nexus-eval` with a binary of the same name. Tasks 3–7 add modules to it.

- [ ] **Step 1: Write the failing boundary test**

In `crates/nexus-cli/tests/boundaries.rs`, beside `nothing_but_the_composition_root_depends_on_the_fixture_generator`:

```rust
/// `nexus-eval` marks the graph's homework. Nothing may depend on it — a crate that both
/// produces a number and grades it has nothing checking it, which is the same argument
/// `nexus-fixtures` makes in the other direction: the generator must not mark its own work.
///
/// It also drags in protobuf and the `scip` types, which have no business in a binary whose
/// whole claim is that it is deterministic and dependency-light.
#[test]
fn nothing_depends_on_the_evaluator() {
    let g = dependency_graph();
    for crate_name in g.keys() {
        if crate_name == "nexus-eval" {
            continue;
        }
        assert_forbidden(
            &g,
            crate_name,
            "nexus-eval",
            "the evaluator must not be reachable from anything it measures",
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p nexus-cli --test boundaries nothing_depends_on_the_evaluator`
Expected: PASS trivially — `assert_forbidden` skips a `to` absent from the graph, exactly as its doc comment at `boundaries.rs:309` says. That is correct and expected; the test becomes load-bearing the moment the crate exists. Proceed.

- [ ] **Step 3: Create the crate**

`crates/nexus-eval/Cargo.toml`:

```toml
[package]
name = "nexus-eval"
description = "Measures whether a resolved edge points at the right symbol, against a SCIP oracle. Development only; never shipped."
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[dependencies]
scip = "0.9"
# rust-protobuf, not prost, and pinned exactly: `scip` pins `=3.7.2` and the generated
# types must unify or they are different types with the same name.
protobuf = "=3.7.2"
serde = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }
```

`crates/nexus-eval/src/lib.rs`:

```rust
//! Does a resolved edge point at the *right* symbol?
//!
//! Nexus reports coverage — the share of call sites that found a destination. Nothing in the
//! product checks that the destination is correct, and the confidence on every edge is a
//! probability claim nobody has ever tested. This crate tests both, against an index produced
//! by a real compiler frontend.
//!
//! **Boundary.** Nothing in the workspace may depend on this crate; `nexus-cli/tests/
//! boundaries.rs` fails the build if anything does. It is the mirror of `nexus-fixtures`,
//! which generates repositories and must never index them: a component that produces a number
//! and also grades it has nothing checking it.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod edges;
pub mod matcher;
pub mod metrics;
pub mod oracle;
pub mod report;
```

`crates/nexus-eval/src/main.rs`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    nexus_eval::run(std::env::args_os())
}
```

Add `"crates/nexus-eval"` to `members` in the workspace `Cargo.toml`. Do **not** add it to `[workspace.dependencies]` — nothing may depend on it.

- [ ] **Step 4: `make check`, then commit**

Run: `make check`
Expected: PASS. `nothing_depends_on_the_evaluator` is now load-bearing.

```bash
git add Cargo.toml Cargo.lock crates/nexus-eval crates/nexus-cli/tests/boundaries.rs
git commit -m "feat(eval): the crate that marks the graph's homework, depended on by nothing"
```

---

### Task 3: Read a SCIP index into a definition map

**Files:**
- Create: `crates/nexus-eval/src/oracle.rs`
- Test: inline `mod tests` in the same file

**Interfaces:**
- Produces: `oracle::Oracle { defs: HashMap<String, Position>, refs: Vec<Reference>, files: HashSet<String> }`, `oracle::Position { file: String, line: i64 }`, `oracle::Reference { file: String, line: i64, symbol: String }`, and `Oracle::load(path: &Path) -> Result<Oracle, OracleError>`. Task 4 consumes it.

- [ ] **Step 1: Write the failing test**

A hand-built index, so the reader is tested against bytes we control rather than against whatever an indexer happened to emit:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;
    use scip::types::{Document, Index, Occurrence, SymbolRole};

    fn occurrence(symbol: &str, line: i32, definition: bool) -> Occurrence {
        let mut o = Occurrence::new();
        o.symbol = symbol.to_string();
        // The legacy three-element form: [line, startChar, endChar]. Deprecated in the proto
        // in favour of `typed_range`, and still what every current indexer emits.
        o.range = vec![line, 0, 10];
        o.symbol_roles = if definition { SymbolRole::Definition as i32 } else { 0 };
        o
    }

    fn index() -> Index {
        let mut def_doc = Document::new();
        def_doc.relative_path = "src/a.rs".into();
        def_doc.occurrences = vec![
            occurrence("rust-analyzer cargo demo 0.1.0 Alpha#save().", 41, true),
            occurrence("local 3", 4, true),
        ];

        let mut ref_doc = Document::new();
        ref_doc.relative_path = "src/b.rs".into();
        ref_doc.occurrences = vec![
            occurrence("rust-analyzer cargo demo 0.1.0 Alpha#save().", 7, false),
            occurrence("rust-analyzer cargo std 1.0.0 Vec#push().", 9, false),
        ];

        let mut ix = Index::new();
        ix.documents = vec![def_doc, ref_doc];
        ix
    }

    fn write(ix: &Index) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("scip-{}.scip", std::process::id()));
        let mut f = std::fs::File::create(&p).expect("create");
        ix.write_to_writer(&mut f).expect("write");
        p
    }

    #[test]
    fn a_definition_is_found_across_documents() {
        // The definition of a symbol referenced in b.rs lives in a.rs. The map must be
        // index-wide: rust-analyzer only sets the Definition role at the defining position,
        // never on the reference site.
        let o = Oracle::load(&write(&index())).expect("load");
        let pos = o
            .defs
            .get("rust-analyzer cargo demo 0.1.0 Alpha#save().")
            .expect("definition found");
        assert_eq!(pos.file, "src/a.rs");
        assert_eq!(pos.line, 41);
    }

    #[test]
    fn a_reference_with_no_definition_in_the_index_is_not_an_error() {
        // `Vec#push` is defined in std, which was not indexed. SymbolInformation in
        // `external_symbols` carries no file and no range, so such a symbol is positionally
        // unlocatable — the normal case, not a failure.
        let o = Oracle::load(&write(&index())).expect("load");
        assert!(!o.defs.contains_key("rust-analyzer cargo std 1.0.0 Vec#push()."));
        assert_eq!(o.refs.len(), 2, "both references are still recorded");
    }

    #[test]
    fn local_symbols_are_skipped() {
        // `local N` numbering restarts per document in scip-typescript and scip-java, so
        // `local 3` names different entities in different files. Function-scoped locals carry
        // no cross-file edges anyway.
        let o = Oracle::load(&write(&index())).expect("load");
        assert!(o.defs.keys().all(|k| !k.starts_with("local ")));
    }

    #[test]
    fn every_document_is_recorded_for_the_coverage_check() {
        // scip-typescript silently skips files over 1MB and scip-python emits partial
        // indexes on timeout. A partial oracle inflates precision, so the file set is a
        // first-class output.
        let o = Oracle::load(&write(&index())).expect("load");
        assert!(o.files.contains("src/a.rs") && o.files.contains("src/b.rs"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-eval oracle`
Expected: FAIL to compile — `cannot find type Oracle`.

- [ ] **Step 3: Write the reader**

```rust
use protobuf::Message;
use scip::types::{Index, SymbolRole};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    #[error("reading {0}: {1}")]
    Io(String, std::io::Error),
    #[error("{0} is not a SCIP index: {1}")]
    Parse(String, protobuf::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub file: String,
    pub line: i64,
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub file: String,
    pub line: i64,
    pub symbol: String,
}

#[derive(Debug, Default)]
pub struct Oracle {
    /// Index-wide, because a cross-file reference's definition lives in another `Document`.
    pub defs: HashMap<String, Position>,
    pub refs: Vec<Reference>,
    /// Every file the oracle actually indexed, for the coverage cross-check.
    pub files: HashSet<String>,
}

/// The start line of an occurrence, handling both encodings.
///
/// `Occurrence.range` is deprecated in the proto in favour of `typed_range`, and every
/// current indexer still emits the deprecated form. A reader that handles only one silently
/// sees zero occurrences — which would read as "Nexus resolved nothing correctly".
fn start_line(occ: &scip::types::Occurrence) -> Option<i64> {
    if let Some(first) = occ.range.first() {
        return Some(*first as i64);
    }
    match occ.typed_range.as_ref()? {
        scip::types::occurrence::Typed_range::SingleLineRange(r) => Some(r.line as i64),
        scip::types::occurrence::Typed_range::MultiLineRange(r) => Some(r.start_line as i64),
    }
}

impl Oracle {
    pub fn load(path: &Path) -> Result<Self, OracleError> {
        let name = path.display().to_string();
        let file = std::fs::File::open(path).map_err(|e| OracleError::Io(name.clone(), e))?;
        let mut reader = std::io::BufReader::new(file);
        let index = Index::parse_from_reader(&mut reader)
            .map_err(|e| OracleError::Parse(name.clone(), e))?;

        let mut out = Oracle::default();
        for doc in &index.documents {
            out.files.insert(doc.relative_path.clone());
            for occ in &doc.occurrences {
                // The `local ` prefix is reserved by the grammar and its numbering restarts
                // per document in two of the four indexers, so a bare `local N` is not a key.
                if occ.symbol.starts_with("local ") {
                    continue;
                }
                let Some(line) = start_line(occ) else { continue };
                if occ.symbol_roles & (SymbolRole::Definition as i32) != 0 {
                    out.defs.insert(
                        occ.symbol.clone(),
                        Position { file: doc.relative_path.clone(), line },
                    );
                } else {
                    out.refs.push(Reference {
                        file: doc.relative_path.clone(),
                        line,
                        symbol: occ.symbol.clone(),
                    });
                }
            }
        }
        Ok(out)
    }
}
```

Add `thiserror = { workspace = true }` to the crate's dependencies.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-eval oracle`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-eval/src/oracle.rs crates/nexus-eval/Cargo.toml Cargo.lock
git commit -m "feat(eval): read a SCIP index, both range encodings, locals skipped"
```

---

### Task 4: Match positionally, and state the comparable set

**Files:**
- Create: `crates/nexus-eval/src/edges.rs` (read the NDJSON dump), `crates/nexus-eval/src/matcher.rs`
- Test: inline `mod tests` in `matcher.rs`

**Interfaces:**
- Consumes: `oracle::Oracle` (Task 3), `EdgeRecord`'s JSON shape (Task 1).
- Produces: `matcher::Judged { site: (String, i64), tier: String, confidence: f64, correct: bool }`, `matcher::Comparison { judged: Vec<Judged>, sites_total: usize, excluded_non_project: usize, excluded_oracle_blind: usize }`, and `matcher::compare(&[edges::Edge], &Oracle) -> Comparison`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::Edge;
    use crate::oracle::{Oracle, Position, Reference};

    fn edge(site_line: i64, dst_file: &str, start: i64, end: i64, tier: &str, conf: f64) -> Edge {
        Edge {
            src_fqn: "demo::Caller#run".into(),
            src_file: "src/b.rs".into(),
            site_line: Some(site_line),
            edge_type: "calls".into(),
            dst_fqn: Some("demo::Alpha#save".into()),
            dst_file: Some(dst_file.into()),
            dst_start_line: Some(start),
            dst_end_line: Some(end),
            resolution: tier.into(),
            confidence: conf,
        }
    }

    fn oracle() -> Oracle {
        let mut o = Oracle::default();
        o.defs.insert("S Alpha#save().".into(), Position { file: "src/a.rs".into(), line: 41 });
        o.refs.push(Reference { file: "src/b.rs".into(), line: 7, symbol: "S Alpha#save().".into() });
        o.files.insert("src/a.rs".into());
        o.files.insert("src/b.rs".into());
        o
    }

    #[test]
    fn a_destination_whose_span_contains_the_definition_is_correct() {
        // Positional, never by name: SCIP writes `Alpha#save().` and Nexus writes
        // `demo::Alpha#save(&self)`. Every rule mapping one to the other is a place a nicer
        // number could be manufactured; a line number has no knobs.
        let c = compare(&[edge(7, "src/a.rs", 40, 43, "heuristic", 0.6)], &oracle());
        assert_eq!(c.judged.len(), 1);
        assert!(c.judged[0].correct, "definition at 41 falls inside 40..=43");
    }

    #[test]
    fn a_destination_in_the_right_file_but_the_wrong_span_is_wrong() {
        let c = compare(&[edge(7, "src/a.rs", 90, 99, "heuristic", 0.6)], &oracle());
        assert!(!c.judged[0].correct);
    }

    #[test]
    fn a_fan_out_is_judged_per_edge_so_three_candidates_are_three_judgements() {
        // Precision is edge-level on purpose: one right and two wrong at a single site is
        // 1/3, not 1/1. This is the arithmetic the old row-counted metric got backwards.
        let c = compare(
            &[
                edge(7, "src/a.rs", 40, 43, "heuristic", 0.2),
                edge(7, "src/c.rs", 1, 5, "heuristic", 0.2),
                edge(7, "src/d.rs", 1, 5, "heuristic", 0.2),
            ],
            &oracle(),
        );
        assert_eq!(c.judged.len(), 3);
        assert_eq!(c.judged.iter().filter(|j| j.correct).count(), 1);
    }

    #[test]
    fn an_edge_the_oracle_cannot_speak_about_is_excluded_not_counted_wrong() {
        // SCIP has no opinion about a GraphQL seam or a Spring bean. Counting the oracle's
        // blind spots as Nexus's errors is the mistake ADR-017 already caught once.
        let mut e = edge(7, "src/a.rs", 40, 43, "heuristic", 0.6);
        e.edge_type = "calls_graphql".into();
        let c = compare(&[e], &oracle());
        assert!(c.judged.is_empty());
        assert_eq!(c.excluded_oracle_blind, 1);
    }

    #[test]
    fn a_site_the_oracle_never_saw_is_excluded_not_counted_wrong() {
        // No reference recorded at that line: the oracle is silent, not contradicting.
        let c = compare(&[edge(999, "src/a.rs", 40, 43, "heuristic", 0.6)], &oracle());
        assert!(c.judged.is_empty());
        assert_eq!(c.excluded_non_project, 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-eval matcher`
Expected: FAIL to compile — `cannot find function compare`.

- [ ] **Step 3: Write the NDJSON reader**

`crates/nexus-eval/src/edges.rs`:

```rust
use serde::Deserialize;
use std::path::Path;

/// Mirrors `nexus_core::report::EdgeRecord`. Duplicated rather than imported because this
/// crate must not depend on `nexus-core` — it reads a file, not a library.
#[derive(Debug, Clone, Deserialize)]
pub struct Edge {
    pub src_fqn: String,
    pub src_file: String,
    pub site_line: Option<i64>,
    pub edge_type: String,
    pub dst_fqn: Option<String>,
    pub dst_file: Option<String>,
    pub dst_start_line: Option<i64>,
    pub dst_end_line: Option<i64>,
    pub resolution: String,
    pub confidence: f64,
}

pub fn load(path: &Path) -> std::io::Result<Vec<Edge>> {
    let body = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (n, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(e) => out.push(e),
            // One malformed line must not discard the run: say which, keep the rest.
            Err(e) => eprintln!("{}:{}: skipped unparseable edge: {e}", path.display(), n + 1),
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Write the matcher**

`crates/nexus-eval/src/matcher.rs`:

```rust
use crate::edges::Edge;
use crate::oracle::Oracle;
use std::collections::HashMap;

/// Edge types SCIP can judge. Everything else — the GraphQL and HTTP seam, route tables,
/// ORM persistence, renders — is a relationship no compiler frontend models, so it is
/// excluded from both numerator and denominator rather than scored.
const COMPARABLE_EDGE_TYPES: &[&str] = &["calls", "implements", "extends", "imports"];

/// Tiers whose edges nobody resolved against a symbol table.
const NON_RESOLVING_TIERS: &[&str] = &["external", "sibling", "external-graph", "unresolved", "framework"];

#[derive(Debug, Clone)]
pub struct Judged {
    pub site: (String, i64),
    pub tier: String,
    pub confidence: f64,
    pub correct: bool,
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub judged: Vec<Judged>,
    pub sites_total: usize,
    pub excluded_non_project: usize,
    pub excluded_oracle_blind: usize,
}

pub fn compare(edges: &[Edge], oracle: &Oracle) -> Comparison {
    // Where the oracle says each reference resolves to.
    let mut truth: HashMap<(String, i64), &crate::oracle::Position> = HashMap::new();
    for r in &oracle.refs {
        if let Some(pos) = oracle.defs.get(&r.symbol) {
            truth.insert((r.file.clone(), r.line), pos);
        }
    }

    let mut out = Comparison::default();
    let mut sites = std::collections::HashSet::new();

    for e in edges {
        let Some(line) = e.site_line else { continue };
        let site = (e.src_file.clone(), line);
        sites.insert(site.clone());

        if !COMPARABLE_EDGE_TYPES.contains(&e.edge_type.as_str())
            || NON_RESOLVING_TIERS.contains(&e.resolution.as_str())
        {
            out.excluded_oracle_blind += 1;
            continue;
        }
        let Some(def) = truth.get(&site) else {
            // The oracle recorded no in-project reference here: it is silent, not
            // contradicting. Excluded, and reported separately.
            out.excluded_non_project += 1;
            continue;
        };
        let correct = match (&e.dst_file, e.dst_start_line, e.dst_end_line) {
            (Some(f), Some(s), Some(en)) => *f == def.file && def.line >= s && def.line <= en,
            _ => false,
        };
        out.judged.push(Judged {
            site,
            tier: e.resolution.clone(),
            confidence: e.confidence,
            correct,
        });
    }
    out.sites_total = sites.len();
    out
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nexus-eval matcher`
Expected: PASS, 5 tests.

- [ ] **Step 6: `make check`, then commit**

```bash
make check
git add crates/nexus-eval/src/edges.rs crates/nexus-eval/src/matcher.rs crates/nexus-eval/src/lib.rs
git commit -m "feat(eval): match a bound destination by position, never by name"
```

---

### Task 5: The metrics, with intervals

**Files:**
- Create: `crates/nexus-eval/src/metrics.rs`
- Test: inline `mod tests`, asserting against values computed by hand

**Interfaces:**
- Consumes: `matcher::Comparison`.
- Produces: `metrics::wilson(k: u64, n: u64) -> (f64, f64)` returning `(low, high)`; `metrics::Scores { precision: Rate, recall: Rate, strict: Rate, f1: f64 }`; `metrics::Rate { value: f64, low: f64, high: f64, n: u64 }`; `metrics::score(&Comparison) -> Scores`. Task 6 reuses `wilson` and `Rate`.

- [ ] **Step 1: Write the failing test**

The instrument is worthless unless it reproduces arithmetic done on paper:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 5e-4, "{msg}: got {a}, expected {b}");
    }

    #[test]
    fn wilson_matches_the_hand_computed_interval() {
        // n = 100, k = 80, z = 1.96.
        //   centre = (0.8 + 1.9208/200) / (1 + 0.038416)     = 0.80960/1.038416 = 0.779534
        //   half   = (1.96/1.038416) · sqrt(0.16/100 + 0.0384/40000)
        //          = 1.887528 · sqrt(0.0016 + 0.00000096) = 1.887528 · 0.0400120 = 0.075523
        let (low, high) = wilson(80, 100);
        close(low, 0.704011, "lower bound");
        close(high, 0.855057, "upper bound");
    }

    #[test]
    fn wilson_stays_inside_zero_and_one_at_the_extremes() {
        // The reason this is Wilson and not the normal approximation: at k == n the normal
        // interval runs past 1.0, and per-tier accuracies sit near 1.0.
        let (low, high) = wilson(12, 12);
        assert!(low > 0.0 && high <= 1.0, "12/12 gave [{low}, {high}]");
        let (low, high) = wilson(0, 12);
        assert!(low >= 0.0 && high < 1.0, "0/12 gave [{low}, {high}]");
    }

    #[test]
    fn an_empty_sample_is_a_zero_width_claim_about_nothing() {
        let (low, high) = wilson(0, 0);
        assert_eq!((low, high), (0.0, 1.0), "no data means no information, not certainty");
    }

    #[test]
    fn precision_is_edge_level_and_recall_is_site_level() {
        // One site, three candidate edges, one correct: precision 1/3, recall 1/1.
        // Reporting recall alone is the old failure — it is the number that *rises* when the
        // resolver fans out.
        let c = Comparison {
            judged: vec![
                judged("a.rs", 7, true),
                judged("a.rs", 7, false),
                judged("a.rs", 7, false),
            ],
            sites_total: 1,
            ..Default::default()
        };
        let s = score(&c);
        close(s.precision.value, 1.0 / 3.0, "precision");
        close(s.recall.value, 1.0, "recall");
        close(s.strict.value, 0.0, "strict: the site was ambiguous, so it is not strictly right");
        close(s.f1, 0.5, "f1 of 1/3 and 1");
    }

    #[test]
    fn a_single_correct_edge_at_a_site_is_strictly_right() {
        let c = Comparison {
            judged: vec![judged("a.rs", 7, true)],
            sites_total: 1,
            ..Default::default()
        };
        let s = score(&c);
        close(s.strict.value, 1.0, "one candidate, correct");
    }
}
```

with the helper, in the same module:

```rust
    fn judged(file: &str, line: i64, correct: bool) -> crate::matcher::Judged {
        crate::matcher::Judged {
            site: (file.into(), line),
            tier: "heuristic".into(),
            confidence: 0.6,
            correct,
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-eval metrics`
Expected: FAIL to compile — `cannot find function wilson`.

- [ ] **Step 3: Write the metrics**

```rust
use crate::matcher::Comparison;
use std::collections::HashMap;

const Z: f64 = 1.96;

/// The Wilson score interval.
///
/// Not the normal approximation: per-tier samples are small and their accuracies sit near
/// 1.0, exactly where the normal interval extends past 100 % and stops being an interval.
pub fn wilson(k: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        // No data is no information. Claiming (0,0) would assert certainty of failure.
        return (0.0, 1.0);
    }
    let n_f = n as f64;
    let p = k as f64 / n_f;
    let denom = 1.0 + Z * Z / n_f;
    let centre = (p + Z * Z / (2.0 * n_f)) / denom;
    let half = (Z / denom) * (p * (1.0 - p) / n_f + Z * Z / (4.0 * n_f * n_f)).sqrt();
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Rate {
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub n: u64,
}

impl Rate {
    pub fn new(k: u64, n: u64) -> Self {
        let (low, high) = wilson(k, n);
        Rate {
            value: if n == 0 { 0.0 } else { k as f64 / n as f64 },
            low,
            high,
            n,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Scores {
    /// Edge-level: a fan-out of four with one correct scores 0.25. This is what prices
    /// ambiguity, and the reason precision and recall are always reported as a pair.
    pub precision: Rate,
    /// Site-level: did the truth appear anywhere in the candidate set.
    pub recall: Rate,
    /// Site-level and unforgiving: exactly one candidate, and it is right.
    pub strict: Rate,
    pub f1: f64,
}

pub fn score(c: &Comparison) -> Scores {
    let edges_total = c.judged.len() as u64;
    let edges_correct = c.judged.iter().filter(|j| j.correct).count() as u64;

    let mut per_site: HashMap<&(String, i64), (usize, usize)> = HashMap::new();
    for j in &c.judged {
        let e = per_site.entry(&j.site).or_insert((0, 0));
        e.0 += 1;
        if j.correct {
            e.1 += 1;
        }
    }
    let sites = per_site.len() as u64;
    let sites_hit = per_site.values().filter(|(_, right)| *right > 0).count() as u64;
    let sites_strict = per_site
        .values()
        .filter(|(total, right)| *total == 1 && *right == 1)
        .count() as u64;

    let precision = Rate::new(edges_correct, edges_total);
    let recall = Rate::new(sites_hit, sites);
    let strict = Rate::new(sites_strict, sites);
    let f1 = if precision.value + recall.value > 0.0 {
        2.0 * precision.value * recall.value / (precision.value + recall.value)
    } else {
        0.0
    };
    Scores { precision, recall, strict, f1 }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-eval metrics`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-eval/src/metrics.rs
git commit -m "feat(eval): precision, recall and strict accuracy, each with a Wilson interval"
```

---

### Task 6: Calibration, and a corrected constant that refuses to overfit

**Files:**
- Modify: `crates/nexus-eval/src/metrics.rs`
- Test: inline `mod tests`

**Interfaces:**
- Consumes: `matcher::Comparison`, `metrics::wilson`, `metrics::Rate`.
- Produces: `metrics::brier(&Comparison) -> f64`, `metrics::ece(&[TierResult]) -> f64`, `metrics::TierResult { tier: String, claimed: f64, measured: Rate, verdict: Verdict, proposed: Option<f64> }`, `metrics::Verdict { Ok, Miscalibrated, UnderPowered }`, `metrics::calibrate(&Comparison) -> Vec<TierResult>`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn brier_matches_the_hand_computed_score() {
        // Three edges: 0.9 correct, 0.6 wrong, 1.0 correct.
        //   (0.9-1)² + (0.6-0)² + (1.0-1)² = 0.01 + 0.36 + 0 = 0.37; /3 = 0.123333
        let c = Comparison {
            judged: vec![
                with_conf("heuristic", 0.9, true),
                with_conf("heuristic", 0.6, false),
                with_conf("exact", 1.0, true),
            ],
            sites_total: 3,
            ..Default::default()
        };
        close(brier(&c), 0.123333, "brier");
    }

    #[test]
    fn a_tier_whose_claim_falls_outside_its_interval_is_miscalibrated() {
        // 200 bare-member edges claiming 0.60, of which 80 are right. Measured 0.40, and
        // 0.60 is nowhere near the interval.
        let mut judged = Vec::new();
        for i in 0..200 {
            judged.push(with_conf("heuristic", 0.6, i < 80));
        }
        let c = Comparison { judged, sites_total: 200, ..Default::default() };
        let tiers = calibrate(&c);
        let t = tiers.iter().find(|t| t.tier == "heuristic").expect("tier present");
        assert_eq!(t.verdict, Verdict::Miscalibrated);
        // Jeffreys posterior mean: (80 + 0.5) / (200 + 1) = 0.400498
        close(t.proposed.expect("a proposal"), 0.400498, "jeffreys estimate");
    }

    #[test]
    fn a_tier_with_too_little_evidence_proposes_nothing() {
        // Nine edges cannot justify a config change. Under-powered measurement laundering
        // itself into a constant is R8 wearing a lab coat.
        let judged: Vec<_> = (0..9).map(|i| with_conf("heuristic", 0.6, i < 3)).collect();
        let c = Comparison { judged, sites_total: 9, ..Default::default() };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::UnderPowered);
        assert!(t.proposed.is_none(), "no proposal below the power floor");
    }

    #[test]
    fn a_well_calibrated_tier_is_left_alone() {
        // 200 edges claiming 1.00, 199 right. The claim sits inside the interval.
        let judged: Vec<_> = (0..200).map(|i| with_conf("exact", 1.0, i < 199)).collect();
        let c = Comparison { judged, sites_total: 200, ..Default::default() };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::Ok);
        assert!(t.proposed.is_none());
    }
```

with the helper:

```rust
    fn with_conf(tier: &str, confidence: f64, correct: bool) -> crate::matcher::Judged {
        crate::matcher::Judged {
            site: (format!("{tier}.rs"), 1),
            tier: tier.into(),
            confidence,
            correct,
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-eval metrics`
Expected: FAIL to compile — `cannot find function brier`.

- [ ] **Step 3: Write the calibration**

```rust
/// The Brier score: mean squared error of a probability claim.
///
/// It is a **strictly proper scoring rule** — minimised only by reporting one's true belief.
/// That is what makes it safe to track: the score cannot be improved by inflating confidences
/// to look decisive, or deflating them to look cautious.
pub fn brier(c: &Comparison) -> f64 {
    if c.judged.is_empty() {
        return 0.0;
    }
    let sum: f64 = c
        .judged
        .iter()
        .map(|j| {
            let y = if j.correct { 1.0 } else { 0.0 };
            (j.confidence - y).powi(2)
        })
        .sum();
    sum / c.judged.len() as f64
}

/// Below this an interval is too wide to justify changing a constant.
const MAX_HALF_WIDTH: f64 = 0.15;
/// Below this a tier gets no verdict at all. §5.7: ±0.05 at p≈0.8 needs about 246 samples.
const POWER_FLOOR: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Verdict {
    Ok,
    Miscalibrated,
    UnderPowered,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TierResult {
    pub tier: String,
    pub claimed: f64,
    pub measured: Rate,
    pub verdict: Verdict,
    /// The Jeffreys posterior mean, offered only when the evidence can carry it.
    pub proposed: Option<f64>,
}

pub fn calibrate(c: &Comparison) -> Vec<TierResult> {
    // Bins are tiers, not equal-width buckets: confidence here is a set of discrete
    // constants, so calibration is a hypothesis test per tier rather than a smoothing
    // exercise. A tier carrying more than one claimed value is keyed by both.
    let mut groups: HashMap<(String, u64), (u64, u64)> = HashMap::new();
    for j in &c.judged {
        let key = (j.tier.clone(), (j.confidence * 1000.0).round() as u64);
        let e = groups.entry(key).or_insert((0, 0));
        e.1 += 1;
        if j.correct {
            e.0 += 1;
        }
    }

    let mut out: Vec<TierResult> = groups
        .into_iter()
        .map(|((tier, claimed_milli), (k, n))| {
            let claimed = claimed_milli as f64 / 1000.0;
            let measured = Rate::new(k, n);
            let half = (measured.high - measured.low) / 2.0;
            let (verdict, proposed) = if n < POWER_FLOOR || half > MAX_HALF_WIDTH {
                (Verdict::UnderPowered, None)
            } else if claimed < measured.low || claimed > measured.high {
                // Jeffreys posterior mean under Beta(1/2, 1/2). Not k/n, which proposes
                // 1.00 off a 12-for-12 run.
                (Verdict::Miscalibrated, Some((k as f64 + 0.5) / (n as f64 + 1.0)))
            } else {
                (Verdict::Ok, None)
            };
            TierResult { tier, claimed, measured, verdict, proposed }
        })
        .collect();
    out.sort_by(|a, b| b.measured.n.cmp(&a.measured.n));
    out
}

/// Expected calibration error, weighted by each tier's share of the edges.
pub fn ece(tiers: &[TierResult]) -> f64 {
    let total: u64 = tiers.iter().map(|t| t.measured.n).sum();
    if total == 0 {
        return 0.0;
    }
    tiers
        .iter()
        .map(|t| (t.measured.n as f64 / total as f64) * (t.measured.value - t.claimed).abs())
        .sum()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-eval metrics`
Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-eval/src/metrics.rs
git commit -m "feat(eval): calibration, with a corrected constant that refuses to overfit"
```

---

### Task 7: The coverage cross-check and the report

**Files:**
- Create: `crates/nexus-eval/src/report.rs`
- Modify: `crates/nexus-eval/src/lib.rs` (add `run`)
- Test: inline `mod tests` in `report.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `report::Run { oracle: String, file_coverage: Rate, partial: bool, scores: Scores, brier: f64, ece: f64, tiers: Vec<TierResult>, excluded_non_project: usize, excluded_oracle_blind: usize }`, `report::build(...) -> Run`, `report::render(&Run) -> String`, and `nexus_eval::run(args)`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn an_oracle_that_missed_files_marks_the_run_partial() {
        // scip-typescript silently skips files over 1MB; scip-python emits a partial index on
        // timeout rather than failing. A partial oracle *inflates* precision, because Nexus
        // edges in unindexed files fall out of the comparable set instead of being judged.
        // The harness's failure mode would otherwise be a flattering result.
        let nexus_files = ["a.rs", "b.rs", "c.rs", "d.rs"].map(String::from);
        let oracle_files: std::collections::HashSet<String> =
            ["a.rs", "b.rs"].iter().map(|s| s.to_string()).collect();
        let run = build_for_test(&nexus_files, &oracle_files);
        assert!(run.partial, "2 of 4 files indexed must not be reported as a clean run");
        assert_eq!(run.file_coverage.n, 4);
    }

    #[test]
    fn a_complete_oracle_is_not_partial() {
        let nexus_files = ["a.rs", "b.rs"].map(String::from);
        let oracle_files: std::collections::HashSet<String> =
            ["a.rs", "b.rs"].iter().map(|s| s.to_string()).collect();
        assert!(!build_for_test(&nexus_files, &oracle_files).partial);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-eval report`
Expected: FAIL to compile.

- [ ] **Step 3: Write the report**

```rust
/// Below this share of files indexed, the metrics are advisory rather than a measurement.
const MIN_FILE_COVERAGE: f64 = 0.95;

#[derive(Debug, serde::Serialize)]
pub struct Run {
    pub oracle: String,
    pub file_coverage: crate::metrics::Rate,
    /// True when the oracle did not index everything Nexus did. Metrics on a partial oracle
    /// read high, so this must travel with them rather than being inferable from a ratio
    /// nobody reads.
    pub partial: bool,
    pub scores: crate::metrics::Scores,
    pub brier: f64,
    pub ece: f64,
    pub tiers: Vec<crate::metrics::TierResult>,
    pub excluded_non_project: usize,
    pub excluded_oracle_blind: usize,
}

pub fn build(
    oracle_name: &str,
    nexus_files: &[String],
    oracle_files: &std::collections::HashSet<String>,
    comparison: &crate::matcher::Comparison,
) -> Run {
    let seen = nexus_files.iter().filter(|f| oracle_files.contains(*f)).count() as u64;
    let file_coverage = crate::metrics::Rate::new(seen, nexus_files.len() as u64);
    let tiers = crate::metrics::calibrate(comparison);
    Run {
        oracle: oracle_name.to_string(),
        partial: file_coverage.value < MIN_FILE_COVERAGE,
        file_coverage,
        scores: crate::metrics::score(comparison),
        brier: crate::metrics::brier(comparison),
        ece: crate::metrics::ece(&tiers),
        tiers,
        excluded_non_project: comparison.excluded_non_project,
        excluded_oracle_blind: comparison.excluded_oracle_blind,
    }
}
```

Render it in the column layout the spec fixes in §7.2, and add `run` to `lib.rs`:

```rust
pub fn run(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let m = clap::Command::new("nexus-eval")
        .about("Measure whether a resolved edge points at the right symbol")
        .arg(clap::arg!(--edges <PATH> "NDJSON from `nexus graph --edges`").required(true))
        .arg(clap::arg!(--scip <PATH> "index.scip from a SCIP indexer").required(true))
        .arg(clap::arg!(--json "emit the run as JSON"))
        .get_matches_from(args);

    let edges = edges::load(std::path::Path::new(m.get_one::<String>("edges").ok_or("--edges")?))?;
    let oracle = oracle::Oracle::load(std::path::Path::new(m.get_one::<String>("scip").ok_or("--scip")?))?;
    let files: Vec<String> = {
        let mut v: Vec<String> = edges.iter().map(|e| e.src_file.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    let comparison = matcher::compare(&edges, &oracle);
    let run = report::build("scip", &files, &oracle.files, &comparison);

    if m.get_flag("json") {
        println!("{}", serde_json::to_string_pretty(&run)?);
    } else {
        print!("{}", report::render(&run));
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests, then `make check`**

Run: `cargo test -p nexus-eval && make check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-eval/src
git commit -m "feat(eval): the report, and the coverage check that stops a partial oracle flattering us"
```

---

### Task 8: `make eval`, and the first real measurement

**Files:**
- Modify: `Makefile`
- Create: `scripts/eval.sh`
- Create: `docs/eval/README.md`

**Interfaces:**
- Consumes: the binary from Task 7.
- Produces: `make eval`, and a committed baseline at `docs/eval/baseline.json`.

- [ ] **Step 1: Write the runner**

`scripts/eval.sh`:

```bash
#!/usr/bin/env bash
# Measure resolution accuracy against a SCIP oracle.
#
# Deliberately NOT part of `make check`: it needs four external toolchains and, for Java, a
# full project compile. Wiring that into the commit path gets it disabled inside a fortnight,
# which docs/architecture/13-evaluation.md §2 says in as many words about Tier 2.
set -euo pipefail

PROJECT="${1:-.}"
OUT="${OUT:-target/eval}"
mkdir -p "$OUT"

case "${LANG_KIND:-rust}" in
  rust)
    command -v rust-analyzer >/dev/null || { echo "rust-analyzer not on PATH" >&2; exit 1; }
    rust-analyzer scip "$PROJECT" --output "$OUT/index.scip"
    ;;
  java)
    # scip-java fails closed: a non-zero build exit propagates and `aggregate` never runs, so
    # index.scip is never written. The per-file .scip files survive in the targetroot, so a
    # partial index is recoverable and is marked partial rather than discarded.
    if ! scip-java index --output "$OUT/index.scip"; then
      echo "build failed; recovering a partial index from the targetroot" >&2
      scip-java aggregate "$PROJECT/target/scip-targetroot" --output "$OUT/index.scip"
    fi
    ;;
  *) echo "LANG_KIND must be rust or java" >&2; exit 2 ;;
esac

nexus --project "$PROJECT" scan >/dev/null
nexus --project "$PROJECT" graph --edges "$OUT/edges.ndjson" >/dev/null
nexus-eval --edges "$OUT/edges.ndjson" --scip "$OUT/index.scip" "${@:2}"
```

`chmod +x scripts/eval.sh`, and in the `Makefile`:

```make
eval: ## measure resolution accuracy against a SCIP oracle (needs external indexers)
	@cargo build --release --bin nexus --bin nexus-eval
	@PATH="$(PWD)/target/release:$$PATH" ./scripts/eval.sh $(if $(REPO),$(REPO),.)
.PHONY: eval
```

- [ ] **Step 2: Run it on this repository**

Run: `make eval`
Expected: a report. **Record the actual numbers; do not predict them.** If `rust-analyzer` is absent, `scripts/eval.sh` exits 1 with the reason — install it with `rustup component add rust-analyzer` rather than making the script tolerate its absence.

- [ ] **Step 3: Commit the baseline and what it showed**

```bash
make eval > /dev/null; nexus-eval --edges target/eval/edges.ndjson \
  --scip target/eval/index.scip --json > docs/eval/baseline.json
git add Makefile scripts/eval.sh docs/eval/
git commit -m "feat(eval): make eval, and the first measurement of whether the graph is right"
```

- [ ] **Step 4: Write down what it found**

Create `docs/eval/README.md` recording: the oracle and its pinned version, the commit measured, the four scores with intervals, the Brier score and ECE, and every tier verdict. **Then update `docs/architecture.md` and ADR-003's tier tables**, replacing "None of these confidences has been measured" with the measured values and their intervals — and for any tier the run marks `UnderPowered`, say so rather than quoting a number the evidence cannot carry.

If a tier comes back `Miscalibrated`, that is a finding, not a failure: record it, and leave changing the constant to a separate reviewed commit. A measurement and a behaviour change in one diff cannot be reviewed independently.

---

## Self-review

**Spec coverage.** §3.1 → Task 2. §3.2 → Task 1. §4.1 → Task 8. §4.2 → Task 3. §4.3, §4.4, §4.5 → Task 4. §5.2, §5.3 → Task 5. §5.4, §5.5, §5.7 → Task 6. §8.1 → Task 7. §8.2, §8.3 → Task 8. §8.4 (oracle caching) is **deliberately deferred**: the corpus is two repositories and an index takes seconds, so a cache would be complexity ahead of need. §5.6 (the ambiguous tier's χ² uniformity test) is deferred with it — `calibrate` already keys by `(tier, claimed)`, so each `0.9/n` bucket is measured separately, which answers the useful half without a χ² implementation.

**Placeholders.** None. Every step carries the code or command to run. Task 8 Step 2 deliberately does not predict the numbers.

**Type consistency.** `Rate` is defined in Task 5 and used in Tasks 6 and 7. `Judged` and `Comparison` are defined in Task 4 and consumed in 5, 6, 7. `Edge` is defined in Task 4 Step 3 and mirrors `EdgeRecord` from Task 1 Step 4 field for field — they must stay in step, and the doc comment on `Edge` says why they are duplicated rather than shared.

**Known risk.** `COMPARABLE_EDGE_TYPES` in Task 4 is a judgment call, and it is the one place in this design where a choice could move the headline number. It is a named constant with a doc comment for exactly that reason: changing it changes what is being measured, and that should be visible in a diff rather than buried in a filter expression.
