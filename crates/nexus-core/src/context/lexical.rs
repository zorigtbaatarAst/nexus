//! BM25 over file contents — the control arm's ranking function.
//!
//! This exists so that A1-vs-A5 in the Tier 2 benchmark compares two *rankers* and nothing
//! else. It deliberately knows nothing about symbols, edges, history or memory: if the
//! Context Engine cannot beat term-frequency scoring over raw file text at the same token
//! budget, that is the finding the benchmark is for.

use std::collections::HashMap;

/// Which ranking function builds the package.
///
/// `Engine` is the product. `Lexical` exists for the Tier 2 control arm and is not a
/// documented feature — see `docs/superpowers/specs/2026-09-04-tier2-benchmark-design.md` §5
/// for why it lives in the product binary rather than in the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RankMode {
    #[default]
    Engine,
    Lexical,
}

impl RankMode {
    pub fn parse(value: &str) -> Option<RankMode> {
        match value {
            "engine" => Some(RankMode::Engine),
            "lexical" => Some(RankMode::Lexical),
            _ => None,
        }
    }
}

/// Okapi BM25 with the standard parameters.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Lowercased alphanumeric runs. Deliberately crude: a control arm that needed tuning to be
/// fair would not be a control.
fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Score every document against the query, best first.
///
/// Ties break on path so two runs of the same query rank identically — a benchmark whose
/// control arm reorders itself between runs is recording noise.
pub fn bm25(query: &str, docs: &[(String, String)]) -> Vec<(String, f64)> {
    let n = docs.len() as f64;
    if n == 0.0 {
        return Vec::new();
    }

    let tokenised: Vec<(String, Vec<String>)> = docs
        .iter()
        .map(|(path, body)| (path.clone(), terms(body)))
        .collect();
    let avgdl = tokenised.iter().map(|(_, t)| t.len() as f64).sum::<f64>() / n;

    // How many documents contain each term, for IDF.
    let mut doc_freq: HashMap<&str, f64> = HashMap::new();
    for (_, tokens) in &tokenised {
        let mut seen: Vec<&str> = tokens.iter().map(String::as_str).collect();
        seen.sort_unstable();
        seen.dedup();
        for t in seen {
            *doc_freq.entry(t).or_insert(0.0) += 1.0;
        }
    }

    let query_terms = terms(query);
    let mut scored: Vec<(String, f64)> = tokenised
        .iter()
        .map(|(path, tokens)| {
            let len = tokens.len() as f64;
            let mut counts: HashMap<&str, f64> = HashMap::new();
            for t in tokens {
                *counts.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
            let score = query_terms
                .iter()
                .map(|q| {
                    let f = counts.get(q.as_str()).copied().unwrap_or(0.0);
                    if f == 0.0 {
                        return 0.0;
                    }
                    let df = doc_freq.get(q.as_str()).copied().unwrap_or(0.0);
                    // Standard BM25 IDF, +1 inside the log so it is never negative.
                    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                    idf * (f * (K1 + 1.0)) / (f + K1 * (1.0 - B + B * len / avgdl))
                })
                .sum();
            (path.clone(), score)
        })
        .collect();

    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
}

#[cfg(test)]
mod tests {
    use super::{bm25, RankMode};

    fn docs() -> Vec<(String, String)> {
        vec![
            (
                "src/Payment.java".into(),
                "class Payment { idempotency key String idempotencyKey; }".into(),
            ),
            ("src/Order.java".into(), "class Order { int total; }".into()),
            (
                "README.md".into(),
                "This project handles payment idempotency and orders".into(),
            ),
        ]
    }

    #[test]
    fn a_document_containing_the_query_terms_outranks_one_that_does_not() {
        let ranked = bm25("idempotency key", &docs());
        assert_eq!(ranked[0].0, "src/Payment.java", "{ranked:?}");
        assert!(ranked[0].1 > 0.0);
    }

    #[test]
    fn a_document_matching_nothing_scores_zero() {
        let ranked = bm25("kubernetes", &docs());
        assert!(ranked.iter().all(|(_, s)| *s == 0.0), "{ranked:?}");
    }

    #[test]
    fn a_rare_term_outweighs_a_common_one() {
        // "payment" appears in two of three documents; "idempotencykey" in one. IDF is the
        // whole reason BM25 is a fair control rather than a strawman — a ranker that ignored
        // it would lose to the Context Engine for the wrong reason.
        let ranked = bm25("payment idempotencyKey", &docs());
        assert_eq!(ranked[0].0, "src/Payment.java", "{ranked:?}");
    }

    #[test]
    fn ranking_is_stable_for_equal_scores() {
        // Two runs of the same query must produce the same order, or the golden benchmark
        // records noise. Ties break on path.
        // Use a dedicated fixture that guarantees equal scores (two identical-length docs,
        // same term frequency for the query term).
        let tie_fixture = vec![
            ("B".into(), "class Foo".into()),
            ("A".into(), "class Bar".into()),
        ];
        let first = bm25("class", &tie_fixture);
        let second = bm25("class", &tie_fixture);

        // Verify the fixture actually produces a tie (both docs score identically).
        assert_eq!(
            first[0].1, first[1].1,
            "fixture must produce tied scores for this test to be meaningful: {first:?}"
        );

        // Verify deterministic tie-breaking on path across runs.
        assert_eq!(
            first.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            second.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_rank_mode_parses_only_what_it_offers() {
        assert_eq!(RankMode::parse("lexical"), Some(RankMode::Lexical));
        assert_eq!(RankMode::parse("engine"), Some(RankMode::Engine));
        assert_eq!(RankMode::parse("bm25"), None, "one spelling, not two");
    }
}
