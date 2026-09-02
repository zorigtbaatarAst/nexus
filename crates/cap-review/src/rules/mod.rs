//! Review's rules.
//!
//! Each answers a question about a change that the person accepting it cannot answer by
//! reading the diff — which is the only kind of rule that belongs here. A rule that could be
//! replaced by looking at the patch is either a linter or an opinion, and this capability is
//! neither.

pub mod coverage;
pub mod fanout;
pub mod seam;

pub use nexus_core::rules::{Graph, Rule};

// `pub use` rather than `use`: `lib.rs` names `rules::Graph::of(ctx)`, and re-exporting keeps
// that call site reading the same after the trait moved into the platform.

pub fn all() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(coverage::ChangedWithoutTest),
        Box::new(seam::ChangeCrossesTheSeam),
        Box::new(fanout::CallersDidNotMove),
    ]
}
