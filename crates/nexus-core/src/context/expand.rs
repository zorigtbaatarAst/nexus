//! Stage 3 — what else the seeds reach.
//!
//! `impact::run` unchanged, which §5 requires: one traversal, one set of bounds, one
//! definition of what an edge is worth. A second traversal written for context would be a
//! second answer to "what does this affect", and the two would disagree on a Tuesday.
//!
//! Direction follows intent. Every expanded candidate keeps the `Hop` chain that reached it
//! and the weakest confidence along that chain, which is what makes its presence in the
//! package provable rather than asserted.

use super::seeds::Seed;
use crate::context::intent::Intent;
use crate::impact::{self, Direction, ImpactQuery};
use crate::report::ImpactReport;
use nexus_store::{Store, StoreError, SymbolRef};

/// Which way to walk, as a word. `both` merges a reverse and a forward pass.
///
/// `Build` is reverse on purpose: the question behind "add a thing here" is what already
/// depends on the place it is going, not what that place happens to call.
pub fn direction_for(intent: Intent) -> &'static str {
    match intent {
        Intent::Refactor | Intent::Review | Intent::Build => "reverse",
        Intent::Debug => "forward",
        // A referential turn carries seeds from a previous package but no fresh verb, so
        // neither direction is implied. Both, as with Unknown.
        Intent::Explain | Intent::Unknown | Intent::Referential => "both",
    }
}

fn refs(seeds: &[Seed]) -> Vec<SymbolRef> {
    seeds.iter().map(|s| s.symbol.clone()).collect()
}

/// Expand from the seeds. An empty seed set is an empty report, not an error: stage 2 has
/// already said in its notes that it anchored nothing, and failing here would report the same
/// fact twice as two different kinds of problem.
pub fn run(
    store: &Store,
    project_id: i64,
    seeds: &[Seed],
    intent: Intent,
) -> Result<ImpactReport, StoreError> {
    let direction = direction_for(intent);
    let refs = refs(seeds);
    let base = ImpactQuery {
        target: String::new(),
        direction: Direction::Reverse,
        ..Default::default()
    };

    if refs.is_empty() {
        let mut empty = impact::run(store, project_id, &[], &base)?;
        empty.direction = direction;
        return Ok(empty);
    }

    let forward_query = ImpactQuery {
        direction: Direction::Forward,
        ..base.clone()
    };

    let mut report = match direction {
        "forward" => impact::run(store, project_id, &refs, &forward_query)?,
        "both" => {
            let mut reverse = impact::run(store, project_id, &refs, &base)?;
            let forward = impact::run(store, project_id, &refs, &forward_query)?;
            // A symbol reached both ways is one candidate with the better score, not two.
            // §9 calls deduplication the largest single saving on a dense graph, and doing it
            // here means the budget never sees the duplicate at all.
            for item in forward.items {
                match reverse.items.iter_mut().find(|i| i.fqn == item.fqn) {
                    Some(existing) if existing.score >= item.score => {}
                    Some(existing) => *existing = item,
                    None => reverse.items.push(item),
                }
            }
            reverse.crossed_seam += forward.crossed_seam;
            reverse.truncated_at.extend(forward.truncated_at);
            reverse
                .items
                .sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.fqn.cmp(&b.fqn)));
            reverse
        }
        _ => impact::run(store, project_id, &refs, &base)?,
    };

    report.direction = direction;
    report.target = seeds
        .iter()
        .map(|s| s.symbol.fqn.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(report)
}
