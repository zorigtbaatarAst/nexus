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

/// The Brier score: mean squared error of a probability claim.
///
/// It is a **strictly proper scoring rule** — minimised only by reporting one's true belief.
/// That is what makes it safe to track: the score cannot be improved by inflating confidences
/// to look decisive, or deflating them to look cautious.
pub fn brier(c: &Comparison) -> f64 {
    if c.judged.is_empty() {
        return 0.0;
    }
    let sum: f64 = c
        .judged
        .iter()
        .map(|j| {
            let y = if j.correct { 1.0 } else { 0.0 };
            (j.confidence - y).powi(2)
        })
        .sum();
    sum / c.judged.len() as f64
}

/// Above this an interval is too wide to justify changing a constant.
const MAX_HALF_WIDTH: f64 = 0.15;
/// Below this a tier gets no verdict at all. §5.7: ±0.05 at p≈0.8 needs about 246 samples.
const POWER_FLOOR: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Verdict {
    Ok,
    Miscalibrated,
    UnderPowered,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TierResult {
    pub tier: String,
    pub claimed: f64,
    pub measured: Rate,
    pub verdict: Verdict,
    /// The Jeffreys posterior mean, offered only when the evidence can carry it.
    pub proposed: Option<f64>,
}

pub fn calibrate(c: &Comparison) -> Vec<TierResult> {
    // Bins are tiers, not equal-width buckets: confidence here is a set of discrete
    // constants, so calibration is a hypothesis test per tier rather than a smoothing
    // exercise. A tier carrying more than one claimed value is keyed by both.
    let mut groups: HashMap<(String, u64), (u64, u64)> = HashMap::new();
    for j in &c.judged {
        let key = (j.tier.clone(), (j.confidence * 1000.0).round() as u64);
        let e = groups.entry(key).or_insert((0, 0));
        e.1 += 1;
        if j.correct {
            e.0 += 1;
        }
    }

    let mut out: Vec<TierResult> = groups
        .into_iter()
        .map(|((tier, claimed_milli), (k, n))| {
            let claimed = claimed_milli as f64 / 1000.0;
            let measured = Rate::new(k, n);
            let half = (measured.high - measured.low) / 2.0;
            let (verdict, proposed) = if n < POWER_FLOOR || half > MAX_HALF_WIDTH {
                (Verdict::UnderPowered, None)
            } else if claimed < measured.low || claimed > measured.high {
                // Jeffreys posterior mean under Beta(1/2, 1/2). Not k/n, which proposes
                // 1.00 off a 12-for-12 run.
                (
                    Verdict::Miscalibrated,
                    Some((k as f64 + 0.5) / (n as f64 + 1.0)),
                )
            } else {
                (Verdict::Ok, None)
            };
            TierResult {
                tier,
                claimed,
                measured,
                verdict,
                proposed,
            }
        })
        .collect();
    // Biggest sample first: the tiers whose verdicts are worth acting on lead the report.
    out.sort_by_key(|t| std::cmp::Reverse(t.measured.n));
    out
}

/// Expected calibration error, weighted by each tier's share of the edges.
pub fn ece(tiers: &[TierResult]) -> f64 {
    let total: u64 = tiers.iter().map(|t| t.measured.n).sum();
    if total == 0 {
        return 0.0;
    }
    tiers
        .iter()
        .map(|t| (t.measured.n as f64 / total as f64) * (t.measured.value - t.claimed).abs())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Comparison;

    fn close(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 5e-4, "{msg}: got {a}, expected {b}");
    }

    fn with_conf(tier: &str, confidence: f64, correct: bool) -> crate::matcher::Judged {
        crate::matcher::Judged {
            site: (format!("{tier}.rs"), 1),
            tier: tier.into(),
            confidence,
            correct,
        }
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

    #[test]
    fn brier_matches_the_hand_computed_score() {
        // Three edges: 0.9 correct, 0.6 wrong, 1.0 correct.
        //   (0.9-1)² + (0.6-0)² + (1.0-1)² = 0.01 + 0.36 + 0 = 0.37; /3 = 0.123333
        let c = Comparison {
            judged: vec![
                with_conf("heuristic", 0.9, true),
                with_conf("heuristic", 0.6, false),
                with_conf("exact", 1.0, true),
            ],
            sites_total: 3,
            ..Default::default()
        };
        close(brier(&c), 0.123333, "brier");
    }

    #[test]
    fn a_tier_whose_claim_falls_outside_its_interval_is_miscalibrated() {
        // 200 bare-member edges claiming 0.60, of which 80 are right. Measured 0.40, and
        // 0.60 is nowhere near the interval.
        let mut judged = Vec::new();
        for i in 0..200 {
            judged.push(with_conf("heuristic", 0.6, i < 80));
        }
        let c = Comparison {
            judged,
            sites_total: 200,
            ..Default::default()
        };
        let tiers = calibrate(&c);
        let t = tiers
            .iter()
            .find(|t| t.tier == "heuristic")
            .expect("tier present");
        assert_eq!(t.verdict, Verdict::Miscalibrated);
        // Jeffreys posterior mean: (80 + 0.5) / (200 + 1) = 0.400498
        close(t.proposed.expect("a proposal"), 0.400498, "jeffreys estimate");
    }

    #[test]
    fn a_tier_with_too_little_evidence_proposes_nothing() {
        // Nine edges cannot justify a config change. Under-powered measurement laundering
        // itself into a constant is R8 wearing a lab coat.
        let judged: Vec<_> = (0..9).map(|i| with_conf("heuristic", 0.6, i < 3)).collect();
        let c = Comparison {
            judged,
            sites_total: 9,
            ..Default::default()
        };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::UnderPowered);
        assert!(t.proposed.is_none(), "no proposal below the power floor");
    }

    #[test]
    fn a_well_calibrated_tier_is_left_alone() {
        // 200 edges claiming 0.90, 180 right. Measured 0.90; the claim sits in the interval.
        let judged: Vec<_> = (0..200)
            .map(|i| with_conf("overload", 0.9, i < 180))
            .collect();
        let c = Comparison {
            judged,
            sites_total: 200,
            ..Default::default()
        };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::Ok);
        assert!(t.proposed.is_none());
    }

    #[test]
    fn a_tier_that_has_never_been_wrong_may_claim_certainty() {
        // The `exact` tier claims 1.00, and Wilson's upper bound is *exactly* 1.0 when
        // k == n — the p(1-p) term vanishes and centre + half collapses to (1+z²/n)/(1+z²/n).
        // Without that, a tier with a perfect record would be reported as overclaiming for
        // ever, and a gate that cries wolf on its own best tier gets switched off.
        let judged: Vec<_> = (0..200).map(|_| with_conf("exact", 1.0, true)).collect();
        let c = Comparison {
            judged,
            sites_total: 200,
            ..Default::default()
        };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::Ok, "200 for 200 does not refute p = 1.0");
    }

    #[test]
    fn one_wrong_edge_refutes_a_claim_of_certainty() {
        // 199 of 200 claiming 1.00. The plan's own draft expected `Ok` here; it is not.
        // Confidence 1.00 asserts "never wrong", and a single counter-example ends that
        // claim — the interval [0.9722, 0.9991] excludes 1.0 for exactly that reason.
        let judged: Vec<_> = (0..200).map(|i| with_conf("exact", 1.0, i < 199)).collect();
        let c = Comparison {
            judged,
            sites_total: 200,
            ..Default::default()
        };
        let t = &calibrate(&c)[0];
        assert_eq!(t.verdict, Verdict::Miscalibrated);
        // Jeffreys, not k/n: (199 + 0.5) / (200 + 1) = 0.992537
        close(t.proposed.expect("a proposal"), 0.992537, "jeffreys estimate");
    }
}
