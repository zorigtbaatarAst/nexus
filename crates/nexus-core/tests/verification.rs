//! The gate, against a real toolchain (roadmap 4.4, 4.5, 4.6).
//!
//! The four-cell judgement is unit-tested with a synthetic runner, because logic that can only
//! be tested by running Gradle is logic nobody tests. What is tested here is the part that
//! only shows up when something actually runs: the ledger, the coverage rows, and the refusal
//! to run at all without a committed permission.

use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

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

/// A tiny cargo project, so the gate has a real toolchain to drive.
fn project(name: &str, body: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-verify-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).expect("mkdir");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    // Without this the build artefacts are committed and a baseline worktree inherits a
    // stale binary — which is exactly how an earlier manual run of this fixture reported a
    // broken test as passing.
    fs::write(root.join(".gitignore"), "target\n.nexus\n").expect("write");
    fs::write(root.join("src/lib.rs"), body).expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut e, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    e.scan().expect("scan");
    (root, e)
}

fn allow_host(root: &Path) {
    let p = root.join(".nexus/policy.toml");
    let body = fs::read_to_string(&p).expect("policy");
    fs::write(
        &p,
        body.replace(r#"execute       = "none""#, r#"execute       = "host""#),
    )
    .expect("write");
}

const GREEN: &str =
    "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\n#[test]\nfn works() { assert_eq!(add(1, 1), 2); }\n";

#[test]
fn nothing_runs_without_a_committed_permission() {
    // security.md §2: `execute = "none"` is the default, and the default is the point. A
    // freshly initialized project can index, diff and analyze but cannot run anything until
    // someone commits a change saying otherwise.
    let (root, mut e) = project("permission", GREEN);
    let r = e.verify().expect("verify");
    assert_eq!(r.verdict, "permission_required", "{r:?}");
    assert!(r.checks.is_empty(), "nothing ran: {r:?}");
    assert_eq!(
        e.test_run_count().expect("count"),
        0,
        "and nothing was recorded, because nothing happened"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_run_appends_to_the_ledger_and_never_updates_it() {
    // `test_runs` is append-only, which is what makes "this suite has been red for eleven
    // runs" answerable — a question no single run can answer.
    let (root, mut e) = project("ledger", GREEN);
    allow_host(&root);
    e.verify().expect("first");
    let after_one = e.test_run_count().expect("count");
    assert!(after_one > 0, "a run was recorded");
    e.verify().expect("second");
    assert!(
        e.test_run_count().expect("count") > after_one,
        "the second run appended rather than replacing the first"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_passing_run_is_verified_against_its_baseline() {
    let (root, mut e) = project("green", GREEN);
    allow_host(&root);
    let r = e.verify().expect("verify");
    assert_eq!(r.verdict, "verified", "{r:?}");
    assert!(
        r.checks
            .iter()
            .any(|c| c.kind == nexus_verify::CheckKind::Test),
        "the test command ran: {r:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_suite_that_was_already_red_is_inconclusive_never_failed() {
    // ADR-025 calls this the single assertion that decides whether the gate survives contact
    // with a real project. Asserted here against a real cargo run as well as in the unit
    // tests, because the wiring is as easy to get wrong as the logic.
    let (root, mut e) = project(
        "alreadyred",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\n#[test]\nfn works() { assert_eq!(add(1, 1), 3); }\n",
    );
    allow_host(&root);
    let r = e.verify().expect("verify");
    assert_eq!(r.verdict, "inconclusive", "{r:?}");
    assert!(
        r.why
            .as_deref()
            .is_some_and(|w| w.contains("already failing")),
        "{r:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_verification_records_an_attempt_against_every_finding_it_could_bear_on() {
    // §6: a verification result is not a terminal event. Every run appends, which is what
    // makes "this has been failing for eleven runs" answerable.
    let (root, mut e) = project("feedback", GREEN);
    allow_host(&root);
    e.record_finding(
        "bughunter",
        nexus_core::findings::Finding {
            finding_type: nexus_types::FindingType::Logic,
            title: "a suspected defect".into(),
            component: "demo".into(),
            anchor_fqn: None,
            severity: nexus_types::Severity::Medium,
            confidence: 0.7,
            detector: "test".into(),
            structural_key: "k".into(),
            slug: "suspected".into(),
            evidence: vec![nexus_core::findings::CodeRef {
                file: "src/lib.rs".into(),
                line: 1,
                note: "here".into(),
            }],
            capability_data: None,
        },
    )
    .expect("record");

    e.verify().expect("verify");
    let attempts = e.verification_attempts().expect("attempts");
    assert!(attempts > 0, "an attempt was recorded against the finding");

    // A green run is not evidence that a defect is gone, only that this run did not hit it,
    // so nothing may be marked FIXED here.
    let after = e.findings(None, None, None).expect("findings");
    assert!(
        after.iter().all(|f| f.status != "FIXED"),
        "verification must never set FIXED: {after:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_scoped_run_loads_less_than_a_full_one_and_says_the_graph_is_partial() {
    // Roadmap 5.4, and P7: ProjectContext materialised everything and then narrowed, which
    // is fine at this size and wrong at 500 KLOC. The saving is real only if the snapshot is
    // smaller; the honesty is required because a rule that traverses can no longer see
    // everything.
    let root = std::env::temp_dir().join(format!("nexus-scoped-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, body) in [
        (
            "src/a.rs",
            "pub fn a() { b::b(); }\npub mod b { pub fn b() {} }\n",
        ),
        ("src/c.rs", "pub fn c() {}\npub fn d() {}\npub fn e() {}\n"),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"s\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut e, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    e.scan().expect("scan");

    let full = e.context_symbol_count(None).expect("full");
    let scoped = e
        .context_symbol_count(Some(vec!["src/c.rs".to_string()]))
        .expect("scoped");
    assert!(
        scoped < full,
        "a scoped run must load less: {scoped} of {full}"
    );
    assert!(scoped > 0, "and still load what was asked for");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_reproduction_scaffold_lands_only_inside_the_jail() {
    // T4: Nexus modifies production code. Every generated file goes through the SafeWriter
    // root, and this asserts the path that came back is inside it.
    let (root, mut e) = project("scaffold", GREEN);
    e.record_finding(
        "bughunter",
        nexus_core::findings::Finding {
            finding_type: nexus_types::FindingType::Logic,
            title: "a suspected defect".into(),
            component: "demo".into(),
            anchor_fqn: None,
            severity: nexus_types::Severity::Medium,
            confidence: 0.7,
            detector: "test".into(),
            structural_key: "k".into(),
            slug: "suspected".into(),
            evidence: vec![nexus_core::findings::CodeRef {
                file: "src/lib.rs".into(),
                line: 1,
                note: "here".into(),
            }],
            capability_data: None,
        },
    )
    .expect("record");
    let uid = e.findings(None, None, None).expect("findings")[0]
        .uid
        .clone();

    let written = e.reproduce(&uid).expect("scaffold");
    let jail = root
        .join(".nexus/generated-tests")
        .canonicalize()
        .expect("jail root");
    assert!(
        std::path::Path::new(&written).starts_with(&jail),
        "{written} is outside {jail:?}"
    );

    let body = fs::read_to_string(&written).expect("read");
    assert!(body.contains(&uid), "it names the finding: {body}");
    assert!(
        body.contains("write the assertion"),
        "and fails until somebody writes one: {body}"
    );
    let _ = fs::remove_dir_all(&root);
}
