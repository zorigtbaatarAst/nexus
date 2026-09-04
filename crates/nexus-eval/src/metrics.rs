//! Precision, recall, strict site accuracy — each with an interval.

use crate::matcher::Comparison;
use std::collections::HashMap;

const Z: f64 = 1.96;

/// The Wilson score interval.
///
/// Not the normal approximation: per-tier samples are small and their accuracies sit near
/// 1.0, exactly where the normal interval extends past 100 % and stops being an interval.
pub fn wilson(k: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        // No data is no information. Claiming (0,0) would assert certainty of failure.
        return (0.0, 1.0);
    }
    let n_f = n as f64;
    let p = k as f64 / n_f;
    let denom = 1.0 + Z * Z / n_f;
    let centre = (p + Z * Z / (2.0 * n_f)) / denom;
    let half = (Z / denom) * (p * (1.0 - p) / n_f + Z * Z / (4.0 * n_f * n_f)).sqrt();
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Rate {
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub n: u64,
}

impl Rate {
    pub fn new(k: u64, n: u64) -> Self {
        let (low, high) = wilson(k, n);
        Rate {
            value: if n == 0 { 0.0 } else { k as f64 / n as f64 },
            low,
            high,
            n,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Scores {
    /// Edge-level: a fan-out of four with one correct scores 0.25. This is what prices
    /// ambiguity, and the reason precision and recall are always reported as a pair.
    pub precision: Rate,
    /// Site-level: did the truth appear anywhere in the candidate set.
    pub recall: Rate,
    /// Site-level and unforgiving: exactly one candidate, and it is right.
    pub strict: Rate,
    pub f1: f64,
}

pub fn score(c: &Comparison) -> Scores {
    let edges_total = c.judged.len() as u64;
    let edges_correct = c.judged.iter().filter(|j| j.correct).count() as u64;

    let mut per_site: HashMap<&(String, i64), (usize, usize)> = HashMap::new();
    for j in &c.judged {
        let e = per_site.entry(&j.site).or_insert((0, 0));
        e.0 += 1;
        if j.correct {
            e.1 += 1;
        }
    }
    let sites = per_site.len() as u64;
    let sites_hit = per_site.values().filter(|(_, right)| *right > 0).count() as u64;
    let sites_strict = per_site
        .values()
        .filter(|(total, right)| *total == 1 && *right == 1)
        .count() as u64;

    let precision = Rate::new(edges_correct, edges_total);
    let recall = Rate::new(sites_hit, sites);
    let strict = Rate::new(sites_strict, sites);
    let f1 = if precision.value + recall.value > 0.0 {
        2.0 * precision.value * recall.value / (precision.value + recall.value)
    } else {
        0.0
    };
    Scores {
        precision,
        recall,
        strict,
        f1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Comparison;

    fn close(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 5e-4, "{msg}: got {a}, expected {b}");
    }

    fn judged(file: &str, line: i64, correct: bool) -> crate::matcher::Judged {
        crate::matcher::Judged {
            site: (file.into(), line),
            tier: "heuristic".into(),
            confidence: 0.6,
            correct,
        }
    }

    #[test]
    fn wilson_matches_the_hand_computed_interval() {
        // n = 100, k = 80, z = 1.96, z² = 3.8416.
        //   centre = (0.8 + 3.8416/200) / (1 + 0.038416) = 0.819208/1.038416 = 0.788902
        //   half   = (1.96/1.038416) · sqrt(0.16/100 + 3.8416/40000)
        //          = 1.887528 · sqrt(0.00169604) = 1.887528 · 0.041183 = 0.077733
        //
        // The plan's own working divided z²/2 by 2n and then wrote z²/100 for z², and
        // arrived at [0.704011, 0.855057]. The published interval for 80/100 is
        // [0.7112, 0.8666]; the implementation agrees with it and the plan did not.
        let (low, high) = wilson(80, 100);
        close(low, 0.711169, "lower bound");
        close(high, 0.866634, "upper bound");
    }

    #[test]
    fn wilson_stays_inside_zero_and_one_at_the_extremes() {
        // The reason this is Wilson and not the normal approximation: at k == n the normal
        // interval runs past 1.0, and per-tier accuracies sit near 1.0.
        let (low, high) = wilson(12, 12);
        assert!(low > 0.0 && high <= 1.0, "12/12 gave [{low}, {high}]");
        let (low, high) = wilson(0, 12);
        assert!(low >= 0.0 && high < 1.0, "0/12 gave [{low}, {high}]");
    }

    #[test]
    fn an_empty_sample_is_a_zero_width_claim_about_nothing() {
        let (low, high) = wilson(0, 0);
        assert_eq!(
            (low, high),
            (0.0, 1.0),
            "no data means no information, not certainty"
        );
    }

    #[test]
    fn precision_is_edge_level_and_recall_is_site_level() {
        // One site, three candidate edges, one correct: precision 1/3, recall 1/1.
        // Reporting recall alone is the old failure — it is the number that *rises* when the
        // resolver fans out.
        let c = Comparison {
            judged: vec![
                judged("a.rs", 7, true),
                judged("a.rs", 7, false),
                judged("a.rs", 7, false),
            ],
            sites_total: 1,
            ..Default::default()
        };
        let s = score(&c);
        close(s.precision.value, 1.0 / 3.0, "precision");
        close(s.recall.value, 1.0, "recall");
        close(
            s.strict.value,
            0.0,
            "strict: the site was ambiguous, so it is not strictly right",
        );
        close(s.f1, 0.5, "f1 of 1/3 and 1");
    }

    #[test]
    fn a_single_correct_edge_at_a_site_is_strictly_right() {
        let c = Comparison {
            judged: vec![judged("a.rs", 7, true)],
            sites_total: 1,
            ..Default::default()
        };
        let s = score(&c);
        close(s.strict.value, 1.0, "one candidate, correct");
    }
}
