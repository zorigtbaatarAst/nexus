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
use serde::Serialize;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Included,
    Excluded,
}

/// One candidate's fate. Recorded for winners *and* losers: a ranker that only explains its
/// inclusions cannot be debugged for the failure that matters most, which is the right item
/// that never made it in (§8).
#[derive(Debug, Clone, Serialize)]
pub struct LedgerRow {
    pub kind: ItemKind,
    pub label: String,
    pub decision: Decision,
    pub reason: String,
    pub score: f64,
    pub tokens: usize,
}

/// Every candidate considered, in the order they were considered.
#[derive(Debug, Clone, Default, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct PackageBasis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub dirty: bool,
    /// What produced the selection. Phase 1 says so plainly, because a caller cannot tell a
    /// fixed query from a ranked one by looking at the output.
    pub selection: &'static str,
}

#[derive(Debug, Clone, Serialize)]
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
}

/// A candidate before the budget has had its say.
pub(crate) struct Candidate {
    pub kind: ItemKind,
    pub label: String,
    pub anchor: Option<CodeRef>,
    pub why: String,
    pub text: String,
}

impl Candidate {
    pub(crate) fn tokens(&self) -> usize {
        estimate_tokens(&self.text)
    }
}

/// Greedy fill in the order given, recording every candidate's fate.
///
/// Order *is* the selection in Phase 1 — there is no score to sort by, and inventing one
/// would be the folklore §6 warns against. When a candidate does not fit, it is excluded and
/// the fill continues: a later, smaller item may still belong, and stopping at the first
/// refusal would be truncation rather than selection.
pub(crate) fn fill(
    candidates: Vec<Candidate>,
    budget_tokens: usize,
    spent: usize,
    ledger: &mut InclusionLedger,
) -> (Vec<ContextItem>, usize) {
    let mut items = Vec::new();
    let mut used = spent;
    for c in candidates {
        let tokens = c.tokens();
        let Some(anchor) = c.anchor else {
            // §12: no item without a `file:line` anchor. Recorded, never dropped quietly.
            ledger.excluded(c.kind, c.label, "no file:line anchor".into(), 0.0, tokens);
            continue;
        };
        if used + tokens > budget_tokens {
            ledger.excluded(
                c.kind,
                c.label,
                format!("budget exhausted at {budget_tokens} tokens"),
                0.0,
                tokens,
            );
            continue;
        }
        used += tokens;
        ledger.included(c.kind, c.label, 0.0, tokens);
        items.push(ContextItem {
            kind: c.kind,
            anchor,
            window: None,
            score: 0.0,
            terms: ScoreTerms::default(),
            why: c.why,
            text: c.text,
            tokens,
        });
    }
    (items, used)
}

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
