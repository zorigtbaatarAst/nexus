//! Git access for BugHunter.
//!
//! Deliberately small: HEAD, dirty state, and the changed-path set between two revisions.
//! It knows nothing about languages, symbols or storage.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use git2::{Delta, DiffOptions, Repository, StatusOptions};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("baseline commit {0} is unreachable (force-push, rebase, or shallow clone)")]
    Unreachable(String),
}

pub type Result<T> = std::result::Result<T, VcsError>;

/// What changed between two revisions, at path granularity.
#[derive(Debug, Default, Clone)]
pub struct PathDiff {
    pub changed: BTreeSet<String>,
    pub deleted: BTreeSet<String>,
}

pub struct Repo {
    inner: Repository,
}

impl Repo {
    /// `None` when the directory is not a git repository — a supported configuration,
    /// not an error. Change detection then falls back to a full walk.
    pub fn discover(root: &Path) -> Option<Self> {
        Repository::discover(root).ok().map(|inner| Repo { inner })
    }

    pub fn head_sha(&self) -> Result<Option<String>> {
        match self.inner.head() {
            Ok(h) => Ok(h.target().map(|o| o.to_string())),
            // An empty repository has no HEAD yet. That is not a failure.
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn short_sha(sha: &str) -> &str {
        &sha[..7.min(sha.len())]
    }

    /// Any tracked modification or untracked file makes the tree dirty. A dirty tree means
    /// the commit sha alone cannot identify the working state, so Tier 0 must not short-circuit.
    pub fn is_dirty(&self) -> Result<bool> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .include_ignored(false)
            .include_unmodified(false);
        Ok(!self.inner.statuses(Some(&mut opts))?.is_empty())
    }

    pub fn is_reachable(&self, sha: &str) -> bool {
        self.inner
            .revparse_single(sha)
            .ok()
            .and_then(|o| o.peel_to_commit().ok())
            .is_some()
    }

    /// Paths differing between `from` and HEAD, plus anything dirty in the working tree.
    pub fn changed_paths_since(&self, from: &str) -> Result<PathDiff> {
        if !self.is_reachable(from) {
            return Err(VcsError::Unreachable(from.to_string()));
        }
        let mut diff = PathDiff::default();

        let old_tree = self.inner.revparse_single(from)?.peel_to_commit()?.tree()?;
        let mut opts = DiffOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true);

        // Diffing against the working directory rather than HEAD picks up committed and
        // uncommitted changes in one pass, which is what a rescan actually needs.
        let d = self
            .inner
            .diff_tree_to_workdir_with_index(Some(&old_tree), Some(&mut opts))?;
        d.foreach(
            &mut |delta, _| {
                let old = delta
                    .old_file()
                    .path()
                    .and_then(|p| p.to_str())
                    .map(str::to_string);
                let new = delta
                    .new_file()
                    .path()
                    .and_then(|p| p.to_str())
                    .map(str::to_string);
                match delta.status() {
                    Delta::Deleted => {
                        if let Some(p) = old {
                            diff.deleted.insert(p);
                        }
                    }
                    Delta::Renamed | Delta::Copied => {
                        if let Some(p) = old {
                            diff.deleted.insert(p);
                        }
                        if let Some(p) = new {
                            diff.changed.insert(p);
                        }
                    }
                    _ => {
                        if let Some(p) = new.or(old) {
                            diff.changed.insert(p);
                        }
                    }
                }
                true
            },
            None,
            None,
            None,
        )?;
        Ok(diff)
    }

    pub fn commit_subject(&self, sha: &str) -> Option<String> {
        self.inner
            .revparse_single(sha)
            .ok()?
            .peel_to_commit()
            .ok()?
            .summary()
            .map(str::to_string)
    }

    /// How far HEAD has moved past a revision — the "47 commits behind" line in `doctor`.
    pub fn commits_since(&self, from: &str) -> Option<usize> {
        let head = self.inner.head().ok()?.target()?;
        let base = self
            .inner
            .revparse_single(from)
            .ok()?
            .peel_to_commit()
            .ok()?
            .id();
        let mut walk = self.inner.revwalk().ok()?;
        walk.push(head).ok()?;
        walk.hide(base).ok()?;
        Some(walk.count())
    }
}
