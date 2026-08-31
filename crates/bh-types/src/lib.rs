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
