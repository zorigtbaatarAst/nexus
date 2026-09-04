//! Retrieval cost is bounded by what the request is about, not by what the project remembers.
//!
//! `facts(project_id, None)` loaded every live fact on every request and ranked them in Rust:
//! 14 ms at zero facts, 274 ms at 200,000, against ADR-024's 150 ms budget for a per-prompt
//! hook. Memory is append-only by design, so that number only ever grows.

use nexus_core::context::{Purpose, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::findings::CodeRef;
use nexus_core::{Engine, FactInput};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "src/mn/pay/PaymentService.java";
const SOURCE: &str = r#"package mn.pay;
public class PaymentService {
    private PaymentRepository repo;
    public void pay(String key) { repo.save(key); }
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

/// One directory per test. Two tests sharing a temp directory delete each other's files,
/// which passes locally and fails on a clean checkout.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-scale-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/mn/pay")).expect("mkdir");
    fs::write(root.join(SERVICE), SOURCE).expect("write");
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

fn record(engine: &mut Engine, key: &str, subject: &str) {
    engine
        .record_fact(FactInput {
            key: key.into(),
            scope: "symbol".into(),
            subject: Some(subject.into()),
            claim: format!("something true about {subject}"),
            source: "human".into(),
            evidence: vec![CodeRef {
                file: SERVICE.into(),
                line: 4,
                note: String::new(),
            }],
            confidence: 1.0,
        })
        .expect("record");
}

#[test]
fn unrelated_memory_does_not_enter_the_package_at_all() {
    let (_root, mut engine) = scanned("unrelated");
    let before = engine
        .context(&task("refactor mn.pay.PaymentService#pay"))
        .expect("context")
        .items_considered;

    for i in 0..2_000 {
        record(
            &mut engine,
            &format!("arch.noise-{i:05}"),
            &format!("other.Module{i}"),
        );
    }

    let after = engine
        .context(&task("refactor mn.pay.PaymentService#pay"))
        .expect("context")
        .items_considered;
    assert_eq!(
        before, after,
        "2,000 facts about symbols this request never mentions must not become candidates"
    );
}

#[test]
fn an_ancestor_and_a_descendant_of_a_seed_are_both_retrieved() {
    let (_root, mut engine) = scanned("family");
    record(&mut engine, "arch.module", "mn.pay");
    record(&mut engine, "arch.exact", "mn.pay.PaymentService");
    record(&mut engine, "arch.member", "mn.pay.PaymentService#pay");
    record(&mut engine, "arch.elsewhere", "mn.billing.Invoice");

    let pkg = engine
        .context(&task("refactor mn.pay.PaymentService"))
        .expect("context");
    let claims: Vec<&str> = pkg.items.iter().map(|i| i.text.as_str()).collect();
    for want in [
        "mn.pay",
        "mn.pay.PaymentService",
        "mn.pay.PaymentService#pay",
    ] {
        assert!(
            claims.iter().any(|c| c.contains(want)),
            "a fact about {want} belongs in a package about PaymentService: {claims:?}"
        );
    }
    assert!(
        !claims.iter().any(|c| c.contains("mn.billing.Invoice")),
        "a fact about another module does not: {claims:?}"
    );
}

#[test]
fn a_request_that_anchors_to_nothing_carries_no_facts() {
    // Deliberate. Serving every fact ranked by a subject_match term that is a constant 0.3
    // is the alphabetical flood, and §12 forbids an item with no anchor to the request.
    let (_root, mut engine) = scanned("anchorless");
    record(&mut engine, "arch.a", "mn.pay.PaymentService");
    let pkg = engine
        .context(&task("please make the thing work properly"))
        .expect("context");
    assert_eq!(
        pkg.items_considered, 0,
        "no seeds means no memory query: {:?}",
        pkg.items
    );
}

/// Same shape as `scanned`, but followed by a rescan that adds `count` trivial static
/// methods on one new class — enough, past `SEED_QUERY_CAP`, that a Review intent's seed set
/// must be capped rather than hit SQLite whole.
///
/// A review seeds from the *changed* set (seeds.rs stage 3), which only a rescan populates —
/// `scan` sets the baseline but records no changes against it, there being nothing yet to
/// diff against. So the bulk file is added after the first scan and picked up by `rescan`,
/// same as `renames.rs` does to get a change set to assert on. Own directory, own fixture:
/// see `scanned`'s doc comment on why one test cannot share either with another.
fn scanned_with_bulk_symbols(name: &str, count: usize) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-scale-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/mn/pay")).expect("mkdir");
    fs::write(root.join(SERVICE), SOURCE).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    let mut bulk = String::from("package mn.pay;\npublic class Bulk {\n");
    for i in 0..count {
        bulk.push_str(&format!("    public static void m{i}() {{}}\n"));
    }
    bulk.push_str("}\n");
    fs::write(root.join("src/mn/pay/Bulk.java"), bulk).expect("write");
    engine.rescan().expect("rescan");
    (root, engine)
}

/// Acceptance criterion 3 of the retrieval spec: "With more than `SEED_QUERY_CAP` relevant
/// symbols, the package `notes` says the seed set was capped." A Review intent seeds every
/// symbol the baseline scan reports as changed (§seeds.rs stage 3) — on a fresh project that
/// is every symbol there is, so 550 trivial methods clear the 256 cap with no expansion
/// needed.
///
/// 550, not 300: `SEED_QUERY_CAP` only needs clearing by one to test the note, but the bug
/// this guards against was SQLite's `SQLITE_MAX_COMPOUND_SELECT` (500), not the cap — and
/// 300 seeds stays under 500, so it never reproduced the failure. Before the fix, this test
/// did not fail on a missing note; it failed on the request itself with the same error the
/// Critical reproduced on this repository's 1,813 changed symbols — `too many terms in
/// compound SELECT` — because an uncapped copy of this seed list reached `SignalIndex::build`
/// before the capped copy further down in `task_package` ever ran.
#[test]
fn a_seed_set_over_the_cap_is_capped_and_the_notes_say_so() {
    let (_root, engine) = scanned_with_bulk_symbols("seed-cap", 550);

    let mut req = task("review mn.pay.Bulk#m0");
    req.purpose = Purpose::Review;
    let pkg = engine
        .context(&req)
        .expect("a large seed set must be capped, not sent to SQLite whole");

    assert!(
        pkg.notes
            .iter()
            .any(|n| n.contains("memory was queried for the first 256")),
        "notes must say the seed set was capped: {:?}",
        pkg.notes
    );
}

/// Run with: `cargo test -p nexus-core --test memory_scale -- --ignored --nocapture`
///
/// Ignored because a timing assertion in CI is flaky. It exists anyway because nothing else
/// in the suite fails if retrieval goes back to loading every live fact: the candidate set is
/// identical either way, so only the clock can tell the difference. Ratio, not absolute time,
/// so it means the same thing on a slow machine.
#[test]
#[ignore]
fn retrieval_does_not_slow_down_as_memory_grows() {
    use std::time::Instant;
    let (_root, mut engine) = scanned("scaling");
    let ask = |e: &Engine| {
        let t = Instant::now();
        for _ in 0..20 {
            e.context(&task("refactor mn.pay.PaymentService#pay"))
                .expect("context");
        }
        t.elapsed()
    };

    let small = ask(&engine);
    for i in 0..20_000 {
        record(
            &mut engine,
            &format!("arch.bulk-{i:06}"),
            &format!("unrelated.Mod{i}"),
        );
    }
    let large = ask(&engine);

    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 3.0,
        "retrieval scaled with total memory rather than with the request: \
         {small:?} at 0 facts, {large:?} at 20,000 — {ratio:.1}x. \
         That is the O(all facts) path returning."
    );
}
