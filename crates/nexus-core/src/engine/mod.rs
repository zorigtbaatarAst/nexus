//! `Engine` — the single public API of BugHunter.
//!
//! Every CLI command and (from V1) every MCP tool is one call into this facade. Boundary
//! rule: this crate must not depend on `nexus-mcp`, `nexus-cli`, or any concrete AI provider.
//! `tests/boundaries.rs` fails the build otherwise.

mod analyze;
mod memory;
mod query;
mod rescan;
mod scan;
mod verify;

use crate::capability::{Registry as Capabilities, Scope};
use crate::detect::Detector;
use crate::findings::{CodeRef, Finding};
use crate::impact::{self, ImpactQuery};
use crate::project::{ChangedSymbol, EdgeFacts, FileFacts, ProjectContext, SymbolFacts};
use crate::report::*;
use crate::walk::{self, HashedFile};
use nexus_lang::{LanguageAnalyzer, ParsedFile, Registry, SourceFile};
use nexus_store::{ChangeRecord, NewEdge, NewSymbol, Store, SymbolRef};
use nexus_types::*;
use nexus_vcs::{Repo, VcsError};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] nexus_store::StoreError),
    #[error(transparent)]
    Vcs(#[from] nexus_vcs::VcsError),
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a BugHunter project: {0}\n  run `bughunter init` first")]
    NotInitialized(String),
    #[error("no baseline for this project\n  run `nexus scan` first")]
    NoBaseline,
    /// Asked for something this build does not serve. Named rather than approximated:
    /// answering a different question than the one asked is worse than refusing.
    #[error("{0}")]
    Unsupported(String),
    #[error("unknown capability '{asked}'\n  available: {known}")]
    UnknownCapability { asked: String, known: String },
    #[error("capability failed: {0}")]
    Capability(String),
    #[error("a finding needs at least one file:line of evidence — an assertion nobody can check is not a finding")]
    NoEvidence,
    #[error("evidence points at {0}, which is not in the index — run a scan, or check the path")]
    UnknownEvidenceFile(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Where a project's persistent knowledge lives.
///
/// Named for the platform rather than for one capability: the directory holds the code
/// index, the dependency graph, project memory and findings from every capability, only
/// one of which is BugHunter.
pub const NEXUS_DIR: &str = ".nexus";

/// A model may not grade its own work: only reproduction moves a finding above this.
pub const MODEL_CONFIDENCE_CAP: f64 = 0.75;
pub const DB_FILE: &str = "nexus.db";

/// One stray reference to a package the index does not hold is a typo, a generated
/// artifact, or a package that genuinely exists nowhere. A module's worth of them is a
/// module. Below this the count is still reported — it is never hidden — but it does not
/// earn a warning telling someone their scan is too narrow when it is not.
pub const SIBLING_WARN_FLOOR: usize = 20;

/// What the directory was called before this was a platform. See `migrate_legacy_dir`.
const LEGACY_DIR: &str = ".bughunter";
const LEGACY_DB: &str = "bughunter.db";

pub struct Engine {
    root: PathBuf,
    store: Store,
    repo: Option<Repo>,
    registry: Registry,
    capabilities: Capabilities,
    project_id: ProjectId,
}

impl Engine {
    /// Create `.nexus/`, migrate the database, and record what this project is.
    pub fn init(root: &Path, registry: Registry) -> Result<(Self, Profile)> {
        let root = canonical(root);
        if Self::migrate_legacy_dir(&root)? {
            eprintln!("nexus: moved .bughunter/ to .nexus/ — scans, findings and history kept");
        }
        let dir = root.join(NEXUS_DIR);
        std::fs::create_dir_all(dir.join("cache")).map_err(|e| EngineError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;

        // Self-managing: the store, caches, generated tests and audit log are local and
        // disposable; config and policy are committed team intent.
        write_if_absent(
            &dir.join(".gitignore"),
            "nexus.db\nnexus.db-wal\nnexus.db-shm\ncache/\ngenerated-tests/\naudit.log\n",
        )?;
        write_if_absent(&dir.join("config.toml"), DEFAULT_CONFIG)?;
        write_if_absent(&dir.join("policy.toml"), DEFAULT_POLICY)?;

        let mut engine = Self::open_at(&root)?.with_registry(registry);
        let profile = engine.detect()?;
        engine.save_profile(&profile)?;
        Ok((engine, profile))
    }

    /// Move a pre-Nexus project directory into place.
    ///
    /// A single atomic rename rather than a legacy path supported forever: every project
    /// indexed before the platform rename keeps its scans, findings and history, and there
    /// is no second code path to keep correct. Announced on stderr, never silent.
    fn migrate_legacy_dir(root: &Path) -> Result<bool> {
        let legacy = root.join(LEGACY_DIR);
        let current = root.join(NEXUS_DIR);
        if current.exists() || !legacy.join(LEGACY_DB).exists() {
            return Ok(false);
        }
        std::fs::rename(&legacy, &current).map_err(|e| EngineError::Io {
            path: legacy.display().to_string(),
            source: e,
        })?;
        for (from, to) in [
            (LEGACY_DB, DB_FILE),
            ("bughunter.db-wal", "nexus.db-wal"),
            ("bughunter.db-shm", "nexus.db-shm"),
        ] {
            let src = current.join(from);
            if src.exists() {
                let _ = std::fs::rename(src, current.join(to));
            }
        }
        Ok(true)
    }

    pub fn open(root: &Path, registry: Registry) -> Result<Self> {
        let root = canonical(root);
        if Self::migrate_legacy_dir(&root)? {
            eprintln!("nexus: moved .bughunter/ to .nexus/ — scans, findings and history kept");
        }
        if !root.join(NEXUS_DIR).join(DB_FILE).exists() {
            return Err(EngineError::NotInitialized(root.display().to_string()));
        }
        Ok(Self::open_at(&root)?.with_registry(registry))
    }

    /// Open the project, initializing it first if it has never been set up.
    ///
    /// `init` exists as its own command for people who want to inspect the detected
    /// profile before scanning, but requiring it is a step that only ever produces the
    /// error "you forgot to run init". Returns whether it initialized.
    pub fn open_or_init(root: &Path, registry: impl Fn() -> Registry) -> Result<(Self, bool)> {
        // A factory rather than a value: a `Registry` holds trait objects and cannot be
        // cloned, and this may need one twice — once to open, once to initialize when there
        // was nothing to open.
        match Self::open(root, registry()) {
            Ok(engine) => Ok((engine, false)),
            Err(EngineError::NotInitialized(_)) => {
                let (engine, _) = Self::init(root, registry())?;
                Ok((engine, true))
            }
            Err(e) => Err(e),
        }
    }

    fn open_at(root: &Path) -> Result<Self> {
        let store = Store::open(&root.join(NEXUS_DIR).join(DB_FILE))?;
        let repo = Repo::discover(root);
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into());
        let vcs = if repo.is_some() { "git" } else { "none" };
        let project_id = store.ensure_project(&root.display().to_string(), &name, vcs)?;

        // Empty. The composition root registers what this build understands — the core does
        // not name a language (roadmap 5.1). `doctor` reports an empty registry as an error,
        // so a caller that forgets is told rather than quietly indexing nothing.
        let registry = Registry::new();

        Ok(Engine {
            capabilities: Capabilities::new(),
            root: root.to_path_buf(),
            store,
            repo,
            registry,
            project_id,
        })
    }

    /// Make a capability available to this engine.
    ///
    /// Capabilities are registered by the composition root, never compiled into the core:
    /// `nexus-core` depending on `cap-bughunter` would invert the whole point of the split,
    /// and the boundary test forbids it.
    /// Make a language analyzer available to this engine.
    ///
    /// The same rule as capabilities, for the same reason: the platform provides the trait
    /// and the composition root chooses the implementations, so adding a language is a new
    /// crate and one line at the root rather than an edit to the core.
    pub fn register_analyzer(&mut self, a: Box<dyn LanguageAnalyzer>) -> &mut Self {
        self.registry.register(a);
        self
    }

    /// Register every analyzer in a prepared registry.
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.registry = registry;
        self
    }

    pub fn register_capability(&mut self, c: Box<dyn crate::capability::Capability>) -> &mut Self {
        self.capabilities.register(c);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into())
    }

    // ── detection ────────────────────────────────────────────

    pub fn detect(&self) -> Result<Profile> {
        let files = walk::walk(&self.root, &[]);
        let paths: Vec<String> = files.into_iter().map(|f| f.path).collect();
        let analyzed = self.registry.languages();
        Ok(Detector {
            root: &self.root,
            paths: &paths,
        }
        .run(
            self.name(),
            if self.repo.is_some() { "git" } else { "none" },
            &analyzed,
        ))
    }

    fn save_profile(&mut self, p: &Profile) -> Result<()> {
        self.store.save_profile(
            self.project_id,
            &serde_json::to_string(&p.languages)?,
            &serde_json::to_string(&p.frameworks)?,
            p.build_system.as_deref(),
            p.package_manager.as_deref(),
            &serde_json::to_string(&p.databases)?,
            &serde_json::to_string(&p.containers)?,
            "[]",
        )?;
        Ok(())
    }

    fn load_profile(&self) -> Result<Option<Profile>> {
        let Some((langs, fws, build, pm, dbs, containers)) =
            self.store.load_profile(self.project_id)?
        else {
            return Ok(None);
        };
        Ok(Some(Profile {
            name: self.name(),
            languages: serde_json::from_str(&langs)?,
            frameworks: serde_json::from_str(&fws)?,
            build_system: build,
            package_manager: pm,
            databases: serde_json::from_str(&dbs)?,
            containers: serde_json::from_str(&containers)?,
            vcs: if self.repo.is_some() { "git" } else { "none" }.into(),
        }))
    }

    fn tool_versions(&self) -> String {
        let mut map = self.registry.tool_versions();
        map.insert("schema".into(), nexus_store::SCHEMA_VERSION.to_string());
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
    }

    fn head(&self) -> (Option<String>, bool) {
        match &self.repo {
            Some(r) => (r.head_sha().ok().flatten(), r.is_dirty().unwrap_or(true)),
            None => (None, false),
        }
    }

    // ── scan ─────────────────────────────────────────────────

    // ── rescan ───────────────────────────────────────────────

    // ── status ───────────────────────────────────────────────

    // ── bugs ─────────────────────────────────────────────────

    // ── impact ───────────────────────────────────────────────

    // ── doctor ───────────────────────────────────────────────
}

// ─────────────────────────── helpers ───────────────────────────

enum Outcome {
    Parsed(ParsedFile),
    Failed(String),
    Skipped,
}

type Classified = (
    ParseStatus,
    Option<String>,
    Option<Vec<NewSymbol>>,
    Vec<NewEdge>,
);

fn to_new_edge(e: &nexus_lang::RawEdge) -> NewEdge {
    NewEdge {
        src_fqn: e.src_fqn.clone(),
        dst_hint: e.dst_hint.clone(),
        edge_type: e.edge_type,
        site_line: e.site_line,
    }
}

fn to_new_symbol(s: &nexus_lang::RawSymbol) -> NewSymbol {
    NewSymbol {
        kind: s.kind,
        name: s.name.clone(),
        fqn: s.fqn.clone(),
        parent_fqn: s.parent_fqn.clone(),
        signature: s.signature.clone(),
        visibility: s.visibility.clone(),
        start_line: s.start_line,
        end_line: s.end_line,
        sig_hash: s.sig_hash.clone(),
        body_hash: s.body_hash.clone(),
        annotations: s.annotations.clone(),
        authority: s.authority,
    }
}

fn classify(o: &Outcome) -> Classified {
    match o {
        // A file that partly parsed contributes what it has and says what it could not do.
        // Aborting the scan would make one bad file fatal; staying silent would make the
        // index quietly wrong, which is worse.
        Outcome::Parsed(p) => (
            if p.warnings.is_empty() {
                ParseStatus::Ok
            } else {
                ParseStatus::Partial
            },
            p.warnings.first().cloned(),
            Some(p.symbols.iter().map(to_new_symbol).collect()),
            p.edges.iter().map(to_new_edge).collect(),
        ),
        Outcome::Failed(e) => (ParseStatus::Failed, Some(e.clone()), None, Vec::new()),
        Outcome::Skipped => (ParseStatus::Skipped, None, None, Vec::new()),
    }
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn write_if_absent(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|e| EngineError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

const DEFAULT_CONFIG: &str = r#"# BugHunter project configuration — committed, shared team intent.

[scan]
# Extra path prefixes to exclude, on top of .gitignore and .bughunterignore.
exclude = []

[languages]
# Override auto-detection when a directory needs pinning to one language.
"#;

const DEFAULT_POLICY: &str = r#"# BugHunter permissions — committed, so they are reviewed in a pull request
# rather than depending on whoever happens to run the tool.
#
# Defaults are the safe end of every axis: a freshly initialized project can index,
# diff and analyze, but cannot run anything and cannot call any API until someone
# commits a change saying otherwise.

[permissions]
read_paths    = ["**"]
deny_paths    = ["**/.env*", "**/*.pem", "**/*.key", "**/secrets/**", "**/credentials*"]
execute       = "none"     # docker | host | none
allow_network = false
ai            = "agent"    # agent | provider | off

[execute]
timeout_seconds = 600
memory_limit    = "4g"

[execute.allowlist]
# Templates with typed holes, expanded into an explicit argv. Never a shell string.
commands = [
  "./gradlew test --tests {test}",
  "mvn -q test -Dtest={test}",
  "npm test -- {test}",
  "pytest {test}",
  "cargo test {test}",
]

[context.weights]
# How the Context Engine ranks a candidate. One weighted sum, every term recorded —
# see docs/architecture/05-context-engine.md §6. These are data on purpose: tuning is
# an edit here and a re-run, never a release.
#
# The shipped values are argued, not fitted. Seeds dominate because an explicitly named
# symbol is not a guess; history is next because a regression is the most useful thing to
# know before editing; cost is real but never decisive alone, or the package fills with
# cheap trivia. The first evidence-backed tuning is roadmap 5.7.
seed     = 1.0
graph    = 0.8
churn    = 0.3
recency  = 0.2
history  = 0.6
fact     = 0.5
test     = 0.3
arch     = 0.3
cost     = 0.4
# Below this a candidate is excluded even when budget remains. An unfilled budget is not
# a problem to solve.
min_score = 0.15
# At most this many items from one file before another component gets a turn, so a hot
# class cannot fill the package with its own methods.
max_per_component = 3

[ai]
provider           = "none"
max_context_tokens = 24000
redact             = true
"#;

/// Parse in parallel, write single-threaded.
///
/// A free function rather than a method: `Engine` holds a `Connection` and a git2
/// `Repository`, neither of which is `Sync`, so `&self` cannot cross into a rayon closure.
/// The `Registry` can, precisely because boundary rule 5 forbids an analyzer from touching
/// the store — parallel parsing is a payoff of that rule, not a coincidence.
///
/// Writes stay on one thread because SQLite in WAL mode has one writer; pretending
/// otherwise buys `SQLITE_BUSY` retries, not throughput.
fn parse_all(registry: &Registry, root: &Path, files: &[HashedFile]) -> Vec<(HashedFile, Outcome)> {
    files
        .par_iter()
        .map(|f| {
            let Some(analyzer) = registry.for_path(&f.path) else {
                return (f.clone(), Outcome::Skipped);
            };
            let text = match std::fs::read_to_string(root.join(&f.path)) {
                Ok(t) => t,
                Err(e) => return (f.clone(), Outcome::Failed(e.to_string())),
            };
            match analyzer.parse(&SourceFile {
                path: &f.path,
                text: &text,
            }) {
                Ok(p) => (f.clone(), Outcome::Parsed(p)),
                Err(e) => (f.clone(), Outcome::Failed(e.to_string())),
            }
        })
        .collect()
}
