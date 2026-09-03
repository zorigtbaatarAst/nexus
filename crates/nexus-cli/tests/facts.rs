//! The human entry point (roadmap 3.5).
//!
//! A fact recorded at a terminal was unanchored until now, and that cost twice: nothing could
//! invalidate it when the code moved (roadmap 1.6), and a context package excluded it with
//! `no file:line anchor` (roadmap 1.7). Evidence closes both.

use std::path::{Path, PathBuf};
use std::process::Command;

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const SOURCE: &str = "package mn.pay;\npublic class PaymentService {\n    public void pay(String key) { System.out.println(key); }\n}\n";

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

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-facts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let p = root.join(SERVICE);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, SOURCE).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    root
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(nexus())
        .args(args)
        .arg("--project")
        .arg(root)
        .output()
        .expect("run")
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

#[test]
fn an_anchored_fact_reaches_the_session_package() {
    let root = fixture("anchored");
    run(&root, &["scan"]);
    let out = run(
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
    assert!(out.status.success(), "{out:?}");

    let pkg = run(&root, &["context", "--session"]);
    assert!(
        stdout(&pkg).contains("settles exactly once"),
        "the fact a person recorded must reach the package: {}",
        stdout(&pkg)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unanchored_fact_is_accepted_and_says_what_it_costs() {
    // A human fact needs no anchor to be worth keeping. It does need to be told that nothing
    // will check it — which is the difference between a limitation and a surprise.
    let root = fixture("unanchored");
    run(&root, &["scan"]);
    let out = run(
        &root,
        &["fact", "convention.error-handling", "errors carry context"],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("remembered"), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("context package"),
        "the cost is stated: {}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_malformed_evidence_argument_is_a_usage_error_not_a_stored_guess() {
    let root = fixture("malformed");
    run(&root, &["scan"]);
    let out = run(
        &root,
        &["fact", "arch.pay.x", "y", "--evidence", "no-line-here"],
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_key_outside_the_namespace_list_is_refused_at_the_terminal() {
    let root = fixture("namespace");
    run(&root, &["scan"]);
    let out = run(&root, &["fact", "task.did-a-thing", "y"]);
    assert!(!out.status.success(), "{out:?}");
    let all = format!("{}{}", stdout(&out), String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("transcript"), "{all}");
    let _ = std::fs::remove_dir_all(&root);
}
