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

use nexus_types::{Authority, EdgeType, Language, SymbolKind};
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
    /// The signature hash, from `sig_hash` below — never hand-rolled. Moving means an API
    /// break.
    pub sig_hash: String,
    /// blake3 of the normalized body. Moving alone means a behaviour change.
    pub body_hash: String,
    pub annotations: Vec<String>,
    /// Whether this file states the contract this symbol names, or merely implements it.
    /// Almost always `Declares`; see `Authority` for the one shape that is not.
    pub authority: Authority,
}

/// The one construction for `RawSymbol::sig_hash`, in every language.
///
/// Annotations are sorted, so reordering them is not a change; every one of them
/// contributes, so adding or removing one is. Each is hashed under its own separator rather
/// than joined into a string, because annotation arguments contain commas —
/// `@RequestMapping(value="/x", method=GET)` is one annotation, and a join would let it hash
/// the same as two.
///
/// Analyzers supply the signature and the annotations; they do not hash. There were four
/// independent constructions here, and they disagreed with this contract and with each
/// other: Java sorted and Rust and Python did not, so reordering two attributes read as an
/// API break, and TypeScript omitted annotations entirely, so a decorator appearing or
/// vanishing produced no ripple at all. `nexus-lang-pack/tests/sig_hash_conformance.rs`
/// holds every registered analyzer to this.
pub fn sig_hash(signature: &str, annotations: &[String]) -> String {
    let sorted = canonical_annotations(annotations);
    let mut hasher = blake3::Hasher::new();
    hasher.update(signature.as_bytes());
    // The count, so the separator cannot be impersonated: a signature that happens to
    // contain a 0x1f byte would otherwise hash like one annotation fewer.
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for annotation in sorted {
        hasher.update(b"\x1f");
        hasher.update(annotation.as_bytes());
    }
    hasher.finalize().to_hex()[..32].to_string()
}

/// Annotations in the order the contract sees them: sorted, so that reordering is not a
/// change.
///
/// `sig_hash` hashes this and the ledger compares it — one rule, one place. They were two
/// implementations of it, and the ledger's was order-sensitive, so a swap was reported as
/// `CONTRACT_CHANGED` while the hash said nothing had moved.
pub fn canonical_annotations(annotations: &[String]) -> Vec<&str> {
    let mut sorted: Vec<&str> = annotations.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted
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

    /// Every registered analyzer, so a conformance suite can hold all of them to a contract
    /// rather than whichever one someone remembered to test.
    pub fn analyzers(&self) -> impl Iterator<Item = &dyn LanguageAnalyzer> {
        self.analyzers.iter().map(|a| a.as_ref())
    }

    /// The languages this build actually analyzes.
    ///
    /// The profile's `analyzed` flag was a hardcoded `["java", "typescript"]`, so a Rust
    /// project whose 1838 symbols were indexed was told Rust was "present but not analyzed
    /// in this build" — and Architect then recommended adding an analyzer that ships.
    pub fn languages(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self
            .analyzers
            .iter()
            .map(|a| a.language().as_str())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The version map recorded on every scan, keyed for cache invalidation.
    ///
    /// Keyed by extension as well as language, because two analyzers may report the same
    /// `Language` — the GraphQL schema reader calls itself TypeScript — and a
    /// language-keyed map silently dropped one of their versions. Whichever registered
    /// last won, so bumping the TypeScript analyzer invalidated nothing and the index kept
    /// the symbols the old extraction produced, forever, with no error anywhere.
    pub fn tool_versions(&self) -> BTreeMap<String, String> {
        self.analyzers
            .iter()
            .map(|a| {
                (
                    format!("grammar:{}:{}", a.language(), a.extensions().join(",")),
                    a.grammar_version().to_string(),
                )
            })
            .collect()
    }
}

/// The module a source file belongs to, for namespacing symbols whose names are only
/// unique within one deployable unit.
///
/// A GraphQL schema coordinate is the clearest case. `Query.notifications` is unique inside
/// one service and says nothing across six of them: on a real monorepo, six services each
/// declared it, `symbols` is `UNIQUE (project_id, fqn)`, and five resolvers were silently
/// overwritten by the sixth — so every frontend calling it was wired to whichever service
/// happened to be scanned last.
///
/// The key is the path above the source root, because that is the one boundary every JVM
/// and Node layout agrees on: `sales/backend/src/main/java/…` and
/// `sales/backend/src/main/resources/graphql/…` are the same module, which is what lets a
/// schema and its resolver still meet.
///
/// A single-module project yields `""` and the caller emits exactly what it emits today.
/// That is deliberate: the common case must not pay for the monorepo case, and every
/// existing index stays joinable.
pub fn module_of(path: &str) -> &str {
    match path.find("/src/") {
        Some(i) => &path[..i],
        // A leading `src/` means the module *is* the repository root.
        None => "",
    }
}

#[cfg(test)]
mod sig_hash_tests {
    use super::sig_hash;

    fn anns(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn reordering_annotations_is_not_a_change() {
        assert_eq!(
            sig_hash("void go()", &anns(&["@A", "@B"])),
            sig_hash("void go()", &anns(&["@B", "@A"])),
        );
    }

    #[test]
    fn adding_or_removing_an_annotation_is_a_change() {
        let one = sig_hash("void go()", &anns(&["@A"]));
        let two = sig_hash("void go()", &anns(&["@A", "@Transactional"]));
        let none = sig_hash("void go()", &[]);
        assert_ne!(one, two);
        assert_ne!(one, none);
        assert_ne!(two, none);
    }

    #[test]
    fn an_annotation_holding_a_comma_is_still_one_annotation() {
        // A join on "," would let these two hash alike, and `@RequestMapping(value="/x",
        // method=GET)` is exactly that shape.
        assert_ne!(
            sig_hash("void go()", &anns(&["@M(a,b)"])),
            sig_hash("void go()", &anns(&["@M(a", "b)"])),
        );
    }

    #[test]
    fn the_signature_still_decides() {
        assert_ne!(
            sig_hash("void go()", &anns(&["@A"])),
            sig_hash("void go(int)", &anns(&["@A"])),
        );
        // And the boundary between the two is real, separator included: a signature holding
        // the separator byte itself must not hash like one annotation fewer.
        assert_ne!(
            sig_hash("void go()@A", &[]),
            sig_hash("void go()", &anns(&["@A"]))
        );
        assert_ne!(
            sig_hash("void go()\u{1f}@A", &[]),
            sig_hash("void go()", &anns(&["@A"]))
        );
    }
}

#[cfg(test)]
mod module_tests {
    use super::module_of;

    #[test]
    fn a_schema_and_its_resolver_land_in_the_same_module() {
        // They must, or the contract join stops working — which is the whole seam.
        assert_eq!(
            module_of("sales/backend/src/main/java/mn/a/C.java"),
            "sales/backend"
        );
        assert_eq!(
            module_of("sales/backend/src/main/resources/graphql/n.graphqls"),
            "sales/backend"
        );
    }

    #[test]
    fn a_single_module_project_is_unnamespaced() {
        // The common case keeps today's exact FQNs, so existing indexes stay joinable.
        assert_eq!(module_of("src/main/java/mn/a/C.java"), "");
        assert_eq!(module_of("build.gradle"), "");
    }

    #[test]
    fn a_coordinate_survives_namespacing() {
        use nexus_types::{graphql_coordinate, graphql_fqn};
        let ns = graphql_fqn("sales/backend", "Query.notifications");
        assert_eq!(ns, "graphql:sales/backend:Query.notifications");
        assert_eq!(graphql_coordinate(&ns), Some("Query.notifications"));

        let plain = graphql_fqn("", "Query.notifications");
        assert_eq!(plain, "graphql:Query.notifications");
        assert_eq!(graphql_coordinate(&plain), Some("Query.notifications"));
        assert_eq!(graphql_coordinate("mn.a.C#m()"), None);
    }

    #[test]
    fn sibling_services_are_told_apart() {
        assert_ne!(
            module_of("sales/backend/src/main/java/C.java"),
            module_of("ceo/backend/src/main/java/C.java")
        );
    }
}
