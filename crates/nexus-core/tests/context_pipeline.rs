//! Stages 2 and 3: from a sentence to a candidate set, with provenance at every step.
//!
//! These run against a real index rather than a mock, because the thing most likely to be
//! wrong is the match between what the store returns and what the stage believes it returns.

use nexus_core::context::{expand, Intent, SeedSource, TaskRequest};
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const CONTROLLER: &str = "src/mn/pay/PaymentController.java";

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

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write");
}

/// A controller that calls a service, so there is a real reverse edge to expand along.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-pipe-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    write(
        &root,
        SERVICE,
        "package mn.pay;\npublic class PaymentService {\n    public void pay(String key) {\n        System.out.println(key);\n    }\n}\n",
    );
    write(
        &root,
        CONTROLLER,
        "package mn.pay;\npublic class PaymentController {\n    private PaymentService service;\n    public void create(String key) {\n        service.pay(key);\n    }\n}\n",
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn request(text: &str) -> TaskRequest {
    let mut r = TaskRequest::session(4000);
    r.text = text.into();
    r.purpose = nexus_core::Purpose::Task;
    r
}

#[test]
fn an_explicit_symbol_is_the_highest_priority_seed() {
    let (_root, engine) = scanned("explicit");
    let mut req = request("do something");
    req.symbols = vec!["mn.pay.PaymentService#pay".into()];

    let got = engine.seeds(&req, Intent::Build).expect("seeds");
    assert!(!got.seeds.is_empty(), "{got:?}");
    assert_eq!(got.seeds[0].source, SeedSource::Explicit);
    assert!(got.seeds[0].symbol.fqn.contains("pay"));
}

#[test]
fn an_explicit_file_seeds_every_symbol_in_it() {
    let (_root, engine) = scanned("explicitfile");
    let mut req = request("do something");
    req.files = vec![SERVICE.into()];

    let got = engine.seeds(&req, Intent::Build).expect("seeds");
    assert!(
        got.seeds.iter().all(|s| s.symbol.file_path == SERVICE),
        "{got:?}"
    );
    assert!(got.seeds.len() >= 2, "class and method: {got:?}");
}

#[test]
fn an_fqn_written_in_the_prompt_is_found() {
    let (_root, engine) = scanned("fqn");
    let got = engine
        .seeds(&request("fix mn.pay.PaymentService"), Intent::Debug)
        .expect("seeds");
    assert!(
        got.seeds.iter().any(|s| s.source == SeedSource::Exact),
        "{got:?}"
    );
}

#[test]
fn a_bare_symbol_name_in_the_prompt_is_found() {
    let (_root, engine) = scanned("name");
    let got = engine
        .seeds(
            &request("why does PaymentController do that"),
            Intent::Explain,
        )
        .expect("seeds");
    assert!(
        got.seeds
            .iter()
            .any(|s| s.symbol.fqn.contains("PaymentController")),
        "{got:?}"
    );
}

#[test]
fn a_prompt_that_anchors_to_nothing_reports_zero_seeds_rather_than_inventing_some() {
    // §4: a package built from nothing is worse than an empty package plus "I could not
    // anchor this to the code", because the second lets the agent ask a better question.
    let (_root, engine) = scanned("noseeds");
    let got = engine
        .seeds(&request("make the thing better somehow"), Intent::Unknown)
        .expect("seeds");
    assert!(got.seeds.is_empty(), "{got:?}");
    assert!(
        got.notes.iter().any(|n| n.contains("no seed")),
        "zero seeds is stated, not left to be inferred: {got:?}"
    );
}

#[test]
fn the_empty_ui_strings_table_is_reported_rather_than_silently_contributing_nothing() {
    // Source 5 of §4 cannot work until 5.5 populates the table. A stage that quietly
    // contributes nothing is indistinguishable from one that is broken.
    let (_root, engine) = scanned("uistrings");
    let got = engine
        .seeds(&request("the Confirm button is broken"), Intent::Debug)
        .expect("seeds");
    assert!(
        got.notes.iter().any(|n| n.contains("ui_strings")),
        "{got:?}"
    );
}

#[test]
fn a_seed_is_never_listed_twice_and_keeps_its_best_source() {
    let (_root, engine) = scanned("dedupe");
    let mut req = request("fix mn.pay.PaymentService");
    req.symbols = vec!["mn.pay.PaymentService".into()];

    let got = engine.seeds(&req, Intent::Debug).expect("seeds");
    let mut ids: Vec<i64> = got.seeds.iter().map(|s| s.symbol.id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before, "a symbol appears once: {got:?}");
    let hit = got
        .seeds
        .iter()
        .find(|s| s.symbol.fqn == "mn.pay.PaymentService")
        .expect("the class");
    assert_eq!(
        hit.source,
        SeedSource::Explicit,
        "the highest-priority source wins: {hit:?}"
    );
}

#[test]
fn expansion_reaches_the_caller_on_a_reverse_intent() {
    let (_root, engine) = scanned("expand");
    let mut req = request("refactor pay");
    req.symbols = vec!["mn.pay.PaymentService#pay".into()];
    let seeded = engine.seeds(&req, Intent::Refactor).expect("seeds");
    assert!(!seeded.seeds.is_empty(), "the fixture seeds: {seeded:?}");

    let out = engine
        .expand(&seeded.seeds, Intent::Refactor)
        .expect("expand");
    assert_eq!(out.direction, "reverse");
    assert!(
        out.items
            .iter()
            .any(|i| i.fqn.contains("PaymentController")),
        "the caller must be reachable from the callee: {:?}",
        out.items.iter().map(|i| &i.fqn).collect::<Vec<_>>()
    );
    // §5: every expanded candidate is provable, not asserted.
    for item in &out.items {
        assert!(
            !item.path.is_empty(),
            "an item with no edge chain: {item:?}"
        );
        assert!(item.min_confidence > 0.0, "{item:?}");
    }
}

#[test]
fn direction_follows_intent() {
    assert_eq!(expand::direction_for(Intent::Refactor), "reverse");
    assert_eq!(expand::direction_for(Intent::Review), "reverse");
    assert_eq!(expand::direction_for(Intent::Build), "reverse");
    assert_eq!(expand::direction_for(Intent::Debug), "forward");
    // Both, merged: an explanation needs what this uses and what uses it.
    assert_eq!(expand::direction_for(Intent::Explain), "both");
    assert_eq!(expand::direction_for(Intent::Unknown), "both");
}

#[test]
fn expanding_from_no_seeds_is_empty_and_not_an_error() {
    let (_root, engine) = scanned("noexpand");
    let out = engine.expand(&[], Intent::Debug).expect("expand");
    assert!(out.items.is_empty(), "{out:?}");
}
