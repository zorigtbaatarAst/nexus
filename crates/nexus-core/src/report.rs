//! Result types. Every one of these is `Serialize`, so `--json` and the human renderer
//! render the same value and cannot drift apart about the facts.

use nexus_types::Health;
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
    /// Facts whose evidence pointed at a symbol or file this scan changed or removed.
    /// Always zero on a first scan: there is nothing to remember yet.
    pub facts_invalidated: usize,
    /// Facts whose evidence this scan found intact. Three of these makes a fact durable.
    pub facts_validated: usize,
    pub edges_total: usize,
    pub edges_resolved: usize,
    /// A third-party library, correctly outside the index. ADR-017.
    pub edges_external: usize,
    /// Code this project owns that was not scanned — a sibling module of the same
    /// monorepo. Separate from `edges_external` because an edit here can break it.
    pub edges_sibling: usize,
    pub health: Health,
    pub warnings: Vec<String>,
    pub duration_ms: u128,
    /// What Architect made of the project on this scan.
    ///
    /// Carried inside the scan's own report rather than emitted beside it. `--json` is one
    /// document per command: two concatenated objects on stdout parse as neither, and every
    /// consumer — `jq`, an agent, this project's own CI smoke check — breaks on the second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architect: Option<AnalyzeReport>,
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
    /// Facts whose evidence pointed at a symbol or file this scan changed or removed. They
    /// stay on disk and stop being retrieved.
    pub facts_invalidated: usize,
    /// Facts whose evidence this scan found intact. Three of these makes a fact durable.
    pub facts_validated: usize,
    pub items: Vec<ChangeItem>,
    pub files_failed: usize,
    pub health: Health,
    pub warnings: Vec<String>,
    pub duration_ms: u128,
}

/// What the gate concluded.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// verified | failed | inconclusive | permission_required
    pub verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub checks: Vec<nexus_verify::Check>,
    /// What happened to the baseline half, including why it was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// Reverse queries only: nothing in the index that looks like a test reaches this
    /// symbol. Stated as a field rather than left to be inferred from an empty list,
    /// because it is the answer most likely to change what happens next and an absent
    /// thing is the easiest kind to not notice. Always `false` for a forward query, where
    /// the tests list answers a different question.
    pub uncovered: bool,
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
    /// A third-party library, correctly outside the index. ADR-017.
    pub edges_external: i64,
    /// Code this project owns that was not scanned. Fixable by widening the scan, which
    /// is exactly what makes it worth separating from `edges_external`.
    pub edges_sibling: i64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingSummary {
    pub uid: String,
    pub slug: String,
    pub title: String,
    pub capability: String,
    #[serde(rename = "type")]
    pub finding_type: String,
    pub component: Option<String>,
    pub severity: String,
    pub confidence: f64,
    pub status: String,
    pub detector: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introduced_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingEvent {
    pub scan_uid: String,
    pub commit: Option<String>,
    pub status: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingDetail {
    #[serde(flatten)]
    pub summary: FindingSummary,
    pub fingerprint: String,
    pub evidence: Vec<crate::findings::CodeRef>,
    pub history: Vec<FindingEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityInfo {
    pub id: String,
    pub finding_prefix: String,
    pub describes: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeReport {
    pub capability: String,
    /// What the capability was asked to look at, in words.
    pub scope: String,
    pub scan_uid: Option<String>,
    /// How many symbols the scope actually admitted. The number that shows whether a
    /// targeted analysis narrowed anything, rather than costing what a full one costs.
    pub symbols_examined: usize,
    pub found: usize,
    pub new: usize,
    pub recurring: usize,
    pub regressed: usize,
    pub fixed: usize,
    /// Rejected before storage for having no checkable evidence. Counted and reported,
    /// because a silently discarded finding is indistinguishable from finding nothing.
    pub rejected: usize,
    pub findings: Vec<FindingSummary>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedFinding {
    pub uid: String,
    /// False when this was already known — the fingerprint recognized it rather than
    /// creating a duplicate, which is the point of recording through the platform.
    pub is_new: bool,
    pub status: String,
}

/// What one import of an external graph did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub path: String,
    /// Claims graphify's semantic pass produced.
    pub concepts_read: usize,
    pub facts_recorded: usize,
    /// Claims whose label resolved to exactly one indexed symbol, so the fact is anchored on
    /// the code rather than on the document that discusses it. The difference decides whether
    /// it ever surfaces while someone is editing.
    pub anchored_on_code: usize,
    pub skipped: usize,
    /// Nodes that were prose but not a claim — a heading, a label, a dependency name out of a
    /// fixture. graphify's `concept` nodes are mostly *names of things*, and importing them
    /// put `next`, `react` and `Golden Fixture Repositories` in project memory.
    pub skipped_not_a_claim: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactInput {
    pub key: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    pub subject: Option<String>,
    pub claim: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub evidence: Vec<crate::findings::CodeRef>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_scope() -> String {
    "project".into()
}
fn default_source() -> String {
    "ai".into()
}
fn default_confidence() -> f64 {
    0.8
}

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub key: String,
    pub scope: String,
    pub subject: Option<String>,
    pub claim: String,
    pub source: String,
    pub confidence: f64,
    /// Validated three times, or written by a person. §3's highest retrieval weight.
    pub durable: bool,
    /// Distinct scans whose evidence check this fact survived.
    pub validated_count: i64,
}

// --- `ask`: the questions a person or an agent actually has -------------------------------

/// A question, as a value rather than a string.
///
/// Verb parsing stays in the adapter: `"what-changed"` and `"changed"` are the same question
/// spelled two ways, and which spellings a surface accepts is that surface's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    Changed,
    Affected(String),
    Known(String),
    Facts,
    Next,
}

#[derive(Debug, Serialize)]
#[serde(tag = "question", rename_all = "snake_case")]
pub enum Answer {
    Changed {
        since: Option<String>,
        symbols: Vec<String>,
        files: usize,
    },
    Affected {
        target: String,
        symbols: Vec<Affected>,
        crossed_seam: usize,
    },
    Known {
        target: String,
        findings: Vec<FindingSummary>,
        facts: Vec<Fact>,
    },
    Facts {
        facts: Vec<Fact>,
    },
    Next {
        suggestions: Vec<Suggestion>,
    },
    Unknown {
        asked: String,
        understood: Vec<&'static str>,
    },
}

#[derive(Debug, Serialize)]
pub struct Affected {
    pub fqn: String,
    pub score: f64,
    pub min_confidence: f64,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub target: String,
    pub why: String,
    pub score: f64,
}
