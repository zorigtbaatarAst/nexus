//! Shared vocabulary for every BugHunter crate.
//!
//! This crate exists to break what would otherwise be a dependency cycle between
//! `bh-store` and `bh-lang`: both need to name a `Language` and a `SymbolKind`, and
//! neither may depend on the other. It has no dependencies beyond `serde`.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

pub type ProjectId = i64;
pub type ScanId = i64;
pub type FileId = i64;
pub type SymbolId = i64;

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
    /// The target is genuinely outside the indexed project — a third-party library, or a
    /// sibling module that was not scanned. Distinct from `Unresolved`, which means
    /// BugHunter looked and failed. Conflating them makes the resolution rate a lie.
    External,
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
