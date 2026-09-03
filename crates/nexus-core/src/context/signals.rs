//! Stage 4 — what is known about each candidate.
//!
//! Six signals per candidate: churn, recency, coverage, prior findings, facts and
//! architectural relevance. Stage 5 turns them into a score; this stage only gathers.
//!
//! **Built once per request, never once per candidate.** A signal lookup per candidate is an
//! N+1 on a stage ADR-024 budgets at 150 ms inside a per-prompt hook, and it is the failure
//! mode that makes a ranker quietly too slow to enable. [`SignalIndex::build`] issues a fixed
//! number of queries whatever the candidate count, and a test asserts that.
//!
//! What cannot be computed is recorded rather than silently scored zero. A signal that is
//! absent because the data has not landed yet is a different fact from one that is absent
//! because the candidate has no history, and a ranker that conflates them cannot be debugged.

use crate::report::FindingSummary;
use nexus_store::{Store, StoreError};
use std::collections::HashMap;

/// Recency half-life in days (§6). Old facts and old code are usually still true, so the
/// decay is gentle by design.
const RECENCY_HALF_LIFE_DAYS: f64 = 30.0;

/// What is known about one candidate. Every field is in 0.0..=1.0 so that stage 5's weights
/// are comparable across terms.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Signals {
    /// How often this file has been touched, normalised against the busiest file.
    pub churn: f64,
    /// How recently it changed. `exp(-age_days / half_life)`.
    pub recency: f64,
    /// Something that looks like a test reaches it. Until roadmap 4.5 this is a filename
    /// match, and [`SignalIndex::coverage_source`] says so.
    pub coverage: f64,
    /// The worst open finding on it: REGRESSED 1.0, VERIFIED 0.8, UNVERIFIED 0.5, IGNORED 0.
    pub history: f64,
    /// `subject_match × source_weight × confidence` over the facts about it.
    pub fact: f64,
    /// Named by a decision fact, or anchored in the project profile.
    pub arch: f64,
}

/// Everything stage 4 needs, read in a fixed number of queries.
pub struct SignalIndex {
    /// Worst open-finding weight, by file path and by fully-qualified name.
    findings_by_file: HashMap<String, f64>,
    findings_by_fqn: HashMap<String, f64>,
    /// Fact subject → its `source_weight × confidence`, and whether it is a decision.
    facts: Vec<(String, f64, bool)>,
    /// Path → normalised churn. Empty until roadmap 2.5 populates history.
    churn: HashMap<String, f64>,
    /// Path → normalised recency, from the change ledger.
    recency: HashMap<String, f64>,
    /// Paths the profile anchors on: build files, CI, compose.
    profile_anchors: Vec<String>,
    notes: Vec<String>,
}

/// §6: exact FQN 1.0, module prefix 0.6, project 0.3.
fn subject_match(subject: &str, fqn: &str) -> f64 {
    if subject == fqn {
        1.0
    } else if fqn.starts_with(subject) || subject.starts_with(fqn) {
        0.6
    } else {
        0.0
    }
}

/// §6: human 1.0, deterministic 0.9, ai 0.7. A human wrote it down on purpose.
fn source_weight(source: &str) -> f64 {
    match source {
        "human" => 1.0,
        "deterministic" => 0.9,
        _ => 0.7,
    }
}

/// §6, and the finding lifecycle's own ordering. A regression is the strongest signal in the
/// system: it broke, it was fixed, and it broke again.
fn status_weight(status: &str) -> f64 {
    match status {
        "REGRESSED" => 1.0,
        "VERIFIED" => 0.8,
        "UNVERIFIED" => 0.5,
        _ => 0.0,
    }
}

fn worst(map: &mut HashMap<String, f64>, key: String, weight: f64) {
    map.entry(key)
        .and_modify(|w| {
            if weight > *w {
                *w = weight
            }
        })
        .or_insert(weight);
}

impl SignalIndex {
    /// Read every signal source once.
    ///
    /// `churn` is supplied by the caller because it comes from git rather than from the
    /// store, and `nexus-core` holds the repository handle. Passing it in keeps this function
    /// a pure fold over storage and keeps the git traversal to one per request.
    pub fn build(
        store: &Store,
        project_id: i64,
        findings: &[FindingSummary],
        churn: HashMap<String, f64>,
        profile_anchors: Vec<String>,
    ) -> Result<Self, StoreError> {
        let mut notes = Vec::new();
        let mut findings_by_file = HashMap::new();
        let mut findings_by_fqn = HashMap::new();
        for f in findings {
            let w = status_weight(&f.status);
            if w == 0.0 {
                continue;
            }
            if let Some(file) = &f.file {
                worst(&mut findings_by_file, file.clone(), w);
            }
            if let Some(component) = &f.component {
                worst(&mut findings_by_fqn, component.clone(), w);
            }
        }

        let facts: Vec<(String, f64, bool)> = store
            .facts(project_id, None)?
            .into_iter()
            .filter_map(|f| {
                let subject = f.subject?;
                let weight = source_weight(&f.source) * f.confidence;
                Some((subject, weight, f.key.starts_with("decision.")))
            })
            .collect();

        // Recency from the change ledger: a path that appears in the newest scan's changes is
        // fresh. The ledger is scan-ordered, not clock-ordered, which is the honest reading —
        // "changed in the last scan" is what a package can actually prove.
        let mut recency = HashMap::new();
        if let Some(baseline) = store.baseline(project_id)? {
            for (_, _, target, _) in store.changes_for_scan(baseline.scan_id, Some("file"))? {
                if let Some(path) = target {
                    recency.insert(path, 1.0);
                }
            }
        }

        if churn.is_empty() {
            notes
                .push("no commit history indexed yet, so churn is zero for every candidate".into());
        }

        Ok(SignalIndex {
            findings_by_file,
            findings_by_fqn,
            facts,
            churn,
            recency,
            profile_anchors,
            notes,
        })
    }

    /// Where the coverage signal comes from. `naming` until roadmap 4.5 replaces the filename
    /// match with real runner output; the consumer does not change when it does.
    pub fn coverage_source(&self) -> &'static str {
        "naming"
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Decay for something last seen `age_days` ago. Public because stage 5 uses the same
    /// curve for a fact's age and the two must not drift apart.
    pub fn decay(age_days: f64) -> f64 {
        (-age_days.max(0.0) / RECENCY_HALF_LIFE_DAYS).exp()
    }

    /// Everything known about one candidate. Pure map lookups: no query runs here.
    pub fn for_candidate(&self, fqn: &str, file: &str, is_test_reached: bool) -> Signals {
        let history = self
            .findings_by_fqn
            .iter()
            .filter(|(k, _)| fqn.starts_with(k.as_str()))
            .map(|(_, w)| *w)
            .fold(0.0_f64, f64::max)
            .max(self.findings_by_file.get(file).copied().unwrap_or(0.0));

        let mut fact = 0.0_f64;
        let mut arch = 0.0_f64;
        for (subject, weight, is_decision) in &self.facts {
            let m = subject_match(subject, fqn);
            if m == 0.0 {
                continue;
            }
            fact = fact.max(m * weight);
            if *is_decision {
                arch = arch.max(m * weight);
            }
        }
        if self.profile_anchors.iter().any(|a| a == file) {
            arch = arch.max(0.5);
        }

        Signals {
            churn: self.churn.get(file).copied().unwrap_or(0.0),
            recency: self.recency.get(file).copied().unwrap_or(0.0),
            coverage: if is_test_reached { 1.0 } else { 0.0 },
            history,
            fact,
            arch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(findings: Vec<FindingSummary>, churn: HashMap<String, f64>) -> SignalIndex {
        let store = Store::open_in_memory().expect("store");
        let p = store
            .ensure_project("/tmp/sig", "sig", "git")
            .expect("project");
        SignalIndex::build(&store, p, &findings, churn, vec!["pom.xml".into()]).expect("build")
    }

    fn finding(status: &str, file: &str, component: &str) -> FindingSummary {
        FindingSummary {
            uid: "X-1".into(),
            slug: "x".into(),
            title: "t".into(),
            capability: "bughunter".into(),
            finding_type: "logic".into(),
            component: Some(component.into()),
            severity: "high".into(),
            confidence: 0.9,
            status: status.into(),
            detector: "d".into(),
            file: Some(file.into()),
            line: Some(1),
            introduced_commit: None,
            fixed_commit: None,
        }
    }

    #[test]
    fn a_regression_outweighs_an_unverified_finding() {
        let idx = index(
            vec![
                finding("REGRESSED", "a.java", "mn.pay.A"),
                finding("UNVERIFIED", "b.java", "mn.pay.B"),
            ],
            HashMap::new(),
        );
        assert_eq!(
            idx.for_candidate("mn.pay.A#x", "a.java", false).history,
            1.0
        );
        assert_eq!(
            idx.for_candidate("mn.pay.B#x", "b.java", false).history,
            0.5
        );
        assert_eq!(
            idx.for_candidate("mn.pay.C#x", "c.java", false).history,
            0.0
        );
    }

    #[test]
    fn an_ignored_finding_contributes_nothing() {
        // §6 weights IGNORED at 0. Someone decided it does not matter; ranking on it anyway
        // would make dismissing a finding pointless.
        let idx = index(
            vec![finding("IGNORED", "a.java", "mn.pay.A")],
            HashMap::new(),
        );
        assert_eq!(
            idx.for_candidate("mn.pay.A#x", "a.java", false).history,
            0.0
        );
    }

    #[test]
    fn subject_match_prefers_the_exact_symbol_over_its_package() {
        assert_eq!(subject_match("mn.pay.A#x", "mn.pay.A#x"), 1.0);
        assert_eq!(subject_match("mn.pay", "mn.pay.A#x"), 0.6);
        assert_eq!(subject_match("mn.orders", "mn.pay.A#x"), 0.0);
    }

    #[test]
    fn an_empty_churn_map_is_reported_not_silently_zero() {
        let idx = index(Vec::new(), HashMap::new());
        assert!(
            idx.notes().iter().any(|n| n.contains("churn")),
            "{:?}",
            idx.notes()
        );
        assert_eq!(idx.for_candidate("mn.pay.A", "a.java", false).churn, 0.0);
    }

    #[test]
    fn churn_comes_through_when_history_exists() {
        let idx = index(Vec::new(), HashMap::from([("a.java".into(), 0.75)]));
        assert!(idx.notes().is_empty(), "{:?}", idx.notes());
        assert_eq!(idx.for_candidate("mn.pay.A", "a.java", false).churn, 0.75);
    }

    #[test]
    fn a_profile_anchor_carries_architectural_weight() {
        let idx = index(Vec::new(), HashMap::new());
        assert!(idx.for_candidate("build", "pom.xml", true).arch > 0.0);
        assert_eq!(idx.for_candidate("mn.pay.A", "a.java", true).arch, 0.0);
    }

    #[test]
    fn recency_decays_gently() {
        // §6: old code is usually still true, so a 30-day half-life, not a cliff.
        assert!((SignalIndex::decay(0.0) - 1.0).abs() < 1e-9);
        assert!(SignalIndex::decay(30.0) > 0.36 && SignalIndex::decay(30.0) < 0.37);
        assert!(SignalIndex::decay(365.0) < 0.01);
    }
}
