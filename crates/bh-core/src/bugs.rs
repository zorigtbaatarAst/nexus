//! Bug identity and the lifecycle rules.
//!
//! A finding is not a bug until it has an identity that survives the next scan. ADR-007.

use bh_types::{BugStatus, BugType, Severity};
use serde::{Deserialize, Serialize};

/// A place in the source that supports a claim.
///
/// A candidate with no `CodeRef` is rejected at the boundary rather than down-ranked: an
/// assertion nobody can check is not evidence, and storing it would let the next reader
/// mistake it for one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRef {
    pub file: String,
    pub line: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugCandidate {
    pub bug_type: BugType,
    pub title: String,
    /// The class, module or component this belongs to. Part of the identity, so it must be
    /// stable across formatting — a class name, never a file path.
    pub component: String,
    pub anchor_fqn: Option<String>,
    pub severity: Severity,
    pub confidence: f64,
    /// `detector:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new bug.
    pub detector: String,
    /// The detector's own normalization of what the bug is *about*: the shared state, the
    /// endpoint, the field. This is what separates two different bugs in the same class.
    pub structural_key: String,
    /// Human-readable identity, shown instead of the hash. Never used *as* identity.
    pub slug: String,
    pub evidence: Vec<CodeRef>,
}

impl BugCandidate {
    /// Deterministic detectors carry evidence by construction, so they open at
    /// `UNVERIFIED` rather than `SUSPECTED`: there is nothing left to attach.
    pub fn initial_status(&self) -> BugStatus {
        if self.evidence.is_empty() {
            BugStatus::Suspected
        } else {
            BugStatus::Unverified
        }
    }

    pub fn detector_family(&self) -> &str {
        self.detector.split(':').next().unwrap_or(&self.detector)
    }

    /// ADR-007.
    ///
    /// Excluded on purpose: file path, line numbers, commit sha, title wording, confidence,
    /// severity — every one of them changes without the bug changing.
    ///
    /// Included on purpose: the anchor's *shape* (package, type and member with generics and
    /// parameter types normalized away) so a parameter rename does not invent a duplicate,
    /// while a genuine move to another type does change identity.
    pub fn fingerprint(&self) -> String {
        let anchor = self
            .anchor_fqn
            .as_deref()
            .map(fqn_shape)
            .unwrap_or_default();
        let material = format!(
            "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
            self.bug_type.as_str(),
            self.component,
            anchor,
            self.detector_family(),
            self.structural_key,
        );
        blake3::hash(material.as_bytes()).to_hex()[..32].to_string()
    }
}

/// `mn.pay.PaymentService#createPayment(String,Money)` → `mn.pay.PaymentService#createPayment`
///
/// Dropping the parameter list is what lets an added overload or a renamed parameter leave
/// identity alone. A move to another class still changes it, which is correct: that is a
/// different bug in a different place.
pub fn fqn_shape(fqn: &str) -> String {
    let no_generics: String = {
        let mut out = String::with_capacity(fqn.len());
        let mut depth = 0i32;
        for c in fqn.chars() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    };
    match no_generics.find('(') {
        Some(i) => no_generics[..i].to_string(),
        None => no_generics,
    }
}

/// Whether a stored bug should become `FIXED` this scan.
///
/// The rule from docs/change-analysis.md §10 is that absence is not evidence — but for a
/// *deterministic* detector, absence after re-examining the exact anchor is evidence: the
/// rule ran again over the same code and did not fire. That is a genuinely different
/// situation from an AI-sourced finding whose region simply was not looked at, and
/// conflating them would either never close a fixed bug or close bugs nobody fixed.
pub fn deterministic_fix_confirmed(anchor_reexamined: bool, still_fires: bool) -> bool {
    anchor_reexamined && !still_fires
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(anchor: &str, key: &str) -> BugCandidate {
        BugCandidate {
            bug_type: BugType::Transaction,
            title: "whatever".into(),
            component: "PaymentService".into(),
            anchor_fqn: Some(anchor.into()),
            severity: Severity::High,
            confidence: 0.9,
            detector: "spring:self-invocation".into(),
            structural_key: key.into(),
            slug: "x".into(),
            evidence: vec![CodeRef {
                file: "a.java".into(),
                line: 1,
                note: "n".into(),
            }],
        }
    }

    #[test]
    fn a_parameter_rename_or_an_overload_does_not_change_identity() {
        let a = candidate("mn.pay.PaymentService#pay(String,Money)", "k");
        let b = candidate("mn.pay.PaymentService#pay(String,Money,boolean)", "k");
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn moving_the_method_to_another_class_is_a_different_bug() {
        let a = candidate("mn.pay.PaymentService#pay(String)", "k");
        let b = candidate("mn.billing.PaymentService#pay(String)", "k");
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn two_different_bugs_in_one_class_do_not_collide() {
        let a = candidate("mn.pay.PaymentService#pay(String)", "payment.status");
        let b = candidate("mn.pay.PaymentService#pay(String)", "refund.status");
        assert_ne!(
            a.fingerprint(),
            b.fingerprint(),
            "the structural key is what separates them"
        );
    }

    #[test]
    fn wording_severity_and_confidence_are_not_identity() {
        let mut a = candidate("mn.pay.PaymentService#pay(String)", "k");
        let b = {
            let mut b = a.clone();
            b.title = "rephrased by a model".into();
            b.severity = Severity::Low;
            b.confidence = 0.1;
            b.detector = "spring:renamed-rule".into();
            b
        };
        a.slug = "different-slug".into();
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn generics_are_stripped_from_the_shape() {
        assert_eq!(fqn_shape("p.C#f(List<Map<String,Integer>>)"), "p.C#f");
        assert_eq!(fqn_shape("p.C"), "p.C");
    }

    #[test]
    fn evidence_decides_the_opening_status() {
        let with = candidate("p.C#f()", "k");
        assert_eq!(with.initial_status(), BugStatus::Unverified);
        let mut without = with.clone();
        without.evidence.clear();
        assert_eq!(without.initial_status(), BugStatus::Suspected);
    }

    #[test]
    fn a_fix_needs_the_anchor_to_have_been_looked_at() {
        assert!(deterministic_fix_confirmed(true, false));
        assert!(
            !deterministic_fix_confirmed(false, false),
            "not looking is not evidence"
        );
        assert!(!deterministic_fix_confirmed(true, true));
    }
}
