//! Moving findings and facts between machines, without a server (roadmap 3.6, §7).
//!
//! One JSON document, written by `export` and read by `import`. A committed file is the first
//! answer to a shared store, and N13 keeps it that way until export/import is *proven*
//! insufficient rather than merely felt to be.
//!
//! Two rules the format exists to enforce:
//!
//!   * **Evidence travels as references, never as source text.** A path and a line, so the
//!     file is safe to commit and safe to send. Source text would make a knowledge file a
//!     second copy of the repository with none of its access control.
//!   * **A conflict is reported, never resolved.** Two people who believe different things
//!     about the same key have a disagreement, and silently picking one produces a database
//!     that says something neither of them said.

use serde::{Deserialize, Serialize};

/// The document version. Bumped when a reader could otherwise misread an older file — a
/// number in the file beats guessing from its shape.
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Portable {
    pub format: u32,
    /// Where it came from. Provenance for a human reading a merge conflict, never matched on.
    pub project: String,
    pub exported_at: String,
    #[serde(default)]
    pub facts: Vec<PortableFact>,
    #[serde(default)]
    pub findings: Vec<PortableFinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortableFact {
    pub key: String,
    pub scope: String,
    pub subject: Option<String>,
    pub claim: String,
    pub source: String,
    pub confidence: f64,
    /// `path:line`, never source text.
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortableFinding {
    /// Identity. The same defect found on two machines has the same fingerprint, which is
    /// what makes a merge a merge rather than a pile.
    pub fingerprint: String,
    pub capability: String,
    pub uid: String,
    pub title: String,
    pub finding_type: String,
    pub severity: String,
    pub status: String,
    pub component: Option<String>,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// What an import did, and what it refused to do.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub facts_added: usize,
    pub facts_unchanged: usize,
    pub findings_added: usize,
    pub findings_unchanged: usize,
    /// One line per disagreement. Nothing here was applied.
    pub conflicts: Vec<String>,
}

impl ImportReport {
    pub fn changed(&self) -> bool {
        self.facts_added > 0 || self.findings_added > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_round_trips() {
        let doc = Portable {
            format: FORMAT,
            project: "p".into(),
            exported_at: "2026-09-03T00:00:00Z".into(),
            facts: vec![PortableFact {
                key: "invariant.a".into(),
                scope: "symbol".into(),
                subject: Some("mn.pay.A".into()),
                claim: "c".into(),
                source: "human".into(),
                confidence: 1.0,
                evidence: vec!["a.java:3".into()],
            }],
            findings: Vec::new(),
        };
        let raw = serde_json::to_string(&doc).expect("write");
        let back: Portable = serde_json::from_str(&raw).expect("read");
        assert_eq!(back.facts, doc.facts);
    }

    #[test]
    fn a_document_with_no_findings_key_still_reads() {
        // An older or hand-written file must not fail on an absent list. Refusing to read a
        // file over a missing empty array is a worse failure than the one it prevents.
        let back: Portable =
            serde_json::from_str(r#"{"format":1,"project":"p","exported_at":"t","facts":[]}"#)
                .expect("read");
        assert!(back.findings.is_empty());
    }
}
