//! The session package: what an agent knows before it reads a file.
//!
//! Phase 1 selection is a fixed query, so these tests pin the *contract* rather than a
//! ranking — the budget holds, every item is anchored, and every candidate that did not make
//! it says why. A ranked Phase 2 must keep all three true.

use nexus_core::context::{Decision, ItemKind, Purpose, TaskRequest, SESSION_BUDGET_TOKENS};
use nexus_core::findings::CodeRef;
use nexus_core::{Engine, EngineError, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";

const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    public void pay(String key) {
        System.out.println("pay " + key);
    }
}
"#;

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
    let root = std::env::temp_dir().join(format!("nexus-ctx-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let path = root.join(SERVICE);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, SOURCE).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    root
}

fn scanned(name: &str) -> Engine {
    let root = fixture(name);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    engine
}

fn session(engine: &Engine) -> nexus_core::ContextPackage {
    engine
        .context(&TaskRequest::session(SESSION_BUDGET_TOKENS))
        .expect("context")
}

/// The same package with its reasoning attached. Off by default because an agent pays for
/// every token of it; a test that asserts on the ledger has to ask for it like a human would.
fn explained(engine: &Engine) -> nexus_core::ContextPackage {
    let mut req = TaskRequest::session(SESSION_BUDGET_TOKENS);
    req.explain = true;
    engine.context(&req).expect("context")
}

fn anchored_on_pay() -> Vec<CodeRef> {
    vec![CodeRef {
        file: SERVICE.into(),
        line: 3,
        note: String::new(),
    }]
}

#[test]
fn a_session_package_says_what_the_project_is_and_what_it_was_built_from() {
    let engine = scanned("profile");
    let pkg = session(&engine);

    assert_eq!(pkg.purpose, Purpose::Session);
    assert!(pkg.project.symbols > 0, "the index is not empty");
    let profile = pkg.project.profile.as_ref().expect("a detected profile");
    assert!(
        profile.languages.iter().any(|l| l.lang == "java"),
        "the fixture is Java: {:?}",
        profile.languages
    );
    // §10: a package that does not state its basis implies a clean tree it may not describe.
    assert!(pkg.basis.scan_uid.is_some(), "the package names its scan");
    assert!(pkg.basis.commit.is_some(), "and its commit");
    assert!(
        !pkg.basis.selection.is_empty(),
        "and says how it selected: a caller cannot tell a fixed query from a ranked one"
    );
}

#[test]
fn an_anchored_fact_is_included_and_an_unanchored_one_is_excluded_with_a_reason() {
    let mut engine = scanned("facts");
    engine
        .record_fact(FactInput {
            key: "invariant.pay.idempotent".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "pay is idempotent on key".into(),
            source: "human".into(),
            evidence: anchored_on_pay(),
            confidence: 0.9,
        })
        .expect("anchored");
    engine
        .record_fact(FactInput {
            key: "convention.error-handling".into(),
            scope: "project".into(),
            subject: None,
            claim: "errors carry context".into(),
            source: "human".into(),
            evidence: Vec::new(),
            confidence: 0.9,
        })
        .expect("unanchored");

    let pkg = explained(&engine);
    let facts: Vec<_> = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .collect();
    assert_eq!(
        facts.len(),
        1,
        "only the anchored fact is an item: {facts:?}"
    );
    assert!(facts[0].text.contains("idempotent"));
    assert_eq!(facts[0].anchor.file, SERVICE);

    // §12 forbids an anchorless item; §8 forbids a silent omission. Both, together.
    let row = pkg
        .ledger
        .rows
        .iter()
        .find(|r| r.label.contains("convention.error-handling"))
        .expect("the unanchored fact is in the ledger");
    assert_eq!(row.decision, Decision::Excluded);
    assert!(
        row.reason.contains("anchor"),
        "the reason names the missing anchor: {row:?}"
    );
}

#[test]
fn the_package_stays_within_its_budget_and_accounts_for_every_candidate() {
    let mut engine = scanned("budget");
    // Enough anchored facts that the 800-token ceiling has to refuse some.
    for i in 0..200 {
        engine
            .record_fact(FactInput {
                key: format!("invariant.pay.rule-{i:03}"),
                scope: "symbol".into(),
                subject: Some("mn.pay.PaymentService#pay".into()),
                claim: format!(
                    "rule {i:03}: a payment is settled exactly once, and the ledger row proves it"
                ),
                source: "human".into(),
                evidence: anchored_on_pay(),
                confidence: 0.9,
            })
            .expect("fact");
    }

    // The ceiling applies to what the agent is handed, and the agent is not handed the
    // ledger. Asking for the reasoning is a human's deliberate purchase, so it is measured
    // on its own package below rather than billed against the budget.
    let shipped = session(&engine);
    assert!(
        shipped.tokens_estimated <= SESSION_BUDGET_TOKENS,
        "{} tokens exceeds the {SESSION_BUDGET_TOKENS} ceiling",
        shipped.tokens_estimated
    );

    let pkg = explained(&engine);
    assert!(
        pkg.tokens_estimated > shipped.tokens_estimated,
        "explaining 200 refusals is not free, and the number has to say so"
    );
    assert!(pkg.items_included > 0, "the budget bought something");
    assert!(
        pkg.ledger.count(Decision::Excluded) > 0,
        "200 facts do not fit in 800 tokens, so something was refused"
    );
    // §8: considered = included + excluded. An unexplained omission fails here.
    assert_eq!(
        pkg.items_considered,
        pkg.ledger.rows.len(),
        "every candidate is a ledger row"
    );
    assert_eq!(pkg.items_included, pkg.items.len());
    assert_eq!(
        pkg.items_included,
        pkg.ledger.count(Decision::Included),
        "the ledger and the item list agree"
    );
    for row in pkg
        .ledger
        .rows
        .iter()
        .filter(|r| r.decision == Decision::Excluded)
    {
        assert!(!row.reason.is_empty(), "an unexplained exclusion: {row:?}");
    }
}

#[test]
fn every_item_carries_an_anchor() {
    let engine = scanned("anchors");
    for item in session(&engine).items {
        assert!(
            !item.anchor.file.is_empty(),
            "§12: no item without a file:line anchor: {item:?}"
        );
    }
}

#[test]
fn without_a_baseline_there_is_no_package() {
    let root = fixture("nobaseline");
    let (engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    match engine.context(&TaskRequest::session(SESSION_BUDGET_TOKENS)) {
        Err(EngineError::NoBaseline) => {}
        other => panic!("a package built from nothing is worse than none: {other:?}"),
    }
}

#[test]
fn a_task_request_now_gets_a_ranked_package_and_says_so() {
    // Phase 1 refused every purpose but Session. Stages 1-6 (roadmap 2.1-2.7) serve Task, and
    // the basis distinguishes the two selections so a caller is never guessing which it got.
    let engine = scanned("purpose");
    let mut req = TaskRequest::session(4000);
    req.purpose = Purpose::Task;
    req.text = "fix PaymentService".into();

    let pkg = engine.context(&req).expect("a task package");
    assert_eq!(pkg.purpose, Purpose::Task);
    assert!(
        pkg.basis.selection.contains("ranked"),
        "the basis names the selection: {}",
        pkg.basis.selection
    );
    assert!(pkg.intent.is_some(), "a task package classified its text");
}
