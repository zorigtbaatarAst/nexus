//! Stages 2 and 3: from a sentence to a candidate set, with provenance at every step.
//!
//! These run against a real index rather than a mock, because the thing most likely to be
//! wrong is the match between what the store returns and what the stage believes it returns.

use nexus_core::context::{expand, Intent, SeedSource, TaskRequest};
use nexus_core::findings::CodeRef;
use nexus_core::Engine;
use nexus_core::FactInput;
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
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

/// A request that also asks for the reasoning. The ledger and the score terms are
/// explanation rather than content and are off by default, so a test that asserts on them
/// has to pay for them like any other caller.
fn explaining(text: &str) -> TaskRequest {
    let mut r = request(text);
    r.explain = true;
    r
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
fn a_project_with_no_screen_strings_says_so_rather_than_contributing_nothing_in_silence() {
    // A stage that quietly contributes nothing is indistinguishable from one that is broken.
    let (_root, engine) = scanned("uistrings");
    let got = engine
        .seeds(&request("the Confirm button is broken"), Intent::Debug)
        .expect("seeds");
    assert!(
        got.notes.iter().any(|n| n.contains("screen strings")),
        "{got:?}"
    );
}

#[test]
fn words_on_the_screen_reach_the_code_that_renders_them_in_any_language() {
    // The strongest signal an investigation has, and the one AGENTS.md warns is lost if only
    // keys are indexed: the screenshot is in Mongolian, the source holds an English key.
    let (root, mut engine) = scanned("screentext");
    write(
        &root,
        "src/locales/mn/common.json",
        r#"{"cart": {"confirm": "Захиалга"}}"#,
    );
    write(
        &root,
        "web/Cart.tsx",
        "export function Cart() { return <button aria-label=\"Confirm order\">Go</button>; }\n",
    );
    engine.scan().expect("rescan");
    assert!(
        engine.ui_string_count().expect("count") > 0,
        "strings indexed"
    );

    let mongolian = engine
        .seeds(&request("Захиалга"), Intent::Debug)
        .expect("seeds");
    assert!(
        !mongolian
            .notes
            .iter()
            .any(|n| n.contains("no screen strings")),
        "the non-English value was indexed: {mongolian:?}"
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

#[test]
fn a_scan_records_the_commit_ledger_and_a_rescan_does_not_duplicate_it() {
    // `commits` is a ledger: append-only, so re-seeing history must be a no-op. An UPDATE or
    // a duplicate here destroys the "what did Nexus believe at scan 12" question.
    let (root, mut engine) = scanned("commits");
    let before = engine.commit_count().expect("count");
    assert!(before >= 1, "the fixture has one commit: {before}");

    write(
        &root,
        SERVICE,
        "package mn.pay;\npublic class PaymentService {}\n",
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "second"]);
    engine.rescan().expect("rescan");
    assert_eq!(engine.commit_count().expect("count"), before + 1);

    engine.rescan().expect("rescan again");
    assert_eq!(
        engine.commit_count().expect("count"),
        before + 1,
        "re-seeing the same history must insert nothing"
    );
}

#[test]
fn churn_is_normalised_against_the_busiest_path() {
    let (root, engine) = scanned("churn");
    // Touch one file three more times; the other stays at its single initial commit.
    for i in 0..3 {
        write(
            &root,
            SERVICE,
            &format!("package mn.pay;\npublic class PaymentService {{ int v = {i}; }}\n"),
        );
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "touch"]);
    }
    let churn = engine.churn();
    let hot = churn.get(SERVICE).copied().unwrap_or(0.0);
    let cold = churn.get(CONTROLLER).copied().unwrap_or(0.0);
    assert!(hot > cold, "hot {hot} cold {cold}");
    assert!(
        (hot - 1.0).abs() < 1e-9,
        "the busiest path normalises to 1.0: {hot}"
    );
    assert!(cold > 0.0, "a path touched once is not zero: {cold}");
    assert_eq!(churn.get("nothing/here.java"), None);
}

#[test]
fn a_task_package_is_ranked_anchored_and_fully_accounted_for() {
    let (_root, engine) = scanned("taskpkg");
    let mut req = explaining("refactor PaymentService");
    req.budget_tokens = 4000;

    let pkg = engine.context(&req).expect("package");
    assert!(!pkg.items.is_empty(), "{pkg:?}");
    // §12: every item anchored, and the scores actually order the list.
    for item in &pkg.items {
        assert!(!item.anchor.file.is_empty(), "{item:?}");
    }
    for pair in pkg.items.windows(2) {
        assert!(
            pair[0].score / pair[0].tokens.max(1) as f64
                >= pair[1].score / pair[1].tokens.max(1) as f64 - 1e-9,
            "density order broken: {:?}",
            pkg.items
                .iter()
                .map(|i| (i.score, i.tokens))
                .collect::<Vec<_>>()
        );
    }
    // §8: considered == every ledger row, and no exclusion is unexplained.
    assert_eq!(pkg.items_considered, pkg.ledger.rows.len());
    for row in &pkg.ledger.rows {
        assert!(!row.reason.is_empty(), "{row:?}");
    }
    assert!(pkg.tokens_estimated <= pkg.budget_tokens);
    assert_eq!(
        pkg.intent.as_ref().map(|i| i.intent),
        Some(Intent::Refactor)
    );
}

#[test]
fn a_task_package_carries_every_score_term_it_used() {
    // §8 must be able to answer "why is this here". A total with no decomposition cannot.
    let (_root, engine) = scanned("terms");
    let pkg = engine
        .context(&explaining("refactor PaymentService"))
        .expect("package");
    let seed = pkg
        .items
        .iter()
        .find(|i| i.why.starts_with("seed"))
        .expect("a seed item");
    assert!(seed.terms.seed > 0.0, "{seed:?}");
    assert!(seed.terms.cost < 0.0, "cost is a penalty: {seed:?}");
}

#[test]
fn an_identical_question_is_served_from_cache_and_an_edit_is_a_miss() {
    // §11, and R9: an agent editing without committing is the normal case, so a key over
    // HEAD alone would serve context describing code that no longer exists.
    let (root, engine) = scanned("cache");
    let req = request("refactor PaymentService");

    let first = engine.context(&req).expect("first");

    // Proving a *hit*, not merely a matching answer: a miss recomputes and produces the same
    // package, so comparing two results proves nothing. A sentinel written into the cached
    // file can only come back if the file was actually read.
    let entry = fs::read_dir(root.join(".nexus/cache/context"))
        .expect("cache dir")
        .filter_map(|e| e.ok())
        .next()
        .expect("one entry")
        .path();
    let mut cached: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&entry).expect("read")).expect("json");
    cached["project"]["name"] = serde_json::json!("came-from-the-cache");
    fs::write(&entry, cached.to_string()).expect("write");

    let hit = engine.context(&req).expect("second");
    assert_eq!(
        hit.project.name, "came-from-the-cache",
        "the second identical question must be served from the cache, not recomputed"
    );
    assert_eq!(first.items_considered, hit.items_considered);
    let cached: Vec<_> = fs::read_dir(root.join(".nexus/cache/context"))
        .expect("cache dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(cached.len(), 1, "one entry for one question");

    write(
        &root,
        "src/mn/pay/New.java",
        "package mn.pay;\npublic class New {}\n",
    );
    let after_edit = engine.context(&req).expect("third");
    assert!(
        fs::read_dir(root.join(".nexus/cache/context"))
            .expect("cache dir")
            .count()
            > 1,
        "a dirty tree must key differently, not reuse the clean answer"
    );
    assert_eq!(after_edit.purpose, req.purpose);
}

#[test]
fn a_referential_turn_uses_carried_seeds_and_never_stores_the_previous_message() {
    // §14.1: the harness has the conversation, so it supplies what Nexus cannot know. Nexus
    // stays a pure function of (request, index, memory) — which is what keeps a golden
    // package meaningful and keeps this off the road to a daemon.
    let (_root, engine) = scanned("referential");
    let mut req = request("now do the same for the other one");
    req.carry_seeds = vec!["mn.pay.PaymentService#pay".into()];
    req.recent = Some("refactor mn.pay.PaymentService#pay".into());

    let pkg = engine.context(&req).expect("package");
    assert!(
        pkg.items.iter().any(|i| i.text.contains("PaymentService")),
        "a carried seed anchors the turn: {:?}",
        pkg.items.iter().map(|i| &i.text).collect::<Vec<_>>()
    );
    // The previous message reached the verb table and nothing else.
    assert!(
        engine.facts(None).expect("facts").is_empty(),
        "--recent must never reach the store"
    );
}

#[test]
fn an_unanchored_turn_with_nothing_carried_reports_that_it_does_not_know() {
    let (_root, engine) = scanned("unanchored");
    let pkg = engine
        .context(&request("now do the same for the other one"))
        .expect("package");
    assert_eq!(
        pkg.intent.as_ref().map(|i| i.intent),
        Some(Intent::Unknown),
        "inventing seeds would be worse than saying so: {:?}",
        pkg.intent
    );
    assert!(pkg
        .notes
        .iter()
        .any(|n| n.contains("intent was not determined")));
}

#[test]
fn an_external_graph_is_ignored_unless_the_config_asks_and_is_labelled_when_it_does() {
    // Roadmap 2.12. A scan that silently starts trusting a file someone left in the working
    // tree is not something to ship on by default, so the flag is the whole switch.
    let (root, mut engine) = scanned("graphify");
    write(&root, "svc/handler.py", "def handle():\n    pass\n");
    write(&root, "svc/util.py", "def helper():\n    pass\n");
    fs::create_dir_all(root.join("graphify-out")).expect("mkdir");
    // graphify's own node-link shape: nodes carry the file, links carry node ids. The
    // importer once read `{"edges":[{"from","to"}]}`, which graphify has never written.
    fs::write(
        root.join("graphify-out/graph.json"),
        r#"{"nodes":[
             {"id":"h","label":"handle","file_type":"code","source_file":"svc/handler.py"},
             {"id":"u","label":"helper","file_type":"code","source_file":"svc/util.py"}],
           "links":[{"source":"h","target":"u","relation":"imports","confidence_score":0.99}]}"#,
    )
    .expect("write");

    let external = |g: &nexus_core::GraphReport| -> i64 {
        g.by_resolution
            .iter()
            .find(|(r, _)| r == "external-graph")
            .map_or(0, |(_, n)| *n)
    };

    // Off by default: the file exists and is not read. The fixture's own Java edge is
    // untouched, which is the point — the flag adds a source, it does not replace one.
    engine.scan().expect("scan");
    let before = engine.graph().expect("graph");
    assert_eq!(
        external(&before),
        0,
        "python is unanalysed and the flag is off: {before:?}"
    );
    let parsed_before = before.edges_resolved;

    fs::write(
        root.join(".nexus/config.toml"),
        "[scan]\nresolution = \"external-graph\"\n",
    )
    .expect("config");
    engine.scan().expect("rescan with the flag on");
    let after = engine.graph().expect("graph");
    assert_eq!(
        external(&after),
        1,
        "the edge is labelled, not laundered into heuristic: {after:?}"
    );
    // An edge nobody parsed must not lift the number ADR-017 exists to keep honest.
    assert_eq!(
        after.edges_resolved, parsed_before,
        "external-graph edges stay out of the resolution rate: {after:?}"
    );
}

#[test]
fn naming_a_class_seeds_its_methods_so_expansion_reaches_the_callers() {
    // The dependency graph is method-level: nothing calls a class. Seeding only the class
    // means "refactor PaymentService" — the commonest way anyone names code — expands to
    // nothing at all, which is the whole product claim failing on its most likely input.
    let (_root, engine) = scanned("container");
    let seeded = engine
        .seeds(&request("refactor mn.pay.PaymentService"), Intent::Refactor)
        .expect("seeds");
    assert!(
        seeded.seeds.iter().any(|s| s.symbol.fqn.contains('#')),
        "the class's members are seeds too: {:?}",
        seeded
            .seeds
            .iter()
            .map(|s| &s.symbol.fqn)
            .collect::<Vec<_>>()
    );

    let out = engine
        .expand(&seeded.seeds, Intent::Refactor)
        .expect("expand");
    assert!(
        out.items
            .iter()
            .any(|i| i.fqn.contains("PaymentController")),
        "and the caller is reachable from them: {:?}",
        out.items.iter().map(|i| &i.fqn).collect::<Vec<_>>()
    );
}

#[test]
fn both_retrieval_paths_agree_because_they_call_one_formula() {
    // §4 is one formula. Two rankings over one table would disagree eventually, and the one
    // further from the data is the one that would be wrong.
    let (_root, mut engine) = scanned("oneformula");
    for (key, subject, source) in [
        (
            "invariant.pay.settles-once",
            "mn.pay.PaymentService#pay",
            "human",
        ),
        ("arch.pay.layering", "mn.pay", "ai"),
        ("risk.orders.locking", "mn.orders", "ai"),
    ] {
        engine
            .record_fact(nexus_core::FactInput {
                key: key.into(),
                scope: "symbol".into(),
                subject: Some(subject.into()),
                claim: format!("claim for {key}"),
                source: source.into(),
                evidence: Vec::new(),
                confidence: 0.9,
            })
            .expect("record");
    }

    // The ask path, with no seeds: provenance and state decide, so the human fact leads.
    let ranked = engine.facts(None).expect("facts");
    assert_eq!(ranked[0].key, "invariant.pay.settles-once", "{ranked:?}");
    assert!(ranked[0].durable, "a human fact is durable on arrival");

    // The Context Engine path, seeded on the payment method: the same fact still leads, and
    // the unrelated module's fact is not in the package at all.
    let pkg = engine
        .context(&explaining("refactor mn.pay.PaymentService#pay"))
        .expect("package");
    let fact_labels: Vec<&str> = pkg
        .ledger
        .rows
        .iter()
        .filter(|r| r.kind == nexus_core::context::ItemKind::Fact)
        .map(|r| r.label.as_str())
        .collect();
    assert!(
        fact_labels.contains(&"invariant.pay.settles-once"),
        "{fact_labels:?}"
    );
    assert!(
        !fact_labels.contains(&"risk.orders.locking"),
        "a fact about another module is not relevant here: {fact_labels:?}"
    );
}

#[test]
fn a_fact_key_outside_the_namespace_list_is_refused() {
    let (_root, mut engine) = scanned("namespace");
    let err = engine
        .record_fact(FactInput {
            key: "task.did-a-thing".into(),
            scope: "project".into(),
            subject: None,
            claim: "x".into(),
            source: "human".into(),
            evidence: Vec::new(),
            confidence: 1.0,
        })
        .expect_err("refused");
    assert!(format!("{err}").contains("transcript"), "{err}");
}

#[test]
fn the_reported_token_count_is_what_the_agent_actually_receives() {
    // Measured, not asserted: `tokens_estimated` once counted the text of the included items
    // and nothing else, reporting 253 tokens for a package that put 11,113 on the wire. A
    // budget that measures a twentieth of the payload is not a budget.
    let (_root, engine) = scanned("wirecost");
    let pkg = engine
        .context(&request("refactor PaymentService"))
        .expect("package");
    let wire = serde_json::to_string(&pkg).expect("serialize");
    let actual = wire.len() / 4; // deliberately generous bytes-per-token
    assert!(
        pkg.tokens_estimated >= actual,
        "reported {} but the serialized package is at least {actual} tokens",
        pkg.tokens_estimated
    );
    assert!(
        pkg.tokens_estimated <= pkg.budget_tokens,
        "{} exceeds the {} budget",
        pkg.tokens_estimated,
        pkg.budget_tokens
    );
}

#[test]
fn explanation_is_off_by_default_and_costs_when_asked_for() {
    // §8 requires the package to be *able* to say why. It does not require paying for that
    // on every request — the ledger and the score terms were 5,759 of 11,113 tokens, and the
    // ledger grows with candidates considered, the one number the budget never capped.
    let (_root, engine) = scanned("explaincost");
    let plain = engine
        .context(&request("refactor PaymentService"))
        .expect("plain");
    let full = engine
        .context(&explaining("refactor PaymentService"))
        .expect("explained");

    assert!(plain.ledger.rows.is_empty(), "no reasons unless asked");
    assert!(
        plain.items.iter().all(|i| i.terms == Default::default()),
        "no score terms unless asked"
    );
    assert!(!full.ledger.rows.is_empty(), "and the reasons on request");
    assert_eq!(
        plain.items_considered, full.items_considered,
        "the count of candidates survives even when their reasons do not"
    );
    assert!(
        plain.tokens_estimated < full.tokens_estimated,
        "the cheap package must actually be cheaper: {} vs {}",
        plain.tokens_estimated,
        full.tokens_estimated
    );
}

#[test]
fn recording_a_fact_invalidates_the_cached_package() {
    // The cache key listed the index and the tree but not memory, so the first package was
    // served forever: 140 facts sat in the database while every request returned an answer
    // computed before any of them existed. That is the exact opposite of the promise that an
    // expensive conclusion is reached once and reused.
    let (_root, mut engine) = scanned("cachememory");
    let before = engine
        .context(&request("refactor mn.pay.PaymentService#pay"))
        .expect("first");

    engine
        .record_fact(FactInput {
            key: "invariant.pay.settles-once".into(),
            scope: "symbol".into(),
            subject: Some("mn.pay.PaymentService#pay".into()),
            claim: "a payment settles exactly once".into(),
            source: "human".into(),
            evidence: vec![CodeRef {
                file: SERVICE.into(),
                line: 3,
                note: String::new(),
            }],
            confidence: 1.0,
        })
        .expect("record");

    let after = engine
        .context(&request("refactor mn.pay.PaymentService#pay"))
        .expect("second");
    assert!(
        after
            .items
            .iter()
            .any(|i| i.text.contains("settles exactly once")),
        "the fact recorded a moment ago must reach the very next package: {:?}",
        after.items.iter().map(|i| &i.text).collect::<Vec<_>>()
    );
    assert!(after.items_considered > before.items_considered);
}
