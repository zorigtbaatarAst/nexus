//! Markdown is a view (roadmap 3.4, ADR-023).
//!
//! The rule that keeps the separation honest is that **Nexus never reads it back**. A round
//! trip through Markdown would make an unvalidated text file authoritative over an
//! evidence-checked row, which inverts the whole design — so these tests check the output is
//! generated, regenerable, and marked as such.

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

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run")
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-memexp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let p = root.join("src/mn/pay/PaymentService.java");
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        p,
        "package mn.pay;\npublic class PaymentService {\n    public void pay(String k) { System.out.println(k); }\n}\n",
    )
    .expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    run(&root, &["scan"]);
    run(
        &root,
        &[
            "fact",
            "invariant.pay.settles-once",
            "a payment settles exactly once",
            "--subject",
            "mn.pay.PaymentService#pay",
            "--evidence",
            "src/mn/pay/PaymentService.java:3",
        ],
    );
    run(
        &root,
        &["fact", "convention.error-handling", "errors carry context"],
    );
    root
}

#[test]
fn one_file_per_namespace_each_marked_generated() {
    let root = project("basic");
    let out = run(&root, &["memory", "export", "--markdown", "docs/knowledge"]);
    assert!(out.status.success(), "{out:?}");

    let invariant =
        std::fs::read_to_string(root.join("docs/knowledge/invariant.md")).expect("invariant.md");
    let convention =
        std::fs::read_to_string(root.join("docs/knowledge/convention.md")).expect("convention.md");

    for body in [&invariant, &convention] {
        assert!(
            body.contains("Do not edit"),
            "an unmarked generated file invites an edit that the next export destroys: {body}"
        );
        assert!(body.contains("never reads this directory"), "{body}");
    }
    assert!(invariant.contains("settles exactly once"), "{invariant}");
    assert!(
        invariant.contains("src/mn/pay/PaymentService.java:3"),
        "evidence travels as a reference, not as source text: {invariant}"
    );
    assert!(
        convention.contains("nothing checks this"),
        "an unanchored fact says so in the view too: {convention}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exporting_twice_is_byte_identical() {
    // A view that churns is a view nobody can commit. This is what makes the output
    // reviewable in a pull request, which is the entire reason it exists.
    let root = project("stable");
    run(&root, &["memory", "export", "--markdown", "docs/knowledge"]);
    let first = std::fs::read_to_string(root.join("docs/knowledge/invariant.md")).expect("read");
    run(&root, &["memory", "export", "--markdown", "docs/knowledge"]);
    let second = std::fs::read_to_string(root.join("docs/knowledge/invariant.md")).expect("read");
    assert_eq!(first, second);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_invalidated_fact_leaves_the_view() {
    // The view shows what Nexus believes. A fact whose evidence moved has been withdrawn, and
    // showing it would be the exact trap the lifecycle exists to prevent.
    let root = project("invalidated");
    let file = root.join("src/mn/pay/PaymentService.java");
    let body = std::fs::read_to_string(&file).expect("read");
    std::fs::write(
        &file,
        body.replace("System.out.println(k)", "System.err.println(k)"),
    )
    .expect("write");
    run(&root, &["rescan"]);
    run(&root, &["memory", "export", "--markdown", "docs/knowledge"]);
    assert!(
        !root.join("docs/knowledge/invariant.md").exists(),
        "the only invariant was invalidated, so its namespace has nothing to show"
    );
    assert!(
        root.join("docs/knowledge/convention.md").exists(),
        "the unanchored fact is untouched"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nothing_in_the_workspace_reads_the_export_directory() {
    // §6's rule, checked structurally rather than trusted. If this ever fails, someone has
    // made a text file authoritative over an evidence-checked row.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir");
    let mut offenders = Vec::new();
    let mut stack = vec![crates.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                // A test may read the view to check it was written correctly — that is the
                // opposite of treating it as truth. The rule is about the product.
                if p.components().any(|c| c.as_os_str() == "tests") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for (n, line) in body.lines().enumerate() {
                    let reads = line.contains("read_to_string") || line.contains("read_dir");
                    if reads && line.contains("knowledge") {
                        offenders.push(format!("{}:{}", p.display(), n + 1));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Nexus must never read docs/knowledge/: {offenders:?}"
    );
}
