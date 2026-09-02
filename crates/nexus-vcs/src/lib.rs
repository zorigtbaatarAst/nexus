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
