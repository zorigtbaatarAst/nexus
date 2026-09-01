//! Impact analysis: bounded, weighted, bidirectional BFS over `symbol_edges`.
//!
//! docs/change-analysis.md §5–7. The traversal is deliberately bounded by configuration
//! rather than by graph size, so a god-object with three thousand callers costs the cap
//! rather than three thousand seeks — and says that it was capped.

use crate::report::{Hop, ImpactItem, ImpactReport, SeedRef};
use nexus_store::{Neighbour, Store, SymbolRef};
use nexus_types::{EdgeType, SymbolId};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Who breaks if I change this. The important one.
    Reverse,
    /// What this reaches.
    Forward,
}

#[derive(Debug, Clone)]
pub struct ImpactQuery {
    pub target: String,
    pub direction: Direction,
    pub max_depth: usize,
    pub min_score: f64,
    pub fan_out_cap: usize,
    /// Restrict traversal to edges a body-only change can travel along.
    pub body_only: bool,
    pub limit: usize,
}

impl Default for ImpactQuery {
    fn default() -> Self {
        ImpactQuery {
            target: String::new(),
            direction: Direction::Reverse,
            max_depth: 5,
            min_score: 0.15,
            fan_out_cap: 200,
            body_only: false,
            limit: 100,
        }
    }
}

struct Reached {
    score: f64,
    min_confidence: f64,
    depth: usize,
    path: Vec<Hop>,
    node: Neighbour,
}

pub fn run(
    store: &Store,
    project_id: i64,
    seeds: &[SymbolRef],
    q: &ImpactQuery,
) -> Result<ImpactReport, nexus_store::StoreError> {
    let started = Instant::now();
    let _ = project_id;

    let mut best: HashMap<SymbolId, Reached> = HashMap::new();
    let mut truncated_at: Vec<String> = Vec::new();
    let mut crossed_seam = 0usize;

    // Seeds score 1.0 and are excluded from the result: the question is what *else* is
    // affected.
    let mut frontier: Vec<(SymbolId, f64, f64, usize, Vec<Hop>, String)> = seeds
        .iter()
        .map(|s| (s.id, 1.0, 1.0, 0, Vec::new(), s.fqn.clone()))
        .collect();
    let seed_ids: Vec<SymbolId> = seeds.iter().map(|s| s.id).collect();

    for depth in 1..=q.max_depth {
        let mut next = Vec::new();
        for (id, score, min_conf, _, path, fqn) in frontier.drain(..) {
            let mut neighbours = match q.direction {
                Direction::Reverse => store.edges_into(id)?,
                Direction::Forward => store.edges_out(id)?,
            };

            if q.body_only && depth == 1 {
                // A body edit cannot break a caller's compilation; it reaches one only
                // through shared state or an observable effect.
                neighbours.retain(|n| n.edge_type.carries_body_change());
            }

            if neighbours.len() > q.fan_out_cap {
                truncated_at.push(format!("{fqn} ({} edges)", neighbours.len()));
                neighbours.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
                neighbours.truncate(q.fan_out_cap);
            }

            for n in neighbours {
                let s = score * n.edge_type.weight() * n.confidence;
                if s < q.min_score {
                    continue;
                }
                if seed_ids.contains(&n.symbol_id) {
                    continue;
                }
                let improved = best.get(&n.symbol_id).is_none_or(|prev| s > prev.score);
                if !improved {
                    continue;
                }
                if matches!(n.edge_type, EdgeType::CallsGraphql | EdgeType::CallsHttp) {
                    crossed_seam += 1;
                }
                let mut new_path = path.clone();
                new_path.push(Hop {
                    from: fqn.clone(),
                    edge: n.edge_type.as_str(),
                    resolution: n.resolution.as_str(),
                    confidence: n.confidence,
                });
                let new_min = min_conf.min(n.confidence);
                next.push((
                    n.symbol_id,
                    s,
                    new_min,
                    depth,
                    new_path.clone(),
                    n.fqn.clone(),
                ));
                best.insert(
                    n.symbol_id,
                    Reached {
                        score: s,
                        min_confidence: new_min,
                        depth,
                        path: new_path,
                        node: n,
                    },
                );
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }

    let visited = best.len();
    let mut items: Vec<ImpactItem> = best
        .into_values()
        .map(|r| ImpactItem {
            fqn: r.node.fqn,
            kind: r.node.kind,
            file: r.node.file_path,
            line: r.node.start_line,
            score: (r.score * 1000.0).round() / 1000.0,
            min_confidence: r.min_confidence,
            depth: r.depth,
            path: r.path,
        })
        .collect();
    items.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.fqn.cmp(&b.fqn)));

    // Tests are separated out: "8 related tests" answers a different question from
    // "11 affected symbols", and mixing them buries both.
    let (tests, items): (Vec<_>, Vec<_>) =
        items.into_iter().partition(|i| is_test(&i.file, &i.fqn));

    // A symbol nothing reaches is not "uncovered" in any useful sense — it is unused, or
    // an entry point, and saying "no test covers this" about it would be noise. The claim
    // is only worth making about code something actually depends on.
    let uncovered =
        matches!(q.direction, Direction::Reverse) && tests.is_empty() && !items.is_empty();

    let mut items = items;
    items.truncate(q.limit);

    Ok(ImpactReport {
        target: q.target.clone(),
        direction: match q.direction {
            Direction::Reverse => "reverse",
            Direction::Forward => "forward",
        },
        seeds: seeds
            .iter()
            .map(|s| SeedRef {
                fqn: s.fqn.clone(),
                kind: s.kind.clone(),
                file: s.file_path.clone(),
                line: s.start_line,
            })
            .collect(),
        items,
        tests,
        uncovered,
        crossed_seam,
        truncated_at,
        visited,
        duration_ms: started.elapsed().as_millis(),
    })
}

/// Whether a symbol is test code.
///
/// Public because Review asks the same question — "does anything test this?" — and two
/// definitions of what a test is would let the impact report and the review capability
/// disagree about the same symbol.
pub fn is_test(file: &str, fqn: &str) -> bool {
    file.contains("/test/")
        || file.contains("/tests/")
        || file.ends_with(".test.ts")
        || file.ends_with(".test.tsx")
        || file.ends_with(".spec.ts")
        || fqn.contains("Test#")
        || fqn.ends_with("Test")
        || fqn.ends_with("Tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_only_change_travels_only_through_data_and_effect_edges() {
        assert!(!EdgeType::Implements.carries_body_change());
        assert!(!EdgeType::Extends.carries_body_change());
        assert!(!EdgeType::Imports.carries_body_change());
        assert!(EdgeType::Calls.carries_body_change());
        assert!(EdgeType::Persists.carries_body_change());
        assert!(EdgeType::CallsGraphql.carries_body_change());
    }

    #[test]
    fn score_decays_and_the_threshold_terminates_a_long_chain() {
        // calls at 0.9 with full confidence: 0.9^n falls under the 0.15 default at n = 19,
        // so the depth cap is what actually bounds a healthy graph — as intended.
        let mut s = 1.0f64;
        let mut hops = 0;
        while s >= 0.15 {
            s *= EdgeType::Calls.weight();
            hops += 1;
        }
        assert!(
            hops > 5,
            "the depth cap should bind before the score threshold does"
        );
    }

    #[test]
    fn test_files_are_recognized_on_both_stacks() {
        assert!(is_test(
            "src/test/java/mn/PaymentServiceTest.java",
            "mn.PaymentServiceTest#a()"
        ));
        assert!(is_test("src/lib/cart.test.ts", "src/lib/cart.test#totals"));
        assert!(!is_test(
            "src/main/java/mn/PaymentService.java",
            "mn.PaymentService#pay()"
        ));
    }
}
