//! `graph --edges` writes the uncollapsed edge list for out-of-band accuracy measurement.
//!
//! The summary counts call sites — that is what `metric_agreement.rs` pins. This file counts
//! rows, because precision is an edge-level metric: a fan-out of three candidates at one site
//! is three separate chances to be wrong, and a dump that collapses them cannot see two of
//! them.

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
/// the bare-member tier's fan-out arm. Copied from `metric_agreement.rs`: it is the fixture
/// proven to produce three rows at one site.
///
/// One directory per test: these run in parallel, and a shared path means one test deletes
/// the other's repository mid-scan.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-dump-{name}-{}", std::process::id()));
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

fn run(root: &Path, args: &[&str]) -> String {
    let out = Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run nexus");
    assert!(
        out.status.success(),
        "`nexus {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn the_edge_dump_has_one_line_per_edge_row_not_per_site() {
    let root = project("dump");
    run(&root, &["scan"]);
    let out = root.join("edges.ndjson");
    run(&root, &["graph", "--edges", out.to_str().expect("path")]);

    let body = std::fs::read_to_string(&out).expect("dump written");
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        3,
        "three candidate rows for one call site:\n{body}"
    );

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is one object");
        assert_eq!(v["resolution"], "heuristic");
        assert!(v["site_line"].as_i64().is_some(), "a site needs a line");
        assert!(
            v["dst_file"].as_str().is_some(),
            "a bound edge names its destination file"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
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

    let _ = std::fs::remove_dir_all(&root);
}
