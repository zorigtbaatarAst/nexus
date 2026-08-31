//! Renames must carry symbol identity.
//!
//! Without this a package move reads as every symbol in it being deleted and a set of
//! unrelated ones appearing. Once bug detection exists that duplicates every finding in the
//! moved package — the failure ADR-007 was written to prevent — and it is the kind of gap
//! that is invisible until it is expensive.

use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

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

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-rename-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    root
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "x"]);
}

#[test]
fn a_package_move_is_a_rename_not_a_delete_and_an_add() {
    let root = fixture("package");
    write(
        &root,
        "src/mn/pay/PaymentService.java",
        r#"
package mn.pay;
public class PaymentService {
    public void pay(String key) { System.out.println(key); }
    public void refund(String key) { System.out.println(key); }
}
"#,
    );
    commit(&root);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");

    fs::create_dir_all(root.join("src/mn/payments")).expect("mkdir");
    fs::rename(
        root.join("src/mn/pay/PaymentService.java"),
        root.join("src/mn/payments/PaymentService.java"),
    )
    .expect("mv");
    let moved = root.join("src/mn/payments/PaymentService.java");
    let body = fs::read_to_string(&moved)
        .expect("read")
        .replace("package mn.pay;", "package mn.payments;");
    fs::write(&moved, body).expect("write");

    let report = engine.rescan().expect("rescan");
    let symbols: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.entity == "symbol")
        .collect();

    assert_eq!(
        symbols.len(),
        3,
        "one row per moved symbol, not two: {symbols:?}"
    );
    assert!(
        symbols.iter().all(|i| i.change_type == "renamed"),
        "a package move must not read as deletes and adds: {symbols:?}"
    );
    assert!(
        symbols.iter().all(|i| i.from_fqn.is_some()),
        "a rename must say what the symbol was called"
    );
    assert!(symbols.iter().any(|i| i.fqn.as_deref()
        == Some("mn.payments.PaymentService#pay(String)")
        && i.from_fqn.as_deref() == Some("mn.pay.PaymentService#pay(String)")));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_edited_body_is_not_a_rename() {
    // Identity is carried only when nothing but the name changed. A method that moved *and*
    // changed is a delete and an add, because attaching an old bug history to code that is
    // no longer the same code is worse than losing the link.
    let root = fixture("edited");
    write(
        &root,
        "src/a/S.java",
        "package a;\npublic class S { public void go() { one(); } }\n",
    );
    commit(&root);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");

    fs::remove_file(root.join("src/a/S.java")).expect("rm");
    write(
        &root,
        "src/b/S.java",
        "package b;\npublic class S { public void go() { two(); } }\n",
    );

    let report = engine.rescan().expect("rescan");
    let go: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.fqn.as_deref().is_some_and(|f| f.contains("#go")))
        .collect();
    assert!(
        go.iter().any(|i| i.change_type == "added")
            && go.iter().any(|i| i.change_type == "deleted"),
        "a method whose body also changed is not a rename: {go:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn identical_boilerplate_is_left_alone_rather_than_matched_arbitrarily() {
    // Generated accessors collide on (name, sig_hash, body_hash) constantly. Carrying
    // identity to an arbitrary one of several candidates is worse than reporting a delete
    // and an add, so only unambiguous 1:1 matches count.
    let root = fixture("ambiguous");
    write(
        &root,
        "src/a/A.java",
        "package a;\npublic class A { public String id() { return null; } }\n",
    );
    write(
        &root,
        "src/a/B.java",
        "package a;\npublic class B { public String id() { return null; } }\n",
    );
    commit(&root);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");

    fs::remove_file(root.join("src/a/A.java")).expect("rm");
    fs::remove_file(root.join("src/a/B.java")).expect("rm");
    write(
        &root,
        "src/c/A.java",
        "package c;\npublic class A { public String id() { return null; } }\n",
    );
    write(
        &root,
        "src/c/B.java",
        "package c;\npublic class B { public String id() { return null; } }\n",
    );

    let report = engine.rescan().expect("rescan");
    let ids: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.fqn.as_deref().is_some_and(|f| f.ends_with("#id()")))
        .collect();
    assert!(
        ids.iter().all(|i| i.change_type != "renamed"),
        "two identical accessors must not be matched to each other: {ids:?}"
    );
    // The classes themselves differ by name, so those are unambiguous and do rename.
    let classes: Vec<_> = report
        .items
        .iter()
        .filter(|i| matches!(i.fqn.as_deref(), Some("c.A") | Some("c.B")))
        .collect();
    assert!(
        classes.iter().all(|i| i.change_type == "renamed"),
        "the classes are distinguishable by name and should carry identity: {classes:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_pre_nexus_project_directory_is_migrated_in_place() {
    // The rename moves a user's scans, findings and history. A silent failure here loses
    // all of it, and the symptom would be an empty index rather than an error.
    let root = fixture("legacy");
    write(&root, "src/a/S.java", "package a;\npublic class S { public void go() {} }\n");
    commit(&root);

    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");
    let before = engine.status().expect("status");
    drop(engine);

    // Put it back the way a pre-Nexus install left it.
    fs::rename(root.join(".nexus"), root.join(".bughunter")).expect("rename dir");
    fs::rename(root.join(".bughunter/nexus.db"), root.join(".bughunter/bughunter.db")).expect("rename db");
    for tail in ["-wal", "-shm"] {
        let from = root.join(format!(".bughunter/nexus.db{tail}"));
        if from.exists() {
            fs::rename(from, root.join(format!(".bughunter/bughunter.db{tail}"))).expect("rename wal");
        }
    }

    let engine = Engine::open(&root).expect("open migrates");
    assert!(root.join(".nexus/nexus.db").exists(), "the directory and database move together");
    assert!(!root.join(".bughunter").exists(), "and the old one is gone, not duplicated");

    let after = engine.status().expect("status");
    assert_eq!(after.files, before.files, "the index survives");
    assert_eq!(after.symbols, before.symbols);
    assert_eq!(
        after.baseline.map(|b| b.scan_uid),
        before.baseline.map(|b| b.scan_uid),
        "and so does the baseline"
    );
    let _ = fs::remove_dir_all(&root);
}
