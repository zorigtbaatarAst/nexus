//! Result types. Every one of these is `Serialize`, so `--json` and the human renderer
//! render the same value and cannot drift apart about the facts.

use bh_types::Health;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub languages: Vec<LanguageShare>,
    pub frameworks: Vec<Framework>,
    pub build_system: Option<String>,
    pub package_manager: Option<String>,
    pub databases: Vec<Detected>,
    pub containers: Vec<String>,
    pub vcs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageShare {
    pub lang: String,
    pub files: usize,
    pub analyzed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Framework {
    pub name: String,
    pub version: Option<String>,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detected {
    pub kind: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub scan_uid: String,
    pub kind: &'static str,
    pub commit: Option<String>,
    pub dirty: bool,
    pub files_scanned: usize,
    pub files_failed: usize,
    pub files_skipped: usize,
    pub symbols_indexed: usize,
    pub edges_total: usize,
    pub edges_resolved: usize,
    pub edges_external: usize,
    pub health: Health,
    pub warnings: Vec<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct Revision {
    pub scan_uid: Option<String>,
    pub commit: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeItem {
    pub entity: &'static str,
    pub change_type: &'static str,
    pub kind: Option<&'static str>,
    pub path: Option<String>,
    pub fqn: Option<String>,
    /// For a rename, the name this symbol had before. The durable old→new record lives in
    /// `symbol_aliases`; this is what a reader needs in the report itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_fqn: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RescanReport {
    pub scan_uid: Option<String>,
    pub baseline: Revision,
    pub current: Revision,
    pub unchanged: bool,
    pub forced_full: Option<String>,
    pub files_changed: usize,
    pub files_deleted: usize,
    pub symbols_changed: usize,
    pub items: Vec<ChangeItem>,
    pub files_failed: usize,
    pub health: Health,
    pub warnings: Vec<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub project: String,
    pub profile: Option<Profile>,
    pub baseline: Option<Revision>,
    pub current: Revision,
    pub commits_behind: Option<usize>,
    pub scans: i64,
    pub files: i64,
    pub symbols: i64,
    pub drifted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub level: &'static str, // ok | warn | error
    pub detail: String,
    pub remedy: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedRef {
    pub fqn: String,
    pub kind: String,
    pub file: String,
    pub line: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hop {
    pub from: String,
    pub edge: &'static str,
    pub resolution: &'static str,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactItem {
    pub fqn: String,
    pub kind: String,
    pub file: String,
    pub line: i64,
    pub score: f64,
    /// The weakest link in the chain that reached this symbol. A three-hop heuristic path
    /// scoring 0.4 with `min_confidence` 0.55 is honestly labelled a guess.
    pub min_confidence: f64,
    pub depth: usize,
    pub path: Vec<Hop>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub target: String,
    pub direction: &'static str,
    pub seeds: Vec<SeedRef>,
    pub items: Vec<ImpactItem>,
    pub tests: Vec<ImpactItem>,
    pub crossed_seam: usize,
    /// Nodes whose fan-out exceeded the cap. Reported, never silently dropped: returning
    /// 200 of 3,000 and calling it the impact set is the quiet lie that makes a tool
    /// untrustworthy.
    pub truncated_at: Vec<String>,
    pub visited: usize,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphReport {
    pub edges_total: i64,
    pub edges_resolved: i64,
    /// Pointing at a library or an unscanned sibling module. Correctly outside the index.
    pub edges_external: i64,
    pub by_resolution: Vec<(String, i64)>,
}

/// A result that may not be one thing.
///
/// `Ambiguous` is not an error and not a guess: it hands back the candidates so the caller
/// can choose, which is the CLI's form of `clarification_required`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Resolved<T> {
    #[serde(rename = "ok")]
    One(T),
    #[serde(rename = "ambiguous")]
    Ambiguous(Vec<SeedRef>),
    // A struct variant, not a newtype: serde cannot serialize an internally-tagged
    // newtype variant that wraps a string, and it fails at runtime rather than at compile
    // time — so `--json` on a missing symbol errored instead of reporting not_found.
    #[serde(rename = "not_found")]
    NotFound { target: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Neighbourhood {
    pub fqn: String,
    pub edge: &'static str,
    pub resolution: &'static str,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolDetail {
    pub fqn: String,
    pub kind: String,
    pub file: String,
    pub line: i64,
    pub depends_on: Vec<Neighbourhood>,
    pub depended_on_by: Vec<Neighbourhood>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
