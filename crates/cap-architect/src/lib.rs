//! Architect — what this project is, and what working in it needs.
//!
//! The second Nexus capability, and the first whose findings are advisory rather than
//! defects. A recommendation is still a finding: it carries evidence, it can be dismissed,
//! and it can come back. ADR-021.
//!
//! Every rule here is deterministic. Nothing asks a model, so nothing is subject to the
//! 0.75 clamp — both sides of every claim are in the index or in the project's own files,
//! and comparing them is a query.
//!
//! Boundary rule: this crate depends on `nexus-core` and nothing else. It may not reach
//! `nexus-store`, `nexus-mcp` or `nexus-cli` — `tests/boundaries.rs` discovers `cap-*`
//! crates by prefix, so this is enforced without anyone adding a test for it.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod rules;

use nexus_core::capability::{Capability, CapabilityError, Scope};
use nexus_core::findings::Finding;
use nexus_core::project::ProjectContext;

pub struct Architect;

impl Default for Architect {
    fn default() -> Self {
        Self::new()
    }
}

impl Architect {
    pub fn new() -> Self {
        Architect
    }
}

impl Capability for Architect {
    fn id(&self) -> &'static str {
        "architect"
    }

    fn finding_prefix(&self) -> &'static str {
        "ARC"
    }

    fn describe(&self) -> &'static str {
        "What this project is and what working in it needs: datastores with no agent \
         tooling configured, missing scaffolding, and a scan that is looking at one module \
         of something larger"
    }

    fn analyze(
        &self,
        ctx: &ProjectContext<'_>,
        scope: &Scope,
    ) -> Result<Vec<Finding>, CapabilityError> {
        // Architect describes the project as a whole, so a narrowed run has nothing
        // meaningful to say: "this project has no CI" is not a statement about the three
        // files someone just edited. Reporting it anyway under `--changed` would attach a
        // project-wide claim to an arbitrary subset and make it look newly discovered.
        if !matches!(scope, Scope::Everything) {
            return Ok(Vec::new());
        }

        let scoped = ctx.scoped(scope);
        let mut out = Vec::new();
        for rule in rules::all() {
            out.extend(rule.run(ctx, &scoped));
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
    fn a_narrowed_run_says_nothing() {
        // "This project has no CI" is not a statement about three edited files.
        let symbols: Vec<SymbolFacts> = Vec::new();
        let edges: Vec<EdgeFacts> = Vec::new();
        let files = vec![FileFacts {
            path: "build.gradle".into(),
            lang: None,
        }];
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files);
        let found = Architect
            .analyze(&ctx, &Scope::Files(vec!["build.gradle".into()]))
            .expect("analyze");
        assert!(found.is_empty(), "{found:?}");
    }
}
