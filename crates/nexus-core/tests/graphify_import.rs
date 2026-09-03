//! An external graph's *prose* is knowledge, and knowledge is a fact (roadmap 2.12).
//!
//! graphify runs two passes. The structural one is free and already reaches Nexus as edges.
//! The semantic one costs model calls and produces claims about the project — "SafeWriter
//! jails every write", "hooks fail open" — and those were discarded, because the importer
//! read `{"edges":[{"from","to"}]}`, a shape graphify has never emitted. It imported nothing
//! from a 2986-node graph and reported it as "no edges".
//!
//! What these pin is the part that decides whether the import is worth anything: a claim has
//! to reach the package while someone is editing the code it is about, and it must not cost
//! the agent more tokens to carry knowledge it can use.

use nexus_core::context::{ItemKind, Purpose, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    private PaymentRepository repo;
    public void pay(String key) { repo.save(key); }
}
"#;

/// Two claims about a symbol the index holds, one about a symbol it does not, and one whose
/// only identifying word is an English noun — the case that anchored design claims on
/// whatever symbol happened to end with the word.
const GRAPH: &str = r#"{
  "nodes": [
    {"id": "n1", "label": "PaymentService settles a payment exactly once",
     "file_type": "concept", "source_file": "docs/design.md", "source_location": "L12"},
    {"id": "n2", "label": "PaymentService retries are the caller's job",
     "file_type": "rationale", "source_file": "docs/design.md", "source_location": null},
    {"id": "n3", "label": "Payment is a domain concept",
     "file_type": "concept", "source_file": "docs/design.md", "source_location": null},
    {"id": "n4", "label": "Structural", "file_type": "code", "source_file": "src/x.java"}
  ],
  "links": []
}"#;

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

/// One directory per test. Two tests sharing a temp directory delete each other's files,
/// which passes locally and fails on a clean checkout.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-gimport-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/mn/pay")).expect("mkdir");
    fs::create_dir_all(root.join("docs")).expect("mkdir");
    fs::write(root.join(SERVICE), SOURCE).expect("write");
    fs::write(root.join("docs/design.md"), "# design\n").expect("write");
    fs::write(root.join("graph.json"), GRAPH).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn task(text: &str) -> TaskRequest {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    r
}

fn imported(name: &str) -> (PathBuf, Engine) {
    let (root, mut engine) = scanned(name);
    let r = engine
        .import_graphify(&root.join("graph.json"))
        .expect("import");
    assert_eq!(
        r.concepts_read, 3,
        "three prose nodes, the code node is not one"
    );
    assert_eq!(r.facts_recorded, 3);
    (root, engine)
}

#[test]
fn a_claim_naming_a_symbol_is_anchored_on_the_code_not_on_the_document() {
    let (root, mut engine) = scanned("anchor");
    let r = engine
        .import_graphify(&root.join("graph.json"))
        .expect("import");
    assert_eq!(
        r.anchored_on_code, 2,
        "the two claims naming PaymentService anchor on it; 'Payment' is a sentence word"
    );

    let facts = engine.facts(None).expect("facts");
    let on_code: Vec<_> = facts.iter().filter(|f| f.scope == "symbol").collect();
    assert_eq!(on_code.len(), 2);
    for f in &on_code {
        assert_eq!(f.subject.as_deref(), Some("mn.pay.PaymentService"));
        assert_eq!(f.source, "ai", "a model wrote it and the row says so");
        assert!(
            f.confidence <= 0.75,
            "the model ceiling holds: {}",
            f.confidence
        );
    }
    // A rationale is a decision; a concept describes structure. Both namespaces already exist.
    assert!(facts.iter().any(|f| f.key.starts_with("decision.")));
    assert!(facts.iter().any(|f| f.key.starts_with("arch.")));
}

#[test]
fn a_claim_that_names_no_symbol_still_anchors_on_the_document_that_states_it() {
    let (_root, engine) = imported("document");
    let f = engine
        .facts(None)
        .expect("facts")
        .into_iter()
        .find(|f| f.claim.contains("domain concept"))
        .expect("the unanchorable claim is still recorded");
    assert_eq!(f.scope, "file");
    assert_eq!(
        f.subject.as_deref(),
        Some("docs/design.md"),
        "the document that states it is the subject of last resort"
    );
}

#[test]
fn an_imported_claim_reaches_the_package_for_the_symbol_it_is_about() {
    // The whole point. A fact whose subject *is* the seed was ranked with zero seed
    // proximity, so six claims about `SafeWriter` scored 0.10 against a 0.15 floor while
    // `SafeWriter` itself scored 1.36 — the project's own knowledge could not reach the
    // package that exists to carry it.
    let (_root, engine) = imported("reaches");
    let pkg = engine
        .context(&task("refactor PaymentService"))
        .expect("context");
    let facts: Vec<&str> = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .map(|i| i.text.as_str())
        .collect();
    assert!(
        facts.iter().any(|t| t.contains("exactly once")),
        "an imported claim about the seed must be in the package: {:?}",
        pkg.items.iter().map(|i| &i.text).collect::<Vec<_>>()
    );
}

#[test]
fn knowledge_never_crowds_out_the_code_it_is_about() {
    // Facts had no component, so §7's diversity guard did not apply to them and a
    // well-documented symbol pushed its own methods out of the package.
    let (_root, engine) = imported("diversity");
    let pkg = engine
        .context(&task("refactor PaymentService"))
        .expect("context");
    let facts = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fact)
        .count();
    let symbols = pkg
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Symbol)
        .count();
    assert!(symbols > 0, "the code is still there: {:?}", pkg.items);
    assert!(
        facts <= 3,
        "at most three claims about one subject, like any other component: {facts}"
    );
}

#[test]
fn importing_twice_does_not_duplicate_what_is_already_known() {
    let (root, mut engine) = imported("idempotent");
    let before = engine.facts(None).expect("facts").len();
    engine
        .import_graphify(&root.join("graph.json"))
        .expect("second import");
    assert_eq!(
        engine.facts(None).expect("facts").len(),
        before,
        "a fact key is an identity, so re-importing the same graph updates rather than piles up"
    );
}

#[test]
fn a_graph_that_is_not_there_is_a_report_not_a_failure() {
    let (root, mut engine) = scanned("missing");
    let r = engine
        .import_graphify(&root.join("nope.json"))
        .expect("a missing graph is not an error");
    assert_eq!(r.facts_recorded, 0);
    assert!(r.warnings.iter().any(|w| w.contains("does not exist")));
}
