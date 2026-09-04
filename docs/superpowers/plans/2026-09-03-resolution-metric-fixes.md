# Resolution Metric Fixes — Implementation Plan (Plan A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the resolution tier label truthful and the resolution metric uninflatable, so `scan` and `graph` finally report the same number.

**Architecture:** Two independent defects in `nexus-store`, both prerequisites for the accuracy harness. First, `Resolution` gains the two variants it is missing so `parse` stops relabelling `sibling` and `external-graph` as `heuristic`. Second, `edge_counts` and `edges_by_resolution` count distinct *call sites* rather than edge rows, so the ambiguous tiers' fan-out can no longer inflate the score.

**Tech Stack:** Rust 1.82+, rusqlite/SQLite, cargo workspace.

**Spec:** [`docs/superpowers/specs/2026-09-03-resolution-accuracy-harness-design.md`](../specs/2026-09-03-resolution-accuracy-harness-design.md) — §6.1 and §6.2.

## Global Constraints

- Rust 1.82+ (`rust-version` in the workspace manifest); CI runs `RUSTFLAGS=-D warnings`, so a warning fails the build.
- **Only `nexus-store` contains SQL.** No exceptions.
- Ledger tables are append-only: `scans`, `changes`, `commits`, `finding_occurrences`, `finding_verifications`, `test_runs`, `audit_events` are never `UPDATE`d.
- `nexus-store` denies `clippy::unwrap_used` and `clippy::expect_used` outside tests.
- **Branch discipline on `task/retrieval`: never rebase, amend across, or force-push.** A parallel session is committing to this branch.
- Do not touch the facts region of `crates/nexus-store/src/lib.rs` (around L2399+), `crates/nexus-core/src/memory.rs`, or `crates/nexus-core/src/context/seeds.rs` — another session owns those.
- `make check` must pass before every commit.

---

### Task 1: `Resolution` stops lying about the tier

**Files:**
- Modify: `crates/nexus-types/src/lib.rs:219-253` (the enum, `as_str`, `parse`)
- Modify: `crates/nexus-store/src/lib.rs:1340` (the `unwrap_or` in `neighbours`)
- Test: `crates/nexus-types/src/lib.rs` (inline `mod tests`)

**Interfaces:**
- Produces: `Resolution::Sibling`, `Resolution::ExternalGraph`, with `as_str()` returning `"sibling"` and `"external-graph"`, and `Resolution::parse` accepting both. Task 2 does not depend on these; the accuracy harness (Plan B) does.

- [ ] **Step 1: Write the failing test**

Add to `crates/nexus-types/src/lib.rs`, in the inline `mod tests` (create the module at the end of the file if absent, with `use super::*;`):

```rust
#[test]
fn every_stored_resolution_value_round_trips() {
    // The CHECK constraint in migrations 0006_external_graph.sql permits exactly these.
    // A value the database can hold but the enum cannot name is reported as the wrong
    // tier — `sibling` and `external-graph` both read as `heuristic`, claiming a tier
    // that resolved something when nothing did.
    for s in [
        "exact",
        "framework",
        "contract",
        "heuristic",
        "external",
        "sibling",
        "external-graph",
        "unresolved",
    ] {
        let parsed = Resolution::parse(s)
            .unwrap_or_else(|| panic!("{s} is a stored value the enum cannot name"));
        assert_eq!(parsed.as_str(), s, "{s} did not round-trip");
    }
}

#[test]
fn an_unknown_resolution_is_none_rather_than_a_guess() {
    assert!(Resolution::parse("invented").is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-types every_stored_resolution_value_round_trips`
Expected: FAIL — `sibling is a stored value the enum cannot name`.

- [ ] **Step 3: Add the two variants**

In `crates/nexus-types/src/lib.rs`, extend the enum:

```rust
pub enum Resolution {
    Exact,
    Framework,
    Contract,
    Heuristic,
    /// The target is genuinely outside the indexed project — a third-party library, or a
    /// sibling module that was not scanned. Distinct from `Unresolved`, which means
    /// BugHunter looked and failed. Conflating them makes the resolution rate a lie.
    External,
    /// Code this project owns that was not scanned. Outside the index like `External`,
    /// but for a reason the caller can fix by widening the scan. ADR-017's revision.
    Sibling,
    /// Imported from an external knowledge graph, never resolved against a symbol table.
    /// Carries a confidence ceiling of 0.5 so it cannot outrank a parsed edge — a ceiling
    /// that was defeated for as long as this variant was missing and the value read back
    /// as `Heuristic`.
    ExternalGraph,
    Unresolved,
}
```

Extend `as_str`:

```rust
            Resolution::External => "external",
            Resolution::Sibling => "sibling",
            Resolution::ExternalGraph => "external-graph",
            Resolution::Unresolved => "unresolved",
```

Extend `parse`:

```rust
            "external" => Resolution::External,
            "sibling" => Resolution::Sibling,
            "external-graph" => Resolution::ExternalGraph,
            "unresolved" => Resolution::Unresolved,
```

Note the `#[serde(rename_all = "lowercase")]` on the enum renders `ExternalGraph` as `externalgraph` in JSON, which does **not** match `as_str()`. Add an explicit rename so the two agree:

```rust
    #[serde(rename = "external-graph")]
    ExternalGraph,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p nexus-types every_stored_resolution_value_round_trips an_unknown_resolution_is_none_rather_than_a_guess`
Expected: PASS, 2 tests.

- [ ] **Step 5: Stop `neighbours` defaulting an unknown tier**

In `crates/nexus-store/src/lib.rs:1340`, replace the silent default. An unknown value is a schema/code disagreement and must fail loudly rather than claim a tier:

```rust
                    edge_type: EdgeType::parse(&edge_type).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            unknown_value("edge_type", &edge_type),
                        )
                    })?,
                    resolution: Resolution::parse(&resolution).ok_or_else(|| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            unknown_value("resolution", &resolution),
                        )
                    })?,
```

And add this free function next to `simple_key` at the bottom of the same file:

```rust
/// A column value the schema permits and the code cannot name. Returned as an error rather
/// than defaulted, because the default is indistinguishable from a real value: `resolution`
/// defaulted to `heuristic` for years, so every `sibling` and `external-graph` edge claimed
/// a tier that had resolved something.
fn unknown_value(column: &str, value: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("unknown {column} {value:?} — schema and code disagree"),
    ))
}
```

- [ ] **Step 6: `make check`**

Run: `make check`
Expected: PASS. If a test elsewhere asserted a `heuristic` tier on a sibling edge, it was asserting the bug — read the assertion, and correct it to `sibling`.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-types/src/lib.rs crates/nexus-store/src/lib.rs
git commit -m "fix(types): a sibling edge is not a heuristic one"
```

---

### Task 2: The metric counts call sites, not edge rows

**Files:**
- Modify: `crates/nexus-store/src/lib.rs:1265-1296` (`edge_counts`, `edges_by_resolution`)
- Test: `crates/nexus-store/src/lib.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `EdgeCounts` unchanged in shape — every field now counts distinct call sites. `Store::edge_counts` and `Store::edges_by_resolution` keep their signatures, so no consumer changes.

- [ ] **Step 1: Write the failing test**

Add to the inline `mod tests` in `crates/nexus-store/src/lib.rs`. Follow the fixture shape of `live_views_hide_soft_deleted_rows` in the same module for opening a store and inserting a project/scan/file/symbols; do not invent a new one.

```rust
#[test]
fn a_fanned_out_call_site_counts_once_not_four_times() {
    // The ambiguous tiers write one row per candidate for a single call site. Counting
    // rows means the metric rises as the resolver grows less certain, which is exactly
    // backwards. One site, one outcome.
    let (store, project_id, scan_id, file_id) = fixture_with_one_file();
    let tx = store.transaction().expect("tx");
    let src = insert_symbol(&tx, project_id, file_id, scan_id, "app::Caller#run");
    let a = insert_symbol(&tx, project_id, file_id, scan_id, "app::A#save");
    let b = insert_symbol(&tx, project_id, file_id, scan_id, "app::B#save");
    let c = insert_symbol(&tx, project_id, file_id, scan_id, "app::C#save");

    // One call site at line 42, three candidate destinations — the shape the bare-member
    // tier produces for `x.save()` when three symbols answer to `save`.
    for dst in [a, b, c] {
        tx.execute(
            "INSERT INTO symbol_edges (project_id, src_symbol_id, dst_symbol_id, dst_fqn_hint,
                                       edge_type, resolution, confidence, site_line, last_seen_scan_id)
             VALUES (?1, ?2, ?3, 'save', 'calls', 'heuristic', 0.2, 42, ?4)",
            params![project_id, src, dst, scan_id],
        )
        .expect("insert edge");
    }
    tx.commit().expect("commit");

    let counts = store.edge_counts(project_id).expect("counts");
    assert_eq!(counts.total, 1, "three rows for one call site are one call site");
    assert_eq!(counts.resolved, 1, "and one resolved call site, not three");

    let by = store.edges_by_resolution(project_id).expect("by resolution");
    let heuristic = by
        .iter()
        .find(|(r, _)| r == "heuristic")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    assert_eq!(heuristic, 1, "the breakdown must use the same unit as the total");
}
```

If `fixture_with_one_file` and `insert_symbol` do not already exist in that `mod tests`, write them by copying the setup already used by `live_views_hide_soft_deleted_rows`, returning `(Store, ProjectId, ScanId, FileId)` and a symbol id respectively.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-store a_fanned_out_call_site_counts_once_not_four_times`
Expected: FAIL — `assertion left == right failed: three rows for one call site are one call site; left: 3, right: 1`.

- [ ] **Step 3: Count sites in `edge_counts`**

Replace the body of `edge_counts` (`crates/nexus-store/src/lib.rs:1265`). SQLite has no multi-column `COUNT(DISTINCT …)`, so the grouping happens in a subquery and the outer query aggregates over groups:

```rust
    /// Counts **call sites**, not edge rows.
    ///
    /// The ambiguous tiers write one row per candidate for a single call site, so counting
    /// rows lets the metric rise as the resolver grows less certain. Grouping by
    /// `(src_symbol_id, site_line, dst_fqn_hint)` collapses a fan-out back to the one
    /// question it answers. `external-graph` rows carry NULL in both `site_line` and
    /// `dst_fqn_hint`, so they group per source symbol; they are excluded from `resolved`
    /// regardless, and are reported on their own line.
    pub fn edge_counts(&self, project_id: ProjectId) -> Result<EdgeCounts> {
        let mut stmt = self.conn.prepare(
            "SELECT
               COUNT(*),
               SUM(resolved),
               SUM(is_external),
               SUM(is_sibling),
               SUM(is_external_graph)
             FROM (
               SELECT
                 MAX(dst_symbol_id IS NOT NULL AND resolution <> 'external-graph') AS resolved,
                 MAX(resolution = 'external')       AS is_external,
                 MAX(resolution = 'sibling')        AS is_sibling,
                 MAX(resolution = 'external-graph') AS is_external_graph
               FROM symbol_edges
               WHERE project_id = ?1
               GROUP BY src_symbol_id, site_line, dst_fqn_hint
             )",
        )?;
        let row = stmt.query_row(params![project_id], |r| {
            Ok(EdgeCounts {
                total: r.get::<_, i64>(0)?,
                resolved: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                external: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                sibling: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                external_graph: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })?;
        Ok(row)
    }
```

`MAX(...)` over a group is the group's OR: a site is resolved if any of its candidate rows bound a destination.

- [ ] **Step 4: Count sites in `edges_by_resolution`**

The breakdown must use the same unit, or `monorepo_module.rs:96-113` — which asserts the summary and the breakdown agree — fails:

```rust
    /// The same call-site unit as `edge_counts`. A site whose candidates disagree on tier
    /// is attributed to the strongest one present, so the breakdown sums to the total.
    pub fn edges_by_resolution(&self, project_id: ProjectId) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT resolution, COUNT(*) FROM (
               SELECT MIN(
                 CASE resolution
                   WHEN 'exact'          THEN 1
                   WHEN 'contract'       THEN 2
                   WHEN 'framework'      THEN 3
                   WHEN 'heuristic'      THEN 4
                   WHEN 'sibling'        THEN 5
                   WHEN 'external'       THEN 6
                   WHEN 'external-graph' THEN 7
                   ELSE 8
                 END
               ) AS rank,
               CASE MIN(
                 CASE resolution
                   WHEN 'exact'          THEN 1
                   WHEN 'contract'       THEN 2
                   WHEN 'framework'      THEN 3
                   WHEN 'heuristic'      THEN 4
                   WHEN 'sibling'        THEN 5
                   WHEN 'external'       THEN 6
                   WHEN 'external-graph' THEN 7
                   ELSE 8
                 END
               )
                 WHEN 1 THEN 'exact'
                 WHEN 2 THEN 'contract'
                 WHEN 3 THEN 'framework'
                 WHEN 4 THEN 'heuristic'
                 WHEN 5 THEN 'sibling'
                 WHEN 6 THEN 'external'
                 WHEN 7 THEN 'external-graph'
                 ELSE 'unresolved'
               END AS resolution
               FROM symbol_edges
               WHERE project_id = ?1
               GROUP BY src_symbol_id, site_line, dst_fqn_hint
             )
             GROUP BY resolution ORDER BY 2 DESC",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

Note the selected columns are `rank` then `resolution`, so the outer `SELECT resolution, COUNT(*)` reads index 0 and 1 — adjust the `r.get` indices to `0` and `1` if you drop the `rank` column from the inner select. Prefer dropping it: it exists only to make the `MIN` readable, and the `CASE MIN(...)` already recomputes it.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p nexus-store a_fanned_out_call_site_counts_once_not_four_times`
Expected: PASS.

- [ ] **Step 6: `make check`**

Run: `make check`
Expected: PASS. `crates/nexus-core/tests/monorepo_module.rs` and `context_pipeline.rs` assert on these counts; where a count changes because fan-out no longer multiplies it, the new number is correct and the assertion should move. Where a count changes for any other reason, stop and find out why.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-store/src/lib.rs
git commit -m "fix(store): the resolution metric counts questions, not answers"
```

---

### Task 3: `scan` and `graph` agree, and say what they mean

**Files:**
- Modify: `crates/nexus-cli/src/render.rs:236-267` (scan block), `:625-673` (graph block)
- Modify: `crates/nexus-mcp/src/lib.rs:488` (tool description string)
- Test: `crates/nexus-cli/tests/` — new integration test

**Interfaces:**
- Consumes: `EdgeCounts` from Task 2, now in call-site units.
- Produces: no new types. The rendered strings change.

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-cli/tests/metric_agreement.rs`. Copy the harness shape from an existing integration test in that directory (`json_contract.rs` shows how to build a temp project and invoke the binary):

```rust
/// The bug this pins: `ResolveStats` counted call sites and `edge_counts` counted rows, so
/// the two commands reported different resolution rates for one database — measured at
/// 45% from `scan` and 48% from `graph` on the same clone.
#[test]
fn scan_and_graph_report_the_same_resolution_figure() {
    let project = scanned_fixture();

    let scan: serde_json::Value = run_json(&project, &["scan", "--json"]);
    let graph: serde_json::Value = run_json(&project, &["graph", "--json"]);

    let scan_total = scan["edges_total"].as_i64().expect("scan edges_total");
    let scan_external = scan["edges_external"].as_i64().expect("scan edges_external");
    let scan_resolved = scan["edges_resolved"].as_i64().expect("scan edges_resolved");

    assert_eq!(
        graph["edges_total"].as_i64().expect("graph edges_total"),
        scan_total,
        "scan and graph must count the same edges"
    );
    assert_eq!(
        graph["edges_external"].as_i64().expect("graph edges_external"),
        scan_external,
        "and the same external edges"
    );
    assert_eq!(
        graph["edges_resolved"].as_i64().expect("graph edges_resolved"),
        scan_resolved,
        "and the same resolved edges"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nexus-cli --test metric_agreement`
Expected: FAIL on the first `assert_eq!` — the two totals differ by the number of fan-out rows.

- [ ] **Step 3: Make the scan report read from the same source**

`crates/nexus-core/src/engine/scan.rs:204-207` populates `ScanReport` from `ResolveStats`. Change it to populate from `self.store.edge_counts(self.project_id)?` after the transaction commits, so both commands read one implementation:

```rust
        // One implementation of the denominator, not two. `ResolveStats` counts what this
        // scan resolved; `edge_counts` counts what the index holds. They agreed only by
        // accident, and stopped agreeing when the ambiguous tiers began fanning out.
        let counts = self.store.edge_counts(self.project_id)?;
```

then set `edges_total: counts.total as usize`, `edges_resolved: counts.resolved as usize`, `edges_external: counts.external as usize`, `edges_sibling: counts.sibling as usize`.

- [ ] **Step 4: Say "coverage", and show the fan-out**

In `crates/nexus-cli/src/render.rs`, both the scan block (`:236-267`) and the graph block (`:625-673`) currently print `"({pct:.0}% of {in_scope} in-project resolved, {} external)"`. The number is coverage — the share of call sites that found *a* destination — and nothing checks whether it is the right one. Print that:

```rust
    // Coverage, not accuracy: nothing here checks that a bound destination is the correct
    // one. See docs/superpowers/specs/2026-09-03-resolution-accuracy-harness-design.md.
    writeln!(
        out,
        "  {} sites · {} resolved ({pct:.0}% coverage) · {} external",
        r.edges_total, r.edges_resolved, r.edges_external
    )?;
```

- [ ] **Step 5: Correct the MCP tool description**

`crates/nexus-mcp/src/lib.rs:488` says "Dependency graph size and how much of it resolved, broken down by tier." Replace with:

```rust
    "Dependency graph size and how many call sites resolved, broken down by tier. This is \
     coverage, not accuracy: nothing verifies that a bound destination is the right one."
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p nexus-cli --test metric_agreement`
Expected: PASS.

- [ ] **Step 7: `make check`**

Run: `make check`
Expected: PASS. `scripts/check_smoke.py:31-37` recomputes the percentage itself; it reads `edges_total` and `edges_external` from the scan JSON, whose names and meaning are unchanged, so it needs no edit — but run `make smoke` to confirm.

- [ ] **Step 8: Commit**

```bash
git add crates/nexus-core/src/engine/scan.rs crates/nexus-cli/src/render.rs \
        crates/nexus-mcp/src/lib.rs crates/nexus-cli/tests/metric_agreement.rs
git commit -m "fix(cli): one denominator, and it is called coverage"
```

---

### Task 4: The documentation stops disagreeing with the code

**Files:**
- Modify: `docs/architecture.md:207`, `:251-257`
- Modify: `docs/architecture-decisions.md` (ADR-003 table at `:117-123`, ADR-017 revision at `:801`)
- Modify: `docs/data-model.md:236-244`
- Modify: `commands/nexus-status.md:7`, `commands/nexus-scan.md:9`
- Modify: `integrations/README.md:47`
- Create: `docs/architecture/decisions/ADR-026-coverage-is-not-accuracy.md`

**Interfaces:**
- Consumes: the behaviour shipped in Tasks 1–3.
- Produces: documentation only.

- [ ] **Step 1: Correct the tier tables to match the code**

`docs/architecture.md:251-257` and ADR-003's table at `docs/architecture-decisions.md:117-123` both claim `heuristic` spans 0.70–0.95 and list no `contract` tier. The code has nine constants. Replace both tables with:

| Tier | Mechanism | `symbol_edges.resolution` | Confidence |
|---|---|---|---|
| exact FQN | import table + FQN match | `exact` | 1.00 |
| GraphQL join | both sides name one coordinate | `contract` | 0.95 |
| unique prefix | `Owner#member`, one candidate | `heuristic` | 0.90 |
| overload fan-out | 2–4 candidates | `heuristic` | 0.9 / n |
| inherited member | via a declared supertype | `heuristic` | 0.85 |
| unique simple name | last segment, unique in project | `heuristic` | 0.70 |
| bare member name | `x.foo()`, owner unknown | `heuristic` | 0.60 |
| outside the index | third-party | `external` | n/a |
| owned, unscanned | sibling module | `sibling` | n/a |
| imported claim | external knowledge graph | `external-graph` | ≤ 0.50 |
| nothing matched | hint retained | `unresolved` | 0.00 |

**None of these confidences has been measured.** Add that sentence under both tables, linking the harness spec.

- [ ] **Step 2: Fix the stale pipeline line**

`docs/architecture.md:207` reads `| resolve | edge resolution: exact → framework → heuristic → unresolved |`, which omits `contract`, `external`, `sibling` and `external-graph`. Replace with:

```
| `resolve` | edge resolution: exact → contract → heuristic → sibling → external → unresolved |
```

- [ ] **Step 3: Add the ADR-017 revision**

Append to ADR-017 in `docs/architecture-decisions.md`, after the existing `### Revision — 2026-09-01` section:

```markdown
### Revision — 2026-09-03: the unit was edges, and edges are not questions

ADR-017's argument — that the denominator must exclude what was never in scope — is
unchanged and correct. What was wrong is the *unit*. The ambiguous tiers write one row per
candidate for a single call site, so a resolver that grew less certain scored higher. Both
the numerator and the denominator now count distinct call sites, keyed
`(src_symbol_id, site_line, dst_fqn_hint)`.

This also removed a disagreement nobody had noticed: `scan` read `ResolveStats`, which
already counted sites, and `graph` read `edge_counts`, which counted rows. Measured on one
clone at `46e2fff`, they reported 45 % and 48 % for the same database.
```

- [ ] **Step 4: Correct the agent-facing threshold**

`commands/nexus-status.md:7` says "Report the share of in-project edges that resolved. Below ~80% means impact results are…". The unit changed and the threshold was never measured. Replace with:

```markdown
3. Report the share of in-project **call sites** that resolved. This is coverage, not
   accuracy — nothing verifies a bound destination is the right one, so treat it as "how
   much of the graph exists", never as "how much of it is correct".
```

Apply the same correction of "edges" to "call sites" at `commands/nexus-scan.md:9` and `integrations/README.md:47`.

- [ ] **Step 5: Write ADR-026**

Create `docs/architecture/decisions/ADR-026-coverage-is-not-accuracy.md`, following the format of the existing files in that directory:

```markdown
# ADR-026 — Coverage is not accuracy, and the product measures only one of them

## Status
Accepted — 2026-09-03

## Context
`nexus graph` reports the share of in-project call sites that bound a destination. It has
been read, including by this project's own documents, as a measure of whether the graph is
*right*. It is not. Nothing in the product compares a bound destination against a ground
truth, and `docs/architecture/12-non-goals.md` set an architectural trigger on "impact
recall" while satisfying it with this coverage figure.

## Decision
The in-product metric is named **coverage** everywhere it appears, and every surface that
reports it states that it is not accuracy. Accuracy is measured out-of-band against a
compiler-grade oracle, by `nexus-eval`, and never on a user's machine.

## Consequences
The published figure does not change meaning silently. A future ranking or trust decision
that needs accuracy must cite an eval run, not a scan. The cost is that the product cannot
answer "is my graph correct?" on its own — which is honest, because it never could.
```

- [ ] **Step 6: `make check`**

Run: `make check`
Expected: PASS — documentation only, but `make check` also catches a broken intra-doc link in a Rust doc-comment.

- [ ] **Step 7: Commit**

```bash
git add docs/ commands/ integrations/README.md
git commit -m "docs: the tier table matches the code, and coverage says it is coverage"
```

---

## Self-review

**Spec coverage.** §6.1 → Task 1. §6.2 → Task 2. §1.3 (scan/graph disagreement) → Task 3. §9 migration list → Task 4, plus the README and roadmap sites already corrected in commit `544c2e5`. §3.2 (`--edges` NDJSON) and §4–§8 (the harness) are **Plan B** and deliberately absent.

**Placeholders.** None: every step carries the literal code or text to write.

**Type consistency.** `EdgeCounts` keeps all five fields and their types across Tasks 2 and 3. `Resolution::Sibling` and `Resolution::ExternalGraph` are introduced in Task 1 and used by name nowhere else in this plan — Plan B consumes them.

**Known risk.** Task 2 Step 4's `edges_by_resolution` picks the strongest tier present at a site. A site whose candidates all share one tier — the normal case, since fan-out copies the tier — is unaffected. A site mixing tiers can only arise from the GraphQL coordinate arm, which writes `contract` or `heuristic`; attributing it to `contract` is right, because that arm resolved it.
