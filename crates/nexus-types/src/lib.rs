//! Shared vocabulary for every BugHunter crate.
//!
//! This crate exists to break what would otherwise be a dependency cycle between
//! `nexus-store` and `nexus-lang`: both need to name a `Language` and a `SymbolKind`, and
//! neither may depend on the other. It has no dependencies beyond `serde`.

#![forbid(unsafe_code)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

pub type ProjectId = i64;
pub type ScanId = i64;
pub type FileId = i64;
pub type SymbolId = i64;

/// Which file's version of a symbol survives when two of them name the same FQN.
///
/// Symbols are unique on FQN, and two analyzers can legitimately reach the same one. A
/// `.graphqls` file *declares* `Query.vehicles`; a Spring `@QueryMapping` handler
/// *implements* it. Both must emit a symbol at that coordinate — it is the join key the
/// frontend also points at — so exactly one of them owns the row.
///
/// Without a stated rule the winner was whichever file a scan happened to re-parse last: a
/// rescan touching only the resolver took the coordinate away from the schema and reported
/// the field as newly `ADDED`, on a ledger that is append-only. ADR-014 already decided the
/// precedence — the schema is the contract — and this is where an analyzer says which side
/// of it a symbol is on.
///
/// Yielding is not silence. A project that generates its schema at build time has only the
/// handler, and its route symbol stands: an `Implements` symbol creates the row when nothing
/// has declared it, and may replace another `Implements`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// This file states the contract. It wins. Almost every symbol is this.
    Declares,
    /// This file implements a contract another file may state. It yields to a `Declares` at
    /// the same FQN, and stands in when there is none.
    Implements,
}

impl Authority {
    pub fn as_str(self) -> &'static str {
        match self {
            Authority::Declares => "declares",
            Authority::Implements => "implements",
        }
    }
}

/// The five characters that end an identifier and start the next segment, across every
/// language this project indexes: `:` (Rust `::`, `graphql:`), `#` (member), `.` (Java
/// package, TS `Class.method`), `/` (TS/JS module path), `(` (a signature). Measured across
/// two real indexes rather than guessed — `-` deliberately did not make this set: it sits
/// *inside* identifiers (`some-file`, `nexus-cli`), and treating it as a boundary would let
/// `some` match `some-file`.
///
/// Defined once, here, because it already drifted once: `nexus_core::memory::subject_match`
/// (ranking) and `nexus_store::subject_prefixes` (the SQL that narrows candidates before
/// ranking) both decide what counts as a module boundary, and a fix applied to one and not the
/// other is exactly the class of bug this whole task exists to close. Neither crate may depend
/// on the other, so a shared constant needs a crate neither of them owns — which is what this
/// one is for (see the module doc above).
pub const SUBJECT_ANCHORS: &[u8; 5] = b":.#/(";

// ─────────────────────────── language ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Java,
    TypeScript,
    JavaScript,
    Python,
    Rust,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Java => "java",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Python => "python",
            Language::Rust => "rust",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "java" => Language::Java,
            "typescript" => Language::TypeScript,
            "javascript" => Language::JavaScript,
            "python" => Language::Python,
            "rust" => Language::Rust,
            _ => return None,
        })
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─────────────────────────── symbols ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Module,
    Package,
    Class,
    Interface,
    Enum,
    Record,
    Trait,
    Function,
    Method,
    Constructor,
    Field,
    Route,
    Entity,
    Config,
    Bean,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Module => "module",
            SymbolKind::Package => "package",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Enum => "enum",
            SymbolKind::Record => "record",
            SymbolKind::Trait => "trait",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Constructor => "constructor",
            SymbolKind::Field => "field",
            SymbolKind::Route => "route",
            SymbolKind::Entity => "entity",
            SymbolKind::Config => "config",
            SymbolKind::Bean => "bean",
        }
    }

    /// A type-like symbol can contain other symbols.
    pub fn is_container(self) -> bool {
        matches!(
            self,
            SymbolKind::Class
                | SymbolKind::Interface
                | SymbolKind::Enum
                | SymbolKind::Record
                | SymbolKind::Trait
                | SymbolKind::Module
        )
    }
}

// ─────────────────────────── edges ───────────────────────────

/// How one symbol depends on another. The type decides how much of an upstream change
/// survives the hop — see `weight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Calls,
    Implements,
    Extends,
    Injects,
    Routes,
    Persists,
    Reads,
    Writes,
    Emits,
    Imports,
    Tests,
    CallsHttp,
    CallsGraphql,
    Renders,
}

impl EdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeType::Calls => "calls",
            EdgeType::Implements => "implements",
            EdgeType::Extends => "extends",
            EdgeType::Injects => "injects",
            EdgeType::Routes => "routes",
            EdgeType::Persists => "persists",
            EdgeType::Reads => "reads",
            EdgeType::Writes => "writes",
            EdgeType::Emits => "emits",
            EdgeType::Imports => "imports",
            EdgeType::Tests => "tests",
            EdgeType::CallsHttp => "calls_http",
            EdgeType::CallsGraphql => "calls_graphql",
            EdgeType::Renders => "renders",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "calls" => EdgeType::Calls,
            "implements" => EdgeType::Implements,
            "extends" => EdgeType::Extends,
            "injects" => EdgeType::Injects,
            "routes" => EdgeType::Routes,
            "persists" => EdgeType::Persists,
            "reads" => EdgeType::Reads,
            "writes" => EdgeType::Writes,
            "emits" => EdgeType::Emits,
            "imports" => EdgeType::Imports,
            "tests" => EdgeType::Tests,
            "calls_http" => EdgeType::CallsHttp,
            "calls_graphql" => EdgeType::CallsGraphql,
            "renders" => EdgeType::Renders,
            _ => return None,
        })
    }

    /// How much of an upstream change survives one hop. docs/change-analysis.md §6.
    pub fn weight(self) -> f64 {
        match self {
            EdgeType::Calls => 0.90,
            EdgeType::Implements | EdgeType::Extends => 0.85,
            EdgeType::Injects => 0.80,
            // The seam. A change behind a GraphQL field reaches every caller of that field,
            // and it is an exact join rather than a guess, so it is not discounted further.
            EdgeType::CallsGraphql | EdgeType::CallsHttp => 0.85,
            EdgeType::Routes | EdgeType::Persists => 0.70,
            EdgeType::Renders => 0.65,
            EdgeType::Reads | EdgeType::Writes | EdgeType::Emits => 0.60,
            EdgeType::Tests => 0.50,
            EdgeType::Imports => 0.30,
        }
    }

    /// Whether a body-only change can reach a dependant through this edge.
    ///
    /// A body edit does not break a caller's compilation; it can only reach one through
    /// shared state or an observable effect. Filtering here is what keeps a one-line change
    /// from reporting four hundred affected symbols.
    pub fn carries_body_change(self) -> bool {
        matches!(
            self,
            EdgeType::Reads
                | EdgeType::Writes
                | EdgeType::Persists
                | EdgeType::Emits
                | EdgeType::Calls
                | EdgeType::CallsGraphql
                | EdgeType::CallsHttp
        )
    }
}

/// Which tier of the resolution cascade produced an edge. Reported with every impact
/// result so a three-hop heuristic chain is visibly a guess, not a compiler fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    Exact,
    Framework,
    Contract,
    Heuristic,
    /// The target is genuinely outside the indexed project — a third-party library.
    /// Distinct from `Unresolved`, which means BugHunter looked and failed. Conflating
    /// them makes the resolution rate a lie.
    External,
    /// Code this project owns that was not scanned — a sibling module of the same
    /// monorepo. Outside the index like `External`, but for a reason the caller can fix by
    /// widening the scan. ADR-017.
    Sibling,
    /// Imported from an external knowledge graph, never resolved against a symbol table.
    /// Capped at 0.5 confidence so an imported claim cannot outrank a parsed edge — a
    /// ceiling that was defeated for as long as this variant was missing and the stored
    /// value read back as `Heuristic`, claiming a tier that had resolved something.
    #[serde(rename = "external-graph")]
    ExternalGraph,
    Unresolved,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Exact => "exact",
            Resolution::Framework => "framework",
            Resolution::Contract => "contract",
            Resolution::Heuristic => "heuristic",
            Resolution::External => "external",
            Resolution::Sibling => "sibling",
            Resolution::ExternalGraph => "external-graph",
            Resolution::Unresolved => "unresolved",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "exact" => Resolution::Exact,
            "framework" => Resolution::Framework,
            "contract" => Resolution::Contract,
            "heuristic" => Resolution::Heuristic,
            "external" => Resolution::External,
            "sibling" => Resolution::Sibling,
            "external-graph" => Resolution::ExternalGraph,
            "unresolved" => Resolution::Unresolved,
            _ => return None,
        })
    }
}

// ─────────────────────────── scans ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanKind {
    Full,
    Incremental,
}

impl ScanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanKind::Full => "full",
            ScanKind::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanStatus {
    Running,
    Ok,
    Failed,
    Aborted,
}

impl ScanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanStatus::Running => "running",
            ScanStatus::Ok => "ok",
            ScanStatus::Failed => "failed",
            ScanStatus::Aborted => "aborted",
        }
    }
}

/// A scan that finished but could not index everything is `Degraded`, never silently `Ok`.
/// Partial failure is a first-class outcome — see docs/testing-strategy.md §1.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseStatus {
    Ok,
    Partial,
    Failed,
    Skipped,
}

impl ParseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ParseStatus::Ok => "ok",
            ParseStatus::Partial => "partial",
            ParseStatus::Failed => "failed",
            ParseStatus::Skipped => "skipped",
        }
    }
}

// ─────────────────────────── changes ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Moved,
}

impl ChangeType {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeType::Added => "added",
            ChangeType::Modified => "modified",
            ChangeType::Deleted => "deleted",
            ChangeType::Renamed => "renamed",
            ChangeType::Moved => "moved",
        }
    }
}

/// Which hash moved. This is the whole reason a symbol carries two hashes: the answer
/// selects which edge types a change ripples through. See ADR-010.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChangeKind {
    /// `sig_hash` differs — an API break; ripples to every caller.
    ApiChanged,
    /// `body_hash` differs only — ripples through data and effect edges alone.
    BodyChanged,
    /// Annotations differ — `@Transactional` and friends carry more meaning than signatures.
    ContractChanged,
    /// Both the signature and the body moved.
    ApiAndBodyChanged,
    Added,
    Deleted,
    Renamed,
}

impl ChangeKind {
    /// Every kind, for exhaustive tests. Adding a variant without adding it here makes the
    /// round-trip test fail, which is the point.
    pub const ALL: &'static [ChangeKind] = &[
        ChangeKind::ApiChanged,
        ChangeKind::BodyChanged,
        ChangeKind::ContractChanged,
        ChangeKind::ApiAndBodyChanged,
        ChangeKind::Added,
        ChangeKind::Deleted,
        ChangeKind::Renamed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::ApiChanged => "API_CHANGED",
            ChangeKind::BodyChanged => "BODY_CHANGED",
            ChangeKind::ContractChanged => "CONTRACT_CHANGED",
            ChangeKind::ApiAndBodyChanged => "API_AND_BODY_CHANGED",
            ChangeKind::Added => "ADDED",
            ChangeKind::Deleted => "DELETED",
            ChangeKind::Renamed => "RENAMED",
        }
    }

    /// Reconstruct the kind from the two columns the ledger splits it across.
    ///
    /// `change_type` and `detail` are written by `change_type()` and `detail()` below; this
    /// is their joint inverse, and it lives beside them so the three cannot drift. Without
    /// it, everything read back out of the ledger was `BODY_CHANGED`, which made every rule
    /// that asks "did the contract move?" silently unreachable.
    pub fn from_ledger(change_type: &str, detail: Option<&str>) -> Option<Self> {
        Some(match change_type {
            "added" => ChangeKind::Added,
            "deleted" => ChangeKind::Deleted,
            "renamed" => ChangeKind::Renamed,
            "modified" => match detail {
                Some("signature") => ChangeKind::ApiChanged,
                Some("annotations") => ChangeKind::ContractChanged,
                Some("both") => ChangeKind::ApiAndBodyChanged,
                _ => ChangeKind::BodyChanged,
            },
            _ => return None,
        })
    }

    /// The `changes.detail` column: which component of the symbol moved.
    pub fn detail(self) -> Option<&'static str> {
        match self {
            ChangeKind::ApiChanged => Some("signature"),
            ChangeKind::BodyChanged => Some("body"),
            ChangeKind::ContractChanged => Some("annotations"),
            ChangeKind::ApiAndBodyChanged => Some("both"),
            _ => None,
        }
    }

    pub fn change_type(self) -> ChangeType {
        match self {
            ChangeKind::Added => ChangeType::Added,
            ChangeKind::Deleted => ChangeType::Deleted,
            ChangeKind::Renamed => ChangeType::Renamed,
            _ => ChangeType::Modified,
        }
    }
}

// ─────────────────────────── bugs ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FindingType {
    Concurrency,
    Transaction,
    NullSafety,
    Security,
    Logic,
    Performance,
    ErrorHandling,
    DataConsistency,
    ApiContract,
    ResourceLeak,
    Regression,
    UiState,
    // Advisory kinds. A recommendation is a finding — it has evidence, it can be dismissed,
    // and it can come back — but it describes something the project lacks rather than
    // something the code does wrong. ADR-021.
    /// How the project is put together: its shape, its modules, its missing scaffolding.
    Architecture,
    /// An observation about a change, grounded in the graph and the history rather than in
    /// a reviewer's taste.
    Review,
    /// Tooling an agent working in this project should have and does not.
    Tooling,
}

impl FindingType {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingType::Concurrency => "concurrency",
            FindingType::Transaction => "transaction",
            FindingType::NullSafety => "null-safety",
            FindingType::Security => "security",
            FindingType::Logic => "logic",
            FindingType::Performance => "performance",
            FindingType::ErrorHandling => "error-handling",
            FindingType::DataConsistency => "data-consistency",
            FindingType::ApiContract => "api-contract",
            FindingType::ResourceLeak => "resource-leak",
            FindingType::Regression => "regression",
            FindingType::UiState => "ui-state",
            FindingType::Architecture => "architecture",
            FindingType::Review => "review",
            FindingType::Tooling => "tooling",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "concurrency" => FindingType::Concurrency,
            "transaction" => FindingType::Transaction,
            "null-safety" => FindingType::NullSafety,
            "security" => FindingType::Security,
            "logic" => FindingType::Logic,
            "performance" => FindingType::Performance,
            "error-handling" => FindingType::ErrorHandling,
            "data-consistency" => FindingType::DataConsistency,
            "api-contract" => FindingType::ApiContract,
            "resource-leak" => FindingType::ResourceLeak,
            "regression" => FindingType::Regression,
            "ui-state" => FindingType::UiState,
            "architecture" => FindingType::Architecture,
            "review" => FindingType::Review,
            "tooling" => FindingType::Tooling,
            _ => return None,
        })
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Info => "info",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" => Severity::Medium,
            "low" => Severity::Low,
            "info" => Severity::Info,
            _ => return None,
        })
    }
}

/// docs/change-analysis.md §10. The transitions that matter are the ones that require
/// evidence: `FIXED` is never reached by a bug merely not being seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingStatus {
    Suspected,
    Unverified,
    Verified,
    Fixed,
    Regressed,
    Ignored,
}

impl FindingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingStatus::Suspected => "SUSPECTED",
            FindingStatus::Unverified => "UNVERIFIED",
            FindingStatus::Verified => "VERIFIED",
            FindingStatus::Fixed => "FIXED",
            FindingStatus::Regressed => "REGRESSED",
            FindingStatus::Ignored => "IGNORED",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "SUSPECTED" => FindingStatus::Suspected,
            "UNVERIFIED" => FindingStatus::Unverified,
            "VERIFIED" => FindingStatus::Verified,
            "FIXED" => FindingStatus::Fixed,
            "REGRESSED" => FindingStatus::Regressed,
            "IGNORED" => FindingStatus::Ignored,
            _ => return None,
        })
    }

    /// A human dismissal is sticky, and a proven bug is not re-opened by a scan.
    pub fn is_open(self) -> bool {
        matches!(
            self,
            FindingStatus::Suspected
                | FindingStatus::Unverified
                | FindingStatus::Verified
                | FindingStatus::Regressed
        )
    }
}

// ─────────────────────── GraphQL symbol naming ───────────────────────

/// Build a GraphQL symbol FQN, namespaced by module when there is one.
///
/// Both the schema analyzer and the Java resolver analyzer call this, because a schema
/// coordinate and the resolver that serves it must produce the *same* string or the seam
/// stops joining. That is the whole reason it lives here rather than in either crate.
pub fn graphql_fqn(module: &str, rest: &str) -> String {
    if module.is_empty() {
        format!("graphql:{rest}")
    } else {
        format!("graphql:{module}:{rest}")
    }
}

/// The module inside a namespaced GraphQL FQN, if it carries one.
pub fn graphql_module(fqn: &str) -> Option<&str> {
    fqn.strip_prefix("graphql:")?
        .rsplit_once(':')
        .map(|(m, _)| m)
}

/// The schema coordinate inside a possibly-namespaced GraphQL FQN.
///
/// `graphql:sales/backend:Query.notifications` -> `Query.notifications`, and the
/// unnamespaced form is returned unchanged. A frontend knows the coordinate it calls and
/// not the service that serves it, so this is what its hint is matched on.
pub fn graphql_coordinate(fqn: &str) -> Option<&str> {
    let rest = fqn.strip_prefix("graphql:")?;
    Some(match rest.rsplit_once(':') {
        Some((_, coord)) => coord,
        None => rest,
    })
}

#[cfg(test)]
mod change_kind_tests {
    use super::ChangeKind;

    #[test]
    fn every_kind_survives_the_ledger() {
        // The ledger splits a kind across `change_type` and `detail`, and reading it back
        // wrongly is silent: every capability simply sees BODY_CHANGED and every rule that
        // asks about a contract change becomes unreachable. That is what happened, and this
        // is what would have caught it.
        for &k in ChangeKind::ALL {
            let round = ChangeKind::from_ledger(k.change_type().as_str(), k.detail());
            assert_eq!(
                round,
                Some(k),
                "{k:?} did not survive: stored as ({}, {:?})",
                k.change_type().as_str(),
                k.detail()
            );
        }
    }

    #[test]
    fn an_unknown_change_type_yields_nothing_rather_than_a_default() {
        // Defaulting would put a wrong kind in front of a rule that acts on it.
        assert_eq!(ChangeKind::from_ledger("teleported", None), None);
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::Resolution;

    #[test]
    fn every_stored_resolution_value_round_trips() {
        // The CHECK constraint in migrations 0006_external_graph.sql permits exactly these.
        // A value the database can hold but the enum cannot name is reported as the wrong
        // tier: `sibling` and `external-graph` both read back as `heuristic`, claiming a
        // tier that had resolved something when nothing had.
        for s in [
            "exact",
            "framework",
            "contract",
            "heuristic",
            "external",
            "sibling",
            "external-graph",
            "unresolved",
        ] {
            let parsed = Resolution::parse(s)
                .unwrap_or_else(|| panic!("{s} is a stored value the enum cannot name"));
            assert_eq!(parsed.as_str(), s, "{s} did not round-trip");
        }
    }

    #[test]
    fn an_unknown_resolution_is_none_rather_than_a_guess() {
        assert!(Resolution::parse("invented").is_none());
    }

    #[test]
    fn the_hyphenated_variant_keeps_its_hyphen() {
        // `rename_all = "lowercase"` alone would render this as `externalgraph`, which no
        // migration permits and no `parse` accepts, so the variant carries an explicit
        // `#[serde(rename)]`. Asserted through `as_str` rather than through serde, because
        // this crate depends on nothing but serde and one assertion does not justify
        // pulling `serde_json` in to check it.
        assert_eq!(Resolution::ExternalGraph.as_str(), "external-graph");
        assert_eq!(
            Resolution::parse(Resolution::ExternalGraph.as_str()),
            Some(Resolution::ExternalGraph)
        );
    }
}
