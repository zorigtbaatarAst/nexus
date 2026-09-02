//! What generation emits beside the repository.
//!
//! The repository itself carries only fixture content — a `manifest.json` committed inside it
//! would be indexed by the very scan the fixture exists to test. So the resolved metadata
//! lands next to it, and the evaluation runner reads it from there.

use crate::spec::{Bug, DeprecatedPath, Expect, Spec, Task};
use serde::{Deserialize, Serialize};

/// The bridge between a specification's logical commit ids and the shas a runner must pin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub role: String,
    pub stack: Vec<String>,
    /// The generator that produced this, so a manifest from an older format is recognisable.
    pub generator_version: String,
    /// blake3 of the specification directory. Two manifests with the same digest describe
    /// the same fixture; a differing digest with identical shas means the spec changed in a
    /// way that did not reach the history, which is worth noticing.
    pub spec_digest: String,
    pub default_branch: String,
    pub commits: Vec<CommitRecord>,
    pub branches: Vec<BranchRecord>,
    pub patches: Vec<PatchRecord>,
    pub deprecated_paths: Vec<DeprecatedPath>,
    pub tasks: Vec<ResolvedTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    /// The logical id from the specification: `c3`.
    pub id: String,
    pub sha: String,
    pub branch: String,
    pub message: String,
    /// Unix seconds. Derived, never read from a clock.
    pub timestamp: i64,
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plants_bug: Option<Bug>,
    #[serde(skip_serializing_if = "Expect::is_empty")]
    pub expect: Expect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRecord {
    pub name: String,
    pub from: String,
    pub head: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchRecord {
    pub id: String,
    pub base: String,
    pub base_sha: String,
    /// Relative to the manifest, so the manifest is movable.
    pub file: String,
    pub description: String,
    /// Whether the patch was proved to apply at `base_sha`. Always true in a manifest that
    /// was written — generation fails otherwise — and recorded so a reader need not assume.
    pub verified: bool,
}

/// A task with its commit resolved to a sha: exactly the shape `13-evaluation.md` §3 pins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTask {
    pub id: String,
    pub family: String,
    pub repo: String,
    /// The logical id, kept so a reader can find it in the specification.
    pub commit_id: String,
    pub commit: String,
    pub start_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<TurnRecord>,
    pub required_sites: Vec<String>,
    pub hidden_tests: Vec<String>,
    pub convention_rules: Vec<String>,
    pub timeout_s: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub prompt: String,
    pub required_anchors: Vec<String>,
}

impl Expect {
    pub fn is_empty(&self) -> bool {
        self.symbol_changes.is_none() && self.new_findings.is_none() && self.note.is_none()
    }
}

impl ResolvedTask {
    pub fn from(spec: &Spec, task: &Task, sha: &str) -> Self {
        ResolvedTask {
            id: task.id.clone(),
            family: task.family.clone(),
            repo: spec.name().to_string(),
            commit_id: task.commit.clone(),
            commit: sha.to_string(),
            start_state: task.start_state.as_wire(),
            prompt: task.prompt.clone(),
            turns: task
                .turns
                .iter()
                .map(|t| TurnRecord {
                    prompt: t.prompt.clone(),
                    required_anchors: t.required_anchors.clone(),
                })
                .collect(),
            required_sites: task.required_sites.clone(),
            hidden_tests: task.hidden_tests.clone(),
            convention_rules: task.convention_rules.clone(),
            timeout_s: task.timeout_s,
            note: task.note.clone(),
        }
    }
}
