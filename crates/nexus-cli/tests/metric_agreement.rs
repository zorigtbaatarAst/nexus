//! `scan` and `graph` report the same resolution figure for the same database.
//!
//! They did not, and nothing noticed. `ScanReport` was built from `ResolveStats`, which
//! increments `ambiguous` once per call site; `GraphReport` was built from `edge_counts`,
//! which counted rows — and the overload, GraphQL-coordinate and bare-member tiers each
//! write one row per candidate. Measured on one clone of this repository at `46e2fff`,
//! `scan` said 45 % of 3,606 and `graph` said 48 % of 3,751.
//!
//! The direction of the error is what makes it worth a test: the row count *rises* as the
//! resolver grows less certain, so the headline metric improved when resolution got worse.

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

fn git(root: &Path, args: &[&str]) {
    let ok = Command::new("git")
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

/// Three types answering to one method name, and a caller that reaches it through a field —
/// the bare-member tier's fan-out arm, the shape that used to count three times. Rust
/// because the analyzer emits `#save` for it, which is exactly the ownerless hint the
/// fan-out arm exists to handle.
///
/// One directory per test: these run in parallel, and a shared path means one test deletes
/// the other's repository mid-scan.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-metric-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    std::fs::write(
        root.join("src/lib.rs"),
        r#"
pub struct Alpha;
impl Alpha { pub fn save(&self) {} }

pub struct Beta;
impl Beta { pub fn save(&self) {} }

pub struct Gamma;
impl Gamma { pub fn save(&self) {} }

pub struct Caller { alpha: Alpha }

impl Caller {
    pub fn run(&self) { self.alpha.save(); }
}
"#,
    )
    .expect("write");

    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    root
}

fn run_json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run nexus");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "`nexus {}` did not emit JSON: {e}\n{stdout}",
            args.join(" ")
        )
    });
    // Every command wraps its report in an envelope carrying the command name and schema
    // version; the report itself is under `result`.
    doc["result"].clone()
}

#[test]
fn scan_and_graph_report_the_same_resolution_figure() {
    let root = project("agreement");

    let scan = run_json(&root, &["scan", "--json"]);
    let graph = run_json(&root, &["graph", "--json"]);

    let field = |v: &serde_json::Value, k: &str| -> i64 {
        v[k].as_i64()
            .unwrap_or_else(|| panic!("{k} missing or not an integer in {v}"))
    };

    // Without this the whole test passes on 0 == 0. An earlier draft of it did exactly
    // that, on a Java fixture whose call shape the analyzer emitted no edge for, and
    // proved nothing while looking green.
    assert!(
        field(&graph, "edges_total") > 0,
        "the fixture must actually produce call sites, or this test asserts 0 == 0: {graph}"
    );

    assert_eq!(
        field(&graph, "edges_total"),
        field(&scan, "edges_total"),
        "scan and graph must count the same call sites"
    );
    assert_eq!(
        field(&graph, "edges_external"),
        field(&scan, "edges_external"),
        "and the same external sites"
    );
    assert_eq!(
        field(&graph, "edges_resolved"),
        field(&scan, "edges_resolved"),
        "and the same resolved sites — one denominator, not two"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_breakdown_sums_to_the_total() {
    // `edges_by_resolution` and `edge_counts` must use one unit. When they did not, the
    // breakdown counted rows and the summary counted sites, so the columns of the same
    // report disagreed with its own header.
    let root = project("breakdown");
    // A baseline must exist before `graph` has anything to report.
    run_json(&root, &["scan", "--json"]);

    let graph = run_json(&root, &["graph", "--json"]);
    let total = graph["edges_total"].as_i64().expect("edges_total");
    assert!(total > 0, "the fixture must produce call sites: {graph}");
    let summed: i64 = graph["by_resolution"]
        .as_array()
        .expect("by_resolution is an array")
        .iter()
        .map(|pair| pair[1].as_i64().expect("count"))
        .sum();

    assert_eq!(
        summed, total,
        "the tier breakdown must sum to the total it breaks down"
    );

    let _ = std::fs::remove_dir_all(&root);
}
