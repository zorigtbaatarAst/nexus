//! The rule abstraction every capability shares.
//!
//! A capability owns rules; the platform owns everything else. That split was already the
//! design — it was just declared three times, once per capability, with the same shape and
//! very nearly the same doc comment. This is the one declaration.
//!
//! `Graph` lives here for the same reason. Every capability asking "who depends on this?"
//! needs reverse adjacency, and two implementations of that question can disagree about the
//! same symbol without anything failing. It is the in-memory counterpart to `impact::run`,
//! which answers the same question against the store for callers that have one.

use crate::findings::Finding;
use crate::project::{ProjectContext, Scoped};
use std::collections::{HashMap, HashSet};

pub trait Rule: Send + Sync {
    /// `family:rule`. The family half feeds the fingerprint; the rule half does not, so a
    /// rule can be renamed without inventing a new finding.
    fn id(&self) -> &'static str;

    fn describe(&self) -> &'static str;

    /// `scoped` is the narrowed view; `ctx` is the whole project, for the rules that
    /// genuinely need to look past the scope — a self-invocation needs the callee's
    /// annotations even when the callee itself was not asked about. `graph` is reverse
    /// adjacency, built once per analysis rather than once per rule.
    fn run(&self, ctx: &ProjectContext<'_>, scoped: &Scoped<'_>, graph: &Graph<'_>)
        -> Vec<Finding>;
}

/// Reverse adjacency, built once for the whole run.
///
/// Every rule that asks "who depends on this?" needs it, and rebuilding the answer per rule
/// turns a linear pass over the edge list into a quadratic one — the same mistake
/// `ctx.by_fqn` exists to prevent for symbols. On a six-service monorepo the edge list is
/// 40,000 entries.
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
