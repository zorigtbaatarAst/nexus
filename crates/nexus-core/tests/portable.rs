//! Moving memory between machines over a file (roadmap 3.6, §7).
//!
//! N13's position is that export/import over a committed file is the first answer to a shared
//! store, and that a server waits until this is proven insufficient. These tests pin the two
//! properties that make it trustworthy: a conflict changes nothing, and no source text leaves
//! the project.

use nexus_core::{Engine, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const SECRET_LINE: &str = "String apiKey = \"sk-do-not-export-me\";";

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

fn project(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-portable-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let p = root.join(SERVICE);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(
        p,
        format!("package mn.pay;\npublic class PaymentService {{\n    {SECRET_LINE}\n    public void pay(String k) {{ System.out.println(k); }}\n}}\n"),
    )
    .expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut e, _) = Engine::init(&root).expect("init");
    e.scan().expect("scan");
    (root, e)
}

fn fact(key: &str, claim: &str) -> FactInput {
    FactInput {
        key: key.into(),
        scope: "symbol".into(),
        subject: Some("mn.pay.PaymentService#pay".into()),
        claim: claim.into(),
        source: "human".into(),
        evidence: vec![nexus_core::findings::CodeRef {
            file: SERVICE.into(),
            line: 4,
            note: String::new(),
        }],
        confidence: 1.0,
    }
}

#[test]
fn a_document_moves_facts_into_an_empty_project() {
    let (root_a, mut a) = project("source");
    a.record_fact(fact(
        "invariant.pay.settles-once",
        "a payment settles exactly once",
    ))
    .expect("record");
    let doc = a.export_portable().expect("export");
    assert_eq!(doc.facts.len(), 1, "{doc:?}");

    let (root_b, mut b) = project("target");
    let report = b.import_portable(&doc).expect("import");
    assert_eq!(report.facts_added, 1, "{report:?}");
    let landed = b.facts(None).expect("facts");
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].claim, "a payment settles exactly once");

    // Importing the same document again is a no-op, not a duplicate.
    let again = b.import_portable(&doc).expect("import again");
    assert_eq!(again.facts_added, 0, "{again:?}");
    assert_eq!(again.facts_unchanged, 1, "{again:?}");

    let _ = fs::remove_dir_all(&root_a);
    let _ = fs::remove_dir_all(&root_b);
}

#[test]
fn a_disagreement_is_reported_and_changes_nothing() {
    // Two people who believe different things under one key have a disagreement. Picking one
    // silently produces a database that says something neither of them said.
    let (root_a, mut a) = project("conflictsource");
    a.record_fact(fact("invariant.pay.settles-once", "settles exactly once"))
        .expect("record");
    let doc = a.export_portable().expect("export");

    let (root_b, mut b) = project("conflicttarget");
    b.record_fact(fact("invariant.pay.settles-once", "settles at most once"))
        .expect("record");

    let report = b.import_portable(&doc).expect("import");
    assert_eq!(report.facts_added, 0, "{report:?}");
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert!(
        report.conflicts[0].contains("kept the local one"),
        "{report:?}"
    );
    assert_eq!(
        b.facts(None).expect("facts")[0].claim,
        "settles at most once",
        "the local belief stands untouched"
    );

    let _ = fs::remove_dir_all(&root_a);
    let _ = fs::remove_dir_all(&root_b);
}

#[test]
fn no_source_text_leaves_the_project() {
    // Evidence travels as a path and a line. A knowledge file carrying code would be a second
    // copy of the repository with none of its access control, and the whole claim is that
    // this file is safe to commit and safe to send.
    let (root, mut e) = project("nosource");
    e.record_fact(fact("invariant.pay.settles-once", "settles exactly once"))
        .expect("record");
    let raw = serde_json::to_string(&e.export_portable().expect("export")).expect("json");
    assert!(
        !raw.contains("sk-do-not-export-me"),
        "a line of the fixture's source appeared in the export"
    );
    assert!(
        raw.contains("PaymentService.java:4"),
        "the reference does: {raw}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_newer_format_is_refused_rather_than_half_read() {
    let (root, mut e) = project("format");
    let mut doc = e.export_portable().expect("export");
    doc.format = nexus_core::portable::FORMAT + 1;
    let err = e.import_portable(&doc).expect_err("refused");
    assert!(format!("{err}").contains("upgrade nexus"), "{err}");
    let _ = fs::remove_dir_all(&root);
}
