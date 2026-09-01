//! Review — is this change safe, judged by the graph and the history rather than by taste.
//!
//! In a loop where an agent writes the code, there is no pull request: the review moment
//! collapses into *"it just finished, before I accept it"*. That is the moment this capability
//! serves, and it is why it runs on the working tree rather than on a diff against a branch.
//!
//! `docs/roadmap.md` once listed "be a code review tool" as a never-do. The revision to that
//! non-goal is what this crate has to keep honest, so the constraint is structural rather than
//! aspirational: **every rule here reports something the index can prove.** Naming,
//! formatting, and "this could be cleaner" are the taste the non-goal exists to keep out, and
//! no rule here has an opinion about how code is written — only about what a change reaches,
//! what covers it, and what has gone wrong there before.
//!
//! Boundary rule: depends on `nexus-core` and nothing else, enforced for every `cap-*` crate
//! by `tests/boundaries.rs`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod rules;

use nexus_core::capability::{Capability, CapabilityError, Scope};
use nexus_core::findings::Finding;
use nexus_core::project::ProjectContext;

pub struct Review;

impl Default for Review {
    fn default() -> Self {
        Self::new()
    }
}

impl Review {
    pub fn new() -> Self {
        Review
    }
}

impl Capability for Review {
    fn id(&self) -> &'static str {
        "review"
    }

    fn finding_prefix(&self) -> &'static str {
        "REV"
    }

    fn describe(&self) -> &'static str {
        "What a change reaches and what covers it: edits nothing tests, edits that cross the \
         frontend/backend seam, and signature changes whose callers did not move with them"
    }

    fn analyze(
        &self,
        ctx: &ProjectContext<'_>,
        scope: &Scope,
    ) -> Result<Vec<Finding>, CapabilityError> {
        // Every rule is anchored on something that moved, so with nothing changed there is
        // nothing to review. This is also what makes a full-project run harmless rather than
        // a flood: `ctx.changed` is empty under `Scope::Everything`.
        if ctx.changed.is_empty() {
            return Ok(Vec::new());
        }

        let scoped = ctx.scoped(scope);
        let graph = rules::Graph::of(ctx);
        let mut out = Vec::new();
        for rule in rules::all() {
            out.extend(rule.run(ctx, &scoped, &graph));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::project::{EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    #[test]
    fn nothing_changed_is_nothing_to_review() {
        let symbols: Vec<SymbolFacts> = Vec::new();
        let edges: Vec<EdgeFacts> = Vec::new();
        let files: Vec<FileFacts> = Vec::new();
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files);
        assert!(Review
            .analyze(&ctx, &Scope::Everything)
            .expect("analyze")
            .is_empty());
    }
}
