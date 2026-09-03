//! `--json` is exactly one document per command.
//!
//! "stdout is results, stderr is everything else" is a contract, and `--json | jq` has to work.
//! Two concatenated objects on stdout parse as neither: `jq` fails, `serde_json::from_str`
//! fails, and an agent reading the output fails. This is a regression test for a real
//! failure — `scan` emitted its report and then Architect's findings as a second document, and
//! the project's own CI smoke check died on `Extra data: line 28 column 1`.
//!
//! It went unnoticed because the second document only appeared when Architect actually found
//! something, so every fixture small enough to find nothing passed.

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

/// A Gradle project with no CI, so Architect certainly finds something. A fixture that finds
/// nothing cannot catch this bug, which is exactly why it survived.
fn project(name: &str) -> PathBuf {
    // One directory per test: these run in parallel, and a shared path means one deletes the
    // other's repository mid-scan.
    let root = std::env::temp_dir().join(format!("nexus-json-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src/main/java/demo");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(root.join("build.gradle"), "plugins { id \"java\" }\n").expect("write");
    std::fs::write(
        src.join("Service.java"),
        "package demo;\npublic class Service {\n    public void run(String k) { System.out.println(k); }\n}\n",
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Parse the whole of `stdout` and return how many JSON values it held.
fn documents(stdout: &str) -> usize {
    let de = serde_json::Deserializer::from_str(stdout).into_iter::<serde_json::Value>();
    let mut n = 0;
    for next in de {
        match next {
            Ok(_) => n += 1,
            Err(e) => panic!("stdout is not valid JSON after {n} document(s): {e}\n{stdout}"),
        }
    }
    n
}

#[test]
fn every_json_command_emits_exactly_one_document() {
    let root = project("all");
    for args in [
        vec!["scan", "--json"],
        vec!["rescan", "--json"],
        vec!["status", "--json"],
        vec!["graph", "--json"],
        vec!["findings", "--json"],
        vec!["analyze", "architect", "--json"],
        vec!["context", "--session", "--json"],
        vec!["doctor", "--json"],
    ] {
        let stdout = run(&root, &args);
        assert_eq!(
            documents(&stdout),
            1,
            "`nexus {}` did not emit exactly one JSON document",
            args.join(" ")
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scan_that_finds_something_still_emits_one_document() {
    // The exact shape of the bug: Architect found a finding, and its report went out as a
    // second document beside the scan's.
    let root = project("finding");
    let stdout = run(&root, &["scan", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("one document");
    assert!(
        value["result"]["architect"]["findings"]
            .as_array()
            .is_some_and(|f| !f.is_empty()),
        "the fixture must actually produce a finding, or this test proves nothing: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
