//! The scan is looking at one module of something larger.
//!
//! This is the finding that could not exist before `sibling` edges did. An edge resolved as
//! `sibling` means the target is code this project owns and this scan did not index — and a
//! project scanned that way answers every impact question with a small blast radius and
//! total confidence, which is worse than answering none of them.
//!
//! Measured on a six-service monorepo: scanning one module classified 6,247 edges as
//! sibling, and `impact` on the base class every entity extends answered "no symbol matches".

use super::Rule;
use nexus_core::findings::{CodeRef, Finding};
use nexus_core::project::{ProjectContext, Scoped};
use nexus_types::{FindingType, Severity};

/// One stray reference to an unindexed package is a typo or a generated artifact; a module's
/// worth of them is a module. Matches `SIBLING_WARN_FLOOR` in `nexus-core`, which governs the
/// same judgement for the scan warning — the two must agree or the CLI and the capability
/// disagree about the same project.
const FLOOR: usize = 20;

pub struct ScanningOneModule;

impl Rule for ScanningOneModule {
    fn id(&self) -> &'static str {
        "architect:partial-scan"
    }

    fn describe(&self) -> &'static str {
        "this scan covers one module of a larger project, so impact is understated"
    }

    fn run(&self, ctx: &ProjectContext<'_>, _scoped: &Scoped<'_>) -> Vec<Finding> {
        let siblings: Vec<&nexus_core::project::EdgeFacts> = ctx
            .edges
            .iter()
            .filter(|e| e.resolution == "sibling")
            .collect();
        if siblings.len() < FLOOR {
            return Vec::new();
        }

        // Anchor on a real call site: the file and line where this module reaches into the
        // code that was not scanned. That is both the evidence and the most useful place to
        // start looking.
        let anchor = siblings.iter().find_map(|e| {
            let src = ctx.symbol(&e.src_fqn)?;
            Some((src.file.clone(), e.line.unwrap_or(src.line)))
        });
        let Some((file, line)) = anchor else {
            // Sibling edges whose sources are not in the symbol table cannot be pointed at,
            // and a finding that cannot point at anything is not one.
            return Vec::new();
        };

        let example = siblings
            .iter()
            .filter_map(|e| e.dst_hint.as_deref())
            .next()
            .unwrap_or("another module");

        vec![Finding {
            finding_type: FindingType::Architecture,
            title: format!(
                "{} references reach code this scan does not cover",
                siblings.len()
            ),
            component: "scan".into(),
            anchor_fqn: None,
            // High because it silently invalidates every other answer the tool gives: an
            // impact query here reports a small blast radius with total confidence.
            severity: Severity::High,
            confidence: 0.9,
            detector: self.id().to_string(),
            structural_key: "partial-scan".into(),
            slug: "scanning-one-module".into(),
            evidence: vec![CodeRef {
                file,
                line,
                note: format!(
                    "this reaches {example}, which belongs to this project and is not in \
                     the index — so impact from here stops at the module boundary and \
                     reports fewer affected symbols than there are. Scan from the \
                     repository root instead."
                ),
            }],
            capability_data: Some(serde_json::json!({
                "kind": "partial_scan",
                "sibling_edges": siblings.len(),
                "remedy": "scan from the repository root",
            })),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::capability::Scope;
    use nexus_core::project::{EdgeFacts, FileFacts, SymbolFacts};
    use std::path::Path;

    fn sym(fqn: &str) -> SymbolFacts {
        SymbolFacts {
            fqn: fqn.into(),
            name: fqn.rsplit('.').next().unwrap_or(fqn).into(),
            kind: "method".into(),
            file: "sales/S.java".into(),
            line: 7,
            visibility: Some("public".into()),
            parent_fqn: None,
            annotations: Vec::new(),
        }
    }

    fn edge(res: &str) -> EdgeFacts {
        EdgeFacts {
            src_fqn: "mn.autoland.sales.S".into(),
            dst_fqn: None,
            dst_hint: Some("mn.autoland.model.BaseEntity".into()),
            edge_type: "calls".into(),
            resolution: res.into(),
            line: Some(11),
        }
    }

    fn run(edges: Vec<EdgeFacts>) -> Vec<Finding> {
        let symbols = vec![sym("mn.autoland.sales.S")];
        let files: Vec<FileFacts> = Vec::new();
        let ctx = ProjectContext::new(Path::new("/"), &symbols, &edges, &files);
        let scoped = ctx.scoped(&Scope::Everything);
        ScanningOneModule.run(&ctx, &scoped)
    }

    #[test]
    fn a_module_worth_of_sibling_edges_is_reported() {
        let found = run((0..FLOOR).map(|_| edge("sibling")).collect());
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].evidence[0].file, "sales/S.java");
        assert_eq!(found[0].evidence[0].line, 11);
    }

    #[test]
    fn a_stray_reference_is_not_a_missing_module() {
        // A warning that fires on noise is one people learn to scroll past.
        assert!(run(vec![edge("sibling")]).is_empty());
    }

    #[test]
    fn a_library_is_not_a_sibling() {
        let found = run((0..FLOOR + 5).map(|_| edge("external")).collect());
        assert!(found.is_empty(), "{found:#?}");
    }
}
