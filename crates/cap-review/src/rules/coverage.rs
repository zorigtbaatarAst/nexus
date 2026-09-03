//! A change to code no test reaches.
//!
//! The single most actionable thing to know before accepting an edit: if nothing tests this,
//! a mistake here does not fail loudly — it ships, and turns up later as behaviour nobody can
//! trace to a commit.
//!
//! Test files are already indexed and already emit call edges into production code, so this
//! is a query over the graph rather than an analysis. `nexus_core::impact::is_test` decides
//! what counts as a test, shared deliberately: two definitions would let `impact` and this
//! rule disagree about the same symbol.

use super::{Graph, Rule};
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::impact::is_test;
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{ChangeKind, FindingType, Severity};

/// How far to look for a covering test.
///
/// A test rarely calls the method under test directly — it calls a controller, or a service,
/// which calls it. Stopping at depth 1 would report most of a codebase as untested; going
/// deep enough to reach everything would report none of it, because in a connected graph
/// some test eventually reaches every symbol.
const DEPTH: usize = 4;

pub struct ChangedWithoutTest;

impl Rule for ChangedWithoutTest {
    fn id(&self) -> &'static str {
        "review:untested-change"
    }

    fn describe(&self) -> &'static str {
        "a change to code no test reaches, so a mistake in it fails silently"
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

        // A scoped snapshot holds the named files and one hop (roadmap 5.4). This rule
        // traverses four, so past the first hop it is looking at a hole — and "no test
        // reaches this" would then mean "no test reaches this within what I was given",
        // which is not a finding. Absence is only evidence when you can see everything.
        if ctx.partial_graph {
            return out;
        }

        for changed in ctx.changed {
            // A deletion has nothing left to test, and an addition of a test is not an
            // untested change.
            if matches!(changed.kind, ChangeKind::Deleted) {
                continue;
            }
            if is_test(&changed.path, &changed.fqn) {
                continue;
            }
            if !in_scope.contains(changed.fqn.as_str()) {
                continue;
            }
            let Some(sym) = ctx.symbol(&changed.fqn) else {
                continue;
            };
            // Types are covered through their members; reporting a class and each of its
            // methods separately says the same thing five times.
            if sym.kind != "method" {
                continue;
            }

            let reachable = graph.reachable_from(&changed.fqn, DEPTH);
            // Evidence when there is any, a guess when there is not, and the finding says
            // which. A run that actually happened proves coverage; a filename can only
            // suggest it, and reporting the second as the first is how a heuristic gets
            // mistaken for a measurement.
            let (covered, basis) = if ctx.has_coverage_evidence() {
                (
                    ctx.covered.contains(&changed.fqn),
                    "no test run has reached it",
                )
            } else {
                (
                    reachable.iter().any(|fqn| {
                        ctx.symbol(fqn)
                            .map(|s| is_test(&s.file, &s.fqn))
                            .unwrap_or(false)
                    }),
                    "no file that looks like a test reaches it — nothing has been run, so this \
                     is a guess from filenames rather than from a test run",
                )
            };
            if covered {
                continue;
            }
            // Nothing depends on it at all: that is unused code or an entry point, and
            // "no test covers this" about it is noise rather than a warning.
            if reachable.is_empty() {
                continue;
            }

            out.push(Finding {
                finding_type: FindingType::Review,
                title: format!("{} changed and no test reaches it", sym.name),
                component: sym.component(),
                anchor_fqn: Some(sym.fqn.clone()),
                severity: Severity::Medium,
                confidence: 0.9,
                detector: self.id().to_string(),
                structural_key: format!("untested:{}", sym.name),
                slug: format!("untested-{}", slugify(&sym.name)),
                evidence: vec![CodeRef {
                    file: sym.file.clone(),
                    line: sym.line,
                    note: format!(
                        "this changed in the current scan, {} other symbols depend on it, and \
                         {basis} — so a mistake here will not fail a test run",
                        reachable.len()
                    ),
                }],
                capability_data: Some(serde_json::json!({
                    "kind": "untested_change",
                    "change": format!("{:?}", changed.kind),
                    "dependents_within_depth": reachable.len(),
                    "depth_searched": DEPTH,
                    // Which of the two this rests on. Retiring `impact::is_test` as the
                    // coverage *source* (roadmap 4.5) does not retire the function: it is
                    // still the honest answer when nothing has run.
                    "coverage_source": if ctx.has_coverage_evidence() { "runtime" } else { "naming" },
                })),
            });
        }
        out
    }
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Graph;
    use nexus_core::capability::Scope;
    use nexus_core::project::{ChangedSymbol, EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    fn sym(fqn: &str, file: &str) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(['#', '.']).next().unwrap_or(fqn).into(),
            kind: "method".into(),
            file: file.into(),
            line: 10,
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
            edge_type: "calls".into(),
            resolution: "exact".into(),
            line: Some(3),
        }
    }

    fn run(symbols: Vec<SymbolFacts>, edges: Vec<EdgeFacts>) -> Vec<Finding> {
        let files: Vec<FileFacts> = Vec::new();
        let changed = vec![ChangedSymbol {
            fqn: "mn.a.Svc#save".into(),
            path: "src/main/java/mn/a/Svc.java".into(),
            kind: ChangeKind::BodyChanged,
        }];
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files)
            .with_changes(&changed, None);
        let scoped = ctx.scoped(&Scope::Everything);
        let graph = Graph::of(&ctx);
        ChangedWithoutTest.run(&ctx, &scoped, &graph)
    }

    #[test]
    fn a_change_a_test_reaches_is_not_reported() {
        let found = run(
            vec![
                sym("mn.a.Svc#save", "src/main/java/mn/a/Svc.java"),
                sym("mn.a.SvcTest#saves", "src/test/java/mn/a/SvcTest.java"),
            ],
            vec![edge("mn.a.SvcTest#saves", "mn.a.Svc#save")],
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn a_change_only_production_code_reaches_is_reported() {
        let found = run(
            vec![
                sym("mn.a.Svc#save", "src/main/java/mn/a/Svc.java"),
                sym("mn.a.Ctl#post", "src/main/java/mn/a/Ctl.java"),
            ],
            vec![edge("mn.a.Ctl#post", "mn.a.Svc#save")],
        );
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].finding_type, FindingType::Review);
        assert_eq!(found[0].evidence[0].file, "src/main/java/mn/a/Svc.java");
    }

    #[test]
    fn a_test_reached_through_a_controller_still_counts() {
        // Tests rarely call the method under test directly; stopping at depth 1 would report
        // most of a codebase as untested.
        let found = run(
            vec![
                sym("mn.a.Svc#save", "src/main/java/mn/a/Svc.java"),
                sym("mn.a.Ctl#post", "src/main/java/mn/a/Ctl.java"),
                sym("mn.a.CtlTest#posts", "src/test/java/mn/a/CtlTest.java"),
            ],
            vec![
                edge("mn.a.Ctl#post", "mn.a.Svc#save"),
                edge("mn.a.CtlTest#posts", "mn.a.Ctl#post"),
            ],
        );
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn code_nothing_depends_on_is_not_reported() {
        // Unused code or an entry point. "No test covers this" there is noise.
        let found = run(
            vec![sym("mn.a.Svc#save", "src/main/java/mn/a/Svc.java")],
            vec![],
        );
        assert!(found.is_empty(), "{found:#?}");
    }
}
