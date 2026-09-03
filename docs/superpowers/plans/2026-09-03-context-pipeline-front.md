# Context Pipeline Stages 1–3 (roadmap 2.1, 2.2, 2.3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn a task string into a provenance-carrying candidate set: classify the intent from a verb table, resolve seeds in priority order, and expand from those seeds along the dependency graph in the direction the intent implies.

**Architecture:** Three stages appended to the `nexus-core::context` module that Phase 1 created. Each is a pure function over the store plus the request, and each records *how* it reached its answer — the intent's matched signal, the seed's source, the candidate's edge chain. Nothing here scores, budgets or renders; stages 4 through 7 consume what these three produce. No CLI surface changes, because `--task` is task 2.10 and would be a lie without ranking behind it.

**Tech Stack:** Rust 1.82+, the existing `impact::run` traversal, `Store::find_symbols`.

**Spec:** [`05-context-engine.md`](../../architecture/05-context-engine.md) §3 (intent table), §4 (seed priority), §5 (expansion and direction), §12 (prohibitions); [`13-evaluation.md`](../../architecture/13-evaluation.md) §14.1 for what is deliberately deferred to 2.14; [`10-roadmap.md`](../../architecture/10-roadmap.md) Phase 2.

## Global Constraints

- **Roadmap 2.1, 2.2 and 2.3 are the scope.** Explicitly **do not build**: signals (2.4), the commits table (2.5), ranking or weights (2.6), the density budget (2.7), `--explain` (2.8), the cache (2.9), the `--task` CLI flag or any MCP tool (2.10), the prompt hook (2.11), graphify (2.12), the evaluation harness (2.13), `--carry-seeds`/`--recent`/`Intent::Referential` (2.14).
- **No stage calls a model. Ever.** §12. The whole pipeline is queries, arithmetic and a sort.
- **Never ask a model to classify intent.** §3 states the reason: it replaces a table lookup with a round trip and injects non-determinism into the one part of the system that must be reproducible for a package to be explainable.
- **`Unknown` is a first-class outcome, not a default dressed up as a guess.** When nothing matches, the pipeline says it determined nothing rather than picking the most likely verb.
- **Zero seeds is a legitimate result and is reported as such.** §4. A package built from nothing is worse than an empty package plus "I could not anchor this to the code".
- **`make check` must pass after every task.** Baseline: **205 passing tests**. No task may reduce that count.
- **Only `nexus-store` contains SQL.** These stages use `Store::find_symbols`, `Store::facts` and `Engine::changes` as they are. No new SQL.
- **`nexus-core` gains no dependency.**
- **`deny(clippy::unwrap_used, clippy::expect_used)` outside tests in `nexus-core`.**
- **No public surface regresses.** `Engine::context` keeps its signature and its Phase 1 session behaviour; these stages are additive and are not yet wired into it.
- **`git add` names files.** Never a directory.
- **Commit after every task**, message naming the roadmap id, ending with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## Decisions taken before writing code

**1. `Intent` and `Purpose` are different types and both stay.** `Purpose` is what the caller asked for (`Session`, `Task`, …) and arrives on the request. `Intent` is what the *text* says (`Debug`, `Build`, …) and is derived. Collapsing them would make an explicit `--purpose review` indistinguishable from the word "review" appearing in a sentence, and §3's table is about the sentence.

**2. Longest-match wins, and ties are ordered, not arbitrary.** Several table entries can match one sentence ("review the fix for the broken parser" hits `review`, `fix` and `broken`). The classifier scores each intent by how many of its signals matched, breaks ties by a fixed precedence, and **records the signal that decided it**. An intent nobody can account for is exactly the folklore §6 warns about, one stage earlier.

**3. Seeds carry their source, and the source is an enum.** §4 numbers six sources in priority order. The enum makes the priority a total order in code rather than a comment, and it is what stage 5's `w_seed` term will read. Source 5, text match against `ui_strings`, is **implemented as a no-op that records why**: the table is empty until 5.5, and a stage that silently contributes nothing is indistinguishable from one that is broken.

**4. Expansion reuses `impact::run` unchanged.** §5 says so explicitly. Direction follows intent: `Reverse` for `Refactor` and `Review` (who breaks), `Forward` for `Debug` (what this reaches), and **both, merged, for `Explain` and `Unknown`**. `Build` takes `Reverse`, because the question behind "add a thing here" is what already depends on the place it is going.

**5. Nothing is wired into `Engine::context` yet.** Stages 4 to 7 do not exist, so a `--task` package would be seeds with no selection behind them. The stages ship as tested library functions and `Engine::context` still serves only `Purpose::Session`. This is the roadmap's own ordering and it keeps every commit shippable.

---

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/nexus-core/src/context/intent.rs` | create | The verb table and the classifier. Pure: `&str` in, `IntentMatch` out. No store. |
| `crates/nexus-core/src/context/seeds.rs` | create | The six sources, in priority order, each recording its provenance. |
| `crates/nexus-core/src/context/expand.rs` | create | Direction from intent, then `impact::run`; candidates inherit their chain. |
| `crates/nexus-core/src/context.rs` → `context/mod.rs` | move + modify | The Phase 1 types stay; three `mod` lines and re-exports are added. |
| `crates/nexus-core/tests/context_pipeline.rs` | create | Stages 2 and 3 against a real index. |
| `docs/architecture/10-roadmap.md` | modify | Phase 2 progress. |

The module becomes a directory because three stages plus the type contract in one file would be the god object that task 1.1 existed to undo.

---

### Task 1: Stage 1 — intent (roadmap 2.1)

**Files:**
- Create: `crates/nexus-core/src/context/intent.rs`
- Move: `crates/nexus-core/src/context.rs` → `crates/nexus-core/src/context/mod.rs`
- Modify: `crates/nexus-core/src/context/mod.rs` (add `pub mod intent;` and re-exports)

**Interfaces:**
- Produces:
  ```rust
  pub enum Intent { Debug, Build, Refactor, Review, Explain, Unknown }
  impl Intent { pub fn as_str(self) -> &'static str }
  pub struct IntentMatch { pub intent: Intent, pub signal: Option<&'static str>, pub confident: bool }
  pub fn classify(text: &str) -> IntentMatch
  ```

- [ ] **Step 1: Move the module, then write the failing test**

```bash
mkdir -p crates/nexus-core/src/context
git mv crates/nexus-core/src/context.rs crates/nexus-core/src/context/mod.rs
```

Append to `crates/nexus-core/src/context/mod.rs`:

```rust
pub mod intent;

pub use intent::{classify, Intent, IntentMatch};

#[cfg(test)]
mod intent_tests {
    use super::intent::{classify, Intent};

    /// The table, as a table. Every row of §3 and the cases that made the rules necessary.
    #[test]
    fn the_verb_table_classifies_what_the_spec_says_it_does() {
        let cases: &[(&str, Intent)] = &[
            ("fix the payment idempotency bug", Intent::Debug),
            ("the checkout page is broken", Intent::Debug),
            ("this fails with a NullPointerException", Intent::Debug),
            ("add optimistic locking to PaymentService", Intent::Build),
            ("implement the refund endpoint", Intent::Build),
            ("we need to support partial refunds", Intent::Build),
            ("refactor PaymentService", Intent::Refactor),
            ("rename pay to settle", Intent::Refactor),
            ("extract the validation into its own class", Intent::Refactor),
            ("review my changes", Intent::Review),
            ("is this safe to merge", Intent::Review),
            ("why does the controller enforce idempotency", Intent::Explain),
            ("how does the seam work", Intent::Explain),
            ("what is a FrameworkPack", Intent::Explain),
        ];
        for (text, want) in cases {
            let got = classify(text);
            assert_eq!(got.intent, *want, "{text:?} classified as {got:?}");
            assert!(got.signal.is_some(), "a classification names its signal: {text:?}");
            assert!(got.confident, "{text:?}");
        }
    }

    #[test]
    fn nothing_matching_is_unknown_and_says_so() {
        // §3: `Unknown` is a first-class outcome, not a default dressed up as a guess.
        for text in ["", "   ", "the quick brown fox", "PaymentService"] {
            let got = classify(text);
            assert_eq!(got.intent, Intent::Unknown, "{text:?} -> {got:?}");
            assert!(
                !got.confident,
                "an unmatched prompt must report that it guessed nothing: {text:?}"
            );
            assert!(got.signal.is_none(), "{text:?}");
        }
    }

    #[test]
    fn a_stack_trace_is_a_debug_signal_on_its_own() {
        // The strongest signal a bug report carries, and it contains none of the verbs.
        let got = classify("java.lang.NullPointerException\n\tat mn.pay.PaymentService.pay(PaymentService.java:48)");
        assert_eq!(got.intent, Intent::Debug, "{got:?}");
        assert_eq!(got.signal, Some("stack trace"));
    }

    #[test]
    fn the_winner_is_the_intent_with_the_most_signals_not_the_first_word() {
        // "review the fix for the broken parser" hits review(1) and debug(2). Debug wins on
        // count. A first-word rule would answer Review, and be wrong about the whole package.
        let got = classify("review the fix for the broken parser");
        assert_eq!(got.intent, Intent::Debug, "{got:?}");
    }

    #[test]
    fn a_tie_is_broken_by_a_fixed_precedence_not_by_hash_order() {
        // One signal each. The answer must be the same on every run and on every platform,
        // because a golden package that depends on iteration order is not a golden package.
        let first = classify("fix and refactor");
        for _ in 0..50 {
            assert_eq!(classify("fix and refactor").intent, first.intent);
        }
        assert_eq!(first.intent, Intent::Debug, "debug outranks refactor on a tie");
    }

    #[test]
    fn matching_is_on_words_not_substrings() {
        // "prefix" contains "fix"; "moved" contains "move"; "adding" contains "add".
        // A substring rule turns a sentence about a URL prefix into a debugging session.
        assert_eq!(classify("the url prefix is wrong").intent, Intent::Debug,
            "'wrong' is a debug signal; the point is that 'prefix' is not");
        assert_eq!(classify("document the prefix convention").intent, Intent::Unknown);
        assert_eq!(classify("the file was moved last week").intent, Intent::Unknown);
    }

    #[test]
    fn classification_ignores_case_and_punctuation() {
        assert_eq!(classify("FIX the bug!").intent, Intent::Debug);
        assert_eq!(classify("Why does this work?").intent, Intent::Explain);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-core intent_tests 2>&1 | grep -E "^error" -A 2 | head -6`
Expected: `unresolved module` or `cannot find function 'classify'`.

- [ ] **Step 3: Write the classifier**

Create `crates/nexus-core/src/context/intent.rs`:

```rust
//! Stage 1 — what is being asked.
//!
//! A verb table and a word matcher. Not a classifier, and emphatically not a model: §3 of the
//! Context Engine design rules that out, and the reason is not cost. Intent decides the
//! ranking weights, so a package is only explainable if the same words produce the same intent
//! on every run. A model cannot promise that; a table cannot break it.
//!
//! Three properties the tests pin, each of which was a bug in an obvious implementation:
//!
//!   * **Words, not substrings.** `prefix` contains `fix`. A substring match turns "the url
//!     prefix is wrong" into a debugging session about string matching.
//!   * **Most signals wins, not the first one seen.** "review the fix for the broken parser"
//!     is a debugging task wearing a review verb.
//!   * **Ties break by a written-down precedence.** A golden package whose intent depends on
//!     iteration order is not golden.

/// What the text is asking for. Distinct from [`Purpose`](super::Purpose), which is what the
/// *caller* asked for: an explicit `--purpose review` and the word "review" in a sentence are
/// different facts and must stay distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Debug,
    Build,
    Refactor,
    Review,
    Explain,
    /// Nothing matched. Balanced weights downstream, and the package says it guessed nothing.
    Unknown,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Debug => "debug",
            Intent::Build => "build",
            Intent::Refactor => "refactor",
            Intent::Review => "review",
            Intent::Explain => "explain",
            Intent::Unknown => "unknown",
        }
    }
}

/// The classification and what produced it. `signal` is the evidence: an intent that cannot
/// name why it was chosen cannot be argued with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IntentMatch {
    pub intent: Intent,
    /// The matched phrase, or `None` for `Unknown`.
    pub signal: Option<&'static str>,
    /// False only for `Unknown`. Carried explicitly so a caller cannot mistake "we decided
    /// nothing" for "we decided Unknown".
    pub confident: bool,
}

/// The table from §3, in tie-break precedence order.
///
/// Precedence is deliberate rather than alphabetical: on an even split, a prompt that mentions
/// something being broken is treated as a bug before it is treated as anything else, because
/// the cost of missing a real defect exceeds the cost of over-weighting findings on a package
/// that turned out to be a refactor.
const TABLE: &[(Intent, &[&str])] = &[
    (
        Intent::Debug,
        &[
            "fix", "fixes", "fixing", "bug", "bugs", "broken", "breaks", "fails", "failing",
            "failure", "error", "errors", "crash", "crashes", "wrong", "regression",
        ],
    ),
    (
        Intent::Refactor,
        &[
            "refactor", "rename", "renames", "move", "moves", "extract", "inline",
            "restructure", "clean up", "tidy",
        ],
    ),
    (
        Intent::Build,
        &[
            "add", "adds", "implement", "implements", "build", "support", "create", "introduce",
            "write",
        ],
    ),
    (
        Intent::Review,
        &["review", "check", "is this safe", "done", "audit", "verify"],
    ),
    (
        Intent::Explain,
        &["why", "how does", "how do", "what is", "what are", "explain", "understand"],
    ),
];

/// Split on anything that is not alphanumeric, lowercased. Punctuation is a separator, so
/// "FIX the bug!" and "fix the bug" are the same prompt.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// A signal matches when it is a whole word, or — for a multi-word signal — a run of whole
/// words. Never a substring: `prefix` is not `fix`.
fn matches(signal: &str, words: &[String]) -> bool {
    let parts: Vec<&str> = signal.split(' ').collect();
    if parts.len() == 1 {
        return words.iter().any(|w| w == signal);
    }
    words
        .windows(parts.len())
        .any(|w| w.iter().zip(&parts).all(|(a, b)| a == b))
}

/// Java, Python and JavaScript frames all carry one of these, and none of them carries a verb
/// from the table. A pasted trace is the strongest bug signal there is; missing it means
/// ranking a crash report as `Unknown`.
fn looks_like_a_stack_trace(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("\n\tat ")
        || lower.contains("\n    at ")
        || lower.contains("traceback (most recent call last)")
        || lower.contains("exception")
        || lower.contains("panicked at")
}

/// Classify. Deterministic, allocation-light, and total: every input produces an answer, and
/// "no answer" is one of them.
pub fn classify(text: &str) -> IntentMatch {
    let unknown = IntentMatch {
        intent: Intent::Unknown,
        signal: None,
        confident: false,
    };
    if text.trim().is_empty() {
        return unknown;
    }
    let words = words(text);

    let mut best: Option<(Intent, &'static str, usize)> = None;
    for (intent, signals) in TABLE {
        let mut hits = 0usize;
        let mut first: Option<&'static str> = None;
        for signal in *signals {
            if matches(signal, &words) {
                hits += 1;
                if first.is_none() {
                    first = Some(signal);
                }
            }
        }
        if hits == 0 {
            continue;
        }
        let Some(signal) = first else { continue };
        // Strictly greater: TABLE order is the tie-break, so the earlier intent holds a draw.
        if best.is_none_or(|(_, _, prev)| hits > prev) {
            best = Some((*intent, signal, hits));
        }
    }

    // A trace beats a single incidental verb but not an explicit, repeated one — it is
    // evidence of a symptom, and the words are evidence of a request.
    if looks_like_a_stack_trace(text) && best.is_none_or(|(i, _, n)| i != Intent::Debug && n < 2) {
        return IntentMatch {
            intent: Intent::Debug,
            signal: Some("stack trace"),
            confident: true,
        };
    }

    match best {
        Some((intent, signal, _)) => IntentMatch {
            intent,
            signal: Some(signal),
            confident: true,
        },
        None => unknown,
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nexus-core intent_tests 2>&1 | grep -E "^test |test result"`
Expected: 7 passed.

- [ ] **Step 5: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 212 tests.

```bash
git add crates/nexus-core/src/context/intent.rs crates/nexus-core/src/context/mod.rs
git commit -m "core: stage 1, deterministic intent classification (roadmap 2.1)

A verb table and a word matcher. Intent picks the ranking weights, so the same
words must produce the same intent on every run — which a table guarantees and
a model cannot, and that, not cost, is why §3 forbids one here.

Three rules the tests pin, each a bug in the obvious implementation: match
words rather than substrings, because 'prefix' contains 'fix'; let the most
signals win rather than the first, because 'review the fix for the broken
parser' is a debugging task wearing a review verb; and break ties on a
written-down precedence, because a golden package whose intent depends on
iteration order is not golden.

Unknown is a first-class answer and carries confident:false, so a caller
cannot mistake 'we decided nothing' for 'we decided Unknown'.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 2: Stage 2 — seeds (roadmap 2.2)

**Files:**
- Create: `crates/nexus-core/src/context/seeds.rs`
- Modify: `crates/nexus-core/src/context/mod.rs` (`pub mod seeds;` and re-exports)
- Create: `crates/nexus-core/tests/context_pipeline.rs`

**Interfaces:**
- Consumes: `Store::find_symbols`, `Store::facts`, `Store::changes_for_scan`, `Store::baseline`, `Intent`.
- Produces:
  ```rust
  pub enum SeedSource { Explicit, Exact, Changed, NameMatch, TextMatch, FactSubject }
  pub struct Seed { pub symbol: nexus_store::SymbolRef, pub source: SeedSource, pub why: String }
  pub struct SeedResult { pub seeds: Vec<Seed>, pub notes: Vec<String> }
  pub fn resolve(store: &Store, project_id: i64, req: &TaskRequest, intent: Intent) -> Result<SeedResult, StoreError>
  ```

- [ ] **Step 1: Write the failing test**

Create `crates/nexus-core/tests/context_pipeline.rs`:

```rust
//! Stages 2 and 3: from a sentence to a candidate set, with provenance at every step.
//!
//! These run against a real index rather than a mock, because the thing most likely to be
//! wrong is the match between what the store returns and what the stage believes it returns.

use nexus_core::context::{expand, seeds, Intent, SeedSource, TaskRequest};
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
        .seeds(&request("why does PaymentController do that"), Intent::Explain)
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
    req.symbols = vec!["mn.pay.PaymentService#pay(java.lang.String)".into()];
    let seeded = engine.seeds(&req, Intent::Refactor).expect("seeds");

    let out = engine.expand(&seeded.seeds, Intent::Refactor).expect("expand");
    assert_eq!(out.direction, "reverse");
    assert!(
        out.items.iter().any(|i| i.fqn.contains("PaymentController")),
        "the caller must be reachable from the callee: {:?}",
        out.items.iter().map(|i| &i.fqn).collect::<Vec<_>>()
    );
    // §5: every expanded candidate is provable, not asserted.
    for item in &out.items {
        assert!(!item.path.is_empty(), "an item with no edge chain: {item:?}");
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p nexus-core --test context_pipeline 2>&1 | grep -E "^error" -A 2 | head -8`
Expected: unresolved imports and `no method named 'seeds'`.

- [ ] **Step 3: Write the seed resolver**

Create `crates/nexus-core/src/context/seeds.rs`:

```rust
//! Stage 2 — what in the code this is about.
//!
//! Six sources, in the priority order §4 fixes. Every seed records which source found it,
//! because stage 5 weights an explicitly named symbol differently from one guessed at by
//! name, and because a seed nobody can account for produces a package nobody can argue with.
//!
//! Zero seeds is a legitimate answer and is stated in `notes` rather than left to be inferred
//! from an empty vector. §4 is explicit about why: an empty package plus "I could not anchor
//! this to the code" lets the agent ask a better question, where a package built from nothing
//! sends it confidently into the wrong module.

use super::TaskRequest;
use crate::context::intent::Intent;
use nexus_store::{Store, StoreError, SymbolRef};
use std::collections::BTreeMap;

/// How a seed was found, in priority order — `Ord` is the priority, so a symbol found twice
/// keeps its best provenance by comparison rather than by a rule written in a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSource {
    /// The caller named it. A hook editing a file knows.
    Explicit,
    /// An exact FQN or repository path appearing in the text.
    Exact,
    /// The symbols this rescan reports as changed. Free: the cascade already computed it.
    Changed,
    /// A bare symbol name in the text, matched exactly and then by suffix.
    NameMatch,
    /// A user-visible label, via `ui_strings`. Empty until 5.5.
    TextMatch,
    /// The text names a module some fact is about.
    FactSubject,
}

impl SeedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SeedSource::Explicit => "explicit",
            SeedSource::Exact => "exact",
            SeedSource::Changed => "changed",
            SeedSource::NameMatch => "name match",
            SeedSource::TextMatch => "text match",
            SeedSource::FactSubject => "fact subject",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Seed {
    pub symbol: SymbolRef,
    pub source: SeedSource,
    pub why: String,
}

#[derive(Debug, Clone, Default)]
pub struct SeedResult {
    pub seeds: Vec<Seed>,
    /// What the stage could not do, and why. Never empty when `seeds` is.
    pub notes: Vec<String>,
}

/// Candidate words from the prompt that could name a symbol: anything containing a dot or a
/// slash (an FQN or a path), or starting with a capital (a type name by every convention the
/// indexed languages use). Filtering here rather than querying every word keeps the stage at
/// a handful of indexed lookups instead of one per token.
fn targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '(' | ')'))
        .map(|w| w.trim_end_matches(['.', '?', '!', ':']))
        .filter(|w| w.len() > 2)
        .filter(|w| {
            w.contains('.')
                || w.contains('/')
                || w.contains('#')
                || w.chars().next().is_some_and(char::is_uppercase)
        })
        .map(str::to_string)
        .collect();
    out.dedup();
    out
}

/// Resolve the request to seeds. Sources run in priority order and a symbol keeps the best
/// source that found it.
pub fn resolve(
    store: &Store,
    project_id: i64,
    req: &TaskRequest,
    intent: Intent,
) -> Result<SeedResult, StoreError> {
    let mut found: BTreeMap<i64, Seed> = BTreeMap::new();
    let mut notes = Vec::new();

    let mut offer = |symbol: SymbolRef, source: SeedSource, why: String| {
        found
            .entry(symbol.id)
            .and_modify(|existing| {
                if source < existing.source {
                    existing.source = source;
                    existing.why = why.clone();
                }
            })
            .or_insert(Seed {
                symbol,
                source,
                why,
            });
    };

    // 1 — explicit. The caller has the anchors; nothing here is a guess.
    for fqn in &req.symbols {
        for s in store.find_symbols(project_id, fqn, 25)? {
            offer(s, SeedSource::Explicit, format!("named in the request: {fqn}"));
        }
    }
    for path in &req.files {
        for s in store.find_symbols(project_id, path, 200)? {
            offer(s, SeedSource::Explicit, format!("in a named file: {path}"));
        }
    }

    // 2 and 4 — an FQN or path in the text, then a bare name. One lookup per candidate word;
    // `find_symbols` decides which kind it is, so the two sources differ only in how the
    // result is labelled.
    for target in targets(&req.text) {
        let exact_shape = target.contains('.') || target.contains('/') || target.contains('#');
        for s in store.find_symbols(project_id, &target, 10)? {
            let source = if exact_shape && s.fqn == target {
                SeedSource::Exact
            } else if exact_shape {
                SeedSource::Exact
            } else {
                SeedSource::NameMatch
            };
            offer(s, source, format!("'{target}' in the request"));
        }
    }

    // 3 — the changed set. Free for a review: the rescan already computed it.
    if matches!(intent, Intent::Review) || req.purpose == super::Purpose::Review {
        match store.baseline(project_id)? {
            Some(b) => {
                for (entity, _, target, _) in store.changes_for_scan(b.scan_id, Some("symbol"))? {
                    let _ = entity;
                    let Some(fqn) = target else { continue };
                    for s in store.find_symbols(project_id, &fqn, 5)? {
                        offer(s, SeedSource::Changed, "changed in this scan".into());
                    }
                }
            }
            None => notes.push("no baseline, so the changed set could not seed anything".into()),
        }
    }

    // 5 — text match against `ui_strings`. The table is empty until 5.5, and saying so is the
    // difference between a stage that cannot help yet and one that is broken.
    notes.push(
        "ui_strings is empty until roadmap 5.5, so a user-visible label cannot seed anything"
            .into(),
    );

    // 6 — a fact's subject named in the text. The cheapest way to reach a module the project
    // already recorded knowledge about.
    if !req.text.is_empty() {
        let lower = req.text.to_lowercase();
        for fact in store.facts(project_id, None)? {
            let Some(subject) = fact.subject.as_deref() else {
                continue;
            };
            if subject.len() > 2 && lower.contains(&subject.to_lowercase()) {
                for s in store.find_symbols(project_id, subject, 10)? {
                    offer(
                        s,
                        SeedSource::FactSubject,
                        format!("subject of fact {}", fact.key),
                    );
                }
            }
        }
    }

    let mut seeds: Vec<Seed> = found.into_values().collect();
    seeds.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.symbol.fqn.cmp(&b.symbol.fqn))
    });

    if seeds.is_empty() {
        notes.push(
            "no seed: nothing in the request matched a symbol, a path or a fact subject".into(),
        );
    }
    Ok(SeedResult { seeds, notes })
}
```

Add to `crates/nexus-core/src/context/mod.rs`:

```rust
pub mod seeds;

pub use seeds::{Seed, SeedResult, SeedSource};
```

- [ ] **Step 4: Add the Engine entry points**

Append to the `impl Engine` block in `crates/nexus-core/src/engine/query.rs`, after `context`:

```rust
    /// Stage 2 of the context pipeline: what in the code this request is about.
    ///
    /// Public because the pipeline is assembled stage by stage across Phase 2 and each stage
    /// is testable on its own. `Engine::context` will call it once stages 4 to 7 exist.
    pub fn seeds(&self, req: &TaskRequest, intent: Intent) -> Result<SeedResult> {
        Ok(seeds::resolve(&self.store, self.project_id, req, intent)?)
    }
```

Extend the `use crate::context::{...}` list with `Intent`, `SeedResult`, and `seeds`.

- [ ] **Step 5: Run the seed tests**

Run: `cargo test -p nexus-core --test context_pipeline 2>&1 | grep -E "^test |test result" | head -12`
Expected: the seven seed tests pass; the three expansion tests still fail to compile until Task 3. If the whole binary fails to compile, temporarily confirm with `cargo test -p nexus-core --test context_pipeline seed 2>&1` after Task 3 instead — do not weaken a test to make this step pass.

Because the file will not compile without `Engine::expand`, **Tasks 2 and 3 land in one commit** if the split cannot be made to build. Prefer that over commenting tests out.

- [ ] **Step 6: `make check`, then commit (with Task 3 if needed)**

---

### Task 3: Stage 3 — expand (roadmap 2.3)

**Files:**
- Create: `crates/nexus-core/src/context/expand.rs`
- Modify: `crates/nexus-core/src/context/mod.rs`, `crates/nexus-core/src/engine/query.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn direction_for(intent: Intent) -> &'static str
  pub fn run(store: &Store, project_id: i64, seeds: &[Seed], intent: Intent) -> Result<ImpactReport, StoreError>
  impl Engine { pub fn expand(&self, seeds: &[Seed], intent: Intent) -> Result<ImpactReport> }
  ```

- [ ] **Step 1: Write the module**

Create `crates/nexus-core/src/context/expand.rs`:

```rust
//! Stage 3 — what else the seeds reach.
//!
//! `impact::run` unchanged, which §5 requires: one traversal, one set of bounds, one
//! definition of what an edge is worth. A second traversal written for context would be a
//! second answer to "what does this affect", and the two would disagree on a Tuesday.
//!
//! Direction follows intent. Every expanded candidate keeps the `Hop` chain that reached it
//! and the weakest confidence along that chain, which is what makes its presence in the
//! package provable rather than asserted.

use super::seeds::Seed;
use crate::context::intent::Intent;
use crate::impact::{self, Direction, ImpactQuery};
use crate::report::ImpactReport;
use nexus_store::{Store, StoreError, SymbolRef};

/// Which way to walk, as a word. `both` merges a reverse and a forward pass.
///
/// `Build` is reverse on purpose: the question behind "add a thing here" is what already
/// depends on the place it is going, not what that place happens to call.
pub fn direction_for(intent: Intent) -> &'static str {
    match intent {
        Intent::Refactor | Intent::Review | Intent::Build => "reverse",
        Intent::Debug => "forward",
        Intent::Explain | Intent::Unknown => "both",
    }
}

fn refs(seeds: &[Seed]) -> Vec<SymbolRef> {
    seeds.iter().map(|s| s.symbol.clone()).collect()
}

/// Expand from the seeds. An empty seed set is an empty report, not an error: stage 2 has
/// already said in its notes that it anchored nothing, and failing here would report the same
/// fact twice as two different kinds of problem.
pub fn run(
    store: &Store,
    project_id: i64,
    seeds: &[Seed],
    intent: Intent,
) -> Result<ImpactReport, StoreError> {
    let direction = direction_for(intent);
    let refs = refs(seeds);
    let base = ImpactQuery {
        target: String::new(),
        direction: Direction::Reverse,
        ..Default::default()
    };

    if refs.is_empty() {
        let mut empty = impact::run(store, project_id, &[], &base)?;
        empty.direction = direction;
        return Ok(empty);
    }

    let mut report = match direction {
        "forward" => impact::run(
            store,
            project_id,
            &refs,
            &ImpactQuery {
                direction: Direction::Forward,
                ..base.clone()
            },
        )?,
        "both" => {
            let mut reverse = impact::run(store, project_id, &refs, &base)?;
            let forward = impact::run(
                store,
                project_id,
                &refs,
                &ImpactQuery {
                    direction: Direction::Forward,
                    ..base.clone()
                },
            )?;
            // A symbol reached both ways is one candidate with the better score, not two.
            // §9 calls deduplication the largest single saving on a dense graph, and doing it
            // here means the budget never sees the duplicate at all.
            for item in forward.items {
                match reverse.items.iter_mut().find(|i| i.fqn == item.fqn) {
                    Some(existing) if existing.score >= item.score => {}
                    Some(existing) => *existing = item,
                    None => reverse.items.push(item),
                }
            }
            reverse.crossed_seam += forward.crossed_seam;
            reverse.truncated_at.extend(forward.truncated_at);
            reverse
                .items
                .sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.fqn.cmp(&b.fqn)));
            reverse
        }
        _ => impact::run(store, project_id, &refs, &base)?,
    };

    report.direction = direction;
    report.target = seeds
        .iter()
        .map(|s| s.symbol.fqn.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(report)
}
```

Add to `crates/nexus-core/src/context/mod.rs`: `pub mod expand;`.

Add to `crates/nexus-core/src/engine/query.rs`, after `seeds`:

```rust
    /// Stage 3 of the context pipeline: what else the seeds reach.
    pub fn expand(&self, seeds: &[Seed], intent: Intent) -> Result<ImpactReport> {
        Ok(expand::run(&self.store, self.project_id, seeds, intent)?)
    }
```

- [ ] **Step 2: Run the whole pipeline test**

Run: `cargo test -p nexus-core --test context_pipeline 2>&1 | grep -E "^test |test result"`
Expected: 10 passed.

If `expansion_reaches_the_caller_on_a_reverse_intent` fails because the Java analyzer resolved no edge between the two fixture classes, check the edge exists first:

```bash
cargo run -q --bin nexus -- --project <fixture> impact 'mn.pay.PaymentService#pay' --paths
```

A missing edge there is an analyzer fact, not a stage-3 bug — report it and adjust the fixture to a call shape the analyzer resolves, rather than weakening the assertion.

- [ ] **Step 3: `make check`, then commit**

Run: `make check 2>&1 | tail -5`
Expected: green, 222 tests.

```bash
git add crates/nexus-core/src/context/seeds.rs crates/nexus-core/src/context/expand.rs crates/nexus-core/src/context/mod.rs crates/nexus-core/src/engine/query.rs crates/nexus-core/tests/context_pipeline.rs
git commit -m "core: stages 2 and 3, seeds and expansion (roadmap 2.2, 2.3)

Six seed sources in the priority order §4 fixes, each seed carrying the source
that found it — SeedSource derives Ord, so a symbol found twice keeps its best
provenance by comparison rather than by a rule in a comment.

Zero seeds is stated in notes rather than left to be inferred from an empty
vector, and the empty ui_strings table is reported for the same reason: a stage
that quietly contributes nothing is indistinguishable from one that is broken.

Expansion reuses impact::run unchanged, as §5 requires. A second traversal
written for context would be a second answer to 'what does this affect', and
the two would disagree eventually. Direction follows intent; 'both' merges the
two passes and deduplicates, so the budget never sees a symbol twice.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 4: The roadmap records where Phase 2 stands

- [ ] **Step 1: Add a status line under the Phase 2 feature table**

```
**Status (2026-09-03):** 2.1, 2.2 and 2.3 landed — `nexus-core/src/context/{intent,seeds,expand}.rs`,
pinned by `tests/context_pipeline.rs` and the intent table's own unit tests. They are library
stages, not yet wired into `Engine::context`: a `--task` package without stages 4–7 would be
seeds with no selection behind them, and 2.10 is where the CLI surface belongs. Next: **2.4**.
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/10-roadmap.md docs/superpowers/plans/2026-09-03-context-pipeline-front.md
git commit -m "docs: Phase 2 stages 1-3 landed (roadmap 2.1, 2.2, 2.3)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC"
```

---

### Task 5: Acceptance

- [ ] **Step 1: `make check` from a detached worktree of the tip**

```bash
W=/tmp/claude-1000/-opt-tools-nexus/1b50a100-5dd4-4a0d-90e3-fdaa134b13ec/scratchpad/verify-p2
rm -rf $W; git worktree prune
git worktree add -q --detach $W HEAD && make -C $W check 2>&1 | tail -5
git worktree remove --force $W
```

Expected: green, 222 tests.

- [ ] **Step 2: State the acceptance criteria against the code**

- 2.1 "deterministic verb table; `Unknown` a first-class outcome" — the table test covers every row of §3; `nothing_matching_is_unknown_and_says_so` asserts `confident: false`.
- 2.2 "explicit · fqn/path · changed set · name match · fact subject" — one test per source, plus the zero-seed and deduplication rules.
- 2.3 "reuse `impact::run`, direction from intent" — `direction_follows_intent` and the reverse-reachability test; `expand::run` calls `impact::run` and adds no traversal of its own.

---

## Self-review

**Spec coverage.** §3's six intents and its `Unknown` rule: covered, with the stack-trace signal the table names but does not spell out. §4's six sources in priority order: covered, with source 5 explicitly reporting that it cannot work yet. §4's zero-seed rule: covered. §5's reuse of `impact::run`, direction from intent, and inherited hop chains: covered. §9's deduplication, in the one place a duplicate can be created here: covered.

**Deliberately not covered.** Signal attachment (2.4), ranking (2.6), the density budget (2.7), the CLI and MCP surface (2.10), and `Intent::Referential` with carry-seeds (2.14). The last is worth naming: [`13`](../../architecture/13-evaluation.md) §14.1 makes `Referential` part of the verb table, so 2.14 will extend `TABLE` and `Intent` rather than rework them.

**Placeholders.** None.

**Type consistency.** `Intent` is defined once in `intent.rs` and used by `seeds::resolve`, `expand::direction_for` and both Engine methods. `Seed` carries `nexus_store::SymbolRef`, which is exactly what `impact::run` takes, so stage 3 converts nothing. `SeedSource` derives `Ord` and the ordering *is* the §4 priority — if a source is inserted later, it must go in the right position in the enum, and the deduplication rule follows automatically.

**Risk this plan carries.** `targets()` filters prompt words heuristically (contains a dot, slash or hash, or starts with a capital) before querying. A lowercase single-word symbol name mentioned in a prompt will not be looked up. That is a deliberate trade: the alternative is one indexed lookup per token, on a stage that ADR-024 budgets at 150 ms inside a per-prompt hook. It is recorded here so that when 2.13's harness measures seed recall, the cause of a miss is already written down.
