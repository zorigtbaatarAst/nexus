//! The generator: specification in, git repository out, with the same shas every time.
//!
//! ## Determinism
//!
//! A commit's sha is a hash of its tree, its parents, its message and its two signatures.
//! Every one of those is pinned:
//!
//! * **tree** — file bytes come from `blobs/`, normalised to `\n`; git sorts tree entries, so
//!   the order the operations ran in cannot leak into the object;
//! * **parents** — the history is declared, not discovered;
//! * **message** — from the specification;
//! * **signatures** — author and committer are the same fixed identity, and the timestamp is
//!   `base_epoch + n * commit_interval_s` with a zero offset. **No clock is ever read.**
//!
//! What remains is the environment, and the two parts of it that can move are pinned too:
//! `init.defaultBranch` is overridden with an explicit `initial_head`, and `core.autocrlf` is
//! forced off so a checkout on a machine configured for CRLF cannot change a blob.
//!
//! Executable files are not supported. Recording a mode bit that some filesystems do not
//! carry would make the tree — and therefore every sha after it — depend on where generation
//! ran, which is exactly the property this module exists to remove.

use crate::manifest::{BranchRecord, CommitRecord, Manifest, PatchRecord, ResolvedTask};
use crate::spec::{Op, Select, Spec, TransformKind};
use git2::{IndexAddOption, Repository, RepositoryInitOptions, Signature, Time};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not render task {task}: {source}")]
    Toml {
        task: String,
        #[source]
        source: Box<toml::ser::Error>,
    },
    #[error("{fixture}: commit `{commit}`: {message}")]
    Commit {
        fixture: String,
        commit: String,
        message: String,
    },
    #[error("{fixture}: patch `{patch}` does not apply at `{base}` ({base_sha}): {source}")]
    PatchDoesNotApply {
        fixture: String,
        patch: String,
        base: String,
        base_sha: String,
        #[source]
        source: git2::Error,
    },
    #[error(
        "{path} already exists and is not empty.\n  \
         pass --force to replace it, or --out to write somewhere else"
    )]
    OutputNotEmpty { path: String },
    #[error(
        "refusing to generate into {path}: it looks like a source tree ({why}).\n  \
         generated fixtures must be isolated from the working tree"
    )]
    RefusingToClobber { path: String, why: String },
    #[error("{fixture}: the first commit `{commit}` must be on the default branch `{branch}`")]
    FirstCommitOffDefault {
        fixture: String,
        commit: String,
        branch: String,
    },
    #[error("{fixture}: unsafe path `{path}` — fixture paths are relative and stay inside the repository")]
    UnsafePath { fixture: String, path: String },
}

type Result<T> = std::result::Result<T, GenError>;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Replace a non-empty output directory.
    pub force: bool,
    /// Also write one task file per task, in the shape `13-evaluation.md` §3 pins.
    pub emit_tasks: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Generated {
    pub name: String,
    pub repo: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
}

/// Generate one fixture into `out_root`.
///
/// Layout, and why nothing but content lives in the repository:
///
/// ```text
/// <out_root>/<name>/                the git repository — fixture content only
/// <out_root>/<name>.manifest.json   logical ids resolved to shas
/// <out_root>/<name>.patches/        dirty-start patches
/// ```
///
/// A manifest committed *inside* the repository would be indexed by the scan the fixture
/// exists to exercise, and would then appear in every impact answer measured against it.
pub fn generate(spec: &Spec, out_root: &Path, opts: &Options) -> Result<Generated> {
    let name = spec.name().to_string();
    let repo_path = out_root.join(&name);
    let patches_dir = out_root.join(format!("{name}.patches"));
    let manifest_path = out_root.join(format!("{name}.manifest.json"));

    guard_output(&repo_path)?;
    reset_dir(&repo_path, opts.force)?;
    reset_dir(&patches_dir, true)?;

    let m = &spec.manifest;
    let repo = init_repo(&repo_path, &m.fixture.default_branch)?;

    let mut commits: Vec<CommitRecord> = Vec::new();
    let mut current_branch = m.fixture.default_branch.clone();

    for (n, c) in m.commit.iter().enumerate() {
        let target = c.branch.clone().unwrap_or_else(|| current_branch.clone());
        if n == 0 && target != m.fixture.default_branch {
            return Err(GenError::FirstCommitOffDefault {
                fixture: name.clone(),
                commit: c.id.clone(),
                branch: m.fixture.default_branch.clone(),
            });
        }
        if target != current_branch {
            switch_branch(&repo, spec, &target, &commits)?;
            current_branch = target.clone();
        }

        for op in c.ops() {
            apply_op(spec, &repo_path, &c.id, &op)?;
        }

        let timestamp = m.fixture.base_epoch + (n as i64) * m.fixture.commit_interval_s;
        let sha = write_commit(&repo, spec, &c.message, timestamp)?;

        commits.push(CommitRecord {
            id: c.id.clone(),
            sha,
            branch: current_branch.clone(),
            message: c.message.clone(),
            timestamp,
            files: tracked_files(&repo_path)?,
            plants_bug: c.plants_bug.clone(),
            expect: c.expect.clone(),
        });
    }

    // Leave the fixture on its default branch: a runner that clones or copies it should get
    // the mainline without having to know which branch the last commit happened to be on.
    if current_branch != m.fixture.default_branch {
        switch_branch(&repo, spec, &m.fixture.default_branch, &commits)?;
    }

    let branches = m
        .branch
        .iter()
        .map(|b| BranchRecord {
            name: b.name.clone(),
            from: b.from.clone(),
            head: commits
                .iter()
                .rev()
                .find(|c| c.branch == b.name)
                .map(|c| c.sha.clone())
                .unwrap_or_default(),
        })
        .collect();

    let patches = write_patches(spec, &repo, &commits, &patches_dir)?;

    let manifest = Manifest {
        name: name.clone(),
        description: m.fixture.description.clone(),
        role: m.fixture.role.clone(),
        stack: m.fixture.stack.clone(),
        generator_version: env!("CARGO_PKG_VERSION").to_string(),
        spec_digest: digest_spec(spec)?,
        default_branch: m.fixture.default_branch.clone(),
        tasks: resolve_tasks(spec, &commits),
        deprecated_paths: m.deprecated_path.clone(),
        commits,
        branches,
        patches,
    };

    write_file(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)?.as_bytes(),
    )?;
    if let Some(dir) = &opts.emit_tasks {
        emit_task_files(&manifest, dir)?;
    }

    Ok(Generated {
        name,
        repo: repo_path,
        manifest_path,
        manifest,
    })
}

// --- repository -------------------------------------------------------------------------

fn init_repo(path: &Path, default_branch: &str) -> Result<Repository> {
    let mut o = RepositoryInitOptions::new();
    // `initial_head` because `init.defaultBranch` is a per-machine setting, and a fixture
    // whose branch name depends on whose laptop built it is not reproducible.
    o.initial_head(default_branch);
    // No template directory: a user's global hooks and excludes must not reach a fixture.
    o.external_template(false);
    let repo = Repository::init_opts(path, &o)?;
    {
        let mut cfg = repo.config()?;
        // A machine configured for CRLF would otherwise rewrite blobs on checkout and change
        // every sha downstream of the first text file.
        cfg.set_bool("core.autocrlf", false)?;
        cfg.set_bool("core.ignorecase", false)?;
    }
    Ok(repo)
}

fn switch_branch(
    repo: &Repository,
    spec: &Spec,
    target: &str,
    done: &[CommitRecord],
) -> Result<()> {
    let refname = format!("refs/heads/{target}");
    if repo.find_reference(&refname).is_err() {
        let from_id = spec
            .manifest
            .branch
            .iter()
            .find(|b| b.name == target)
            .map(|b| b.from.clone())
            .unwrap_or_default();
        let sha = done
            .iter()
            .find(|c| c.id == from_id)
            .map(|c| c.sha.clone())
            .ok_or_else(|| GenError::Commit {
                fixture: spec.name().into(),
                commit: from_id.clone(),
                message: format!(
                    "branch `{target}` forks from a commit that has not been created yet"
                ),
            })?;
        let oid = git2::Oid::from_str(&sha)?;
        let commit = repo.find_commit(oid)?;
        repo.branch(target, &commit, false)?;
    }
    repo.set_head(&refname)?;
    let mut co = git2::build::CheckoutBuilder::new();
    // `remove_untracked` because a file created on another branch would otherwise survive
    // the switch, get picked up by the next `add_all`, and appear in a commit nothing wrote.
    co.force().remove_untracked(true);
    repo.checkout_head(Some(&mut co))?;
    Ok(())
}

fn write_commit(repo: &Repository, spec: &Spec, message: &str, timestamp: i64) -> Result<String> {
    let mut index = repo.index()?;
    // Rebuild rather than update: `clear` + `add_all` makes the index a pure function of the
    // working tree, so a delete needs no separate bookkeeping and cannot be forgotten.
    index.clear()?;
    index.add_all(["."].iter(), IndexAddOption::DEFAULT, None)?;
    index.write()?;
    let tree = repo.find_tree(index.write_tree()?)?;

    let a = &spec.manifest.author;
    // Offset zero: a signature carries its timezone, so a non-zero offset would make the sha
    // depend on where the generator ran.
    let sig = Signature::new(&a.name, &a.email, &Time::new(timestamp, 0))?;

    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
    Ok(oid.to_string())
}

// --- operations -------------------------------------------------------------------------

fn apply_op(spec: &Spec, root: &Path, commit: &str, op: &Op) -> Result<()> {
    let fail = |message: String| GenError::Commit {
        fixture: spec.name().into(),
        commit: commit.into(),
        message,
    };
    match op {
        Op::Write {
            path,
            blob,
            content,
        } => {
            let target = safe_join(spec, root, path)?;
            let bytes = match (blob, content) {
                (Some(b), _) => normalize(&read_file(&spec.blobs_dir().join(b))?),
                (None, Some(c)) => normalize(c.as_bytes()),
                (None, None) => return Err(fail(format!("`{path}` has no content"))),
            };
            write_file(&target, &bytes)
        }
        Op::Delete { path } => {
            let target = safe_join(spec, root, path)?;
            if !target.is_file() {
                return Err(fail(format!("cannot delete `{path}`: it is not there")));
            }
            std::fs::remove_file(&target).map_err(|source| GenError::Io {
                path: target.display().to_string(),
                source,
            })
        }
        Op::Move { from, to } => {
            let src = safe_join(spec, root, from)?;
            let dst = safe_join(spec, root, to)?;
            if !src.is_file() {
                return Err(fail(format!("cannot move `{from}`: it is not there")));
            }
            if let Some(p) = dst.parent() {
                create_dir_all(p)?;
            }
            std::fs::rename(&src, &dst).map_err(|source| GenError::Io {
                path: src.display().to_string(),
                source,
            })
        }
        Op::Substitute {
            select,
            find,
            replace,
        } => {
            if find.is_empty() {
                return Err(fail(
                    "substitute with an empty `find` matches nothing useful".into(),
                ));
            }
            edit_selected(root, select, |text| text.replace(find.as_str(), replace))
        }
        Op::Transform { select, kind } => edit_selected(root, select, |text| match kind {
            TransformKind::DoubleIndent => double_indent(text),
            TransformKind::TrimTrailing => trim_trailing(text),
        }),
    }
}

/// Apply a text edit to every tracked file the selector admits.
///
/// Binary files are skipped rather than mangled: a fixture may legitimately carry one, and a
/// lossy `from_utf8_lossy` round trip would corrupt it silently.
fn edit_selected(root: &Path, select: &Select, f: impl Fn(&str) -> String) -> Result<()> {
    for rel in walk(root)? {
        if !select.matches(&rel) {
            continue;
        }
        let path = root.join(&rel);
        let bytes = read_file(&path)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let edited = f(text);
        if edited.as_bytes() != bytes {
            write_file(&path, edited.as_bytes())?;
        }
    }
    Ok(())
}

/// Double every line's leading whitespace: a reformat that moves every line and no symbol.
fn double_indent(text: &str) -> String {
    let trailing_newline = text.ends_with('\n');
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        out.push_str(&line[..indent]);
        out.push_str(line);
    }
    if trailing_newline {
        out.push('\n');
    }
    out
}

fn trim_trailing(text: &str) -> String {
    let trailing_newline = text.ends_with('\n');
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    if trailing_newline {
        out.push('\n');
    }
    out
}

// --- patches ----------------------------------------------------------------------------

fn write_patches(
    spec: &Spec,
    repo: &Repository,
    commits: &[CommitRecord],
    dir: &Path,
) -> Result<Vec<PatchRecord>> {
    let mut out = Vec::new();
    for p in &spec.manifest.patch {
        let base = commits
            .iter()
            .find(|c| c.id == p.base)
            .ok_or_else(|| GenError::Commit {
                fixture: spec.name().into(),
                commit: p.base.clone(),
                message: format!("patch `{}` has no such base commit", p.id),
            })?;
        let bytes = normalize(&read_file(&spec.blobs_dir().join(&p.blob))?);

        // Proved against the tree, not the working directory: `apply_to_tree` returns the
        // resulting index and mutates nothing, so a patch is checked without a checkout and
        // without a temporary clone. A patch that has silently rotted against its base is the
        // failure this catches, and it is invisible until a benchmark run tries to use it.
        let diff = git2::Diff::from_buffer(&bytes)?;
        let tree = repo.find_commit(git2::Oid::from_str(&base.sha)?)?.tree()?;
        repo.apply_to_tree(&tree, &diff, None)
            .map_err(|source| GenError::PatchDoesNotApply {
                fixture: spec.name().into(),
                patch: p.id.clone(),
                base: p.base.clone(),
                base_sha: base.sha.clone(),
                source,
            })?;

        let file = format!("{}.patch", p.id);
        write_file(&dir.join(&file), &bytes)?;
        out.push(PatchRecord {
            id: p.id.clone(),
            base: p.base.clone(),
            base_sha: base.sha.clone(),
            file: format!("{}.patches/{}", spec.name(), file),
            description: p.description.clone(),
            verified: true,
        });
    }
    Ok(out)
}

// --- tasks ------------------------------------------------------------------------------

fn resolve_tasks(spec: &Spec, commits: &[CommitRecord]) -> Vec<ResolvedTask> {
    spec.manifest
        .task
        .iter()
        .filter_map(|t| {
            commits
                .iter()
                .find(|c| c.id == t.commit)
                .map(|c| ResolvedTask::from(spec, t, &c.sha))
        })
        .collect()
}

fn emit_task_files(manifest: &Manifest, dir: &Path) -> Result<()> {
    create_dir_all(dir)?;
    for t in &manifest.tasks {
        let text = toml::to_string_pretty(t).map_err(|source| GenError::Toml {
            task: t.id.clone(),
            source: Box::new(source),
        })?;
        let header = format!(
            "# Generated by `nexus fixture generate` from the {} specification.\n\
             # Do not edit: the commit sha below is resolved from a logical id, and\n\
             # regenerating overwrites this file.\n\n",
            manifest.name
        );
        write_file(
            &dir.join(format!("{}.toml", t.id)),
            (header + &text).as_bytes(),
        )?;
    }
    Ok(())
}

// --- filesystem -------------------------------------------------------------------------

/// Refuse to generate on top of something that looks like real work.
///
/// The default output lives under `target/`, but `--out` accepts anything, and the cost of a
/// mistyped path is somebody's source tree. Cheap to check, expensive to omit.
fn guard_output(path: &Path) -> Result<()> {
    for (marker, why) in [
        ("Cargo.toml", "it contains a Cargo.toml"),
        ("package.json", "it contains a package.json"),
        ("pom.xml", "it contains a pom.xml"),
    ] {
        if path.join(marker).exists() {
            return Err(GenError::RefusingToClobber {
                path: path.display().to_string(),
                why: why.into(),
            });
        }
    }
    Ok(())
}

fn reset_dir(path: &Path, force: bool) -> Result<()> {
    if path.exists() {
        let empty = std::fs::read_dir(path)
            .map_err(|source| GenError::Io {
                path: path.display().to_string(),
                source,
            })?
            .next()
            .is_none();
        if !empty && !force {
            return Err(GenError::OutputNotEmpty {
                path: path.display().to_string(),
            });
        }
        std::fs::remove_dir_all(path).map_err(|source| GenError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    create_dir_all(path)
}

fn safe_join(spec: &Spec, root: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    let unsafe_path = p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || rel.starts_with(".git/")
        || rel == ".git";
    if unsafe_path {
        return Err(GenError::UnsafePath {
            fixture: spec.name().into(),
            path: rel.into(),
        });
    }
    Ok(root.join(p))
}

/// Every file under `root` except the repository's own metadata, as sorted relative paths.
fn walk(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    walk_into(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_into(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|source| GenError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for e in entries {
        let e = e.map_err(|source| GenError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if p.is_dir() {
            walk_into(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn tracked_files(root: &Path) -> Result<Vec<String>> {
    walk(root)
}

/// `\r\n` to `\n` on the way in.
///
/// Blobs are checked into a git repository and a Windows checkout may hand them back with
/// CRLF. Normalising here means the tree — and therefore every sha — is the same either way.
fn normalize(bytes: &[u8]) -> Vec<u8> {
    if !bytes.contains(&b'\r') {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| GenError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        create_dir_all(p)?;
    }
    std::fs::write(path, bytes).map_err(|source| GenError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn create_dir_all(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| GenError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// blake3 over the specification's own bytes: the manifest, then every blob by sorted name.
///
/// Two fixtures with the same digest were generated from the same input. A digest that has
/// moved while the shas have not means the specification changed somewhere the history does
/// not reach — a task, a note, an expectation — which is worth being able to see.
fn digest_spec(spec: &Spec) -> Result<String> {
    let mut h = blake3::Hasher::new();
    h.update(&read_file(&spec.dir.join("fixture.toml"))?);
    let blobs = spec.blobs_dir();
    if blobs.is_dir() {
        for rel in walk(&blobs)? {
            h.update(rel.as_bytes());
            h.update(&normalize(&read_file(&blobs.join(&rel))?));
        }
    }
    Ok(h.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_indent_moves_every_line_and_keeps_the_text() {
        let src = "class A {\n    void b() {\n        c();\n    }\n}\n";
        let out = double_indent(src);
        assert_eq!(
            out,
            "class A {\n        void b() {\n                c();\n        }\n}\n"
        );
        assert_eq!(
            out.replace(' ', ""),
            src.replace(' ', ""),
            "a reformat must not change anything but whitespace"
        );
    }

    #[test]
    fn double_indent_preserves_a_missing_trailing_newline() {
        assert_eq!(double_indent("  a"), "    a");
        assert_eq!(double_indent("  a\n"), "    a\n");
    }

    #[test]
    fn trim_trailing_leaves_leading_whitespace_alone() {
        assert_eq!(trim_trailing("  a  \n  b\t\n"), "  a\n  b\n");
    }

    #[test]
    fn crlf_is_normalized_and_a_lone_cr_is_left_alone() {
        assert_eq!(normalize(b"a\r\nb\n"), b"a\nb\n");
        assert_eq!(
            normalize(b"a\rb"),
            b"a\rb",
            "a lone CR is not a CRLF artifact; leave it"
        );
        assert_eq!(normalize(b"plain\n"), b"plain\n");
    }
}
