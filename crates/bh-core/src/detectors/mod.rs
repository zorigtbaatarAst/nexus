//! Deterministic detectors.
//!
//! docs/ai-integration.md §5: if a rule can express it, do not spend a token guessing at it.
//! Everything here is a query over the index and the graph — no model is asked, so nothing
//! here is subject to the 0.75 clamp that applies to a model's own confidence.
//!
//! Detectors receive a prepared snapshot rather than the store. That keeps them pure and
//! unit-testable, and it keeps SQL in `bh-store` where boundary rule 3 says it belongs.

pub mod graphql;
pub mod secrets;
pub mod spring;

use crate::bugs::BugCandidate;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SymbolFacts {
    pub fqn: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub visibility: Option<String>,
    pub parent_fqn: Option<String>,
    pub annotations: Vec<String>,
}

impl SymbolFacts {
    pub fn has_annotation(&self, name: &str) -> bool {
        self.annotations.iter().any(|a| {
            let bare = a.trim_start_matches('@');
            bare == name || bare.starts_with(&format!("{name}("))
        })
    }

    /// The class or module this belongs to, used as the `component` half of an identity.
    pub fn component(&self) -> String {
        let owner = self.parent_fqn.as_deref().unwrap_or(&self.fqn);
        owner.rsplit('.').next().unwrap_or(owner).to_string()
    }
}

#[derive(Debug, Clone)]
pub struct EdgeFacts {
    pub src_fqn: String,
    pub dst_fqn: Option<String>,
    pub dst_hint: Option<String>,
    pub edge_type: String,
    pub resolution: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: String,
    pub lang: Option<String>,
}

pub struct DetectContext<'a> {
    pub root: &'a Path,
    pub symbols: &'a [SymbolFacts],
    pub edges: &'a [EdgeFacts],
    pub files: &'a [FileFacts],
    /// Indexed lookup, built once: a detector that scans the symbol list per edge turns a
    /// linear pass into a quadratic one, and the graph is the biggest thing here.
    pub by_fqn: BTreeMap<&'a str, &'a SymbolFacts>,
}

impl<'a> DetectContext<'a> {
    pub fn new(
        root: &'a Path,
        symbols: &'a [SymbolFacts],
        edges: &'a [EdgeFacts],
        files: &'a [FileFacts],
    ) -> Self {
        let by_fqn = symbols.iter().map(|s| (s.fqn.as_str(), s)).collect();
        DetectContext {
            root,
            symbols,
            edges,
            files,
            by_fqn,
        }
    }

    pub fn symbol(&self, fqn: &str) -> Option<&SymbolFacts> {
        self.by_fqn.get(fqn).copied()
    }
}

pub trait Detector: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new bug.
    fn id(&self) -> &'static str;
    fn describe(&self) -> &'static str;
    fn run(&self, ctx: &DetectContext<'_>) -> Vec<BugCandidate>;
}

pub fn all() -> Vec<Box<dyn Detector>> {
    vec![
        Box::new(spring::TransactionalNonPublic),
        Box::new(spring::SelfInvocation),
        Box::new(graphql::OrphanOperation),
        Box::new(secrets::HardcodedSecret),
    ]
}
