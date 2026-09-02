//! The fixture specification format.
//!
//! A fixture is a manifest plus a directory of real source files. Content lives in `blobs/`
//! as ordinary `.java`, `.ts` and `.graphqls` files rather than inside TOML strings: they
//! stay diffable, syntax-highlightable, and free of escaping — and a fixture whose source is
//! unreadable is a fixture nobody will maintain.
//!
//! ```text
//! tests/fixtures/specs/spring-payments/
//!   fixture.toml        the manifest: commits, operations, patches, tasks
//!   blobs/              the file contents each operation writes
//! ```
//!
//! Everything here is data. Adding a fixture is a new directory, never a code change — which
//! is the property that keeps the corpus something an evaluation can grow.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("{fixture}: {message}")]
    Invalid { fixture: String, message: String },
}

type Result<T> = std::result::Result<T, SpecError>;

/// A loaded specification: the parsed manifest plus where its blobs live.
#[derive(Debug, Clone)]
pub struct Spec {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Spec {
    pub fn name(&self) -> &str {
        &self.manifest.fixture.name
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.dir.join("blobs")
    }

    /// Load one specification directory.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("fixture.toml");
        let text = std::fs::read_to_string(&path).map_err(|source| SpecError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let manifest: Manifest = toml::from_str(&text).map_err(|source| SpecError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })?;
        let spec = Spec {
            manifest,
            dir: dir.to_path_buf(),
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Every specification directory under `root`, in name order.
    ///
    /// Ordered because generation order must not depend on the filesystem's whim: two
    /// machines that disagree about readdir order would otherwise produce different logs
    /// for the same corpus.
    pub fn load_all(root: &Path) -> Result<Vec<Self>> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let entries = std::fs::read_dir(root).map_err(|source| SpecError::Io {
            path: root.display().to_string(),
            source,
        })?;
        for e in entries {
            let e = e.map_err(|source| SpecError::Io {
                path: root.display().to_string(),
                source,
            })?;
            let p = e.path();
            if p.join("fixture.toml").is_file() {
                dirs.push(p);
            }
        }
        dirs.sort();
        dirs.iter().map(|d| Spec::load(d)).collect()
    }

    /// Everything checkable without touching a filesystem or replaying the history.
    ///
    /// Path-dependent mistakes — moving a file that is not there — are caught at apply time
    /// with the commit that caused them named. Catching a dangling *identifier* here is
    /// worth the duplication because it is the class of error a person makes while editing
    /// TOML, and the message can point at the line rather than at a git failure five commits
    /// later.
    fn validate(&self) -> Result<()> {
        let m = &self.manifest;
        let fixture = m.fixture.name.clone();
        let bad = |message: String| SpecError::Invalid {
            fixture: fixture.clone(),
            message,
        };

        if m.commit.is_empty() {
            return Err(bad("a fixture needs at least one commit".into()));
        }
        if m.fixture.commit_interval_s <= 0 {
            return Err(bad("commit_interval_s must be positive".into()));
        }

        let mut ids: BTreeSet<&str> = BTreeSet::new();
        for c in &m.commit {
            if !ids.insert(&c.id) {
                return Err(bad(format!("duplicate commit id `{}`", c.id)));
            }
        }

        let branches: BTreeSet<&str> = m.branch.iter().map(|b| b.name.as_str()).collect();
        for b in &m.branch {
            if !ids.contains(b.from.as_str()) {
                return Err(bad(format!(
                    "branch `{}` starts from unknown commit `{}`",
                    b.name, b.from
                )));
            }
        }

        let blobs = self.blobs_dir();
        for c in &m.commit {
            if let Some(br) = &c.branch {
                if br != &m.fixture.default_branch && !branches.contains(br.as_str()) {
                    return Err(bad(format!(
                        "commit `{}` is on undeclared branch `{}`",
                        c.id, br
                    )));
                }
            }
            for op in &c.ops() {
                if let Op::Write { blob: Some(b), .. } = op {
                    if !blobs.join(b).is_file() {
                        return Err(bad(format!(
                            "commit `{}` writes missing blob `blobs/{}`",
                            c.id, b
                        )));
                    }
                }
                if let Op::Write {
                    blob: None,
                    content: None,
                    path,
                    ..
                } = op
                {
                    return Err(bad(format!(
                        "commit `{}` writes `{}` with neither `blob` nor `content`",
                        c.id, path
                    )));
                }
            }
        }

        for p in &m.patch {
            if !ids.contains(p.base.as_str()) {
                return Err(bad(format!(
                    "patch `{}` has unknown base commit `{}`",
                    p.id, p.base
                )));
            }
            if !blobs.join(&p.blob).is_file() {
                return Err(bad(format!(
                    "patch `{}` references missing blob `blobs/{}`",
                    p.id, p.blob
                )));
            }
        }

        let patches: BTreeSet<&str> = m.patch.iter().map(|p| p.id.as_str()).collect();
        for t in &m.task {
            if !ids.contains(t.commit.as_str()) {
                return Err(bad(format!(
                    "task `{}` starts at unknown commit `{}`",
                    t.id, t.commit
                )));
            }
            if let Some(p) = t.start_state.patch_id() {
                if !patches.contains(p) {
                    return Err(bad(format!(
                        "task `{}` starts dirty from unknown patch `{}`",
                        t.id, p
                    )));
                }
            }
            if t.prompt.is_none() && t.turns.is_empty() {
                return Err(bad(format!(
                    "task `{}` has neither `prompt` nor `turns`",
                    t.id
                )));
            }
            if t.prompt.is_some() && !t.turns.is_empty() {
                return Err(bad(format!(
                    "task `{}` has both `prompt` and `turns`; a task is single-turn or \
                     multi-turn, never both",
                    t.id
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub fixture: Fixture,
    pub author: Author,
    #[serde(default)]
    pub branch: Vec<Branch>,
    #[serde(default)]
    pub commit: Vec<Commit>,
    #[serde(default)]
    pub patch: Vec<Patch>,
    /// Plausible-but-wrong code the live path does not use. Family H's whole subject.
    #[serde(default)]
    pub deprecated_path: Vec<DeprecatedPath>,
    #[serde(default)]
    pub task: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub name: String,
    pub description: String,
    /// Free-form: `["java", "spring", "jpa"]`. Recorded, never interpreted.
    #[serde(default)]
    pub stack: Vec<String>,
    /// Which evaluation role this repository plays. Documentation, not dispatch.
    #[serde(default)]
    pub role: String,
    /// The clock origin, in seconds since the Unix epoch. Commit *n* is stamped
    /// `base_epoch + n * commit_interval_s`, so a history is a pure function of its spec.
    pub base_epoch: i64,
    #[serde(default = "default_interval")]
    pub commit_interval_s: i64,
    #[serde(default = "default_branch")]
    pub default_branch: String,
}

fn default_interval() -> i64 {
    86_400
}
fn default_branch() -> String {
    "main".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub name: String,
    /// The commit id this branch forks from.
    pub from: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    /// Stable logical name — `c3`. Tasks and patches refer to this, never to a sha, because
    /// a sha does not exist until the fixture has been generated.
    pub id: String,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,

    #[serde(default, rename = "write")]
    pub writes: Vec<Write>,
    #[serde(default, rename = "delete")]
    pub deletes: Vec<Delete>,
    #[serde(default, rename = "move")]
    pub moves: Vec<Move>,
    /// Literal, non-regex text replacement over selected files.
    #[serde(default, rename = "substitute")]
    pub substitutions: Vec<Substitute>,
    /// A named whole-file reformat — the commit that must produce zero symbol changes.
    #[serde(default, rename = "transform")]
    pub transforms: Vec<Transform>,

    /// A defect this commit deliberately introduces.
    #[serde(default)]
    pub plants_bug: Option<Bug>,
    /// What the evaluation expects of this commit. **Recorded, never checked here** — the
    /// generator has no index and, by boundary rule, cannot acquire one.
    #[serde(default)]
    pub expect: Expect,
}

impl Commit {
    /// The operations of this commit, in the order they are applied: writes, then moves,
    /// then substitutions and transforms, then deletes.
    ///
    /// Fixed rather than declaration-ordered so that a spec cannot depend on how TOML tables
    /// happened to be interleaved. Deletes run last so a file can be written, read by a
    /// substitution and then removed within one commit.
    pub fn ops(&self) -> Vec<Op> {
        let mut ops = Vec::new();
        for w in &self.writes {
            ops.push(Op::Write {
                path: w.path.clone(),
                blob: w.blob.clone(),
                content: w.content.clone(),
            });
        }
        for m in &self.moves {
            ops.push(Op::Move {
                from: m.from.clone(),
                to: m.to.clone(),
            });
        }
        for s in &self.substitutions {
            ops.push(Op::Substitute {
                select: s.select.clone(),
                find: s.find.clone(),
                replace: s.replace.clone(),
            });
        }
        for t in &self.transforms {
            ops.push(Op::Transform {
                select: t.select.clone(),
                kind: t.kind,
            });
        }
        for d in &self.deletes {
            ops.push(Op::Delete {
                path: d.path.clone(),
            });
        }
        ops
    }
}

#[derive(Debug, Clone)]
pub enum Op {
    Write {
        path: String,
        blob: Option<String>,
        content: Option<String>,
    },
    Delete {
        path: String,
    },
    Move {
        from: String,
        to: String,
    },
    Substitute {
        select: Select,
        find: String,
        replace: String,
    },
    Transform {
        select: Select,
        kind: TransformKind,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub path: String,
    /// A file under `blobs/`. Mutually exclusive with `content`.
    #[serde(default)]
    pub blob: Option<String>,
    /// Inline content, for the one-liners where a separate file is noise.
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Substitute {
    #[serde(flatten)]
    pub select: Select,
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    #[serde(flatten)]
    pub select: Select,
    pub kind: TransformKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransformKind {
    /// Double every line's leading whitespace. A whitespace-only reformat, which is exactly
    /// what the "must produce zero symbol changes" assertion needs to be tested against.
    DoubleIndent,
    /// Strip trailing whitespace from every line.
    TrimTrailing,
}

/// Which files an operation touches.
///
/// Deliberately **not** globs. `extensions` + `under` covers every selection the corpus
/// needs, and a hand-rolled glob matcher is a well-known source of quiet wrongness — a
/// fixture that silently reformats the wrong file set would corrupt the very assertion the
/// reformat commit exists to make.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    /// Exact paths. When present, `extensions` and `under` are ignored.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Extensions without the dot: `["java"]`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Path prefix: `"src/main/"`.
    #[serde(default)]
    pub under: Option<String>,
}

impl Select {
    pub fn matches(&self, rel: &str) -> bool {
        if !self.paths.is_empty() {
            return self.paths.iter().any(|p| p == rel);
        }
        if let Some(u) = &self.under {
            if !rel.starts_with(u.as_str()) {
                return false;
            }
        }
        if self.extensions.is_empty() {
            return true;
        }
        let ext = rel.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        self.extensions.iter().any(|e| e == ext)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bug {
    pub id: String,
    /// Free-form: `concurrency`, `contract`, `security`.
    pub kind: String,
    pub summary: String,
    /// Where it lives, as `path:line` where a line is meaningful.
    #[serde(default)]
    pub anchor: Option<String>,
    /// The commit id that fixes it, when the history contains one.
    #[serde(default)]
    pub fixed_by: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// The reformat commit's assertion: a whole-repo reformat must move zero symbols.
    #[serde(default)]
    pub symbol_changes: Option<u32>,
    #[serde(default)]
    pub new_findings: Option<u32>,
    #[serde(default)]
    pub note: Option<String>,
}

/// A working-tree patch, for the `dirty` start states of `13-evaluation.md` §13.2.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub id: String,
    /// A unified diff under `blobs/`.
    pub blob: String,
    /// The commit it must apply cleanly to.
    pub base: String,
    #[serde(default)]
    pub description: String,
}

/// Plausible-but-wrong code that the live path does not use.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeprecatedPath {
    pub id: String,
    pub paths: Vec<String>,
    /// Why it is a decoy: what makes it look right, and what makes it wrong.
    pub note: String,
    /// The live code a careless reader would confuse it with.
    pub live_path: String,
    /// The task whose ranking it is designed to mislead.
    #[serde(default)]
    pub decoy_for: Option<String>,
}

/// Benchmark task metadata, in the shape `13-evaluation.md` §3 defines.
///
/// Authored against a **logical commit id**; generation resolves it to a sha. Hand-copying
/// shas into task files is the kind of clerical step that is wrong once and then wrong
/// forever, so the generator does it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    /// A · B · C · D · E · N · H · M — the families of `13-evaluation.md` §4.
    pub family: String,
    pub commit: String,
    #[serde(default)]
    pub start_state: StartState,
    #[serde(default)]
    pub prompt: Option<String>,
    /// Multi-turn scripts. Mutually exclusive with `prompt`.
    #[serde(default)]
    pub turns: Vec<Turn>,
    /// Every place that must change. `13-evaluation.md` §7 grades L3 against this.
    #[serde(default)]
    pub required_sites: Vec<String>,
    #[serde(default)]
    pub hidden_tests: Vec<String>,
    #[serde(default)]
    pub convention_rules: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_s: u32,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_timeout() -> u32 {
    900
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub prompt: String,
    /// What the package for *this* turn must still contain. The anchor-retention
    /// measurement of `13-evaluation.md` §14.2.
    #[serde(default)]
    pub required_anchors: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartState {
    #[default]
    Clean,
    /// The named patch is applied but not committed: work in progress.
    Dirty(String),
}

impl StartState {
    pub fn patch_id(&self) -> Option<&str> {
        match self {
            StartState::Clean => None,
            StartState::Dirty(p) => Some(p),
        }
    }

    /// The `13-evaluation.md` §3 wire form: `"clean"` or `"dirty:<patch-id>"`.
    pub fn as_wire(&self) -> String {
        match self {
            StartState::Clean => "clean".into(),
            StartState::Dirty(p) => format!("dirty:{p}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selector_without_extensions_takes_everything_under_the_prefix() {
        let s = Select {
            under: Some("src/".into()),
            ..Default::default()
        };
        assert!(s.matches("src/a.java"));
        assert!(s.matches("src/b.txt"));
        assert!(!s.matches("web/a.java"));
    }

    #[test]
    fn extensions_and_prefix_both_bind() {
        let s = Select {
            extensions: vec!["java".into()],
            under: Some("src/".into()),
            ..Default::default()
        };
        assert!(s.matches("src/a.java"));
        assert!(!s.matches("src/a.ts"), "extension must bind");
        assert!(!s.matches("web/a.java"), "prefix must bind");
    }

    #[test]
    fn explicit_paths_win_over_every_other_filter() {
        let s = Select {
            paths: vec!["only/this.ts".into()],
            extensions: vec!["java".into()],
            under: Some("src/".into()),
        };
        assert!(s.matches("only/this.ts"));
        assert!(!s.matches("src/a.java"));
    }

    #[test]
    fn a_dotted_path_with_no_extension_matches_no_extension_filter() {
        let s = Select {
            extensions: vec!["java".into()],
            ..Default::default()
        };
        assert!(!s.matches("Makefile"));
    }

    #[test]
    fn start_state_round_trips_through_its_wire_form() {
        assert_eq!(StartState::Clean.as_wire(), "clean");
        assert_eq!(StartState::Dirty("wip".into()).as_wire(), "dirty:wip");
        assert_eq!(StartState::Dirty("wip".into()).patch_id(), Some("wip"));
        assert_eq!(StartState::Clean.patch_id(), None);
    }
}
