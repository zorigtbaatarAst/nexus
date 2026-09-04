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

pub fn compare(edges: &[Edge], oracle: &Oracle) -> Comparison {
    // Where the oracle says each reference resolves to.
    let mut truth: HashMap<(String, i64), &crate::oracle::Position> = HashMap::new();
    for r in &oracle.refs {
        if let Some(pos) = oracle.defs.get(&r.symbol) {
            truth.insert((r.file.clone(), r.line), pos);
        }
    }

    let mut out = Comparison::default();
    let mut sites = std::collections::HashSet::new();

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
        let Some(def) = truth.get(&site) else {
            // The oracle recorded no in-project reference here: it is silent, not
            // contradicting. Excluded, and reported separately.
            out.excluded_non_project += 1;
            continue;
        };
        let correct = match (&e.dst_file, e.dst_start_line, e.dst_end_line) {
            (Some(f), Some(s), Some(en)) => *f == def.file && def.line >= s && def.line <= en,
            _ => false,
        };
        out.judged.push(Judged {
            site,
            tier: e.resolution.clone(),
            confidence: e.confidence,
            correct,
        });
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
    fn a_site_the_oracle_never_saw_is_excluded_not_counted_wrong() {
        // No reference recorded at that line: the oracle is silent, not contradicting.
        let c = compare(&[edge(999, "src/a.rs", 40, 43, "heuristic", 0.6)], &oracle());
        assert!(c.judged.is_empty());
        assert_eq!(c.excluded_non_project, 1);
    }
}
