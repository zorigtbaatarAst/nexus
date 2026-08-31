//! The language extension point.
//!
//! Boundary rule: an analyzer takes source text and returns a `ParsedFile`. It never learns
//! about scans, baselines or the store — which is also exactly why parsing parallelizes
//! cleanly across cores. `tests/boundaries.rs` fails the build if a `nexus-lang-*` crate
//! acquires a dependency on `nexus-store` or `nexus-core`.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_types::{EdgeType, Language, SymbolKind};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum LangError {
    #[error("could not load grammar for {0}")]
    Grammar(Language),
    #[error("parser produced no tree")]
    NoTree,
    #[error("{0}")]
    Other(String),
}

/// A symbol as an analyzer produces it: no ids, no scan, no database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSymbol {
    pub kind: SymbolKind,
    pub name: String,
    /// Fully-qualified: `mn.pay.PaymentService#createPayment(String,Money)`
    pub fqn: String,
    pub parent_fqn: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    /// blake3 of signature + sorted annotations. Moving means an API break.
    pub sig_hash: String,
    /// blake3 of the normalized body. Moving alone means a behaviour change.
    pub body_hash: String,
    pub annotations: Vec<String>,
}

/// A dependency as an analyzer sees it: two names and a kind. Turning `dst_hint` into a
/// real symbol id is resolution's job, in `nexus-core`, once every symbol in the project is
/// known — an analyzer cannot do it because it only ever sees one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEdge {
    pub src_fqn: String,
    /// A fully-qualified name when the analyzer could produce one, otherwise the best
    /// available shape: `PaymentRepository#save`, or `graphql:Query.vehicles` for a seam.
    pub dst_hint: String,
    pub edge_type: EdgeType,
    pub site_line: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    pub symbols: Vec<RawSymbol>,
    pub edges: Vec<RawEdge>,
    pub imports: Vec<String>,
    pub package: Option<String>,
    /// Non-fatal problems. A file that partly parsed still contributes what it has, and
    /// says what it could not do — silence here would make the index quietly wrong.
    pub warnings: Vec<String>,
}

pub struct SourceFile<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

pub trait LanguageAnalyzer: Send + Sync {
    fn language(&self) -> Language;
    fn extensions(&self) -> &'static [&'static str];

    /// Feeds `scans.tool_versions_json`. When this changes, every file in this language is
    /// re-parsed even though its content hash still matches — otherwise upgrading a grammar
    /// silently keeps the symbols the old one produced, forever, with no error anywhere.
    fn grammar_version(&self) -> &'static str;

    fn parse(&self, src: &SourceFile<'_>) -> Result<ParsedFile, LangError>;
}

/// Extension detection and dispatch. Framework packs are a *separate* axis (ADR-012):
/// Spring knowledge is not Java knowledge.
#[derive(Default)]
pub struct Registry {
    analyzers: Vec<Box<dyn LanguageAnalyzer>>,
    by_ext: BTreeMap<String, usize>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, analyzer: Box<dyn LanguageAnalyzer>) -> &mut Self {
        let idx = self.analyzers.len();
        for ext in analyzer.extensions() {
            self.by_ext.insert((*ext).to_string(), idx);
        }
        self.analyzers.push(analyzer);
        self
    }

    pub fn for_path(&self, path: &str) -> Option<&dyn LanguageAnalyzer> {
        let ext = path.rsplit('.').next()?;
        self.by_ext.get(ext).map(|i| self.analyzers[*i].as_ref())
    }

    pub fn language_for_path(&self, path: &str) -> Option<Language> {
        self.for_path(path).map(|a| a.language())
    }

    pub fn is_empty(&self) -> bool {
        self.analyzers.is_empty()
    }

    /// The version map recorded on every scan, keyed for cache invalidation.
    pub fn tool_versions(&self) -> BTreeMap<String, String> {
        self.analyzers
            .iter()
            .map(|a| {
                (
                    format!("grammar:{}", a.language()),
                    a.grammar_version().to_string(),
                )
            })
            .collect()
    }
}
