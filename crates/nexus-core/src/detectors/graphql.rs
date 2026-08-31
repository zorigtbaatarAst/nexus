//! The seam, used as a detector.
//!
//! Once both sides of a GraphQL contract are indexed, a frontend operation selecting a root
//! field that no backend resolver serves is a bug that falls out of the join — no model, no
//! heuristic. It is the shape behind a typo in an operation, a field renamed on one side
//! only, and an endpoint deleted while its caller stayed.

use super::{DetectContext, Detector};
use crate::bugs::{BugCandidate, CodeRef};
use nexus_types::{BugType, Severity};

pub struct OrphanOperation;

impl Detector for OrphanOperation {
    fn id(&self) -> &'static str {
        "graphql:orphan-operation"
    }

    fn describe(&self) -> &'static str {
        "a frontend operation selecting a field no backend resolver serves"
    }

    fn run(&self, ctx: &DetectContext<'_>) -> Vec<BugCandidate> {
        // Guard: a frontend-only scan has no resolvers at all, and reporting every
        // operation as orphaned would be technically true and completely useless. The rule
        // only means something when both sides are in the index.
        let backend_serves_anything = ctx.symbols.iter().any(|s| {
            s.kind == "route" && s.fqn.starts_with("graphql:") && !s.fqn.starts_with("graphql:op:")
        });
        if !backend_serves_anything {
            return Vec::new();
        }

        let mut out = Vec::new();
        for e in ctx.edges {
            if e.edge_type != "calls_graphql" || e.dst_fqn.is_some() {
                continue;
            }
            let Some(hint) = e.dst_hint.as_deref() else {
                continue;
            };
            // Only a schema coordinate. An unresolved `graphql:op:` edge means the operation
            // document was not indexed, which is a scan-coverage matter, not a bug.
            if !hint.starts_with("graphql:") || hint.starts_with("graphql:op:") {
                continue;
            }
            let Some(src) = ctx.symbol(&e.src_fqn) else {
                continue;
            };
            let field = hint.trim_start_matches("graphql:");

            out.push(BugCandidate {
                bug_type: BugType::ApiContract,
                title: format!(
                    "operation {} selects {field}, which no resolver serves",
                    src.name
                ),
                component: src.name.clone(),
                anchor_fqn: Some(src.fqn.clone()),
                severity: Severity::High,
                confidence: 0.9,
                detector: self.id().to_string(),
                structural_key: format!("orphan-field:{field}"),
                slug: format!("graphql-orphan-{}", field.replace('.', "-").to_lowercase()),
                evidence: vec![CodeRef {
                    file: src.file.clone(),
                    line: e.line.unwrap_or(src.line),
                    note: format!(
                        "this document selects the root field `{field}`, and no @QueryMapping, \
                         @MutationMapping or @SchemaMapping in the project serves it — the \
                         request will fail at runtime with a validation error"
                    ),
                }],
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detectors::{EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    fn sym(fqn: &str, kind: &str) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(':').next().unwrap_or(fqn).into(),
            kind: kind.into(),
            file: "x.ts".into(),
            line: 3,
            visibility: None,
            parent_fqn: None,
            annotations: vec![],
        }
    }

    fn edge(src: &str, hint: &str, resolved: bool) -> EdgeFacts {
        EdgeFacts {
            src_fqn: src.into(),
            dst_fqn: resolved.then(|| hint.to_string()),
            dst_hint: Some(hint.into()),
            edge_type: "calls_graphql".into(),
            resolution: if resolved {
                "contract".into()
            } else {
                "unresolved".into()
            },
            line: Some(7),
        }
    }

    fn run(symbols: Vec<SymbolFacts>, edges: Vec<EdgeFacts>) -> Vec<BugCandidate> {
        let files: Vec<FileFacts> = vec![];
        let ctx = DetectContext::new(Path::new("/"), &symbols, &edges, &files);
        OrphanOperation.run(&ctx)
    }

    #[test]
    fn an_unserved_field_is_reported() {
        let found = run(
            vec![
                sym("graphql:op:Vehicles", "route"),
                sym("graphql:Query.vehicles", "route"),
            ],
            vec![edge("graphql:op:Vehicles", "graphql:Query.vehiclez", false)],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].bug_type, BugType::ApiContract);
        assert!(found[0].evidence[0].note.contains("vehiclez"));
    }

    #[test]
    fn a_served_field_is_not_reported() {
        let found = run(
            vec![
                sym("graphql:op:Vehicles", "route"),
                sym("graphql:Query.vehicles", "route"),
            ],
            vec![edge("graphql:op:Vehicles", "graphql:Query.vehicles", true)],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn a_frontend_only_scan_reports_nothing() {
        // Otherwise every operation in the project is "orphaned" — true, and useless.
        let found = run(
            vec![sym("graphql:op:Vehicles", "route")],
            vec![edge("graphql:op:Vehicles", "graphql:Query.vehicles", false)],
        );
        assert!(
            found.is_empty(),
            "a missing backend is not a hundred bugs: {found:?}"
        );
    }
}
