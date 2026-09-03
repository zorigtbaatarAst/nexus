//! What git knows, turned into signals and a ledger.
//!
//! Two separate things, deliberately not one. The `commits` table is a **ledger** — append
//! only, one row per commit, so "what did this project look like at scan 12" stays
//! answerable. Churn is a **derivation** — per-path touch counts over a window — and it is
//! recomputed rather than stored, because `commits` has no path column and adding a
//! file-touch index would be a second source of truth about history.

use nexus_store::CommitRecord;
use nexus_vcs::{CommitInfo, Repo, HISTORY_WINDOW_COMMITS};
use std::collections::HashMap;

pub fn to_record(c: CommitInfo) -> CommitRecord {
    CommitRecord {
        sha: c.sha,
        parent_shas: c.parent_shas,
        author: c.author,
        authored_at: c.authored_at,
        subject: c.subject,
    }
}

/// Per-path churn in 0.0..=1.0, normalised against the busiest path in the window.
///
/// `log1p(n) / log1p(max)` rather than `n / max`: the difference between one touch and five
/// says much more than the difference between forty and forty-five, and a linear scale lets
/// one pathologically hot file flatten every other candidate to nearly zero.
pub fn churn(repo: Option<&Repo>) -> HashMap<String, f64> {
    let Some(repo) = repo else {
        return HashMap::new();
    };
    let Ok(counts) = repo.touch_counts(HISTORY_WINDOW_COMMITS) else {
        return HashMap::new();
    };
    let Some(max) = counts.values().copied().max() else {
        return HashMap::new();
    };
    if max == 0 {
        return HashMap::new();
    }
    let denominator = (max as f64 + 1.0).ln();
    counts
        .into_iter()
        .map(|(path, n)| (path, (n as f64 + 1.0).ln() / denominator))
        .collect()
}
