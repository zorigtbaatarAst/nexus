//! The context package: what an agent is handed, and why each thing is in it.
//!
//! The types here are the Context Engine's contract (`docs/architecture/05-context-engine.md`
//! §2). Phase 1 ships the contract and one fixed query behind it — the session package, which
//! is profile plus open findings plus durable facts under a token ceiling. Phase 2 replaces
//! the body of [`Engine::context`](crate::Engine::context) with the seven-stage ranked
//! pipeline; nothing here changes shape when it does, which is the reason the types land
//! first.
//!
//! Two rules from §12 are enforced in this file rather than trusted to callers:
//!
//!   * **Every item carries a `file:line` anchor.** A candidate without one is an *excluded*
//!     ledger row, never a silent omission and never an anchorless item.
//!   * **Remaining budget is never padded.** Selection stops when the next candidate does not
//!     fit; it does not go looking for a smaller one to fill the gap.

use crate::findings::CodeRef;
use crate::report::Profile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The default ceiling for a session package. The `SessionStart` hook's budget in ADR-024.
pub const SESSION_BUDGET_TOKENS: usize = 800;

/// Bytes per token. The estimator `budget::fit` already uses, and an estimate on purpose:
/// a real tokenizer is a dependency bought for a rounding error (§6).
const BYTES_PER_TOKEN: f64 = 3.5;

/// Estimated tokens for a rendered string. Always at least 1 for non-empty input, so that
/// nothing is ever free and the greedy fill cannot loop.
pub fn estimate_tokens(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    ((s.len() as f64 / BYTES_PER_TOKEN).ceil() as usize).max(1)
}

/// Why a package was asked for. Phase 1 serves `Session`; the rest are the Phase 2 surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Session,
    Task,
    Review,
    Debug,
    Verify,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Session => "session",
            Purpose::Task => "task",
            Purpose::Review => "review",
            Purpose::Debug => "debug",
            Purpose::Verify => "verify",
        }
    }
}

/// What was asked for. `text`, `files` and `symbols` are the Phase 2 seed inputs and are
/// accepted now so the signature does not move when ranking lands.
#[derive(Debug, Clone)]
pub struct TaskRequest {
    pub text: String,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub budget_tokens: usize,
    pub purpose: Purpose,
}

impl TaskRequest {
    /// The session package: no text, no explicit anchors, 800 tokens by default.
    pub fn session(budget_tokens: usize) -> Self {
        Self {
            text: String::new(),
            files: Vec::new(),
            symbols: Vec::new(),
            budget_tokens,
            purpose: Purpose::Session,
        }
    }
}

/// What the project is. Answered from the stored profile, never inferred at query time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub name: String,
    pub profile: Option<Profile>,
    pub files: i64,
    pub symbols: i64,
    /// Present when the scan looks like one module of something larger. Silence here is what
    /// lets an impact answer report a small blast radius with total confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Symbol,
    File,
    Finding,
    Fact,
    Change,
    Test,
    Decision,
}

/// Every weighted term that produced a score, individually. Phase 1 records zeros and says so
/// in the package's `basis`: a fixed query has no ranking to decompose. The struct exists now
/// because Phase 2.6's contract is that *every* term is recorded, and a field added later is a
/// field some caller has already learned to live without.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreTerms {
    pub seed: f64,
    pub graph: f64,
    pub churn: f64,
    pub recency: f64,
    pub history: f64,
    pub fact: f64,
    pub test: f64,
    pub arch: f64,
    pub cost: f64,
}

/// One thing in the package. Anchors, not contents: Nexus says where, and the agent has
/// `Read` (§2, principle 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    pub kind: ItemKind,
    pub anchor: CodeRef,
    /// At most a few lines. Never a whole file. Phase 1 emits none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    pub score: f64,
    pub terms: ScoreTerms,
    /// One clause, human-readable, saying why this is here.
    pub why: String,
    /// The rendered line. Carried so the token estimate and the output cannot disagree.
    pub text: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Included,
    Excluded,
}

/// One candidate's fate. Recorded for winners *and* losers: a ranker that only explains its
/// inclusions cannot be debugged for the failure that matters most, which is the right item
/// that never made it in (§8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRow {
    pub kind: ItemKind,
    pub label: String,
    pub decision: Decision,
    pub reason: String,
    pub score: f64,
    pub tokens: usize,
}

/// Every candidate considered, in the order they were considered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InclusionLedger {
    pub rows: Vec<LedgerRow>,
}

impl InclusionLedger {
    pub fn included(&mut self, kind: ItemKind, label: String, score: f64, tokens: usize) {
        self.rows.push(LedgerRow {
            kind,
            label,
            decision: Decision::Included,
            reason: "selected".into(),
            score,
            tokens,
        });
    }
    pub fn excluded(
        &mut self,
        kind: ItemKind,
        label: String,
        reason: String,
        score: f64,
        tokens: usize,
    ) {
        self.rows.push(LedgerRow {
            kind,
            label,
            decision: Decision::Excluded,
            reason,
            score,
            tokens,
        });
    }
    pub fn count(&self, decision: Decision) -> usize {
        self.rows.iter().filter(|r| r.decision == decision).count()
    }
}

/// What the package describes and what it was built from.
///
/// A package that does not state its basis implies a clean tree it may not have been built
/// from (§10). All four fields are also the Phase 2.9 cache key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageBasis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub dirty: bool,
    /// What produced the selection. Phase 1 says so plainly, because a caller cannot tell a
    /// fixed query from a ranked one by looking at the output. A `String` rather than a
    /// `&'static str` because a package round-trips through the cache file (§11), and a
    /// borrowed field cannot come back from disk.
    pub selection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackage {
    pub purpose: Purpose,
    pub project: ProjectSummary,
    pub items: Vec<ContextItem>,
    pub ledger: InclusionLedger,
    pub basis: PackageBasis,
    pub budget_tokens: usize,
    pub tokens_estimated: usize,
    pub items_considered: usize,
    pub items_included: usize,
    /// What the text was taken to be asking for. `None` for a session package, which has no
    /// text to classify — distinct from an `Unknown` classification, which means text was
    /// read and nothing matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<IntentMatch>,
    /// What the pipeline could not do, and why. A stage that contributes nothing in silence
    /// is indistinguishable from one that is broken.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A candidate before the budget has had its say.
pub(crate) struct Candidate {
    pub kind: ItemKind,
    pub label: String,
    pub anchor: Option<CodeRef>,
    pub why: String,
    pub text: String,
    /// Stage 5's score. Zero for the Phase 1 session package, which has no ranking and says
    /// so in its basis.
    pub score: f64,
    pub terms: ScoreTerms,
    /// The file this belongs to, for the diversity guard. Empty means "no component", which
    /// exempts it from the cap rather than lumping every such item together.
    pub component: String,
}

impl Candidate {
    pub(crate) fn tokens(&self) -> usize {
        estimate_tokens(&self.text)
    }
    /// Score per token. §7 sorts by this, not by raw score: a 40-token fact scoring 0.6 beats
    /// a 900-token class scoring 0.9, and that is where the token optimisation actually
    /// happens.
    pub(crate) fn density(&self) -> f64 {
        let t = self.tokens();
        if t == 0 {
            return f64::NEG_INFINITY;
        }
        self.score / t as f64
    }
}

/// How stage 6 chooses.
///
/// The two are not interchangeable and the distinction is not cosmetic. A ranked package
/// earned its order from stage 5 and the floor and the diversity cap are meaningful. A fixed
/// query has no scores, so sorting it by density would reorder findings and facts by how long
/// their text happens to be, and applying a floor to scores that are all zero would empty it.
/// The session package (roadmap 1.7) is `Ordered` and its basis says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Take them as given. Only the anchor rule and the budget apply.
    Ordered,
    /// §7 in full: density sort, diversity guard, score floor.
    Ranked {
        min_score_x1000: i64,
        max_per_component: usize,
    },
}

/// Stage 6 — selection, not truncation.
///
/// §7's four rules in order: sort by density, fill greedily, cap how much one component may
/// contribute, and refuse anything below the floor even when budget remains. Every refusal
/// names the rule that made it, because "excluded" without a reason is the failure §8 exists
/// to prevent.
///
/// Order is stable: candidates with equal density keep the order they arrived in, so a
/// package is reproducible and a golden test means something.
pub(crate) fn fill(
    candidates: Vec<Candidate>,
    budget_tokens: usize,
    spent: usize,
    selection: Selection,
    ledger: &mut InclusionLedger,
) -> (Vec<ContextItem>, usize) {
    let (min_score, max_per_component) = match selection {
        Selection::Ordered => (f64::NEG_INFINITY, 0),
        Selection::Ranked {
            min_score_x1000,
            max_per_component,
        } => (min_score_x1000 as f64 / 1000.0, max_per_component),
    };
    let mut ranked: Vec<(usize, Candidate)> = candidates.into_iter().enumerate().collect();
    if selection != Selection::Ordered {
        ranked.sort_by(|(ia, a), (ib, b)| {
            b.density().total_cmp(&a.density()).then_with(|| ia.cmp(ib))
        });
    }

    let mut items = Vec::new();
    let mut used = spent;
    let mut per_component: BTreeMap<String, usize> = BTreeMap::new();

    for (_, c) in ranked {
        let tokens = c.tokens();
        let Some(anchor) = c.anchor else {
            // §12: no item without a `file:line` anchor. Recorded, never dropped quietly.
            ledger.excluded(
                c.kind,
                c.label,
                "no file:line anchor".into(),
                c.score,
                tokens,
            );
            continue;
        };
        // The floor comes before the budget check: "we did not want it" and "it did not fit"
        // are different answers, and reporting the second when the first is true would send
        // someone to raise a budget that was never the constraint.
        if c.score < min_score {
            ledger.excluded(
                c.kind,
                c.label,
                format!("below floor ({min_score:.2})"),
                c.score,
                tokens,
            );
            continue;
        }
        if !c.component.is_empty() && max_per_component > 0 {
            let n = per_component.entry(c.component.clone()).or_insert(0);
            if *n >= max_per_component {
                ledger.excluded(
                    c.kind,
                    c.label,
                    format!("at most {max_per_component} items from {}", c.component),
                    c.score,
                    tokens,
                );
                continue;
            }
        }
        if used + tokens > budget_tokens {
            ledger.excluded(
                c.kind,
                c.label,
                format!("budget exhausted at {budget_tokens} tokens"),
                c.score,
                tokens,
            );
            continue;
        }
        used += tokens;
        if !c.component.is_empty() {
            *per_component.entry(c.component.clone()).or_insert(0) += 1;
        }
        ledger.included(c.kind, c.label, c.score, tokens);
        items.push(ContextItem {
            kind: c.kind,
            anchor,
            window: None,
            score: c.score,
            terms: c.terms,
            why: c.why,
            text: c.text,
            tokens,
        });
    }
    (items, used)
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    fn candidate(label: &str, score: f64, text: &str, component: &str) -> Candidate {
        Candidate {
            kind: ItemKind::Symbol,
            label: label.into(),
            anchor: Some(CodeRef {
                file: "a.java".into(),
                line: 1,
                note: String::new(),
            }),
            why: String::new(),
            text: text.into(),
            score,
            terms: ScoreTerms::default(),
            component: component.into(),
        }
    }

    fn run(cs: Vec<Candidate>, budget: usize) -> (Vec<ContextItem>, InclusionLedger) {
        let mut ledger = InclusionLedger::default();
        let (items, _) = fill(
            cs,
            budget,
            0,
            Selection::Ranked {
                min_score_x1000: 150,
                max_per_component: 3,
            },
            &mut ledger,
        );
        (items, ledger)
    }

    #[test]
    fn an_ordered_selection_keeps_its_order_and_ignores_the_floor() {
        // The session package is a fixed query with no scores. Density-sorting it would
        // reorder findings and facts by text length, and a floor over zeros would empty it.
        let cs = vec![
            candidate("first", 0.0, &"a".repeat(400), "x.java"),
            candidate("second", 0.0, "tiny", "x.java"),
            candidate("third", 0.0, "also tiny", "x.java"),
            candidate("fourth", 0.0, "still tiny", "x.java"),
        ];
        let mut ledger = InclusionLedger::default();
        let (items, _) = fill(cs, 10_000, 0, Selection::Ordered, &mut ledger);
        assert_eq!(items.len(), 4, "no floor and no component cap: {items:?}");
        assert!(items[0].text.starts_with("aaaa"), "order is preserved");
    }

    #[test]
    fn density_beats_raw_score() {
        // §7's headline: a 40-token fact scoring 0.6 beats a 900-token class scoring 0.9.
        let cheap = candidate("cheap", 0.6, &"x".repeat(140), "a.java");
        let dear = candidate("dear", 0.9, &"y".repeat(3150), "b.java");
        let (items, _) = run(vec![dear, cheap], 100);
        assert_eq!(items.first().map(|i| i.score), Some(0.6), "{items:?}");
    }

    #[test]
    fn one_component_cannot_fill_the_package() {
        // Without the guard a hot class fills the budget with its own methods, and the
        // package describes one file instead of one change.
        let cs: Vec<Candidate> = (0..8)
            .map(|i| candidate(&format!("m{i}"), 0.9, "short text here", "Hot.java"))
            .collect();
        let (items, ledger) = run(cs, 10_000);
        assert_eq!(items.len(), 3, "{items:?}");
        assert!(ledger
            .rows
            .iter()
            .any(|r| r.reason.contains("at most 3 items")));
    }

    #[test]
    fn an_item_below_the_floor_is_refused_even_with_budget_to_spare() {
        // §7: an unfilled budget is not a problem to solve. Padding is what the core
        // principle forbids.
        let (items, ledger) = run(vec![candidate("weak", 0.01, "tiny", "a.java")], 10_000);
        assert!(items.is_empty(), "{items:?}");
        assert!(
            ledger.rows[0].reason.contains("below floor"),
            "and not 'budget exhausted', which would send someone to raise a budget that was              never the constraint: {:?}",
            ledger.rows[0]
        );
    }

    #[test]
    fn a_refusal_always_names_the_rule_that_made_it() {
        let mut cs = vec![candidate("weak", 0.01, "tiny", "a.java")];
        cs.push(candidate("big", 0.9, &"z".repeat(10_000), "b.java"));
        let (_, ledger) = run(cs, 50);
        for row in ledger
            .rows
            .iter()
            .filter(|r| r.decision == Decision::Excluded)
        {
            assert!(!row.reason.is_empty(), "{row:?}");
        }
        assert!(ledger
            .rows
            .iter()
            .any(|r| r.reason.contains("budget exhausted")));
    }

    #[test]
    fn equal_density_keeps_arrival_order_so_a_package_is_reproducible() {
        let cs: Vec<Candidate> = ["a", "b", "c"]
            .iter()
            .map(|l| candidate(l, 0.5, "same length text", "x.java"))
            .collect();
        let (items, _) = run(cs, 10_000);
        let labels: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(labels.len(), 3);
        for _ in 0..20 {
            let cs: Vec<Candidate> = ["a", "b", "c"]
                .iter()
                .map(|l| candidate(l, 0.5, "same length text", "x.java"))
                .collect();
            let (again, _) = run(cs, 10_000);
            assert_eq!(
                again.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
                labels
            );
        }
    }

    #[test]
    fn an_unanchored_candidate_is_still_a_ledger_row() {
        let mut c = candidate("no-anchor", 0.9, "text", "a.java");
        c.anchor = None;
        let (items, ledger) = run(vec![c], 10_000);
        assert!(items.is_empty());
        assert!(
            ledger.rows[0].reason.contains("anchor"),
            "{:?}",
            ledger.rows[0]
        );
    }
}

pub mod cache;
pub mod expand;
pub mod intent;
pub mod rank;
pub mod seeds;
pub mod signals;

pub use intent::{classify, Intent, IntentMatch};
pub use seeds::{Seed, SeedResult, SeedSource};
pub use signals::{SignalIndex, Signals};

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
            (
                "extract the validation into its own class",
                Intent::Refactor,
            ),
            ("review my changes", Intent::Review),
            ("is this safe to merge", Intent::Review),
            (
                "why does the controller enforce idempotency",
                Intent::Explain,
            ),
            ("how does the seam work", Intent::Explain),
            ("what is a FrameworkPack", Intent::Explain),
        ];
        for (text, want) in cases {
            let got = classify(text);
            assert_eq!(got.intent, *want, "{text:?} classified as {got:?}");
            assert!(
                got.signal.is_some(),
                "a classification names its signal: {text:?}"
            );
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
        let got = classify(
            "java.lang.NullPointerException\n\tat mn.pay.PaymentService.pay(PaymentService.java:48)",
        );
        assert_eq!(got.intent, Intent::Debug, "{got:?}");
        assert_eq!(got.signal.as_deref(), Some("stack trace"));
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
        assert_eq!(
            first.intent,
            Intent::Debug,
            "debug outranks refactor on a tie"
        );
    }

    #[test]
    fn matching_is_on_words_not_substrings() {
        // "prefix" contains "fix"; "moved" contains "move"; "adding" contains "add".
        // A substring rule turns a sentence about a URL prefix into a debugging session.
        assert_eq!(
            classify("the url prefix is wrong").intent,
            Intent::Debug,
            "'wrong' is a debug signal; the point is that 'prefix' is not"
        );
        assert_eq!(
            classify("document the prefix convention").intent,
            Intent::Unknown
        );
        assert_eq!(
            classify("the file was moved last week").intent,
            Intent::Unknown
        );
    }

    #[test]
    fn classification_ignores_case_and_punctuation() {
        assert_eq!(classify("FIX the bug!").intent, Intent::Debug);
        assert_eq!(classify("Why does this work?").intent, Intent::Explain);
    }
}
