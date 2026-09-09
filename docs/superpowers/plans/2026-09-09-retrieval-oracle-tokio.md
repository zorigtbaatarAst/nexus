# Retrieval Oracle on tokio — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make "did the context package point at the files a fix must touch" a free, deterministic, ratcheted check on the tokio corpus, so the defect that cost $28.88 to find is found in seconds.

**Architecture:** Extend the existing `crates/nexus-core/tests/debug_supply.rs` rather than build a second harness. It already asks this question, already uses the golden-plus-rebaseline ritual, and already owns the `supply()` and `git()` helpers whose duplication would be the drift risk. New: a corpus reader joining `fixture.toml` to `tokio.manifest.json`, a site-recall score, a ratchet comparison, a `make tier1-retrieval` target, and a CI step.

**Tech Stack:** Rust 1.82+, `serde` / `serde_json` / `toml` (all already regular dependencies of `nexus-core`), `nexus_lang_pack::default_registry()`, GNU Make, GitHub Actions.

**Spec:** [`docs/superpowers/specs/2026-09-09-retrieval-oracle-tokio-design.md`](../specs/2026-09-09-retrieval-oracle-tokio-design.md)

## Global Constraints

- **Rust 1.82+**; CI runs with `RUSTFLAGS=-D warnings`, so any warning fails the build.
- **`cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass** — they are `make check` and they are what CI runs.
- **Budget parity with the benchmark's A1 arm is exact and must stay so:** `budget_tokens: nexus_core::context::TASK_BUDGET_TOKENS`, which is `4000` (`crates/nexus-core/src/context/mod.rs:26`), and `rank: RankMode::default()`.
- **Purpose for tokio cases is `Purpose::Task`, not `Purpose::Debug`.** A1's hook passed no `--purpose`, and the CLI's `declared_purpose` defaults to `nexus_core::Purpose::Task` (`crates/nexus-cli/src/main.rs:926`). The existing generated-fixture cases keep `Purpose::Debug` — that is deliberate and documented in their own comment; do not change them.
- **The golden path is `crates/nexus-core/tests/golden/retrieval_tokio.json`**, beside the two goldens already in that directory.
- **No new dependency may be added to `nexus-core`.** `serde`, `serde_json` and `toml` are already there; anything else is out of scope.
- **`make check` must not require the tokio clone.** It stays fast and offline.

---

### Task 1: Corpus reader — join `fixture.toml` to `tokio.manifest.json`

The corpus is described in two files on purpose. `tests/fixtures/corpora/tokio/fixture.toml` is committed and says what the tasks *are* (`id`, `prompt`, `required_sites`). `target/fixtures/tokio.manifest.json` is written by `scripts/eval/tokio_fixture.sh` and records what was actually materialised (`task` → `sha`). The join must be by task id, never by order, because nothing guarantees the two files agree on order.

**Files:**
- Modify: `crates/nexus-core/tests/debug_supply.rs` (add to the existing file, near the top after the current `use` block at line 40)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `struct TokioTask { id: String, prompt: String, required_sites: Vec<String>, sha: String }` and `fn tokio_tasks(corpus_toml: &str, manifest_json: &str) -> Vec<TokioTask>`, used by Task 3.

- [ ] **Step 1: Write the failing test**

Add to `crates/nexus-core/tests/debug_supply.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p nexus-core --test debug_supply selftest_corpus_reader`

Expected: FAIL to compile, with `cannot find function 'tokio_tasks' in this scope`.

- [ ] **Step 3: Write the minimal implementation**

Add to `crates/nexus-core/tests/debug_supply.rs`, after the `use` block:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p nexus-core --test debug_supply selftest_corpus_reader`

Expected: PASS, 2 tests.

- [ ] **Step 5: Check formatting and lint**

Run: `cargo fmt --all && cargo clippy -p nexus-core --all-targets -- -D warnings`

Expected: no output from clippy.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-core/tests/debug_supply.rs
git commit -m "test(retrieval): read the tokio corpus, joining sites to start states by id"
```

---

### Task 2: Site recall and the ratchet

Two pure functions, tested without touching a repository. The ratchet is the part that must be exactly right: it asserts nothing about what recall *should* be, only that it may not fall below what was recorded. That distinction is what lets R2 go into the baseline at its true `0.0` and still leave the check green today.

**Files:**
- Modify: `crates/nexus-core/tests/debug_supply.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `struct SiteRecall` (serialisable — it is the golden's row type) and `fn ratchet_failures(got: &[SiteRecall], want: &[SiteRecall]) -> Vec<String>`, both used by Task 3.

- [ ] **Step 1: Write the failing test**

Add to `crates/nexus-core/tests/debug_supply.rs`:

```rust
#[cfg(test)]
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
    assert_eq!(got.found.len(), 1, "an unrelated file is not a hit: {:?}", got.found);
    assert_eq!(got.wanted, wanted);
    assert_eq!(got.target, 1.0);
    assert_eq!(got.items_included, 7);
    assert_eq!(got.tokens_estimated, 1234);

    let none = site_recall("R9-none", &wanted, &BTreeMap::new(), 0, 0, "task");
    assert_eq!(none.recall, 0.0);
    assert!(none.found.is_empty());
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p nexus-core --test debug_supply selftest_`

Expected: FAIL to compile — `cannot find function 'site_recall'`, `cannot find function 'ratchet_failures'`, `cannot find type 'SiteRecall'`.

- [ ] **Step 3: Write the minimal implementation**

Add to `crates/nexus-core/tests/debug_supply.rs`:

```rust
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

/// The ratchet: a task may improve or hold, never fall.
///
/// This is deliberately *not* a threshold. `debug_supply.rs` argues against thresholds
/// because a number chosen before the evidence exists is folklore, and that argument is
/// right and is adopted here. A ratchet asserts nothing about what recall should be — only
/// that a ranking change may not quietly destroy it.
fn ratchet_failures(got: &[SiteRecall], want: &[SiteRecall]) -> Vec<String> {
    let baseline: BTreeMap<&str, &SiteRecall> =
        want.iter().map(|r| (r.task.as_str(), r)).collect();
    let mut failures = Vec::new();
    for g in got {
        match baseline.get(g.task.as_str()) {
            None => failures.push(format!(
                "{}: no baseline row — record one with NEXUS_REBASELINE=1",
                g.task
            )),
            Some(b) if g.recall < b.recall => failures.push(format!(
                "{}: recall fell {} -> {}. wanted {:?}, found {:?}",
                g.task,
                b.recall,
                g.recall,
                g.wanted,
                g.found.keys().collect::<Vec<_>>()
            )),
            Some(_) => {}
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p nexus-core --test debug_supply selftest_`

Expected: PASS, 4 tests (the two from Task 1 and the two here).

- [ ] **Step 5: Check formatting and lint**

Run: `cargo fmt --all && cargo clippy -p nexus-core --all-targets -- -D warnings`

Expected: no output from clippy.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-core/tests/debug_supply.rs
git commit -m "test(retrieval): site recall, and a ratchet that holds or improves but never falls"
```

---

### Task 3: The measurement, gated so a skip can never be the gating result

This is where the two pure pieces meet a real repository. Three things must be right: the purpose must match A1's (`Purpose::Task`), the skip must be impossible under `NEXUS_TIER1_REQUIRED=1`, and a control test must prove the machinery can find anything at all — without it, R2's zero is indistinguishable from a path-shape bug that empties every row.

**Files:**
- Modify: `crates/nexus-core/tests/debug_supply.rs` — change `supply()` at line ~181 to take a `Purpose`, update its two existing callers, add the tokio test and its control.

**Interfaces:**
- Consumes: `TokioTask` / `tokio_tasks` (Task 1), `SiteRecall` / `site_recall` / `ratchet_failures` (Task 2).
- Produces: `crates/nexus-core/tests/golden/retrieval_tokio.json`, read by Task 5.

- [ ] **Step 1: Parameterise `supply()` by purpose**

In `crates/nexus-core/tests/debug_supply.rs`, change the signature and the `purpose` field:

```rust
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
        // (crates/nexus-cli/src/main.rs:926), and intent is upstream of every ranker weight,
        // so pinning Debug there would measure a pipeline the benchmark never ran.
        purpose,
        rank: RankMode::default(),
        explain: false,
        carry_seeds: Vec::new(),
        recent: None,
    };
```

Update both existing call sites to pass `Purpose::Debug`:

```rust
let (ranks, included, tokens, intent) = supply(&generated.repo, request_text, Purpose::Debug);
```

```rust
let (ranks, included, _, intent) =
    supply(&generated.repo, "PaymentService is charging twice", Purpose::Debug);
```

- [ ] **Step 2: Run the existing tests to prove nothing moved**

Run: `cargo test -p nexus-core --test debug_supply`

Expected: PASS. The two pre-existing tests must be unchanged in behaviour — the generated-fixture golden must not move. If `debug_supply.json` now differs, the parameterisation changed a default and must be fixed rather than re-baselined.

- [ ] **Step 3: Write the failing tokio test and its control**

Add to `crates/nexus-core/tests/debug_supply.rs`:

```rust
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
    let manifest_json = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("{}: {e} — rebuild with make tokio-fixture", manifest.display()));
    let corpus_toml = std::fs::read_to_string(&corpus)
        .unwrap_or_else(|e| panic!("{}: {e}", corpus.display()));
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
        let body = serde_json::to_string_pretty(&got).expect("serialize");
        std::fs::write(&path, format!("{body}\n")).expect("write golden");
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no baseline yet — record one:\n  NEXUS_REBASELINE=1 make tier1-retrieval"
        )
    });
    let want: Vec<SiteRecall> = serde_json::from_str(&raw).expect("baseline is valid JSON");

    let failures = ratchet_failures(&got, &want);
    assert!(
        failures.is_empty(),
        "retrieval recall regressed:\n  {}\n\nIf a drop was deliberate, it needs an argument \
         in the commit message, not a re-baseline. If an improvement is what you meant:\n  \
         NEXUS_REBASELINE=1 make tier1-retrieval\n  \
         git diff crates/nexus-core/tests/golden/retrieval_tokio.json\n\n\
         Read every line of that diff.",
        failures.join("\n  ")
    );
}

/// The control arm: a request naming a required site's own type reaches that file.
///
/// Without this, a zero in the baseline is unfalsifiable. A package path formatted
/// differently from `required_sites` would empty every `found` in the golden, and the
/// recorded zeroes would be an artefact of formatting rather than a finding about
/// retrieval — the same trap the generated-fixture control next door exists to close.
#[test]
fn the_tokio_harness_finds_a_site_when_the_request_names_its_type() {
    let Some((repo, tasks)) = tokio_corpus_or_skip() else {
        return;
    };
    let r1 = tasks
        .iter()
        .find(|t| t.id.starts_with("R1-"))
        .expect("the corpus has an R1 task");

    git(&repo, &["checkout", "-q", &r1.sha]);
    let _ = std::fs::remove_dir_all(repo.join(".nexus"));
    let (ranks, included, _, _) = supply(
        &repo,
        "StreamMap size_hint overflows when summing child hints",
        Purpose::Task,
    );

    assert!(included > 0, "naming a type must select something");
    assert!(
        ranks.contains_key("tokio-stream/src/stream_map.rs"),
        "the named type's file must be present under the same path shape required_sites \
         uses, or every zero in the baseline is a formatting artefact: {:?}",
        ranks.keys().collect::<Vec<_>>()
    );
}
```

- [ ] **Step 4: Run the tokio tests without the corpus, to prove the skip is honest**

Run: `cargo test -p nexus-core --test debug_supply the_task_package_reaches -- --nocapture`

Expected: PASS with the skip message on stderr **if** `target/fixtures/tokio` is absent; PASS or a missing-baseline panic if it is present. Either is correct here — Step 5 is what proves the gate.

- [ ] **Step 5: Prove absence is fatal when it is the gating run**

Run: `NEXUS_TIER1_REQUIRED=1 cargo test -p nexus-core --test debug_supply the_task_package_reaches`

Expected, when `target/fixtures/tokio` is absent: FAIL with `NEXUS_TIER1_REQUIRED is set but ... make tokio-fixture`. If the corpus is already present on your machine, temporarily rename it to see this, then rename it back:

```bash
mv target/fixtures/tokio target/fixtures/tokio.away
NEXUS_TIER1_REQUIRED=1 cargo test -p nexus-core --test debug_supply the_task_package_reaches; \
  mv target/fixtures/tokio.away target/fixtures/tokio
```

- [ ] **Step 6: Check formatting and lint**

Run: `cargo fmt --all && cargo clippy -p nexus-core --all-targets -- -D warnings`

Expected: no output from clippy.

- [ ] **Step 7: Commit**

```bash
git add crates/nexus-core/tests/debug_supply.rs
git commit -m "test(retrieval): score the tokio package against required_sites, at A1's purpose"
```

---

### Task 4: `make tier1-retrieval` and the CI step

The self-tests run first and separately, so the scoring logic is proven before it grades anything — the same ordering `scripts/eval/sweep.sh` uses when it gates on `scripts/eval/test_grade.sh` before spending money.

**Files:**
- Modify: `Makefile` (add target; add to the `.PHONY` list at line 5)
- Modify: `.github/workflows/ci.yml` (add a step and a cache, after the existing smoke-test step)

**Interfaces:**
- Consumes: the test names from Tasks 1–3 (`selftest_` prefix, and the two tokio tests).
- Produces: `make tier1-retrieval`, used by Task 5 and by CI.

- [ ] **Step 1: Add the Makefile target**

In `Makefile`, add to the `.PHONY` list on line 5 (append `tier1-retrieval` to the existing names), then add after the `tokio-fixture` target (line 71–72):

```make
# The retrieval oracle: does the context package point at the files a fix must touch?
# docs/superpowers/specs/2026-09-09-retrieval-oracle-tokio-design.md.
#
# Not part of `make check`, which must stay fast and needs no network — this needs the tokio
# clone. This IS the run that gates: NEXUS_TIER1_REQUIRED makes an absent corpus fatal, so a
# skip can never be reported as a pass.
#
# Two invocations, self-tests first: the scoring logic is proven before it grades anything,
# the same order in which sweep.sh gates on test_grade.sh before spending money.
tier1-retrieval: tokio-fixture
	cargo test -p nexus-core --test debug_supply selftest_
	NEXUS_TIER1_REQUIRED=1 cargo test -p nexus-core --test debug_supply
```

- [ ] **Step 2: Run it**

Run: `make tier1-retrieval`

Expected: the first invocation passes 4 self-tests. The second clones tokio if needed, then panics with `no baseline yet — record one: NEXUS_REBASELINE=1 make tier1-retrieval`. That panic is correct at this point — Task 5 records the baseline.

- [ ] **Step 3: Add the CI step**

In `.github/workflows/ci.yml`, after the `Smoke-test against a real repository` step, add:

```yaml
      # The tokio clone is the expensive part and tokio_fixture.sh reuses an existing one, so
      # cache it. Keyed on the corpus description: when the tasks change, the replayed start
      # states change and a stale clone would be scored against the wrong commits.
      - name: Cache the tokio corpus
        uses: actions/cache@v4
        with:
          path: target/fixtures/tokio
          key: tokio-corpus-${{ hashFiles('tests/fixtures/corpora/tokio/fixture.toml', 'scripts/eval/tokio_fixture.sh') }}

      # The retrieval oracle: free, deterministic, and the question run 20260907T093144Z
      # answered for $28.88. Not in `make check` because it needs the corpus above.
      - name: Retrieval oracle on tokio
        run: make tier1-retrieval
```

- [ ] **Step 4: Verify the workflow is valid YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml parses')"`

Expected: `ci.yml parses`

- [ ] **Step 5: Commit**

```bash
git add Makefile .github/workflows/ci.yml
git commit -m "build(eval): make tier1-retrieval, and run the retrieval oracle in CI"
```

---

### Task 5: Record the baseline, and verify it reproduces the sweep's finding

This is the acceptance test for the whole instrument. `R2-lines-codec-invalid-utf8` must record `recall: 0.0` — the sweep's package for that prompt named `tokio/src/time/error.rs` and `tokio/tests/test_clock.rs` and nothing from `tokio-util/src/codec/`. If it scores above zero, the parity claimed in the spec's §6 is wrong somewhere and must be found before the baseline is trusted.

**Files:**
- Create: `crates/nexus-core/tests/golden/retrieval_tokio.json`

**Interfaces:**
- Consumes: `make tier1-retrieval` (Task 4).
- Produces: the committed baseline every later ranking change is ratcheted against.

- [ ] **Step 1: Record the baseline**

Run: `NEXUS_REBASELINE=1 make tier1-retrieval`

Expected: `crates/nexus-core/tests/golden/retrieval_tokio.json` is written with five rows.

- [ ] **Step 2: Read it before trusting it**

Run: `cat crates/nexus-core/tests/golden/retrieval_tokio.json`

Check, and do not skip this — the diff is the review:
- Five rows, one per task, ids matching `tests/fixtures/corpora/tokio/fixture.toml`.
- Every row's `wanted` equals that task's `required_sites`.
- Every row's `intent` is what the classifier produced for a `Purpose::Task` request; if any row reads `none`, intent classification failed and the numbers below it are not meaningful.
- `R2-lines-codec-invalid-utf8` has `recall: 0.0` and an empty `found`.

- [ ] **Step 3: Verify the control test passes**

Run: `NEXUS_TIER1_REQUIRED=1 cargo test -p nexus-core --test debug_supply the_tokio_harness_finds_a_site`

Expected: PASS. **If this fails while R2 records 0.0, the zeroes are a path-formatting artefact and not a finding** — stop and fix the path shape before committing the baseline.

- [ ] **Step 4: Verify the ratchet actually bites**

Hand-edit `crates/nexus-core/tests/golden/retrieval_tokio.json` to raise one task's `recall` above what was measured (for example set R1's to `1.0` if it recorded lower), then run:

Run: `make tier1-retrieval`

Expected: FAIL with `retrieval recall regressed: R1-...: recall fell 1.0 -> ...`. Then restore the file:

```bash
git checkout crates/nexus-core/tests/golden/retrieval_tokio.json 2>/dev/null || \
  NEXUS_REBASELINE=1 make tier1-retrieval
```

- [ ] **Step 5: Run the whole check to prove nothing else moved**

Run: `make check`

Expected: PASS, and the tokio tests skip with their message — `make check` is not the gating run.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-core/tests/golden/retrieval_tokio.json
git commit -m "test(retrieval): baseline the tokio oracle — R2 scores 0.0 for \$0"
```

---

## Self-Review

**Spec coverage.** §3 what is measured → Tasks 1–3. §4 where it runs → Tasks 3 (gate) and 4 (target, CI). §5 the ratchet → Task 2, verified biting in Task 5 Step 4. §6 parity → Task 3 Step 1, with the `Purpose::Task` value pinned in Global Constraints. §7 non-goals: nothing here asserts the rest of the package, tunes a weight, or adds a second package source — the `variant` key the spec mentions is deliberately **not** implemented, since no second arm exists to key and an unused key is not a seam, it is dead schema. §8 risks: the clone cost is addressed by the Task 4 cache; file granularity and n=5 are accepted, not mitigated, as the spec says. §9 acceptance: criteria 1–2 Task 4/3, 3 Task 5 Step 2, 4 Task 5 Step 4, 5 Task 5 Step 1, 6 Task 4 Step 1, 7 CI in Task 4.

One spec statement is **not** implemented as written: §7's baseline `variant` key. Recorded here rather than silently dropped — adding a graphify or BM25 arm later means one schema change to a file with five rows, which is cheaper than carrying an unused key that no test exercises.

**Placeholder scan.** No TBD/TODO. Every code step carries compilable code; every run step carries an exact command and its expected output.

**Type consistency.** `TokioTask { id, prompt, required_sites, sha }` produced in Task 1 is consumed in Task 3 by those field names. `SiteRecall` defined in Task 2 is the golden row type written in Task 3 and read in Task 5. `site_recall(task, wanted, ranks, items_included, tokens_estimated, intent)` and `ratchet_failures(got, want)` are called in Task 3 with exactly those arities. `supply()` gains a third parameter in Task 3 Step 1 and all three call sites are updated in that same step.
