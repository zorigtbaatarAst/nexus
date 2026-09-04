//! Judge each bound destination against the oracle, by position.
//!
//! Never by name. SCIP writes `Alpha#save().` and Nexus writes `demo::Alpha#save(&self)`;
//! every rule mapping one spelling to the other is a place a nicer number could be
//! manufactured. A line number has no knobs.

use crate::edges::Edge;
use crate::oracle::Oracle;
use std::collections::HashMap;

/// Edge types SCIP can judge. Everything else — the GraphQL and HTTP seam, route tables,
/// ORM persistence, renders — is a relationship no compiler frontend models, so it is
/// excluded from both numerator and denominator rather than scored.
///
/// This is the one constant in the design that could move the headline number, which is why
/// it is named and documented rather than inlined into a filter: changing what is measured
/// must be visible in a diff.
const COMPARABLE_EDGE_TYPES: &[&str] = &["calls", "implements", "extends", "imports"];

/// Tiers whose edges nobody resolved against a symbol table. Judging them would score the
/// oracle's blind spots as Nexus's errors — the mistake ADR-017 already caught once.
const NON_RESOLVING_TIERS: &[&str] = &[
    "external",
    "sibling",
    "external-graph",
    "unresolved",
    "framework",
];

#[derive(Debug, Clone)]
pub struct Judged {
    pub site: (String, i64),
    pub tier: String,
    pub confidence: f64,
    pub correct: bool,
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub judged: Vec<Judged>,
    pub sites_total: usize,
    pub excluded_non_project: usize,
    pub excluded_oracle_blind: usize,
}

/// The width of the destination span, when the edge is bound and contains `def`.
///
/// `None` means "does not contain it", which is the same answer as an unbound edge.
fn containing_width(e: &Edge, def: &crate::oracle::Position) -> Option<i64> {
    match (&e.dst_file, e.dst_start_line, e.dst_end_line) {
        (Some(f), Some(start), Some(end))
            if *f == def.file && def.line >= start && def.line <= end =>
        {
            Some(end - start)
        }
        _ => None,
    }
}

pub fn compare(edges: &[Edge], oracle: &Oracle) -> Comparison {
    // Where the oracle says each reference resolves to. A line can carry more than one
    // reference — `a.foo().bar()` is two — so this is a list, and an edge is right if it
    // agrees with *any* of them. Keyed on one position, the second reference would overwrite
    // the first and the site's other candidate would be scored wrong for being right.
    let mut truth: HashMap<(String, i64), Vec<&crate::oracle::Position>> = HashMap::new();
    for r in &oracle.refs {
        if let Some(pos) = oracle.defs.get(&r.symbol) {
            truth.entry((r.file.clone(), r.line)).or_default().push(pos);
        }
    }

    let mut out = Comparison::default();
    let mut sites = std::collections::HashSet::new();
    // Per site, the narrowest containing span seen. §4.4: "with the innermost span winning
    // when spans nest." A method's span sits inside its `impl` block's, and both contain the
    // definition line — counting the enclosing one correct would pay the resolver for
    // pointing at the neighbourhood instead of the house.
    let mut narrowest: HashMap<(String, i64), i64> = HashMap::new();
    let mut pending: Vec<(Judged, Option<i64>)> = Vec::new();

    for e in edges {
        let Some(line) = e.site_line else { continue };
        let site = (e.src_file.clone(), line);
        sites.insert(site.clone());

        if !COMPARABLE_EDGE_TYPES.contains(&e.edge_type.as_str())
            || NON_RESOLVING_TIERS.contains(&e.resolution.as_str())
        {
            out.excluded_oracle_blind += 1;
            continue;
        }
        let Some(defs) = truth.get(&site) else {
            // The oracle recorded no in-project reference here: it is silent, not
            // contradicting. Excluded, and reported separately.
            out.excluded_non_project += 1;
            continue;
        };
        let width = defs.iter().filter_map(|d| containing_width(e, d)).min();
        if let Some(w) = width {
            let seen = narrowest.entry(site.clone()).or_insert(w);
            *seen = (*seen).min(w);
        }
        pending.push((
            Judged {
                site,
                tier: e.resolution.clone(),
                confidence: e.confidence,
                correct: false,
            },
            width,
        ));
    }

    // Second pass, because "innermost" is only known once every candidate at the site is in.
    for (mut judged, width) in pending {
        judged.correct = match (width, narrowest.get(&judged.site)) {
            (Some(w), Some(best)) => w == *best,
            _ => false,
        };
        out.judged.push(judged);
    }
    out.sites_total = sites.len();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edges::Edge;
    use crate::oracle::{Oracle, Position, Reference};

    fn edge(site_line: i64, dst_file: &str, start: i64, end: i64, tier: &str, conf: f64) -> Edge {
        Edge {
            src_fqn: "demo::Caller#run".into(),
            src_file: "src/b.rs".into(),
            site_line: Some(site_line),
            edge_type: "calls".into(),
            dst_fqn: Some("demo::Alpha#save".into()),
            dst_file: Some(dst_file.into()),
            dst_start_line: Some(start),
            dst_end_line: Some(end),
            resolution: tier.into(),
            confidence: conf,
        }
    }

    fn oracle() -> Oracle {
        let mut o = Oracle::default();
        o.defs.insert(
            "S Alpha#save().".into(),
            Position {
                file: "src/a.rs".into(),
                line: 41,
            },
        );
        o.refs.push(Reference {
            file: "src/b.rs".into(),
            line: 7,
            symbol: "S Alpha#save().".into(),
        });
        o.files.insert("src/a.rs".into());
        o.files.insert("src/b.rs".into());
        o
    }

    #[test]
    fn a_destination_whose_span_contains_the_definition_is_correct() {
        let c = compare(&[edge(7, "src/a.rs", 40, 43, "heuristic", 0.6)], &oracle());
        assert_eq!(c.judged.len(), 1);
        assert!(c.judged[0].correct, "definition at 41 falls inside 40..=43");
    }

    #[test]
    fn a_destination_in_the_right_file_but_the_wrong_span_is_wrong() {
        let c = compare(&[edge(7, "src/a.rs", 90, 99, "heuristic", 0.6)], &oracle());
        assert!(!c.judged[0].correct);
    }

    #[test]
    fn a_fan_out_is_judged_per_edge_so_three_candidates_are_three_judgements() {
        // Precision is edge-level on purpose: one right and two wrong at a single site is
        // 1/3, not 1/1. This is the arithmetic the old row-counted metric got backwards.
        let c = compare(
            &[
                edge(7, "src/a.rs", 40, 43, "heuristic", 0.2),
                edge(7, "src/c.rs", 1, 5, "heuristic", 0.2),
                edge(7, "src/d.rs", 1, 5, "heuristic", 0.2),
            ],
            &oracle(),
        );
        assert_eq!(c.judged.len(), 3);
        assert_eq!(c.judged.iter().filter(|j| j.correct).count(), 1);
    }

    #[test]
    fn an_edge_the_oracle_cannot_speak_about_is_excluded_not_counted_wrong() {
        // SCIP has no opinion about a GraphQL seam or a Spring bean. Counting the oracle's
        // blind spots as Nexus's errors is the mistake ADR-017 already caught once.
        let mut e = edge(7, "src/a.rs", 40, 43, "heuristic", 0.6);
        e.edge_type = "calls_graphql".into();
        let c = compare(&[e], &oracle());
        assert!(c.judged.is_empty());
        assert_eq!(c.excluded_oracle_blind, 1);
    }

    #[test]
    fn two_references_on_one_line_are_both_available_as_truth() {
        // `self.alpha.save()` and a second call on the same line: the oracle records two
        // references there. Keyed on one position the second overwrites the first, and the
        // candidate that agreed with the first is then scored wrong for being right.
        let mut o = oracle();
        o.defs.insert(
            "S Beta#load().".into(),
            Position {
                file: "src/a.rs".into(),
                line: 88,
            },
        );
        o.refs.push(Reference {
            file: "src/b.rs".into(),
            line: 7,
            symbol: "S Beta#load().".into(),
        });

        let c = compare(
            &[
                edge(7, "src/a.rs", 40, 43, "heuristic", 0.6),
                edge(7, "src/a.rs", 87, 90, "heuristic", 0.6),
            ],
            &o,
        );
        assert_eq!(
            c.judged.iter().filter(|j| j.correct).count(),
            2,
            "each candidate agrees with one of the two references on that line"
        );
    }

    #[test]
    fn when_spans_nest_only_the_innermost_is_correct() {
        // §4.4: "with the innermost span winning when spans nest." The method's span sits
        // inside its `impl` block's, and both contain the definition line. Paying for the
        // enclosing one rewards pointing at the neighbourhood instead of the house.
        let c = compare(
            &[
                edge(7, "src/a.rs", 30, 60, "heuristic", 0.6),
                edge(7, "src/a.rs", 40, 43, "heuristic", 0.6),
            ],
            &oracle(),
        );
        assert_eq!(c.judged.len(), 2);
        assert!(!c.judged[0].correct, "the enclosing span is not the answer");
        assert!(c.judged[1].correct, "the innermost span is");
    }

    #[test]
    fn a_site_the_oracle_never_saw_is_excluded_not_counted_wrong() {
        // No reference recorded at that line: the oracle is silent, not contradicting.
        let c = compare(
            &[edge(999, "src/a.rs", 40, 43, "heuristic", 0.6)],
            &oracle(),
        );
        assert!(c.judged.is_empty());
        assert_eq!(c.excluded_non_project, 1);
    }
}
