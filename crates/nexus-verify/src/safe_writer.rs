//! The write jail (AGENTS.md rule 5).
//!
//! Nexus writes generated files into a developer's project. That is the single most dangerous
//! thing it does, and T4 in the threat model is exactly it: *Nexus modifies production code*.
//! Every write goes through here, rooted at `.nexus/generated-tests/`.
//!
//! # The rule that makes it a jail
//!
//! **Canonicalize the parent before the prefix check.** A jail that compares unresolved paths
//! is not a jail: `root/../../etc/passwd` has the root as a textual prefix and resolves
//! somewhere else entirely, and a symlink inside the root points wherever it likes. The parent
//! is resolved on the filesystem first, and the resolved path is what is checked.
//!
//! The parent, not the file: the file does not exist yet, so it cannot be canonicalized. Its
//! directory can, and a file cannot escape a directory that is inside the root.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JailError {
    #[error("{0} escapes the generated-tests root")]
    Escapes(String),
    #[error("{0} is absolute; generated paths are relative to the root")]
    Absolute(String),
    #[error("could not prepare {path}: {detail}")]
    Io { path: String, detail: String },
}

/// A writer that can only write beneath one directory.
pub struct SafeWriter {
    root: PathBuf,
}

impl SafeWriter {
    /// Root at `<project>/.nexus/generated-tests`, creating it.
    pub fn at(project_root: &Path) -> Result<Self, JailError> {
        let root = project_root.join(".nexus").join("generated-tests");
        std::fs::create_dir_all(&root).map_err(|e| JailError::Io {
            path: root.display().to_string(),
            detail: e.to_string(),
        })?;
        let root = root.canonicalize().map_err(|e| JailError::Io {
            path: root.display().to_string(),
            detail: e.to_string(),
        })?;
        Ok(SafeWriter { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a relative path inside the jail, or refuse.
    ///
    /// Refuses before touching the filesystem where it can — an absolute path or a `..`
    /// component is rejected on sight, so a traversal never even creates a directory on the
    /// way to being caught.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, JailError> {
        let candidate = Path::new(relative);
        if candidate.is_absolute() {
            return Err(JailError::Absolute(relative.to_string()));
        }
        if candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(JailError::Escapes(relative.to_string()));
        }

        let target = self.root.join(candidate);
        let parent = target.parent().unwrap_or(&self.root).to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|e| JailError::Io {
            path: parent.display().to_string(),
            detail: e.to_string(),
        })?;

        // The check that matters. The parent is resolved on the filesystem — following every
        // symlink — and *then* compared. A textual prefix check would accept a symlink inside
        // the root that points at /etc.
        let resolved = parent.canonicalize().map_err(|e| JailError::Io {
            path: parent.display().to_string(),
            detail: e.to_string(),
        })?;
        if !resolved.starts_with(&self.root) {
            return Err(JailError::Escapes(relative.to_string()));
        }
        Ok(resolved.join(target.file_name().unwrap_or_default()))
    }

    /// Write a file inside the jail.
    pub fn write(&self, relative: &str, contents: &str) -> Result<PathBuf, JailError> {
        let path = self.resolve(relative)?;
        std::fs::write(&path, contents).map_err(|e| JailError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jail(name: &str) -> (PathBuf, SafeWriter) {
        let root = std::env::temp_dir().join(format!("nexus-jail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let w = SafeWriter::at(&root).expect("jail");
        (root, w)
    }

    #[test]
    fn an_ordinary_relative_path_is_written_inside_the_root() {
        let (root, w) = jail("ok");
        let p = w
            .write("java/PaymentTest.java", "class X {}")
            .expect("write");
        assert!(p.starts_with(w.root()), "{p:?}");
        assert_eq!(std::fs::read_to_string(&p).expect("read"), "class X {}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_traversal_is_refused_before_it_touches_anything() {
        let (root, w) = jail("traversal");
        for bad in [
            "../escaped.java",
            "a/../../escaped.java",
            "../../../etc/passwd",
        ] {
            assert_eq!(
                w.resolve(bad),
                Err(JailError::Escapes(bad.to_string())),
                "{bad} was allowed"
            );
        }
        assert!(
            !root.join("escaped.java").exists(),
            "a refused write must leave nothing behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let (root, w) = jail("absolute");
        assert!(matches!(
            w.resolve("/etc/passwd"),
            Err(JailError::Absolute(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_out_of_the_root_is_refused_because_the_parent_is_resolved_first() {
        // The whole reason the check canonicalizes. A textual prefix comparison accepts this:
        // the path starts with the root, and resolves somewhere else entirely.
        let (root, w) = jail("symlink");
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::os::unix::fs::symlink(&outside, w.root().join("escape")).expect("symlink");

        assert_eq!(
            w.resolve("escape/evil.java"),
            Err(JailError::Escapes("escape/evil.java".to_string()))
        );
        assert!(
            !outside.join("evil.java").exists(),
            "nothing was written outside the root"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nested_directory_inside_the_root_is_allowed_and_created() {
        let (root, w) = jail("nested");
        let p = w.write("a/b/c/T.java", "x").expect("write");
        assert!(p.starts_with(w.root()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
