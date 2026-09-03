//! Stage 5 — one score per candidate, and every term that produced it.
//!
//! §6 is a single weighted sum, and that is the whole design. A ranker assembled from special
//! cases cannot be debugged: with one formula, every surprising inclusion decomposes into
//! terms you can read and a weight you can change. A rule of the form "always include the
//! controller" is how a ranker becomes folklore, and there are none here.
//!
//! Every term is recorded per candidate, not just the total. §8 requires a package to answer
//! *why was this included* and *why was this excluded*, and a total alone answers neither.

use super::signals::Signals;
use super::ScoreTerms;
use crate::policy::Weights;

/// Seed proximity, or the impact score for an expanded candidate.
///
/// A seed is 1.0 by definition: it is what the request was about. Everything else inherits
/// the graph score that reached it, which is the product of edge weights and confidences
/// along its chain — so a candidate's proximity is already proven rather than asserted.
pub struct Inputs<'a> {
    pub seed_proximity: f64,
    pub graph_score: f64,
    pub signals: &'a Signals,
    /// Estimated tokens for this candidate over the whole budget, in 0.0..=1.0.
    pub token_cost_norm: f64,
}

/// Score one candidate. Pure arithmetic: no store, no git, no model.
pub fn score(inputs: &Inputs<'_>, w: &Weights) -> (f64, ScoreTerms) {
    let s = inputs.signals;
    let terms = ScoreTerms {
        seed: w.seed * inputs.seed_proximity,
        graph: w.graph * inputs.graph_score,
        churn: w.churn * s.churn,
        recency: w.recency * s.recency,
        history: w.history * s.history,
        fact: w.fact * s.fact,
        test: w.test * s.coverage,
        arch: w.arch * s.arch,
        // Subtracted, so it reads as a penalty in the ledger rather than as a term whose
        // sign a reader has to remember.
        cost: -(w.cost * inputs.token_cost_norm.clamp(0.0, 1.0)),
    };
    let total = terms.seed
        + terms.graph
        + terms.churn
        + terms.recency
        + terms.history
        + terms.fact
        + terms.test
        + terms.arch
        + terms.cost;
    (total, terms)
}

/// The sum of a term set. Used by `--explain` to show that the parts account for the whole —
/// a decomposition that does not add up is worse than none.
pub fn total(t: &ScoreTerms) -> f64 {
    t.seed + t.graph + t.churn + t.recency + t.history + t.fact + t.test + t.arch + t.cost
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig() -> Signals {
        Signals::default()
    }

    #[test]
    fn a_seed_outscores_a_distant_expansion_on_the_same_signals() {
        let w = Weights::default();
        let s = sig();
        let (seed, _) = score(
            &Inputs {
                seed_proximity: 1.0,
                graph_score: 0.0,
                signals: &s,
                token_cost_norm: 0.0,
            },
            &w,
        );
        let (far, _) = score(
            &Inputs {
                seed_proximity: 0.0,
                graph_score: 0.2,
                signals: &s,
                token_cost_norm: 0.0,
            },
            &w,
        );
        assert!(seed > far, "seed {seed} far {far}");
    }

    #[test]
    fn every_term_is_recorded_and_they_sum_to_the_score() {
        // §8: a package must explain an inclusion. A total with no decomposition cannot.
        let w = Weights::default();
        let s = Signals {
            churn: 0.5,
            recency: 0.4,
            coverage: 1.0,
            history: 0.8,
            fact: 0.6,
            arch: 0.3,
        };
        let (got, terms) = score(
            &Inputs {
                seed_proximity: 1.0,
                graph_score: 0.7,
                signals: &s,
                token_cost_norm: 0.25,
            },
            &w,
        );
        for (name, value) in [
            ("seed", terms.seed),
            ("graph", terms.graph),
            ("churn", terms.churn),
            ("recency", terms.recency),
            ("history", terms.history),
            ("fact", terms.fact),
            ("test", terms.test),
            ("arch", terms.arch),
        ] {
            assert!(value > 0.0, "{name} did not contribute: {terms:?}");
        }
        assert!(terms.cost < 0.0, "cost is a penalty: {terms:?}");
        assert!((total(&terms) - got).abs() < 1e-9, "{terms:?} != {got}");
    }

    #[test]
    fn a_regression_lifts_a_candidate_above_an_identical_one_without_history() {
        let w = Weights::default();
        let clean = sig();
        let regressed = Signals {
            history: 1.0,
            ..Signals::default()
        };
        fn inputs<'a>(s: &'a Signals) -> Inputs<'a> {
            Inputs {
                seed_proximity: 0.0,
                graph_score: 0.5,
                signals: s,
                token_cost_norm: 0.1,
            }
        }
        assert!(score(&inputs(&regressed), &w).0 > score(&inputs(&clean), &w).0);
    }

    #[test]
    fn cost_penalises_but_does_not_decide_alone() {
        // A cheap irrelevant item must not outrank an expensive seed, or the package fills
        // with trivia that happened to be short.
        let w = Weights::default();
        let s = sig();
        let (expensive_seed, _) = score(
            &Inputs {
                seed_proximity: 1.0,
                graph_score: 0.0,
                signals: &s,
                token_cost_norm: 1.0,
            },
            &w,
        );
        let (cheap_noise, _) = score(
            &Inputs {
                seed_proximity: 0.0,
                graph_score: 0.05,
                signals: &s,
                token_cost_norm: 0.0,
            },
            &w,
        );
        assert!(
            expensive_seed > cheap_noise,
            "{expensive_seed} {cheap_noise}"
        );
    }

    #[test]
    fn a_reweighting_changes_the_order_without_a_recompile() {
        let s = Signals {
            churn: 1.0,
            ..Signals::default()
        };
        let quiet = Signals::default();
        fn inputs<'a>(x: &'a Signals) -> Inputs<'a> {
            Inputs {
                seed_proximity: 0.0,
                graph_score: 0.4,
                signals: x,
                token_cost_norm: 0.0,
            }
        }
        let default = Weights::default();
        assert!(score(&inputs(&s), &default).0 > score(&inputs(&quiet), &default).0);

        let ignore_churn = Weights {
            churn: 0.0,
            ..Weights::default()
        };
        assert!(
            (score(&inputs(&s), &ignore_churn).0 - score(&inputs(&quiet), &ignore_churn).0).abs()
                < 1e-9,
            "zeroing a weight must remove its term entirely"
        );
    }
}
