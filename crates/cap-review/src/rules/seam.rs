//! A backend change that reaches the frontend, where the frontend did not change with it.
//!
//! This is the rule reading the diff cannot replace. Nothing in the source text connects a
//! TypeScript `fetch` to a Java `@QueryMapping`; the two are unrelated symbols in any graph
//! that has not been joined at the schema. So an agent editing a service method sees no
//! reason to think a React component depends on it, and neither does the person accepting
//! the change.

use super::{Graph, Rule};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{ChangeKind, FindingType, Severity};

const DEPTH: usize = 5;

/// Frontend code, by the only signal available without a framework pack: the file it lives in.
fn is_frontend(file: &str) -> bool {
    file.ends_with(".ts")
        || file.ends_with(".tsx")
        || file.ends_with(".jsx")
        || file.ends_with(".js")
}

pub struct ChangeCrossesTheSeam;

impl Rule for ChangeCrossesTheSeam {
    fn id(&self) -> &'static str {
        "review:crosses-seam"
    }

    fn describe(&self) -> &'static str {
        "a backend change that reaches frontend code which did not change with it"
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
            // Only a contract change can break a caller that was not touched. A body-only
            // edit ripples through behaviour, not through compilation, and reporting every
            // one of those would make this fire on nearly every change.
            if !matches!(
                changed.kind,
                ChangeKind::ApiChanged
                    | ChangeKind::ApiAndBodyChanged
                    | ChangeKind::ContractChanged
            ) {
                continue;
            }
            if is_frontend(&changed.path) || !in_scope.contains(changed.fqn.as_str()) {
                continue;
            }
            let Some(sym) = ctx.symbol(&changed.fqn) else {
                continue;
            };

            // Frontend symbols this change reaches, that did not move with it. One that did
            // move is the normal case — someone changed both sides — and reporting it would
            // punish exactly the behaviour this rule wants.
            let reached: Vec<&str> = graph
                .reachable_from(&changed.fqn, DEPTH)
                .into_iter()
                .filter(|fqn| {
                    ctx.symbol(fqn)
                        .map(|s| is_frontend(&s.file))
                        .unwrap_or(false)
                        && !graph.is_changed(fqn)
                })
                .collect();
            if reached.is_empty() {
                continue;
            }

            let example = reached
                .iter()
                .filter_map(|f| ctx.symbol(f))
                .next()
                .map(|s| format!("{} ({}:{})", s.name, s.file, s.line))
                .unwrap_or_else(|| "a frontend component".into());

            out.push(Finding {
                finding_type: FindingType::Review,
                title: format!(
                    "{} changed its contract and {} frontend symbols depend on it",
                    sym.name,
                    reached.len()
                ),
                component: sym.component(),
                anchor_fqn: Some(sym.fqn.clone()),
                severity: Severity::High,
                confidence: 0.85,
                detector: self.id().to_string(),
                structural_key: format!("crosses-seam:{}", sym.name),
                slug: format!("seam-{}", sym.name.to_lowercase()),
                evidence: vec![CodeRef {
                    file: sym.file.clone(),
                    line: sym.line,
                    note: format!(
                        "this changed its signature or annotations, and reaches {example} \
                         across the GraphQL seam. Nothing in either file mentions the other, \
                         so neither the diff nor the compiler will say so.",
                    ),
                }],
                capability_data: Some(serde_json::json!({
                    "kind": "crosses_seam",
                    "change": format!("{:?}", changed.kind),
                    "frontend_symbols_reached": reached.len(),
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

    fn sym(fqn: &str, file: &str) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(['#', '.']).next().unwrap_or(fqn).into(),
            kind: "method".into(),
            file: file.into(),
            line: 4,
            visibility: Some("public".into()),
            parent_fqn: None,
            annotations: Vec::new(),
        }
    }

    fn edge(src: &str, dst: &str) -> EdgeFacts {
        EdgeFacts {
            src_fqn: src.into(),
            dst_fqn: Some(dst.into()),
            dst_hint: None,
            edge_type: "calls_graphql".into(),
            resolution: "contract".into(),
            line: Some(2),
        }
    }

    fn run(kind: ChangeKind, changed_fqns: &[&str]) -> Vec<Finding> {
        let symbols = vec![
            sym("mn.a.Svc#list", "backend/src/main/java/mn/a/Svc.java"),
            sym("web/page#Page", "frontend/src/app/page.tsx"),
        ];
        let edges = vec![edge("web/page#Page", "mn.a.Svc#list")];
        let files: Vec<FileFacts> = Vec::new();
        let changed: Vec<ChangedSymbol> = changed_fqns
            .iter()
            .map(|f| ChangedSymbol {
                fqn: (*f).into(),
                path: if f.starts_with("web/") {
                    "frontend/src/app/page.tsx".into()
                } else {
                    "backend/src/main/java/mn/a/Svc.java".into()
                },
                kind,
            })
            .collect();
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files)
            .with_changes(&changed, None);
        let scoped = ctx.scoped(&Scope::Everything);
        let graph = Graph::of(&ctx);
        ChangeCrossesTheSeam.run(&ctx, &scoped, &graph)
    }

    #[test]
    fn a_contract_change_reaching_an_untouched_component_is_reported() {
        let found = run(ChangeKind::ApiChanged, &["mn.a.Svc#list"]);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].severity, Severity::High);
    }

    #[test]
    fn changing_both_sides_together_is_the_normal_case() {
        // Reporting it would punish exactly the behaviour this rule wants to see.
        let found = run(ChangeKind::ApiChanged, &["mn.a.Svc#list", "web/page#Page"]);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_body_only_edit_does_not_break_a_caller() {
        // It ripples through behaviour, not through compilation. Firing here would make the
        // rule report nearly every change.
        let found = run(ChangeKind::BodyChanged, &["mn.a.Svc#list"]);
        assert!(found.is_empty(), "{found:#?}");
    }
}
