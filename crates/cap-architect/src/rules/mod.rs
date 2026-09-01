//! Architect's rules.
//!
//! Each takes the prepared project snapshot and returns findings. Rules never touch storage
//! or git: `nexus-core` decides whether a finding is new, recurring, fixed or regressed.
//!
//! Every rule here must anchor its evidence on a real `file:line`, including the rules whose
//! subject is something the project *lacks*. A missing CI workflow anchors on the build file
//! that would have driven it. A rule that cannot name such a place does not ship — the
//! evidence requirement is not relaxed for advisories, because relaxing it would let a
//! capability claim anything. ADR-021.

pub mod scaffolding;
pub mod scope;
pub mod tooling;

use nexus_core::findings::Finding;
use nexus_core::project::{ProjectContext, Scoped};

pub trait Rule: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new finding.
    fn id(&self) -> &'static str;
    fn describe(&self) -> &'static str;
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>) -> Vec<Finding>;
}

pub fn all() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(tooling::DatastoreWithoutTooling),
        Box::new(scaffolding::NoContinuousIntegration),
        Box::new(scope::ScanningOneModule),
    ]
}

/// The first line of a `path:line` evidence string produced by `detect`.
///
/// Detections carry the file and line that proved them so `status` can be argued with rather
/// than believed, and an advisory finding inherits that: it points at the same line.
pub(crate) fn split_evidence(evidence: &str) -> (String, u32) {
    match evidence.rsplit_once(':') {
        Some((path, line)) => (path.to_string(), line.parse().unwrap_or(1)),
        None => (evidence.to_string(), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::split_evidence;

    #[test]
    fn evidence_keeps_the_line_that_proved_it() {
        assert_eq!(
            split_evidence("docker-compose.yml:12"),
            ("docker-compose.yml".to_string(), 12)
        );
        // A detection with no line still anchors on its file rather than nowhere.
        assert_eq!(
            split_evidence("build.gradle"),
            ("build.gradle".to_string(), 1)
        );
    }
}
