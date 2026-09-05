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
    let out = run(
        &root,
        &["context", "--task", "the save method", "--rank", "lexical"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("bm25"),
        "items should say how they were ranked:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_flag_is_absent_from_help() {
    // Undocumented is the deal. If this ever fails, either hide the flag again or update
    // cli-spec.md and 09-tooling.md to admit the surface exists — but do not do it silently.
    let out = Command::new(nexus())
        .args(["context", "--help"])
        .output()
        .expect("help");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("--rank"),
        "the control arm's flag must stay hidden:\n{text}"
    );
}

#[test]
fn an_unknown_rank_is_a_usage_error() {
    let root = project("typo");
    let out = run(&root, &["context", "--task", "x", "--rank", "bm25"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}
