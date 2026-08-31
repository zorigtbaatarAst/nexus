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
