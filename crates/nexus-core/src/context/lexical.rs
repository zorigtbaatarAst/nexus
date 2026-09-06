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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// How many distinct *discriminating* query terms the corpus contains.
///
/// The fallback's gate. A prompt that shares one stray word with the code — "thanks, that
/// works" catching `works` in a comment — is noise, and the package it produces is paid on
/// every prompt of every session by the `UserPromptSubmit` hook: exactly the cost `--brief`
/// exists to hold at zero.
///
/// Two filters, both borrowed rather than invented. `seeds::is_noise_word` is the standard
/// the seeder already applies to a bare word — too short, or one every prompt contains — and
/// sharing it stops the gate and the seeder disagreeing about what an ordinary word is. On top
/// of that, a term in most of the documents separates nothing, which is what BM25's own IDF
/// says about it. Counting bare presence made "park it, decision kept" look as corroborated as
/// a real symptom, because `it` is rare enough in a small code corpus to look discriminating.
///
/// Document frequency rather than a score threshold: BM25 scores scale with corpus size and
/// term frequency, so a cutoff calibrated on one repository transfers to no other. "In fewer
/// than half the files" means the same thing everywhere.
pub fn query_overlap(query: &str, docs: &[(String, String)]) -> usize {
    if docs.is_empty() {
        return 0;
    }
    let mut doc_freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, body) in docs {
        let mut seen: Vec<String> = terms(body);
        seen.sort();
        seen.dedup();
        for t in seen {
            *doc_freq.entry(t).or_insert(0) += 1;
        }
    }
    let ceiling = docs.len().div_ceil(2);
    let mut hits: Vec<String> = terms(query)
        .into_iter()
        .filter(|t| !crate::context::seeds::is_noise_word(t))
        .filter(|t| doc_freq.get(t).is_some_and(|&df| df < ceiling))
        .collect();
    hits.sort();
    hits.dedup();
    hits.len()
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
    /// The gate decides whether a package is sent on every prompt of every session, so each
    /// of its three filters is pinned separately: a word too ordinary to carry evidence, a
    /// word the corpus does not contain, and a word in so much of the corpus that it
    /// separates nothing.
    fn corpus() -> Vec<(String, String)> {
        vec![
            (
                "a.rs".into(),
                "the payment charge is idempotent here".into(),
            ),
            ("b.rs".into(), "the payment ledger writes a row".into()),
            ("c.rs".into(), "the payment retry backs off".into()),
        ]
    }

    #[test]
    fn two_distinctive_words_the_corpus_has_are_enough() {
        // `ledger` and `retry` are in one file each; `wrong` is in none, and `row` would be
        // excluded on length even though the corpus contains it — the seeder would not seed
        // on a three-letter word either.
        assert_eq!(query_overlap("the ledger retry is wrong", &corpus()), 2);
        assert_eq!(query_overlap("the ledger row is wrong", &corpus()), 1);
    }

    /// The ceiling is `df < ceil(n/2)`, and the boundary is where an off-by-one would hide.
    /// Four documents, so the ceiling is 2: a term in one file counts, a term in two does not.
    #[test]
    fn the_ceiling_is_fewer_than_half_the_files_not_half_or_fewer() {
        let four = vec![
            ("a.rs".into(), "alpha shared".to_string()),
            ("b.rs".into(), "beta shared".to_string()),
            ("c.rs".into(), "gamma".to_string()),
            ("d.rs".into(), "delta".to_string()),
        ];
        // `shared` is in two of four — half, which separates too little to corroborate.
        assert_eq!(query_overlap("shared", &four), 0);
        // `alpha` is in one of four, under the ceiling.
        assert_eq!(query_overlap("alpha", &four), 1);
        // Both together are still one, because `shared` never counts.
        assert_eq!(query_overlap("alpha shared", &four), 1);
        assert_eq!(query_overlap("alpha beta shared", &four), 2);
    }

    #[test]
    fn a_word_in_most_of_the_corpus_separates_nothing() {
        // `payment` is in every file, so it corroborates nothing — the same thing BM25's
        // IDF says about it. Only `ledger` counts, which is below the caller's bar of two.
        assert_eq!(query_overlap("the payment ledger", &corpus()), 1);
    }

    #[test]
    fn ordinary_words_do_not_corroborate_however_rare_they_are() {
        // "it" and "off" are rare here and would look discriminating on document frequency
        // alone. They are excluded because the seeder would not seed on them either — the
        // shared standard is what stops the two disagreeing.
        assert_eq!(query_overlap("park it, decision kept off", &corpus()), 0);
    }

    #[test]
    fn a_word_the_corpus_does_not_contain_counts_for_nothing() {
        assert_eq!(query_overlap("kubernetes helm chart rollout", &corpus()), 0);
    }

    #[test]
    fn an_empty_corpus_corroborates_nothing_rather_than_dividing_by_zero() {
        assert_eq!(query_overlap("ledger retry", &[]), 0);
    }

    use super::{bm25, query_overlap, RankMode};

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
