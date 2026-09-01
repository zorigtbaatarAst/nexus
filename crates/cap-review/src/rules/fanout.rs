//! A signature changed and its callers did not move with it.
//!
//! In a language the compiler checks, most of these fail the build and never reach a review.
//! The ones that matter are the callers a compiler cannot see: a Spring bean resolved by
//! name, a reflective lookup, a GraphQL field served through an annotation — and the callers
//! in another module that was not rebuilt.
//!
//! Anchored on the changed symbol rather than on each caller, because the change is one
//! decision. Reporting it once per caller turns a single question into thirty.

use super::{Graph, Rule};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{ChangeKind, FindingType, Severity};

/// Below this, a changed signature with a couple of stale callers is ordinary work in
/// progress. Above it, a contract moved under a lot of code at once.
const MANY: usize = 3;

pub struct CallersDidNotMove;

impl Rule for CallersDidNotMove {
    fn id(&self) -> &'static str {
        "review:stale-callers"
    }

    fn describe(&self) -> &'static str {
        "a signature changed while the code calling it did not"
    }

    fn run(
        &self,
        ctx: &ProjectContext<'_>,
        scoped: &Scoped<'_>,
        graph: &Graph<'_>,
    ) -> Vec<Finding> {
        let in_scope: std::collections::HashSet<&str> =
            scoped.symbols.iter().map(|s| s.fqn.as_str()).collect();
        let mut out = Vec::new();

        for changed in ctx.changed {
            if !matches!(
                changed.kind,
                ChangeKind::ApiChanged | ChangeKind::ApiAndBodyChanged
            ) {
                continue;
            }
            if !in_scope.contains(changed.fqn.as_str()) {
                continue;
            }
            let Some(sym) = ctx.symbol(&changed.fqn) else {
                continue;
            };

            let stale: Vec<&str> = graph
                .dependents_of(&changed.fqn)
                .iter()
                .copied()
                .filter(|f| !graph.is_changed(f))
                .collect();
            if stale.len() < MANY {
                continue;
            }

            let example = stale
                .iter()
                .filter_map(|f| ctx.symbol(f))
                .next()
                .map(|s| format!("{}:{}", s.file, s.line))
                .unwrap_or_else(|| "elsewhere".into());

            out.push(Finding {
                finding_type: FindingType::Review,
                title: format!(
                    "{}'s signature changed and {} callers did not",
                    sym.name,
                    stale.len()
                ),
                component: sym.component(),
                anchor_fqn: Some(sym.fqn.clone()),
                severity: Severity::Medium,
                confidence: 0.8,
                detector: self.id().to_string(),
                structural_key: format!("stale-callers:{}", sym.name),
                slug: format!("stale-callers-{}", sym.name.to_lowercase()),
                evidence: vec![CodeRef {
                    file: sym.file.clone(),
                    line: sym.line,
                    note: format!(
                        "the signature moved here while {} direct callers stayed as they \
                         were — the first is at {example}. A compiler catches most of these; \
                         it does not catch a caller resolved by name, by reflection, or in a \
                         module that was not rebuilt.",
                        stale.len()
                    ),
                }],
                capability_data: Some(serde_json::json!({
                    "kind": "stale_callers",
                    "unchanged_callers": stale.len(),
                })),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::capability::Scope;
    use nexus_core::project::{ChangedSymbol, EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    fn sym(fqn: &str) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(['#', '.']).next().unwrap_or(fqn).into(),
            kind: "method".into(),
            file: "A.java".into(),
            line: 3,
            visibility: Some("public".into()),
            parent_fqn: None,
            annotations: Vec::new(),
        }
    }

    fn run(callers: usize, also_changed: usize) -> Vec<Finding> {
        let mut symbols = vec![sym("mn.a.Svc#list")];
        let mut edges = Vec::new();
        for i in 0..callers {
            let f = format!("mn.a.C{i}#m");
            symbols.push(sym(&f));
            edges.push(EdgeFacts {
                src_fqn: f,
                dst_fqn: Some("mn.a.Svc#list".into()),
                dst_hint: None,
                edge_type: "calls".into(),
                resolution: "exact".into(),
                line: Some(1),
            });
        }
        let mut changed = vec![ChangedSymbol {
            fqn: "mn.a.Svc#list".into(),
            path: "A.java".into(),
            kind: ChangeKind::ApiChanged,
        }];
        for i in 0..also_changed {
            changed.push(ChangedSymbol {
                fqn: format!("mn.a.C{i}#m"),
                path: "A.java".into(),
                kind: ChangeKind::BodyChanged,
            });
        }
        let files: Vec<FileFacts> = Vec::new();
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files)
            .with_changes(&changed, None);
        let scoped = ctx.scoped(&Scope::Everything);
        let graph = Graph::of(&ctx);
        CallersDidNotMove.run(&ctx, &scoped, &graph)
    }

    #[test]
    fn many_untouched_callers_are_reported_once() {
        // Once, not once per caller: the change is a single decision.
        let found = run(5, 0);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].title.contains("5 callers"), "{}", found[0].title);
    }

    #[test]
    fn callers_that_moved_with_it_do_not_count() {
        let found = run(5, 5);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_couple_of_stale_callers_is_work_in_progress() {
        assert!(run(2, 0).is_empty());
    }
}
