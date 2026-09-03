//! A fact is invalidated by change, not by age.
//!
//! `facts.invalidated_at` was read in the retrieval query and written nowhere, so a fact
//! anchored at `PaymentService#pay():4` outlived that method's deletion and was served
//! forever as established knowledge. These tests pin the rule from memory-model.md §2:
//! edit the anchored symbol and the fact stops surfacing — while the row stays on disk,
//! which the store's own test asserts, because only the store can run SQL.

use nexus_core::findings::CodeRef;
use nexus_core::{Engine, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";

const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    public void pay(String key) {
        System.out.println("pay " + key);
    }
    public void refund(String key) {
        System.out.println("refund " + key);
    }
}
"#;

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
    let root = std::env::temp_dir().join(format!("nexus-fact-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    root
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "x"]);
}

/// The 1-based line of `pay`'s signature — inside the method's span.
fn pay_line() -> u32 {
    let idx = SOURCE
        .lines()
        .position(|l| l.contains("public void pay"))
        .expect("pay is in the fixture");
    idx as u32 + 1
}

/// Scanned fixture with one fact anchored on `pay`.
fn scanned_with_fact(name: &str, source: &str, evidence: Vec<CodeRef>) -> (PathBuf, Engine) {
    let root = fixture(name);
    write(&root, SERVICE, SOURCE);
    commit(&root);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");
    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is idempotent on key".into(),
            source: source.into(),
            evidence,
            confidence: 0.7,
        })
        .expect("record");
    assert_eq!(
        engine.facts(None).expect("facts").len(),
        1,
        "the fact is live"
    );
    (root, engine)
}

fn on_pay() -> Vec<CodeRef> {
    vec![CodeRef {
        file: SERVICE.into(),
        line: pay_line(),
        note: String::new(),
    }]
}

fn edit(root: &Path, from: &str, to: &str) {
    let path = root.join(SERVICE);
    let body = fs::read_to_string(&path).expect("read");
    assert!(body.contains(from), "fixture must contain {from:?}");
    fs::write(&path, body.replace(from, to)).expect("write");
}

#[test]
fn editing_the_anchored_symbol_invalidates_the_fact() {
    let (root, mut engine) = scanned_with_fact("edit", "ai", on_pay());
    edit(&root, r#""pay ""#, r#""paid ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 1, "{report:?}");
    assert!(
        engine.facts(None).expect("facts").is_empty(),
        "a fact about code that changed must not be retrieved"
    );
}

#[test]
fn editing_another_symbol_leaves_the_fact_alone() {
    let (root, mut engine) = scanned_with_fact("other", "ai", on_pay());
    edit(&root, r#""refund ""#, r#""refunded ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.symbols_changed, 1, "refund changed: {report:?}");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn a_reformat_does_not_invalidate() {
    // normalize_body is pinned so that a reformat produces zero symbol changes; a fact
    // must ride on that, not on the raw text.
    let (root, mut engine) = scanned_with_fact("reformat", "ai", on_pay());
    edit(
        &root,
        "        System.out.println(\"pay \" + key);",
        "            System.out.println(\"pay \" + key);",
    );

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.symbols_changed, 0, "{report:?}");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn deleting_the_evidence_file_invalidates_the_fact() {
    let (root, mut engine) = scanned_with_fact("delete", "ai", on_pay());
    fs::remove_file(root.join(SERVICE)).expect("rm");

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 1, "{report:?}");
    assert!(engine.facts(None).expect("facts").is_empty());
}

#[test]
fn a_fact_without_evidence_is_never_invalidated() {
    // A human fact with no anchor is about the project, not a line; nothing a scan sees
    // can contradict it.
    let (root, mut engine) = scanned_with_fact("human", "human", Vec::new());
    edit(&root, r#""pay ""#, r#""paid ""#);

    let report = engine.rescan().expect("rescan");
    assert_eq!(report.facts_invalidated, 0, "{report:?}");
    assert_eq!(engine.facts(None).expect("facts").len(), 1);
}

#[test]
fn an_invalidated_fact_can_be_re_established_under_the_same_key() {
    let (root, mut engine) = scanned_with_fact("reestablish", "ai", on_pay());
    edit(&root, r#""pay ""#, r#""paid ""#);
    engine.rescan().expect("rescan");
    assert!(engine.facts(None).expect("facts").is_empty());

    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is still idempotent on key".into(),
            source: "ai".into(),
            evidence: on_pay(),
            confidence: 0.7,
        })
        .expect("record again");
    let facts = engine.facts(None).expect("facts");
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].claim, "pay is still idempotent on key");
}
