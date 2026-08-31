//! BugHunter's rules.
//!
//! Each takes the prepared project snapshot and the symbols in scope, and returns findings.
//! Detectors never touch storage or git: `nexus-core` decides whether a finding is new,
//! recurring, fixed or regressed, so no rule re-implements that answer.

pub mod graphql;
pub mod secrets;
pub mod spring;

use nexus_core::findings::Finding;
use nexus_core::project::{ProjectContext, Scoped};

pub trait Detector: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new finding.
    fn id(&self) -> &'static str;
    fn describe(&self) -> &'static str;
    /// `scoped` is the narrowed view; `ctx` is the whole project for the rules that
    /// genuinely need to look past the scope — a self-invocation needs the callee's
    /// annotations even when the callee itself was not asked about.
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>) -> Vec<Finding>;
}

pub fn all() -> Vec<Box<dyn Detector>> {
    vec![
        Box::new(spring::TransactionalNonPublic),
        Box::new(spring::SelfInvocation),
        Box::new(graphql::OrphanOperation),
        Box::new(secrets::HardcodedSecret),
    ]
}
