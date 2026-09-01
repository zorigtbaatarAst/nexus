//! Review's rules.
//!
//! Each answers a question about a change that the person accepting it cannot answer by
//! reading the diff — which is the only kind of rule that belongs here. A rule that could be
//! replaced by looking at the patch is either a linter or an opinion, and this capability is
//! neither.

pub mod coverage;
pub mod fanout;
pub mod seam;

use nexus_core::findings::Finding;
use nexus_core::project::{ProjectContext, Scoped};
use std::collections::{HashMap, HashSet};

pub trait Rule: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not.
    fn id(&self) -> &'static str;
    fn describe(&self) -> &'static str;
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>, graph: &Graph<'_>)
        -> Vec<Finding>;
}

pub fn all() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(coverage::ChangedWithoutTest),
        Box::new(seam::ChangeCrossesTheSeam),
        Box::new(fanout::CallersDidNotMove),
    ]
}

/// Reverse adjacency, built once for the whole run.
///
/// Every rule here asks "who depends on this?", and rebuilding the answer per rule turns a
/// linear pass over the edge list into a quadratic one — the same mistake `ctx.by_fqn` exists
/// to prevent for symbols. On a six-service monorepo the edge list is 40,000 entries.
pub struct Graph<'a> {
    dependents: HashMap<&'a str, Vec<&'a str>>,
    changed: HashSet<&'a str>,
}

impl<'a> Graph<'a> {
    pub fn of(ctx: &'a ProjectContext<'a>) -> Self {
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in ctx.edges {
            if let Some(dst) = e.dst_fqn.as_deref() {
                dependents.entry(dst).or_default().push(&e.src_fqn);
            }
        }
        Graph {
            dependents,
            changed: ctx.changed.iter().map(|c| c.fqn.as_str()).collect(),
        }
    }

    /// Direct dependents of a symbol.
    pub fn dependents_of(&self, fqn: &str) -> &[&'a str] {
        self.dependents.get(fqn).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Everything reachable *backwards* from a symbol, to a bounded depth.
    ///
    /// Bounded because a base class in a shared module reaches most of a monorepo, and a
    /// review that reports eight hundred affected symbols is not a review. The depth is the
    /// same one `impact` defaults to.
    pub fn reachable_from(&self, fqn: &'a str, max_depth: usize) -> HashSet<&'a str> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut frontier: Vec<&str> = vec![fqn];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for node in frontier {
                for dep in self.dependents_of(node).iter().copied() {
                    if seen.insert(dep) {
                        next.push(dep);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        seen
    }

    /// Whether this symbol moved in the scan under review.
    pub fn is_changed(&self, fqn: &str) -> bool {
        self.changed.contains(fqn)
    }
}
