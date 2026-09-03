//! §11 — the package cache.
//!
//! Keyed on `(intent, seeds, HEAD sha, dirty hash, budget, weights hash)`. Every component
//! matters, but the dirty hash is the one that earns its keep: an agent editing files without
//! committing is the normal case, and a cache keyed only on HEAD would serve context
//! describing code that no longer exists — risk R9, and the kind of wrong answer that is
//! worse than no answer because it looks authoritative.
//!
//! **A cache failure is a miss, never an error.** Unreadable file, bad JSON, missing
//! directory: all mean "compute it". A cache that can fail a request has made the request
//! less reliable than it was without one, which is a strange thing to ship for a speedup.

use super::ContextPackage;
use std::path::{Path, PathBuf};

/// Everything that makes two requests the same question.
pub struct Key<'a> {
    pub intent: &'a str,
    pub seeds: Vec<String>,
    pub commit: Option<&'a str>,
    pub dirty_hash: &'a str,
    pub budget_tokens: usize,
    pub weights_hash: &'a str,
    /// A package with its reasoning is a different package. Without this the first plain
    /// request poisoned every later `--explain` with a ledger-less hit.
    pub explain: bool,
    /// What the project remembers. §11 lists the index and the tree; it does not list
    /// memory, and the omission meant recording a fact changed nothing until something else
    /// moved — the opposite of "an expensive conclusion should be reached once".
    pub memory: &'a str,
}

impl Key<'_> {
    pub fn digest(&self) -> String {
        let mut seeds = self.seeds.clone();
        seeds.sort();
        let material = format!(
            // The build is part of the key. A cached package outliving the upgrade that
            // changed how packages are built is a stale answer with no way to notice.
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            env!("CARGO_PKG_VERSION"),
            self.intent,
            seeds.join(","),
            self.commit.unwrap_or("-"),
            self.dirty_hash,
            self.budget_tokens,
            self.weights_hash,
            self.explain,
            self.memory
        );
        blake3::hash(material.as_bytes()).to_hex()[..32].to_string()
    }
}

fn path_for(cache_dir: &Path, digest: &str) -> PathBuf {
    cache_dir.join("context").join(format!("{digest}.json"))
}

/// A previously computed package, or `None` for any reason at all.
pub fn get(cache_dir: &Path, key: &Key<'_>) -> Option<ContextPackage> {
    let raw = std::fs::read_to_string(path_for(cache_dir, &key.digest())).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Store a package. Failure is ignored on purpose: a read-only or full disk must not turn a
/// successful request into a failed one.
pub fn put(cache_dir: &Path, key: &Key<'_>, package: &ContextPackage) {
    let path = path_for(cache_dir, &key.digest());
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(body) = serde_json::to_string(package) {
        let _ = std::fs::write(path, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key<'a>(commit: Option<&'a str>, dirty: &'a str, budget: usize) -> Key<'a> {
        Key {
            intent: "debug",
            seeds: vec!["mn.pay.A".into(), "mn.pay.B".into()],
            commit,
            dirty_hash: dirty,
            budget_tokens: budget,
            weights_hash: "w1",
            explain: false,
            memory: "m1",
        }
    }

    #[test]
    fn the_same_question_has_the_same_digest_regardless_of_seed_order() {
        let a = key(Some("abc"), "clean", 4000);
        let mut b = key(Some("abc"), "clean", 4000);
        b.seeds.reverse();
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn every_component_of_the_key_changes_the_digest() {
        let base = key(Some("abc"), "clean", 4000).digest();
        assert_ne!(base, key(Some("def"), "clean", 4000).digest(), "commit");
        assert_ne!(base, key(Some("abc"), "dirty", 4000).digest(), "dirty hash");
        assert_ne!(base, key(Some("abc"), "clean", 800).digest(), "budget");
        let mut k = key(Some("abc"), "clean", 4000);
        k.intent = "refactor";
        assert_ne!(base, k.digest(), "intent");
        let mut k = key(Some("abc"), "clean", 4000);
        k.weights_hash = "w2";
        assert_ne!(base, k.digest(), "weights");
        let mut k = key(Some("abc"), "clean", 4000);
        k.explain = true;
        assert_ne!(base, k.digest(), "explain");
        let mut k = key(Some("abc"), "clean", 4000);
        k.memory = "m2";
        assert_ne!(base, k.digest(), "memory");
        let mut k = key(Some("abc"), "clean", 4000);
        k.seeds.push("mn.pay.C".into());
        assert_ne!(base, k.digest(), "seeds");
    }

    #[test]
    fn a_missing_or_corrupt_entry_is_a_miss_and_not_an_error() {
        let dir = std::env::temp_dir().join(format!("nexus-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let k = key(Some("abc"), "clean", 4000);
        assert!(get(&dir, &k).is_none(), "nothing stored yet");

        std::fs::create_dir_all(dir.join("context")).expect("mkdir");
        std::fs::write(
            dir.join("context").join(format!("{}.json", k.digest())),
            "{ not json",
        )
        .expect("write");
        assert!(
            get(&dir, &k).is_none(),
            "a corrupt entry must be a miss, never a failed request"
        );
    }
}
