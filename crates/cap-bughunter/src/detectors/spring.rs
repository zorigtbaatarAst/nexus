//! Spring detectors.
//!
//! Both rules here catch annotations that silently do nothing. Spring applies
//! `@Transactional` through a proxy, so a call that never leaves the object never passes
//! through it — the annotation compiles, reads correctly, and has no effect. A compiler
//! cannot express that property, but a call graph can, which is exactly the division of
//! labour the design argues for.

use super::Detector;
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped, SymbolFacts};
use nexus_types::{FindingType, Severity};

const TX: &str = "Transactional";

/// A Spring bean, as opposed to a plain class where the proxy never applies at all.
fn is_bean(owner: Option<&SymbolFacts>) -> bool {
    owner.is_some_and(|o| {
        [
            "Service",
            "Component",
            "Repository",
            "Controller",
            "RestController",
        ]
        .iter()
        .any(|a| o.has_annotation(a))
    })
}

pub struct TransactionalNonPublic;

impl Detector for TransactionalNonPublic {
    fn id(&self) -> &'static str {
        "spring:transactional-non-public"
    }

    fn describe(&self) -> &'static str {
        "@Transactional on a method the proxy cannot intercept"
    }

    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>) -> Vec<Finding> {
        let mut out = Vec::new();
        for s in &scoped.symbols {
            if s.kind != "method" || !s.has_annotation(TX) {
                continue;
            }
            let visibility = s.visibility.as_deref().unwrap_or("package-private");
            if visibility == "public" {
                continue;
            }
            let owner = s.parent_fqn.as_deref().and_then(|p| ctx.symbol(p));
            if !is_bean(owner) {
                continue;
            }
            out.push(Finding {
                finding_type: FindingType::Transaction,
                title: format!(
                    "@Transactional on {visibility} method {} has no effect",
                    s.name
                ),
                component: s.component(),
                anchor_fqn: Some(s.fqn.clone()),
                severity: Severity::High,
                confidence: 0.95,
                detector: self.id().to_string(),
                // The visibility is what makes it a bug, so it is part of what the finding
                // is about — changing the method to public resolves it rather than moving it.
                structural_key: format!("transactional-visibility:{visibility}"),
                slug: format!("transactional-not-public-{}", slugify(&s.name)),
                evidence: vec![CodeRef {
                    file: s.file.clone(),
                    line: s.line,
                    note: format!(
                        "Spring proxies intercept public methods only; on a {visibility} method \
                         @Transactional is never applied and the work runs outside a transaction"
                    ),
                }],
                capability_data: None,
            });
        }
        out
    }
}

pub struct SelfInvocation;

impl Detector for SelfInvocation {
    fn id(&self) -> &'static str {
        "spring:self-invocation"
    }

    fn describe(&self) -> &'static str {
        "an internal call bypassing the proxy that applies @Transactional"
    }

    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>) -> Vec<Finding> {
        let mut out = Vec::new();
        // Edges whose *source* is in scope: the finding is anchored on the caller.
        let in_scope: std::collections::HashSet<&str> =
            scoped.symbols.iter().map(|s| s.fqn.as_str()).collect();
        for e in ctx.edges {
            if e.edge_type != "calls" || !in_scope.contains(e.src_fqn.as_str()) {
                continue;
            }
            let Some(dst_fqn) = e.dst_fqn.as_deref() else {
                continue;
            };
            let (Some(src), Some(dst)) = (ctx.symbol(&e.src_fqn), ctx.symbol(dst_fqn)) else {
                continue;
            };
            // Same class on both ends is what makes it a self-invocation.
            if src.parent_fqn.is_none() || src.parent_fqn != dst.parent_fqn {
                continue;
            }
            if src.fqn == dst.fqn || !dst.has_annotation(TX) {
                continue;
            }
            // The caller being transactional already means one transaction is open, so the
            // callee's annotation is redundant rather than broken.
            if src.has_annotation(TX) {
                continue;
            }
            let owner = src.parent_fqn.as_deref().and_then(|p| ctx.symbol(p));
            if !is_bean(owner) {
                continue;
            }
            out.push(Finding {
                finding_type: FindingType::Transaction,
                title: format!(
                    "{} calls @Transactional {} internally, bypassing the proxy",
                    src.name, dst.name
                ),
                component: src.component(),
                anchor_fqn: Some(src.fqn.clone()),
                severity: Severity::High,
                confidence: 0.9,
                detector: self.id().to_string(),
                structural_key: format!("self-invocation:{}", dst.name),
                slug: format!(
                    "self-invocation-{}-{}",
                    slugify(&src.name),
                    slugify(&dst.name)
                ),
                evidence: vec![
                    CodeRef {
                        file: src.file.clone(),
                        line: e.line.unwrap_or(src.line),
                        note: format!(
                            "this call does not leave the object, so it never passes through \
                             the Spring proxy and {}'s @Transactional is not applied",
                            dst.name
                        ),
                    },
                    CodeRef {
                        file: dst.file.clone(),
                        line: dst.line,
                        note: format!("{} is annotated @Transactional", dst.name),
                    },
                ],
                capability_data: None,
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
    use nexus_core::project::{EdgeFacts, FileFacts};
    use std::path::Path;

    fn sym(fqn: &str, kind: &str, vis: &str, parent: Option<&str>, anns: &[&str]) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit(['#', '.']).next().unwrap_or(fqn).into(),
            kind: kind.into(),
            file: "S.java".into(),
            line: 10,
            visibility: Some(vis.into()),
            parent_fqn: parent.map(str::to_string),
            annotations: anns.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn run<D: Detector>(d: D, symbols: Vec<SymbolFacts>, edges: Vec<EdgeFacts>) -> Vec<Finding> {
        let files: Vec<FileFacts> = vec![];
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files);
        let scoped = ctx.scoped(&nexus_core::capability::Scope::Everything);
        d.run(&ctx, &scoped)
    }

    #[test]
    fn transactional_on_a_private_method_is_reported() {
        let found = run(
            TransactionalNonPublic,
            vec![
                sym("p.S", "class", "public", None, &["@Service"]),
                sym(
                    "p.S#doIt()",
                    "method",
                    "private",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].severity, Severity::High);
        assert!(!found[0].evidence.is_empty(), "a finding must be checkable");
    }

    #[test]
    fn transactional_on_a_public_method_is_fine() {
        let found = run(
            TransactionalNonPublic,
            vec![
                sym("p.S", "class", "public", None, &["@Service"]),
                sym(
                    "p.S#doIt()",
                    "method",
                    "public",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn a_plain_class_is_not_proxied_so_there_is_nothing_to_report() {
        // Without the bean check this rule fires on every private @Transactional method in
        // test helpers and plain POJOs, which is how a useful rule becomes noise.
        let found = run(
            TransactionalNonPublic,
            vec![
                sym("p.S", "class", "public", None, &[]),
                sym(
                    "p.S#doIt()",
                    "method",
                    "private",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn an_internal_call_to_a_transactional_method_is_reported() {
        let found = run(
            SelfInvocation,
            vec![
                sym("p.S", "class", "public", None, &["@Service"]),
                sym("p.S#outer()", "method", "public", Some("p.S"), &[]),
                sym(
                    "p.S#inner()",
                    "method",
                    "public",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![EdgeFacts {
                src_fqn: "p.S#outer()".into(),
                dst_fqn: Some("p.S#inner()".into()),
                dst_hint: None,
                edge_type: "calls".into(),
                resolution: "exact".into(),
                line: Some(12),
            }],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].evidence.len(),
            2,
            "both the call site and the annotation"
        );
    }

    #[test]
    fn a_call_from_another_class_goes_through_the_proxy_and_is_fine() {
        let found = run(
            SelfInvocation,
            vec![
                sym("p.S", "class", "public", None, &["@Service"]),
                sym("p.T", "class", "public", None, &["@Service"]),
                sym("p.T#outer()", "method", "public", Some("p.T"), &[]),
                sym(
                    "p.S#inner()",
                    "method",
                    "public",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![EdgeFacts {
                src_fqn: "p.T#outer()".into(),
                dst_fqn: Some("p.S#inner()".into()),
                dst_hint: None,
                edge_type: "calls".into(),
                resolution: "exact".into(),
                line: Some(12),
            }],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn an_already_transactional_caller_is_redundant_not_broken() {
        let found = run(
            SelfInvocation,
            vec![
                sym("p.S", "class", "public", None, &["@Service"]),
                sym(
                    "p.S#outer()",
                    "method",
                    "public",
                    Some("p.S"),
                    &["@Transactional"],
                ),
                sym(
                    "p.S#inner()",
                    "method",
                    "public",
                    Some("p.S"),
                    &["@Transactional"],
                ),
            ],
            vec![EdgeFacts {
                src_fqn: "p.S#outer()".into(),
                dst_fqn: Some("p.S#inner()".into()),
                dst_hint: None,
                edge_type: "calls".into(),
                resolution: "exact".into(),
                line: Some(12),
            }],
        );
        assert!(found.is_empty(), "a transaction is already open: {found:?}");
    }
}
