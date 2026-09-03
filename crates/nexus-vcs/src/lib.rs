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
    #[error("could not prepare a worktree for {sha}: {detail}")]
    Worktree { sha: String, detail: String },
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

    /// Paths with uncommitted changes, sorted. The cheap half of `is_dirty`: which files, not
    /// what changed in them.
    pub fn dirty_paths(&self) -> Result<Vec<String>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        let mut out: Vec<String> = self
            .inner
            .statuses(Some(&mut opts))?
            .iter()
            .filter_map(|e| e.path().map(str::to_string))
            .collect();
        out.sort();
        Ok(out)
    }

    /// Check `sha` out into `dir` as a detached worktree, reusing one that is already there.
    ///
    /// **`git stash` is never used, anywhere.** A verifier that mutates the developer's
    /// working tree can lose uncommitted work, and one that loses uncommitted work is
    /// uninstalled the first time it does — rightly. A worktree touches nothing the developer
    /// is holding.
    ///
    /// Reused per sha, because a baseline is a property of a commit and computing it twice
    /// costs a full build for no new information.
    pub fn detached_worktree(&self, sha: &str, dir: &Path) -> Result<bool> {
        if dir.join(".git").exists() {
            return Ok(false); // already there, and a commit's contents do not change
        }
        if !self.is_reachable(sha) {
            return Err(VcsError::Unreachable(sha.to_string()));
        }
        let workdir = self.inner.workdir().ok_or_else(|| VcsError::Worktree {
            sha: sha.to_string(),
            detail: "this is a bare repository, so it has no worktree".into(),
        })?;

        // Drop stale registrations first. A cache directory that was deleted — by a cleanup,
        // by a person, by `rm -rf` on a scratch area — leaves git still holding the
        // registration, and without this the baseline for that sha can never be built again.
        let _ = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(workdir)
            .output();

        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let out = std::process::Command::new("git")
            .args(["worktree", "add", "--detach", "--quiet"])
            .arg(dir)
            .arg(sha)
            .current_dir(workdir)
            .output()
            .map_err(|e| VcsError::Worktree {
                sha: sha.to_string(),
                detail: e.to_string(),
            })?;
        if !out.status.success() {
            return Err(VcsError::Worktree {
                sha: sha.to_string(),
                detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(true)
    }

    /// Remove a worktree created by [`Self::detached_worktree`]. Best effort: a leftover
    /// directory under the cache is untidy, and failing a verification over it would be worse.
    pub fn remove_worktree(&self, dir: &Path) {
        let Some(workdir) = self.inner.workdir() else {
            return;
        };
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(dir)
            .current_dir(workdir)
            .output();
        let _ = std::fs::remove_dir_all(dir);
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

/// One commit, as the `commits` ledger records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub parent_shas: String,
    pub author: Option<String>,
    /// ISO 8601, UTC. Stored as text because that is what every other timestamp here is,
    /// and because a sortable string needs no timezone library to compare.
    pub authored_at: String,
    pub subject: Option<String>,
}

/// How many commits back history questions look.
///
/// A cap rather than the whole history: churn over a five-year window answers a question
/// nobody asked, and an unbounded revwalk on a large repository is the one thing that could
/// put this stage over ADR-024's 150 ms budget.
pub const HISTORY_WINDOW_COMMITS: usize = 500;

impl Repo {
    /// The most recent commits reachable from HEAD, newest first.
    ///
    /// One revwalk. An empty or unborn repository yields an empty vector rather than an
    /// error: a project with no commits is a supported configuration, and history questions
    /// about it have the honest answer "none".
    pub fn recent_commits(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let Some(head) = self.head_sha()? else {
            return Ok(Vec::new());
        };
        let head = match self.inner.revparse_single(&head) {
            Ok(o) => o.id(),
            Err(_) => return Ok(Vec::new()),
        };
        let mut walk = self.inner.revwalk()?;
        walk.push(head)?;
        let mut out = Vec::new();
        for oid in walk.take(limit) {
            let Ok(oid) = oid else { continue };
            let Ok(commit) = self.inner.find_commit(oid) else {
                continue;
            };
            out.push(CommitInfo {
                sha: oid.to_string(),
                parent_shas: commit
                    .parent_ids()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                author: commit.author().name().map(str::to_string),
                authored_at: format_iso8601(commit.time().seconds()),
                subject: commit.summary().map(str::to_string),
            });
        }
        Ok(out)
    }

    /// How many of the last `limit` commits touched each path.
    ///
    /// One revwalk, one diff per commit against its first parent. This is the raw material
    /// for the churn signal, and it is computed here rather than stored because the `commits`
    /// table has no path column — it is a commit ledger, not a file-touch index, and adding
    /// a second source of truth about history to avoid one traversal is a bad trade.
    ///
    /// A merge is diffed against its first parent only. Counting a merge's whole second side
    /// would attribute every commit in a long-lived branch to the day it landed, which makes
    /// churn a measure of merge strategy rather than of change.
    pub fn touch_counts(&self, limit: usize) -> Result<std::collections::HashMap<String, usize>> {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let Some(head) = self.head_sha()? else {
            return Ok(counts);
        };
        let head = match self.inner.revparse_single(&head) {
            Ok(o) => o.id(),
            Err(_) => return Ok(counts),
        };
        let mut walk = self.inner.revwalk()?;
        walk.push(head)?;
        for oid in walk.take(limit) {
            let Ok(oid) = oid else { continue };
            let Ok(commit) = self.inner.find_commit(oid) else {
                continue;
            };
            let Ok(tree) = commit.tree() else { continue };
            let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let mut opts = DiffOptions::new();
            let Ok(diff) =
                self.inner
                    .diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut opts))
            else {
                continue;
            };
            for delta in diff.deltas() {
                for file in [delta.new_file().path(), delta.old_file().path()]
                    .into_iter()
                    .flatten()
                {
                    if let Some(p) = file.to_str() {
                        *counts.entry(p.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        Ok(counts)
    }
}

/// Seconds since the epoch to `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled for the same reason the store's own formatter is: a date library is a large
/// dependency to buy for one format string, and this one is total — it cannot panic and has
/// no locale.
fn format_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days`, the standard branch-free conversion.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A repository with one commit, built with git2 directly.
    fn repo_with_one_commit() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().to_path_buf();
        let repo = git2::Repository::init(&path).expect("init");
        std::fs::write(path.join("a.txt"), "one\n").expect("write");

        let mut index = repo.index().expect("index");
        index.add_path(Path::new("a.txt")).expect("add");
        index.write().expect("write index");
        let tree = repo
            .find_tree(index.write_tree().expect("tree"))
            .expect("find");
        let sig =
            git2::Signature::new("T", "t@example.invalid", &git2::Time::new(1_700_000_000, 0))
                .expect("sig");
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "one", &tree, &[])
            .expect("commit");
        (dir, path, oid.to_string())
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_none_not_an_error() {
        let dir = tempfile::tempdir().expect("tmp");
        assert!(
            Repo::discover(dir.path()).is_none(),
            "a project without git is a supported configuration, not a failure"
        );
    }

    #[test]
    fn head_is_the_commit_that_was_just_made() {
        let (_d, path, sha) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        assert_eq!(repo.head_sha().expect("head"), Some(sha));
    }

    #[test]
    fn an_empty_repository_has_no_head_and_that_is_not_a_failure() {
        let dir = tempfile::tempdir().expect("tmp");
        git2::Repository::init(dir.path()).expect("init");
        let repo = Repo::discover(dir.path()).expect("discovered");
        assert_eq!(
            repo.head_sha().expect("an unborn branch is not an error"),
            None
        );
    }

    #[test]
    fn an_untracked_file_makes_the_tree_dirty() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        assert!(!repo.is_dirty().expect("clean"), "nothing has changed yet");

        // Untracked, not modified: the case a status check that only looks at tracked files
        // gets wrong, and the common one — a new source file.
        std::fs::write(path.join("new.txt"), "x\n").expect("write");
        assert!(
            repo.is_dirty().expect("dirty"),
            "an untracked file means the commit sha alone cannot identify the working state"
        );
    }

    #[test]
    fn a_modified_tracked_file_makes_the_tree_dirty() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        std::fs::write(path.join("a.txt"), "two\n").expect("write");
        assert!(repo.is_dirty().expect("dirty"));
    }

    #[test]
    fn short_sha_is_seven_characters_and_survives_a_shorter_input() {
        let (_d, _p, sha) = repo_with_one_commit();
        assert_eq!(Repo::short_sha(&sha).len(), 7);
        // A truncated sha must not panic on the slice. This is the guard, not a formality.
        assert_eq!(Repo::short_sha("abc"), "abc");
        assert_eq!(Repo::short_sha(""), "");
    }

    #[test]
    fn an_unreachable_baseline_is_an_error_that_names_itself() {
        let (_d, path, _) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        let missing = "0".repeat(40);
        assert!(!repo.is_reachable(&missing));

        let err = repo
            .changed_paths_since(&missing)
            .expect_err("a force-push or a shallow clone must be reported, not guessed around");
        assert!(
            matches!(err, VcsError::Unreachable(_)),
            "the caller has to tell this apart from a git failure: {err}"
        );
    }

    #[test]
    fn changed_paths_reports_a_new_file_and_a_deletion() {
        let (_d, path, base) = repo_with_one_commit();
        let repo = Repo::discover(&path).expect("discovered");
        std::fs::write(path.join("b.txt"), "two\n").expect("write");
        std::fs::remove_file(path.join("a.txt")).expect("remove");

        let diff = repo.changed_paths_since(&base).expect("diff");
        assert!(
            diff.changed.contains("b.txt"),
            "an added file is a change: {diff:?}"
        );
        // A deletion lands in `deleted` and nowhere else. The distinction matters: `changed`
        // drives re-parsing, and a path queued for re-parse that is no longer on disk is an
        // error the scan has to handle instead of a file it can read.
        assert!(
            diff.deleted.contains("a.txt"),
            "a deleted file belongs in `deleted`: {diff:?}"
        );
        assert!(
            !diff.changed.contains("a.txt"),
            "and must not also be queued for re-parsing: {diff:?}"
        );
    }
}
