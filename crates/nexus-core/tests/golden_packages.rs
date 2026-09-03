//! Tier 1 of `13-evaluation.md`: golden packages.
//!
//! Five fixed tasks over one fixture, with the package's *shape* asserted against a committed
//! golden. The point is not that these particular items are the right ones — nobody knows that
//! yet, which is exactly why the roadmap forbids weight tuning in this phase. The point is
//! that **a ranking change that alters them must be deliberate**. A ranker nobody notices
//! drifting is a ranker nobody can trust.
//!
//! What is asserted, and why each:
//!
//!   * **Intent** — the verb table is the one part of the pipeline that must never move by
//!     accident, because everything downstream is weighted by it.
//!   * **The included set, in order** — this is the ranking, and it is the thing that drifts.
//!   * **Every candidate carries a reason** — §8's rule, checked on every task rather than
//!     once, because an unexplained exclusion is the failure that hides the missing item.
//!   * **The budget holds** — §7.
//!
//! What is deliberately *not* asserted: scores. A float in a golden file turns every harmless
//! re-weighting into a merge conflict and teaches people to re-baseline without reading. The
//! order already captures what a score change means.
//!
//! **Re-baselining.** When a change to the ranker is intended:
//!
//! ```text
//! NEXUS_REBASELINE=1 cargo test -p nexus-core --test golden_packages
//! git diff crates/nexus-core/tests/golden/    # read every line before committing
//! ```
//!
//! The diff is the review. Re-baselining without reading it is how a golden test becomes a
//! rubber stamp, so the failure message says so and the environment variable is deliberately
//! not a flag anyone types by habit.

use nexus_core::context::{Decision, TaskRequest};
use nexus_core::{Engine, Purpose};
use std::fs;
use std::path::{Path, PathBuf};

/// One golden task: what was asked, and what the package looked like.
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct Golden {
    task: String,
    intent: String,
    /// Included item labels, in package order. The ranking, captured.
    included: Vec<String>,
    /// Excluded labels with the rule that refused each, sorted for a stable file.
    excluded: Vec<String>,
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

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write");
}

/// A small payments service with a controller, a service, a repository and a test — enough
/// shape for reverse impact, coverage and a fact to all have something to say.
fn fixture(name: &str) -> PathBuf {
    // One directory per test: these run in parallel, and a shared path means one test
    // deletes the other's repository mid-scan.
    let root = std::env::temp_dir().join(format!("nexus-golden-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");
    git(&root, &["init", "-q", "-b", "main"]);
    write(
        &root,
        "src/mn/pay/PaymentService.java",
        "package mn.pay;\npublic class PaymentService {\n    private PaymentRepository repo;\n    public void pay(String key) { repo.save(key); }\n    public void refund(String key) { repo.save(key); }\n}\n",
    );
    write(
        &root,
        "src/mn/pay/PaymentController.java",
        "package mn.pay;\npublic class PaymentController {\n    private PaymentService service;\n    public void create(String key) { service.pay(key); }\n}\n",
    );
    write(
        &root,
        "src/mn/pay/PaymentRepository.java",
        "package mn.pay;\npublic class PaymentRepository {\n    public void save(String key) { }\n}\n",
    );
    write(
        &root,
        "src/test/mn/pay/PaymentServiceTest.java",
        "package mn.pay;\npublic class PaymentServiceTest {\n    public void testsPay() { new PaymentService().pay(\"k\"); }\n}\n",
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "initial"]);
    root
}

fn engine(root: &Path) -> Engine {
    let (mut e, _) = Engine::init(root).expect("init");
    e.scan().expect("scan");
    e.record_fact(nexus_core::FactInput {
        key: "invariant.payment.idempotency".into(),
        scope: "symbol".into(),
        subject: Some("mn.pay.PaymentService#pay".into()),
        claim: "pay is idempotent on the key".into(),
        source: "human".into(),
        evidence: vec![nexus_core::findings::CodeRef {
            file: "src/mn/pay/PaymentService.java".into(),
            line: 4,
            note: String::new(),
        }],
        confidence: 0.95,
    })
    .expect("fact");
    e
}

/// The five tasks. One per intent the verb table can reach from a realistic prompt.
const TASKS: &[(&str, &str)] = &[
    ("debug", "fix the bug in mn.pay.PaymentService"),
    ("build", "add retries to mn.pay.PaymentService"),
    ("refactor", "refactor mn.pay.PaymentService"),
    ("review", "review mn.pay.PaymentController"),
    ("explain", "why does mn.pay.PaymentController do that"),
];

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.json"))
}

fn capture(e: &Engine, task: &str) -> Golden {
    let req = TaskRequest {
        text: task.into(),
        files: Vec::new(),
        symbols: Vec::new(),
        budget_tokens: 4000,
        purpose: Purpose::Task,
        carry_seeds: Vec::new(),
        recent: None,
    };
    let pkg = e.context(&req).expect("package");

    // §8, on every task rather than once: an unexplained exclusion is the failure that hides
    // the item that should have been included.
    for row in &pkg.ledger.rows {
        assert!(!row.reason.is_empty(), "unexplained candidate: {row:?}");
    }
    // §7.
    assert!(
        pkg.tokens_estimated <= pkg.budget_tokens,
        "{} > {}",
        pkg.tokens_estimated,
        pkg.budget_tokens
    );

    let mut excluded: Vec<String> = pkg
        .ledger
        .rows
        .iter()
        .filter(|r| r.decision == Decision::Excluded)
        .map(|r| format!("{}  {}", r.label, r.reason))
        .collect();
    excluded.sort();

    Golden {
        task: task.into(),
        intent: pkg
            .intent
            .as_ref()
            .map_or("none".into(), |i| i.intent.as_str().to_string()),
        included: pkg
            .ledger
            .rows
            .iter()
            .filter(|r| r.decision == Decision::Included)
            .map(|r| r.label.clone())
            .collect(),
        excluded,
    }
}

#[test]
fn five_golden_packages_hold() {
    let root = fixture("packages");
    let e = engine(&root);
    let rebaseline = std::env::var("NEXUS_REBASELINE").is_ok();
    let mut drifted = Vec::new();

    for (name, task) in TASKS {
        let got = capture(&e, task);
        let path = golden_path(name);
        if rebaseline {
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            let body = serde_json::to_string_pretty(&got).expect("serialize");
            fs::write(&path, format!("{body}\n")).expect("write golden");
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            drifted.push(format!("{name}: no golden yet"));
            continue;
        };
        let want: Golden = serde_json::from_str(&raw).expect("golden is valid JSON");
        if got != want {
            drifted.push(format!(
                "{name}\n  want intent {} included {:?}\n  got  intent {} included {:?}",
                want.intent, want.included, got.intent, got.included
            ));
        }
    }

    let _ = fs::remove_dir_all(&root);
    assert!(
        drifted.is_empty(),
        "the ranker moved:\n{}\n\nIf that was deliberate:\n  \
         NEXUS_REBASELINE=1 cargo test -p nexus-core --test golden_packages\n  \
         git diff crates/nexus-core/tests/golden/\n\
         Read every line of that diff. Re-baselining without reading it is how a golden test \
         becomes a rubber stamp.",
        drifted.join("\n")
    );
}

#[test]
fn a_golden_task_is_reproducible_within_one_index() {
    // A package is a pure function of (request, index, memory). If two runs disagree, a
    // golden asserts nothing and neither does any measurement built on one.
    let root = fixture("reproducible");
    let e = engine(&root);
    let first = capture(&e, TASKS[0].1);
    for _ in 0..5 {
        assert_eq!(capture(&e, TASKS[0].1), first);
    }
    let _ = fs::remove_dir_all(&root);
}
