//! BugHunter's rules.
//!
//! Each takes the prepared project snapshot and the symbols in scope, and returns findings.
//! Detectors never touch storage or git: `nexus-core` decides whether a finding is new,
//! recurring, fixed or regressed, so no rule re-implements that answer.

pub mod graphql;
pub mod spring;

// The trait was called `Detector` here and `Rule` in the other two capabilities, for the
// same shape. One declaration now; the alias keeps `detectors::all()` reading naturally.
pub use nexus_core::rules::Rule as Detector;
pub use nexus_core::rules::{Graph, Rule};

pub fn all() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(spring::TransactionalNonPublic),
        Box::new(spring::SelfInvocation),
        Box::new(graphql::OrphanOperation),
    ]
}
