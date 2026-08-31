//! BugHunter — the first Nexus capability.
//!
//! Deterministic detectors over the project index. No model is asked, so nothing here is
//! subject to the 0.75 clamp that applies to a model's own confidence: both sides of every
//! claim are in the index, and comparing them is a query.
//!
//! Boundary rule: this crate depends on `nexus-core` and nothing else. It may not reach
//! `nexus-store`, `nexus-mcp` or `nexus-cli` — which is the concrete meaning of "BugHunter
//! is not coupled to a Nexus UI", and it is enforced by `tests/boundaries.rs`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod detectors;

use nexus_core::capability::{Capability, CapabilityError, Scope};
use nexus_core::findings::Finding;
use nexus_core::project::ProjectContext;

pub struct BugHunter;

impl Default for BugHunter {
    fn default() -> Self {
        Self::new()
    }
}

impl BugHunter {
    pub fn new() -> Self {
        BugHunter
    }
}

impl Capability for BugHunter {
    fn id(&self) -> &'static str {
        "bughunter"
    }

    fn finding_prefix(&self) -> &'static str {
        "BUG"
    }

    fn describe(&self) -> &'static str {
        "Deterministic bug detection: Spring proxy mistakes, GraphQL fields no resolver \
         serves, and credentials committed to source"
    }

    fn analyze(
        &self,
        ctx: &ProjectContext<'_>,
        scope: &Scope,
    ) -> Result<Vec<Finding>, CapabilityError> {
        // Narrowing happens once, here, rather than in each detector: a rule that forgets
        // to respect the scope makes a targeted analysis quietly cost as much as a full one.
        let scoped = ctx.scoped(scope);
        let mut out = Vec::new();
        for d in detectors::all() {
            out.extend(d.run(ctx, &scoped));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::project::{EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    fn service(
        fqn: &str,
        vis: &str,
        parent: Option<&str>,
        anns: &[&str],
        file: &str,
    ) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(['#', '.']).next().unwrap_or(fqn).into(),
            kind: if parent.is_some() {
                "method".into()
            } else {
                "class".into()
            },
            file: file.into(),
            line: 10,
            visibility: Some(vis.into()),
            parent_fqn: parent.map(str::to_string),
            annotations: anns.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn a_narrow_scope_examines_less() {
        // The claim the platform makes is that it does not re-analyze what it already
        // understands. If a scope does not actually reduce the work, that claim is false.
        let symbols = vec![
            service("p.A", "public", None, &["@Service"], "A.java"),
            service(
                "p.A#bad()",
                "private",
                Some("p.A"),
                &["@Transactional"],
                "A.java",
            ),
            service("p.B", "public", None, &["@Service"], "B.java"),
            service(
                "p.B#alsoBad()",
                "private",
                Some("p.B"),
                &["@Transactional"],
                "B.java",
            ),
        ];
        let edges: Vec<EdgeFacts> = vec![];
        let files: Vec<FileFacts> = vec![];
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files);

        let all = BugHunter::new()
            .analyze(&ctx, &Scope::Everything)
            .expect("all");
        assert_eq!(all.len(), 2);

        let one = BugHunter::new()
            .analyze(&ctx, &Scope::Files(vec!["A.java".into()]))
            .expect("one file");
        assert_eq!(
            one.len(),
            1,
            "a file scope must not examine the other file: {one:?}"
        );
        assert!(one[0].anchor_fqn.as_deref() == Some("p.A#bad()"));
    }

    #[test]
    fn the_capability_reports_its_own_identity() {
        let c = BugHunter::new();
        assert_eq!(c.id(), "bughunter");
        assert_eq!(c.finding_prefix(), "BUG", "findings are numbered BUG-n");
    }
}
