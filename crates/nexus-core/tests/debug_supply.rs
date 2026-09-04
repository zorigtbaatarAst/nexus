//! Does the debug package contain the files the fix has to touch?
//!
//! The claim that Nexus makes bug-fixing cheaper has never been measured. The figure in both
//! READMEs — ~34,000 tokens against ~1,500 — is arithmetic comparing *reading ten files* with
//! *one index query*: two lookups, not two bug fixes. `13-evaluation.md` defines the real
//! number, cost-per-success, and says it is "currently unmeasurable".
//!
//! Measuring it means running an agent on the same bug twice, several times over, for a
//! median: real money per run, and non-deterministic. This is the deterministic proxy that
//! has to hold *before* that number could ever be good — **if the package does not contain
//! the files the fix touches, no token saving is possible.**
//!
//! ## The requests are hand-written, and that is the judgement call
//!
//! Each is the sentence a person would type on noticing the symptom. They are deliberately
//! not generated from `plants_bug.summary`: that text names the cause, and a request naming
//! the cause tests nothing, because the answer is already in the question. A request that
//! names the anchor symbol outright is a review failure, not a passing test.
//!
//! ## Ground truth, and where it is weaker
//!
//! Where the corpus records a `fixed_by` commit, ground truth is the set of files that commit
//! touched — what an agent actually needs open to make the edit. Only one of the three planted
//! bugs has one. For the other two the corpus never fixed the bug, so ground truth falls back
//! to the file the planted anchor names, which is a weaker question: a package can reach the
//! buggy line and still omit the file where the repair goes. The rule used is recorded per bug
//! in the golden, so the weaker cases are visible rather than averaged in.
//!
//! ## Re-baselining
//!
//! ```text
//! NEXUS_REBASELINE=1 cargo test -p nexus-core --test debug_supply
//! git diff crates/nexus-core/tests/golden/debug_supply.json
//! ```
//!
//! The diff is the review. No threshold is asserted: a number chosen before the evidence
//! exists is the folklore `11-risks.md` R8 names. This records what happens and fails when it
//! changes.

use nexus_core::{Engine, Purpose, TaskRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The symptom sentence for each planted bug, by bug id and planting commit.
///
/// Keyed by `(fixture, commit id)` because `spring-payments` plants the same bug id twice:
/// once as the original defect and once as the regression that re-opens it.
const REQUESTS: &[(&str, &str, &str)] = &[
    // The idempotency check moved outside the transaction. A person sees the consequence in
    // a support ticket, not in the code: they know two charges happened, and nothing else.
    (
        "spring-payments",
        "c3",
        "a customer was charged twice for one order",
    ),
    // The Java field was renamed and the schema was not. The page renders NaN; the person
    // reporting it has never heard of the schema.
    (
        "next-storefront",
        "c3",
        "the order summary shows NaN where the amount should be",
    ),
    // The regression: a migration drops the unique index that made the fix work. The symptom
    // is identical to c3's, which is the point — nothing in the Java code changed.
    (
        "spring-payments",
        "c7",
        "duplicate charges are happening again after the last release",
    ),
];

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct BugSupply {
    fixture: String,
    planted_at: String,
    bug: String,
    request: String,
    /// `fix-commit` where the corpus records a fixing commit, `anchor` where it does not.
    ground_truth: String,
    /// The files ground truth asks for, sorted.
    wanted: Vec<String>,
    /// Those the package actually anchored on, with the rank at which each appeared.
    found: BTreeMap<String, usize>,
    items_included: usize,
    tokens_estimated: usize,
    intent: String,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The files a fix has to touch, named as they are called *at the moment the bug exists*.
///
/// Three corrections live in this function, and the first version of it had none of them.
///
/// It asked `git diff <plant>..<fixed>`, which is the cumulative diff over every commit
/// between the two. `spring-payments` renames the whole `mn.pay` package to `mn.payments` in
/// between, so that range named fourteen files, eleven of which do not exist at the commit
/// the harness checks out. A denominator full of paths no package could ever contain is not
/// a measurement.
///
/// So: the *fixing commit's own* files, then translated back through renames to the names
/// they had at the planting commit, then filtered to those that actually existed then. What
/// is left is what an agent would have to open, called what it is called when the bug is
/// reported.
fn ground_truth(
    repo: &Path,
    planted_sha: &str,
    fixed_sha: Option<&str>,
    anchor: Option<&str>,
) -> (String, Vec<String>) {
    match fixed_sha {
        Some(fixed) => {
            let touched = git(repo, &["show", "--name-only", "--format=", fixed]);

            // `R100\told\tnew` for every rename between the two commits.
            let renames = git(repo, &["diff", "-M", "--name-status", planted_sha, fixed]);
            let mut was_called: BTreeMap<&str, &str> = BTreeMap::new();
            for line in renames.lines() {
                let mut parts = line.split('\t');
                let status = parts.next().unwrap_or("");
                if !status.starts_with('R') {
                    continue;
                }
                if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
                    was_called.insert(new, old);
                }
            }

            let mut files: Vec<String> = touched
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|f| was_called.get(f).copied().unwrap_or(f).to_string())
                // A file the fix creates did not exist when the bug was reported, so no
                // package could have offered it and asking for it measures nothing.
                .filter(|f| {
                    Command::new("git")
                        .args(["cat-file", "-e", &format!("{planted_sha}:{f}")])
                        .current_dir(repo)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                })
                .collect();
            files.sort();
            files.dedup();
            ("fix-commit".to_string(), files)
        }
        None => {
            // `path:line`; the line is where the defect is, and the file is what a package
            // would have to reach for anyone to see it.
            let file = anchor
                .and_then(|a| a.rsplit_once(':').map(|(p, _)| p.to_string()))
                .expect("a bug with no fixing commit must at least name an anchor");
            ("anchor".to_string(), vec![file])
        }
    }
}

fn engine(root: &Path) -> Engine {
    let (mut e, _) = Engine::init(root, nexus_lang_pack::default_registry()).expect("init");
    e.scan().expect("scan");
    e
}

fn supply(repo: &Path, request_text: &str) -> (BTreeMap<String, usize>, usize, usize, String) {
    let e = engine(repo);
    let req = TaskRequest {
        text: request_text.to_string(),
        files: Vec::new(),
        symbols: Vec::new(),
        budget_tokens: nexus_core::context::TASK_BUDGET_TOKENS,
        // Declared, not derived: the harness knows this is a defect hunt, and #28 exists so
        // that knowledge does not have to survive a round trip through a verb table.
        purpose: Purpose::Debug,
        explain: false,
        carry_seeds: Vec::new(),
        recent: None,
    };
    let pkg = e.context(&req).expect("package");
    let mut ranks = BTreeMap::new();
    for (rank, item) in pkg.items.iter().enumerate() {
        ranks.entry(item.anchor.file.clone()).or_insert(rank + 1);
    }
    (
        ranks,
        pkg.items_included,
        pkg.tokens_estimated,
        pkg.intent
            .as_ref()
            .map(|i| i.intent.as_str().to_string())
            .unwrap_or_else(|| "none".into()),
    )
}

#[test]
fn the_debug_package_reaches_what_a_fix_would_touch() {
    let out_root = std::env::temp_dir().join(format!("nexus-supply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_root);
    std::fs::create_dir_all(&out_root).expect("mkdir");

    let specs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(nexus_fixtures::DEFAULT_SPEC_DIR);

    let mut got: Vec<BugSupply> = Vec::new();

    // Generated once per fixture, not once per bug: `spring-payments` plants two, and the
    // generator refuses to overwrite a repository it did not just create.
    let mut built: BTreeMap<&str, nexus_fixtures::Generated> = BTreeMap::new();

    for (fixture, commit_id, request_text) in REQUESTS {
        let generated = built.entry(fixture).or_insert_with(|| {
            let spec = nexus_fixtures::Spec::load(&specs.join(fixture)).expect("spec loads");
            nexus_fixtures::generate(
                &spec,
                &out_root,
                &nexus_fixtures::Options {
                    force: true,
                    emit_tasks: None,
                },
            )
            .expect("fixture generates")
        });

        let planting = generated
            .manifest
            .commits
            .iter()
            .find(|c| &c.id == commit_id)
            .unwrap_or_else(|| panic!("{fixture} has no commit {commit_id}"));
        let bug = planting
            .plants_bug
            .as_ref()
            .unwrap_or_else(|| panic!("{fixture}:{commit_id} plants no bug"));

        let fixed_sha = bug.fixed_by.as_ref().map(|id| {
            generated
                .manifest
                .commits
                .iter()
                .find(|c| &c.id == id)
                .unwrap_or_else(|| panic!("{fixture} has no commit {id}"))
                .sha
                .clone()
        });
        let (rule, wanted) = ground_truth(
            &generated.repo,
            &planting.sha,
            fixed_sha.as_deref(),
            bug.anchor.as_deref(),
        );

        // The package is built against the index as it stands at the moment the bug exists.
        // The index goes with it: two bugs in one fixture are two different commits, and a
        // baseline carried across them would answer for the wrong tree.
        git(&generated.repo, &["checkout", "-q", &planting.sha]);
        let _ = std::fs::remove_dir_all(generated.repo.join(".nexus"));
        let (ranks, included, tokens, intent) = supply(&generated.repo, request_text);

        let found: BTreeMap<String, usize> = wanted
            .iter()
            .filter_map(|f| ranks.get(f).map(|r| (f.clone(), *r)))
            .collect();

        got.push(BugSupply {
            fixture: (*fixture).to_string(),
            planted_at: (*commit_id).to_string(),
            bug: bug.id.clone(),
            request: (*request_text).to_string(),
            ground_truth: rule,
            wanted,
            found,
            items_included: included,
            tokens_estimated: tokens,
            intent,
        });
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("debug_supply.json");

    if std::env::var("NEXUS_REBASELINE").is_ok() {
        let body = serde_json::to_string_pretty(&got).expect("serialize");
        std::fs::write(&path, format!("{body}\n")).expect("write golden");
        let _ = std::fs::remove_dir_all(&out_root);
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no golden yet — record one:\n  NEXUS_REBASELINE=1 cargo test -p nexus-core \
             --test debug_supply"
        )
    });
    let want: Vec<BugSupply> = serde_json::from_str(&raw).expect("golden is valid JSON");
    let _ = std::fs::remove_dir_all(&out_root);

    assert_eq!(
        got, want,
        "debug supply moved.\n\nIf that was deliberate:\n  \
         NEXUS_REBASELINE=1 cargo test -p nexus-core --test debug_supply\n  \
         git diff crates/nexus-core/tests/golden/debug_supply.json\n\n\
         Read every line of that diff. A package that stops reaching a fix file is the \
         regression this exists to catch, and re-baselining without reading is how a golden \
         becomes a rubber stamp."
    );
}

/// The control arm: a request that names a symbol *does* reach its file.
///
/// Without this, the golden above is unfalsifiable. Three empty results look identical
/// whether the seeder is conservative or the harness is broken — the same trap Plan A's own
/// worst near-miss described, a green test asserting `0 == 0`. This proves the machinery
/// works, so the zeroes next door mean what they say.
#[test]
fn the_harness_finds_a_file_when_the_request_names_its_symbol() {
    let out_root = std::env::temp_dir().join(format!("nexus-supply-ctl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_root);
    std::fs::create_dir_all(&out_root).expect("mkdir");

    let specs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(nexus_fixtures::DEFAULT_SPEC_DIR);
    let spec = nexus_fixtures::Spec::load(&specs.join("spring-payments")).expect("spec loads");
    let generated = nexus_fixtures::generate(
        &spec,
        &out_root,
        &nexus_fixtures::Options {
            force: true,
            emit_tasks: None,
        },
    )
    .expect("fixture generates");

    let planting = generated
        .manifest
        .commits
        .iter()
        .find(|c| c.id == "c3")
        .expect("c3");
    git(&generated.repo, &["checkout", "-q", &planting.sha]);

    let (ranks, included, _, intent) = supply(&generated.repo, "PaymentService is charging twice");
    let _ = std::fs::remove_dir_all(&out_root);

    assert_eq!(intent, "debug");
    assert!(included > 0, "naming a symbol must select something");
    // Looked up exactly as the golden looks its files up, on a repository-relative path taken
    // from the fixture rather than from the package. A `contains("PaymentService")` here would
    // pass even if every path in the package were formatted differently from ground truth —
    // and that mismatch would empty every `found` next door while this still went green.
    assert!(
        ranks.contains_key("src/main/java/mn/pay/PaymentService.java"),
        "the named symbol's file must be present under the same path shape ground truth \
         uses, or the golden's zeroes are an artefact of formatting: {ranks:?}"
    );
}
