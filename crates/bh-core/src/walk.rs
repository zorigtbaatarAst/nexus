//! Filesystem traversal and content hashing.
//!
//! The stat fast path is what makes a full walk survivable: on a large repository it
//! eliminates hashing for the ~99% of files that did not change, and hashing is the only
//! I/O-bound step in the pipeline.

use ignore::WalkBuilder;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct WalkedFile {
    pub path: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
}

#[derive(Debug, Clone)]
pub struct HashedFile {
    pub path: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub content_hash: String,
    pub loc: u32,
}

/// Paths BugHunter must never index, whatever the source of the candidate.
///
/// The walker excludes these structurally, but Tier 1 candidates can also come from
/// `git diff`, which knows nothing about the walker's filter — so the rule has to live
/// somewhere both paths consult, or BugHunter ends up indexing its own state.
pub fn is_excluded(path: &str) -> bool {
    path == ".bughunter"
        || path.starts_with(".bughunter/")
        || path == ".git"
        || path.starts_with(".git/")
}

/// Ignore-aware traversal. `.gitignore`, `.ignore` and `.bughunterignore` are honoured, and
/// `.bughunter/` is always excluded so BugHunter never indexes its own state.
pub fn walk(root: &Path, extra_excludes: &[String]) -> Vec<WalkedFile> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .add_custom_ignore_filename(".bughunterignore")
        .filter_entry(|e| e.file_name() != ".git" && e.file_name() != ".bughunter");

    let mut out = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let path = rel.to_string_lossy().replace('\\', "/");
        if extra_excludes.iter().any(|e| path.starts_with(e.as_str())) {
            continue;
        }
        let (size_bytes, mtime_ns) = match entry.metadata() {
            Ok(m) => (m.len(), mtime_ns(&m)),
            Err(_) => (0, 0),
        };
        out.push(WalkedFile {
            path,
            size_bytes,
            mtime_ns,
        });
    }
    out
}

fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Hash a set of files in parallel. Unreadable files are dropped from the result rather
/// than failing the walk — a scan that aborts because one file is unreadable is useless.
pub fn hash_all(root: &Path, files: &[WalkedFile]) -> Vec<HashedFile> {
    files
        .par_iter()
        .filter_map(|f| {
            let bytes = std::fs::read(root.join(&f.path)).ok()?;
            Some(HashedFile {
                path: f.path.clone(),
                size_bytes: f.size_bytes,
                mtime_ns: f.mtime_ns,
                content_hash: hash_bytes(&bytes),
                loc: bytes.iter().filter(|b| **b == b'\n').count() as u32 + 1,
            })
        })
        .collect()
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..32].to_string()
}

/// Merkle root over sorted `(path, content_hash)`.
///
/// A commit sha says nothing about a dirty tree; this does. It is the single value that
/// answers "is anything at all different" for the baseline.
pub fn working_tree_hash(files: &BTreeMap<String, String>) -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, content) in files {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_tree_hash_is_order_independent_and_content_sensitive() {
        let mut a = BTreeMap::new();
        a.insert("b.java".to_string(), "h2".to_string());
        a.insert("a.java".to_string(), "h1".to_string());

        let mut b = BTreeMap::new();
        b.insert("a.java".to_string(), "h1".to_string());
        b.insert("b.java".to_string(), "h2".to_string());
        assert_eq!(working_tree_hash(&a), working_tree_hash(&b));

        b.insert("a.java".to_string(), "h9".to_string());
        assert_ne!(working_tree_hash(&a), working_tree_hash(&b));
    }

    #[test]
    fn a_renamed_file_changes_the_tree_hash() {
        let mut a = BTreeMap::new();
        a.insert("old.java".to_string(), "h1".to_string());
        let mut b = BTreeMap::new();
        b.insert("new.java".to_string(), "h1".to_string());
        assert_ne!(working_tree_hash(&a), working_tree_hash(&b));
    }
}
