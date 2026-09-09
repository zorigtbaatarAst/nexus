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

use nexus_core::{Engine, Purpose, RankMode, TaskRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The join is by task id, never by order: `fixture.toml` lists tasks in corpus order and
/// the manifest lists them in the order `tokio_fixture.sh` happened to build them. This
/// fixture deliberately shuffles the manifest so an order-based join fails here rather than
/// silently scoring R1's package against R2's required sites.
#[test]
fn selftest_corpus_reader_joins_sites_to_start_states() {
    let corpus = r#"
[fixture]
name = "tokio"

[[task]]
id = "R1-x"
family = "R"
prompt = "a panic in debug builds"
required_sites = ["a/b.rs"]

[[task]]
id = "R2-y"
family = "R"
prompt = "invalid utf8 is not reported"
required_sites = ["c/d.rs", "c/e.rs"]
"#;
    let manifest = r#"{"commits":[
        {"id":"R2","sha":"bbbbbbb","branch":"bench/R2","task":"R2-y"},
        {"id":"R1","sha":"aaaaaaa","branch":"bench/R1","task":"R1-x"}]}"#;

    let got = tokio_tasks(corpus, manifest);

    assert_eq!(got.len(), 2);
    assert_eq!(got[0].id, "R1-x");
    assert_eq!(got[0].sha, "aaaaaaa");
    assert_eq!(got[0].prompt, "a panic in debug builds");
    assert_eq!(got[0].required_sites, vec!["a/b.rs".to_string()]);
    assert_eq!(got[1].id, "R2-y");
    assert_eq!(got[1].sha, "bbbbbbb");
    assert_eq!(
        got[1].required_sites,
        vec!["c/d.rs".to_string(), "c/e.rs".to_string()]
    );
}

/// A task the corpus describes but the clone never materialised must fail loudly. Silently
/// dropping it would measure four tasks and report them as five.
#[test]
#[should_panic(expected = "no start state")]
fn selftest_corpus_reader_refuses_a_task_with_no_start_state() {
    let corpus = r#"
[[task]]
id = "R1-x"
prompt = "a panic"
required_sites = ["a/b.rs"]
"#;
    let manifest = r#"{"commits":[]}"#;
    let _ = tokio_tasks(corpus, manifest);
}

/// One task of the tokio corpus, joined from the two files that describe it.
#[derive(Debug, Clone, PartialEq)]
struct TokioTask {
    id: String,
    prompt: String,
    required_sites: Vec<String>,
    sha: String,
}

#[derive(Deserialize)]
struct CorpusToml {
    task: Vec<CorpusTask>,
}

#[derive(Deserialize)]
struct CorpusTask {
    id: String,
    prompt: String,
    required_sites: Vec<String>,
}

#[derive(Deserialize)]
struct TokioManifest {
    commits: Vec<TokioManifestCommit>,
}

#[derive(Deserialize)]
struct TokioManifestCommit {
    sha: String,
    task: String,
}

fn recall_row(task: &str, recall: f64) -> SiteRecall {
    SiteRecall {
        task: task.to_string(),
        wanted: vec!["a/b.rs".to_string()],
        found: BTreeMap::new(),
        recall,
        target: 1.0,
        items_included: 0,
        tokens_estimated: 0,
        intent: "task".to_string(),
    }
}

/// Recall is `found / wanted`, rounded to three places so a float never makes a golden diff
/// unreadable, and `0.0` when a package reached none of the sites — not an error and not
/// absent. R2's whole value to this instrument is that it records a real zero.
#[test]
fn selftest_recall_is_found_over_wanted() {
    let wanted = vec!["a/b.rs".to_string(), "c/d.rs".to_string()];
    let mut ranks = BTreeMap::new();
    ranks.insert("a/b.rs".to_string(), 3usize);
    ranks.insert("z/unrelated.rs".to_string(), 1usize);

    let got = site_recall("R9-half", &wanted, &ranks, 7, 1234, "task");

    assert_eq!(got.recall, 0.5);
    assert_eq!(got.found.get("a/b.rs"), Some(&3));
    assert_eq!(
        got.found.len(),
        1,
        "an unrelated file is not a hit: {:?}",
        got.found
    );
    assert_eq!(got.wanted, wanted);
    assert_eq!(got.target, 1.0);
    assert_eq!(got.items_included, 7);
    assert_eq!(got.tokens_estimated, 1234);

    let none = site_recall("R9-none", &wanted, &BTreeMap::new(), 0, 0, "task");
    assert_eq!(none.recall, 0.0);
    assert!(none.found.is_empty());

    // Test rounding with non-terminating fractions to ensure the formula is correct.
    // 1 of 3 must round to 0.333, 2 of 3 must round to 0.667.
    let wanted_three = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
    let mut ranks_one = BTreeMap::new();
    ranks_one.insert("a.rs".to_string(), 1usize);
    let one_of_three = site_recall("R9-one-third", &wanted_three, &ranks_one, 0, 0, "task");
    assert_eq!(one_of_three.recall, 0.333, "1/3 must round to 0.333");

    let mut ranks_two = BTreeMap::new();
    ranks_two.insert("a.rs".to_string(), 1usize);
    ranks_two.insert("b.rs".to_string(), 2usize);
    let two_of_three = site_recall("R9-two-thirds", &wanted_three, &ranks_two, 0, 0, "task");
    assert_eq!(two_of_three.recall, 0.667, "2/3 must round to 0.667");

    // Test empty wanted guard: must return 0.0 and not panic or produce NaN.
    let empty_wanted = vec![];
    let mut ranks_some = BTreeMap::new();
    ranks_some.insert("a.rs".to_string(), 1usize);
    let empty_guard = site_recall("R9-empty", &empty_wanted, &ranks_some, 0, 0, "task");
    assert_eq!(empty_guard.recall, 0.0, "empty wanted must yield 0.0");
    assert!(
        empty_guard.found.is_empty(),
        "empty wanted means no matches"
    );
}

/// The ratchet: hold or improve, never fall. And a task that disappears from the run is a
/// corpus that shrank, which must fail rather than pass by vacuity.
#[test]
fn selftest_the_ratchet_holds_and_improves_but_never_falls() {
    let base = vec![recall_row("R1", 1.0), recall_row("R2", 0.0)];

    assert!(
        ratchet_failures(&[recall_row("R1", 1.0), recall_row("R2", 0.0)], &base).is_empty(),
        "holding must pass"
    );
    assert!(
        ratchet_failures(&[recall_row("R1", 1.0), recall_row("R2", 1.0)], &base).is_empty(),
        "improving must pass"
    );

    let fell = ratchet_failures(&[recall_row("R1", 0.5), recall_row("R2", 0.0)], &base);
    assert_eq!(fell.len(), 1, "{fell:?}");
    assert!(fell[0].contains("R1"), "{fell:?}");

    let vanished = ratchet_failures(&[recall_row("R1", 1.0)], &base);
    assert!(
        vanished.iter().any(|m| m.contains("R2")),
        "a task missing from the run must fail: {vanished:?}"
    );

    let unknown = ratchet_failures(&[recall_row("R3", 1.0)], &[]);
    assert!(
        unknown.iter().any(|m| m.contains("R3")),
        "a task with no baseline must fail rather than pass silently: {unknown:?}"
    );
}

/// Same task, same recall, different `required_sites`: must fail.
///
/// Without this, emptying `required_sites` in `fixture.toml` (or pointing it at a file the
/// package already contains) leaves a `0.0` baseline trivially satisfiable forever — the
/// ratchet would hold green while the ground truth, not the ranker, is what moved.
#[test]
fn selftest_the_ratchet_catches_a_moved_ground_truth() {
    let base = vec![recall_row("R2", 0.0)];

    let mut moved = recall_row("R2", 0.0);
    moved.wanted = vec!["different/file.rs".to_string()];

    let failures = ratchet_failures(&[moved], &base);
    assert!(
        failures
            .iter()
            .any(|m| m.contains("R2") && m.contains("required_sites changed")),
        "a changed wanted set at an unchanged recall must fail: {failures:?}"
    );
}

/// Recall held; the answer sank and the package tripled. That must fail.
///
/// The numbers are C2's, the change this guard was written for: R5's answer moved rank 3 -> 37
/// and R2's package 3 items -> 34, both at unchanged recall, and the recall-only ratchet passed
/// them. The jitter case is asserted in the same test, because a guard that fires on 3 -> 4 is
/// a guard someone re-baselines past without reading.
#[test]
fn selftest_the_ratchet_catches_a_sunk_answer_and_a_ballooned_package() {
    let row = |task: &str, rank: usize, items: usize, tokens: usize| {
        let mut r = recall_row(task, 1.0);
        r.found = BTreeMap::from([("a/b.rs".to_string(), rank)]);
        r.items_included = items;
        r.tokens_estimated = tokens;
        r
    };

    let base = vec![row("R5", 3, 49, 3934), row("R2", 4, 3, 423)];

    let sank = ratchet_failures(&[row("R5", 37, 49, 3934), row("R2", 4, 3, 423)], &base);
    assert!(
        sank.iter().any(|m| m.contains("R5") && m.contains("sank")),
        "an answer falling rank 3 -> 37 at unchanged recall must fail: {sank:?}"
    );

    let grew = ratchet_failures(&[row("R5", 3, 49, 3934), row("R2", 4, 34, 2883)], &base);
    assert!(
        grew.iter().any(|m| m.contains("R2") && m.contains("items")),
        "a package growing 3 -> 34 items must fail: {grew:?}"
    );
    assert!(
        grew.iter().any(|m| m.contains("R2") && m.contains("tokens")),
        "and so must 423 -> 2883 tokens: {grew:?}"
    );

    // Holding, improving, and tie-order jitter all stay green.
    assert!(
        ratchet_failures(&[row("R5", 2, 40, 3000), row("R2", 5, 4, 500)], &base).is_empty(),
        "improving, and one slot of jitter on a small number, must pass"
    );
}

/// Join the corpus description to the materialised clone.
///
/// Both halves are load-bearing and neither is sufficient: `fixture.toml` carries
/// `required_sites` — the ground truth — and no start-state sha; the manifest carries the sha
/// and no sites. Joined by task id rather than by position, because the two files are written
/// by different tools and nothing makes them agree on order.
fn tokio_tasks(corpus_toml: &str, manifest_json: &str) -> Vec<TokioTask> {
    let corpus: CorpusToml = toml::from_str(corpus_toml).expect("corpus fixture.toml parses");
    let manifest: TokioManifest =
        serde_json::from_str(manifest_json).expect("tokio.manifest.json parses");
    corpus
        .task
        .into_iter()
        .map(|t| {
            let sha = manifest
                .commits
                .iter()
                .find(|c| c.task == t.id)
                .unwrap_or_else(|| {
                    panic!(
                        "tokio.manifest.json has no start state for {} — the corpus describes \
                         a task the clone never materialised. Rebuild it:\n  make tokio-fixture",
                        t.id
                    )
                })
                .sha
                .clone();
            TokioTask {
                id: t.id,
                prompt: t.prompt,
                required_sites: t.required_sites,
                sha,
            }
        })
        .collect()
}

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

/// What the package reached, of what the task requires.
///
/// `target` is recorded per row rather than assumed, so a task sitting below perfect stays
/// visible in the golden instead of being normalised into "what we score".
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct SiteRecall {
    task: String,
    /// The files `required_sites` asks for, in corpus order.
    wanted: Vec<String>,
    /// Those the package anchored on, with the rank at which each appeared.
    found: BTreeMap<String, usize>,
    recall: f64,
    target: f64,
    items_included: usize,
    tokens_estimated: usize,
    intent: String,
}

fn site_recall(
    task: &str,
    wanted: &[String],
    ranks: &BTreeMap<String, usize>,
    items_included: usize,
    tokens_estimated: usize,
    intent: &str,
) -> SiteRecall {
    let found: BTreeMap<String, usize> = wanted
        .iter()
        .filter_map(|f| ranks.get(f).map(|r| (f.clone(), *r)))
        .collect();
    // Rounded to three places: an unrounded ratio puts 0.6666666666666666 in a golden and
    // turns every diff into a reading exercise.
    let recall = if wanted.is_empty() {
        0.0
    } else {
        ((found.len() as f64 / wanted.len() as f64) * 1000.0).round() / 1000.0
    };
    SiteRecall {
        task: task.to_string(),
        wanted: wanted.to_vec(),
        found,
        recall,
        target: 1.0,
        items_included,
        tokens_estimated,
        intent: intent.to_string(),
    }
}

/// How much worse a recorded number may get before the ratchet calls it a regression.
///
/// A ratio with an absolute floor, because both numbers this guards start small: rank 3 and a
/// 3-item package are one tie-break away from rank 4 and 4 items, and a guard that fired on
/// that would be re-baselined into meaninglessness inside a week.
///
/// `1.5` is not a number picked before the evidence. It is read off the change that exposed
/// the gap. C2 (`225cea1`, since reverted) moved R1's correct answer from rank 4 to 11 and
/// R5's from 3 to 37, and grew three of five packages — R1 1.7×, R2 6.8×, R3 2.8× — while
/// leaving the other two at 1.3× and 1.4×. Half again is the line those two groups fall
/// either side of.
const RATCHET_SLACK: f64 = 1.5;
/// The floor beneath which a ratio means nothing: 3 -> 4 is tie order, not a regression.
const RATCHET_FLOOR: usize = 3;

/// Bigger by half again *and* by more than a few. Both, so neither small-number jitter nor a
/// large proportional drift alone counts.
fn materially_worse(got: usize, base: usize) -> bool {
    got as f64 > base as f64 * RATCHET_SLACK && got > base + RATCHET_FLOOR
}

/// The ratchet: a task may improve or hold, never fall.
///
/// This is deliberately *not* a threshold on what recall should be. `debug_supply.rs` argues
/// against thresholds because a number chosen before the evidence exists is folklore, and that
/// argument is right and is adopted here. A ratchet asserts nothing about the level — only
/// that a ranking change may not quietly destroy what the last one earned.
///
/// **Recall alone is not enough, and that gap shipped a regression green.** Recall is a set
/// membership test: a correct answer at rank 37 is as "found" as one at rank 3, and a package
/// of 34 items scores the same 0.0 as one of 3. C2 pushed R5's answer from rank 3 to 37 and
/// grew R2's package from 3 items to 34 with recall unmoved, and passed four reviews as green
/// because nothing here looked at the two other numbers the golden already records. So rank
/// and package size are guarded too, on the same hold-or-improve terms: a correct answer may
/// not sink and a package may not balloon unless someone re-baselines on purpose.
fn ratchet_failures(got: &[SiteRecall], want: &[SiteRecall]) -> Vec<String> {
    let baseline: BTreeMap<&str, &SiteRecall> = want.iter().map(|r| (r.task.as_str(), r)).collect();
    let mut failures = Vec::new();
    for g in got {
        let Some(b) = baseline.get(g.task.as_str()) else {
            failures.push(format!(
                "{}: no baseline row — record one with NEXUS_REBASELINE=1",
                g.task
            ));
            continue;
        };
        // The ground truth moving invalidates every other comparison below it, so it is the
        // one check that stops the row rather than adding to it.
        if g.wanted != b.wanted {
            failures.push(format!(
                "{}: required_sites changed {:?} -> {:?} — the ground truth moved, not the ranker",
                g.task, b.wanted, g.wanted
            ));
            continue;
        }
        if g.recall < b.recall {
            failures.push(format!(
                "{}: recall fell {} -> {}. wanted {:?}, found {:?}",
                g.task,
                b.recall,
                g.recall,
                g.wanted,
                g.found.keys().collect::<Vec<_>>()
            ));
        }
        for (file, rank) in &g.found {
            let Some(was) = b.found.get(file) else { continue };
            if materially_worse(*rank, *was) {
                failures.push(format!(
                    "{}: {file} sank from rank {was} to {rank}. Recall held, but nobody reads \
                     that far down a package — re-baseline only if the sink is deliberate",
                    g.task
                ));
            }
        }
        // Both size numbers, not one. The density budget caps `tokens_estimated`, so once a
        // package is at budget `items_included` is the only one of the two still free to grow
        // — R5 went 49 items to 69 under C2 while its token count did not move at all.
        if materially_worse(g.items_included, b.items_included) {
            failures.push(format!(
                "{}: the package grew from {} items to {} — re-baseline only if that is \
                 deliberate",
                g.task, b.items_included, g.items_included
            ));
        }
        if materially_worse(g.tokens_estimated, b.tokens_estimated) {
            failures.push(format!(
                "{}: the package grew from {} tokens to {} — re-baseline only if that is \
                 deliberate",
                g.task, b.tokens_estimated, g.tokens_estimated
            ));
        }
    }
    for b in want {
        if !got.iter().any(|g| g.task == b.task) {
            failures.push(format!(
                "{}: in the baseline but not in this run — the corpus shrank",
                b.task
            ));
        }
    }
    failures
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

fn supply(
    repo: &Path,
    request_text: &str,
    purpose: Purpose,
) -> (BTreeMap<String, usize>, usize, usize, String) {
    let e = engine(repo);
    let req = TaskRequest {
        text: request_text.to_string(),
        files: Vec::new(),
        symbols: Vec::new(),
        budget_tokens: nexus_core::context::TASK_BUDGET_TOKENS,
        // Declared by the caller, because the two corpora are asking different questions.
        // The generated fixtures declare Debug: the harness knows it is a defect hunt and
        // #28 exists so that knowledge need not survive a round trip through a verb table.
        // The tokio cases declare Task, because that is what A1's hook actually ran — the
        // CLI defaults `declared_purpose` to Purpose::Task when no --purpose is given
        // (crates/nexus-cli/src/main.rs:927), and intent is upstream of every ranker weight,
        // so pinning Debug there would measure a pipeline the benchmark never ran.
        purpose,
        rank: RankMode::default(),
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
        let (ranks, included, tokens, intent) =
            supply(&generated.repo, request_text, Purpose::Debug);

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

    let (ranks, included, _, intent) = supply(
        &generated.repo,
        "PaymentService is charging twice",
        Purpose::Debug,
    );
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

/// The two tokio tests check out different commits in one shared clone, so they cannot run
/// concurrently — cargo's default runner is multi-threaded and two `git checkout`s race on
/// the index lock. Serialised here rather than with `--test-threads=1` on a make target,
/// because `make check` runs the whole workspace multi-threaded and would flake for anyone
/// who has built the corpus.
static TOKIO_CLONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Where `scripts/eval/tokio_fixture.sh` materialises the corpus, and where the committed
/// description of it lives.
fn tokio_paths() -> (PathBuf, PathBuf, PathBuf) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    (
        root.join("target/fixtures/tokio"),
        root.join("target/fixtures/tokio.manifest.json"),
        root.join("tests/fixtures/corpora/tokio/fixture.toml"),
    )
}

/// Present, or a loud refusal when this is the run that gates.
///
/// `cargo test --workspace` reaches this test on machines that have never built the corpus,
/// and a plain skip there is the silently-green check the spec rejects. So absence is fatal
/// exactly when `NEXUS_TIER1_REQUIRED` is set — which `make tier1-retrieval` and CI set, and
/// `make check` does not. The skip can therefore never be the result of the gating run.
fn tokio_corpus_or_skip() -> Option<(PathBuf, Vec<TokioTask>)> {
    let (repo, manifest, corpus) = tokio_paths();
    let required = std::env::var("NEXUS_TIER1_REQUIRED").is_ok();
    if !repo.join(".git").is_dir() {
        assert!(
            !required,
            "NEXUS_TIER1_REQUIRED is set but {} is not a git repository.\n\
             Build the corpus first:\n  make tokio-fixture",
            repo.display()
        );
        eprintln!(
            "skipping the tokio retrieval oracle: {} is absent (make tokio-fixture).\n\
             This is not the run that gates — `make tier1-retrieval` is.",
            repo.display()
        );
        return None;
    }
    let manifest_json = std::fs::read_to_string(&manifest).unwrap_or_else(|e| {
        panic!(
            "{}: {e} — rebuild with make tokio-fixture",
            manifest.display()
        )
    });
    let corpus_toml =
        std::fs::read_to_string(&corpus).unwrap_or_else(|e| panic!("{}: {e}", corpus.display()));
    Some((repo, tokio_tasks(&corpus_toml, &manifest_json)))
}

/// Does the package point at the files the fix has to touch, on a corpus large enough for
/// the question to mean anything?
///
/// The generated-fixture golden next door asks this of 6–8 kB repositories, which
/// `docs/eval/tier2-corpus-verdict.md` established a bare agent simply reads in full. tokio
/// is ~800 files and ~165 KLOC per start state, and `required_sites` is curated per task and
/// is the same ground truth `scripts/eval/grade.sh` scores the agent's diff against.
#[test]
fn the_task_package_reaches_the_sites_a_fix_must_touch() {
    // Poisoning is expected, not exceptional: this test panics by design until Task 5 records
    // a baseline, and that panic must not stop the control test next door from running.
    let _guard = TOKIO_CLONE.lock().unwrap_or_else(|e| e.into_inner());

    let Some((repo, tasks)) = tokio_corpus_or_skip() else {
        return;
    };
    assert!(!tasks.is_empty(), "the tokio corpus describes no tasks");

    let mut got: Vec<SiteRecall> = Vec::new();
    for t in &tasks {
        // The package is built against the index as it stands at the start state, and the
        // index goes with it: five tasks are five different trees, and an index carried
        // across them would answer for the wrong one.
        git(&repo, &["checkout", "-q", &t.sha]);
        let _ = std::fs::remove_dir_all(repo.join(".nexus"));
        let (ranks, included, tokens, intent) = supply(&repo, &t.prompt, Purpose::Task);
        got.push(site_recall(
            &t.id,
            &t.required_sites,
            &ranks,
            included,
            tokens,
            &intent,
        ));
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("retrieval_tokio.json");

    if std::env::var("NEXUS_REBASELINE").is_ok() {
        assert!(
            std::env::var("NEXUS_TIER1_REQUIRED").is_err(),
            "refusing to re-baseline the gating run: NEXUS_REBASELINE and NEXUS_TIER1_REQUIRED are both set"
        );
        let body = serde_json::to_string_pretty(&got).expect("serialize");
        std::fs::write(&path, format!("{body}\n")).expect("write golden");
        return;
    }

    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("no baseline yet — record one:\n  make tier1-rebaseline"));
    let want: Vec<SiteRecall> = serde_json::from_str(&raw).expect("baseline is valid JSON");

    let failures = ratchet_failures(&got, &want);
    assert!(
        failures.is_empty(),
        "retrieval recall regressed:\n  {}\n\nIf a drop was deliberate, it needs an argument \
         in the commit message, not a re-baseline. If an improvement is what you meant:\n  \
         make tier1-rebaseline\n  \
         git diff crates/nexus-core/tests/golden/retrieval_tokio.json\n\n\
         Read every line of that diff.",
        failures.join("\n  ")
    );
}

/// The control arm: a request naming a required site's own type reaches that file.
///
/// Checked out against **R5**, not R1: R1 already scores 1.0, so a control planted there
/// proves nothing about the four rows still at the floor. R5 is one of them — recall 0.0 —
/// so this is the proof that the machinery can reach a required site at all on a tree where
/// the real task fails, which is the only tree where the question is worth asking. Without
/// this, a zero in the baseline is unfalsifiable: a package path formatted differently from
/// `required_sites` would empty every `found` in the golden, and the recorded zeroes would be
/// an artefact of formatting rather than a finding about retrieval — the same trap the
/// generated-fixture control next door exists to close.
#[test]
fn the_tokio_harness_finds_a_site_when_the_request_names_its_type() {
    let _guard = TOKIO_CLONE.lock().unwrap_or_else(|e| e.into_inner());

    let Some((repo, tasks)) = tokio_corpus_or_skip() else {
        return;
    };
    let r5 = tasks
        .iter()
        .find(|t| t.id.starts_with("R5-"))
        .expect("the corpus has an R5 task");

    git(&repo, &["checkout", "-q", &r5.sha]);
    let _ = std::fs::remove_dir_all(repo.join(".nexus"));
    let (ranks, included, _, _) = supply(
        &repo,
        "BatchSemaphore forgets a permit after close",
        Purpose::Task,
    );

    assert!(included > 0, "naming a type must select something");
    assert!(
        ranks.contains_key("tokio/src/sync/batch_semaphore.rs"),
        "the named type's file must be present under the same path shape required_sites \
         uses, or every zero in the baseline is a formatting artefact: {:?}",
        ranks.keys().collect::<Vec<_>>()
    );
}
