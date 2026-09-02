//! `Engine` — the single public API of BugHunter.
//!
//! Every CLI command and (from V1) every MCP tool is one call into this facade. Boundary
//! rule: this crate must not depend on `nexus-mcp`, `nexus-cli`, or any concrete AI provider.
//! `tests/boundaries.rs` fails the build otherwise.

use crate::capability::{Registry as Capabilities, Scope};
use crate::detect::Detector;
use crate::findings::{CodeRef, Finding};
use crate::impact::{self, ImpactQuery};
use crate::project::{ChangedSymbol, EdgeFacts, FileFacts, ProjectContext, SymbolFacts};
use crate::report::*;
use crate::walk::{self, HashedFile};
use nexus_lang::{ParsedFile, Registry, SourceFile};
use nexus_lang_graphql::GraphQlSchemaAnalyzer;
use nexus_lang_java::JavaAnalyzer;
use nexus_lang_ts::TypeScriptAnalyzer;
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
    pub fn init(root: &Path) -> Result<(Self, Profile)> {
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

        let mut engine = Self::open_at(&root)?;
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

    pub fn open(root: &Path) -> Result<Self> {
        let root = canonical(root);
        if Self::migrate_legacy_dir(&root)? {
            eprintln!("nexus: moved .bughunter/ to .nexus/ — scans, findings and history kept");
        }
        if !root.join(NEXUS_DIR).join(DB_FILE).exists() {
            return Err(EngineError::NotInitialized(root.display().to_string()));
        }
        Self::open_at(&root)
    }

    /// Open the project, initializing it first if it has never been set up.
    ///
    /// `init` exists as its own command for people who want to inspect the detected
    /// profile before scanning, but requiring it is a step that only ever produces the
    /// error "you forgot to run init". Returns whether it initialized.
    pub fn open_or_init(root: &Path) -> Result<(Self, bool)> {
        match Self::open(root) {
            Ok(engine) => Ok((engine, false)),
            Err(EngineError::NotInitialized(_)) => {
                let (engine, _) = Self::init(root)?;
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

        let mut registry = Registry::new();
        registry
            .register(Box::new(JavaAnalyzer::new()))
            .register(Box::new(TypeScriptAnalyzer::new()))
            // The schema is indexed as the contract both sides are generated from, so
            // "no resolver serves this" means the field is absent from the schema — not
            // merely that no annotation shape this analyzer knows was found.
            .register(Box::new(GraphQlSchemaAnalyzer::new()));

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
        let analyzed: Vec<&str> = vec!["java", "typescript"];
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

    /// Full scan. The only run that reads the whole repository.
    pub fn scan(&mut self) -> Result<ScanReport> {
        let started = Instant::now();
        let walked = walk::walk(&self.root, &[]);
        let hashed = walk::hash_all(&self.root, &walked);

        let tree: BTreeMap<String, String> = hashed
            .iter()
            .map(|h| (h.path.clone(), h.content_hash.clone()))
            .collect();
        let (commit, dirty) = self.head();
        let (scan_id, scan_uid) = self.store.begin_scan(
            self.project_id,
            ScanKind::Full,
            None,
            commit.as_deref(),
            &walk::working_tree_hash(&tree),
            dirty,
            &self.tool_versions(),
        )?;

        let parsed = parse_all(&self.registry, &self.root, &hashed);
        let existing: BTreeSet<String> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|f| f.path)
            .collect();
        let seen: BTreeSet<String> = hashed.iter().map(|h| h.path.clone()).collect();

        let mut warnings = Vec::new();
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut symbols_indexed = 0usize;

        let tx = self.store.transaction()?;
        let mut pending_edges: Vec<(FileId, Vec<NewEdge>)> = Vec::new();
        for (file, outcome) in &parsed {
            let (status, error, symbols, edges) = classify(outcome);
            match status {
                ParseStatus::Failed => failed += 1,
                ParseStatus::Skipped => skipped += 1,
                _ => {}
            }
            if let Some(e) = &error {
                warnings.push(format!("{}: {e}", file.path));
            }
            let lang = self
                .registry
                .language_for_path(&file.path)
                .map(|l| l.as_str());
            let file_id = Store::upsert_file(
                &tx,
                self.project_id,
                scan_id,
                &file.path,
                lang,
                &file.content_hash,
                file.size_bytes as i64,
                Some(file.loc as i64),
                Some(file.mtime_ns),
                status,
                error.as_deref(),
            )?;
            if let Some(syms) = symbols {
                symbols_indexed +=
                    Store::replace_symbols(&tx, self.project_id, file_id, scan_id, &syms)?;
            }
            if !edges.is_empty() {
                pending_edges.push((file_id, edges));
            }
        }
        for gone in existing.difference(&seen) {
            Store::mark_file_deleted(&tx, self.project_id, gone, scan_id)?;
        }
        // Edges are written only after every symbol exists: an edge's source must be
        // resolvable, and resolution needs the complete symbol table — which is precisely
        // why an analyzer cannot do this itself.
        for (file_id, edges) in &pending_edges {
            Store::replace_edges_for_file(&tx, self.project_id, *file_id, scan_id, edges)?;
        }
        let resolve = Store::resolve_edges(&tx, self.project_id)?;
        tx.commit().map_err(nexus_store::StoreError::from)?;
        if resolve.unresolved > 0 {
            warnings.push(format!(
                "{} edges point inside the project but matched no symbol (overloads, inherited methods)",
                resolve.unresolved
            ));
        }
        // The most consequential thing a scan can discover about itself: it is looking at
        // one module of something larger. Silence here is what lets an impact query report
        // a small blast radius with total confidence.
        if resolve.sibling >= SIBLING_WARN_FLOOR {
            let owner = resolve.owner.as_deref().unwrap_or("this project");
            warnings.push(format!(
                "{} edges point at {owner}.* code that was not scanned — this looks like one \
                 module of a larger project; scan from the repository root to see it",
                resolve.sibling
            ));
        }

        let health = if failed > 0 {
            Health::Degraded
        } else {
            Health::Ok
        };
        self.store.finish_scan(
            scan_id,
            ScanStatus::Ok,
            hashed.len() as i64,
            failed as i64,
            symbols_indexed as i64,
            None,
        )?;
        self.store.set_baseline(self.project_id, scan_id)?;

        Ok(ScanReport {
            scan_uid,
            kind: "full",
            commit,
            dirty,
            files_scanned: hashed.len(),
            files_failed: failed,
            files_skipped: skipped,
            symbols_indexed,
            edges_resolved: resolve.resolved(),
            edges_total: resolve.total,
            edges_external: resolve.external,
            edges_sibling: resolve.sibling,
            health,
            warnings,
            duration_ms: started.elapsed().as_millis(),
        })
    }

    // ── rescan ───────────────────────────────────────────────

    /// The everyday command. Cost is proportional to what changed, not to how much exists.
    pub fn rescan(&mut self) -> Result<RescanReport> {
        let started = Instant::now();
        let Some(baseline) = self.store.baseline(self.project_id)? else {
            return Err(EngineError::NoBaseline);
        };
        let (commit, dirty) = self.head();
        let base_rev = Revision {
            scan_uid: Some(baseline.scan_uid.clone()),
            commit: baseline.commit_sha.clone(),
            dirty: baseline.dirty,
        };
        let cur_rev = Revision {
            scan_uid: None,
            commit: commit.clone(),
            dirty,
        };

        // A grammar or analyzer upgrade must force a re-parse even though every content hash
        // still matches — otherwise the index silently keeps the symbols the old grammar
        // produced, indefinitely, with no error anywhere.
        let forced_full = (baseline.tool_versions_json != self.tool_versions())
            .then(|| "analyzer or grammar version changed since the baseline".to_string());

        // ── Tier 0: repo gate ──
        if forced_full.is_none()
            && !dirty
            && !baseline.dirty
            && commit.is_some()
            && commit == baseline.commit_sha
        {
            return Ok(RescanReport {
                scan_uid: None,
                baseline: base_rev,
                current: cur_rev,
                unchanged: true,
                forced_full: None,
                files_changed: 0,
                files_deleted: 0,
                symbols_changed: 0,
                items: Vec::new(),
                files_failed: 0,
                health: Health::Ok,
                warnings: Vec::new(),
                duration_ms: started.elapsed().as_millis(),
            });
        }

        // ── Tier 1: candidate file set ──
        let stored: BTreeMap<String, nexus_store::FileRow> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        let mut warnings = Vec::new();
        let (candidates, deleted, exhaustive) =
            self.candidates(&baseline, forced_full.is_some(), &mut warnings);

        let mut changed: Vec<HashedFile> = Vec::new();
        let mut deleted_paths: BTreeSet<String> = deleted;

        for path in &candidates {
            let abs = self.root.join(path);
            let Ok(meta) = std::fs::metadata(&abs) else {
                if stored.contains_key(path) {
                    deleted_paths.insert(path.clone());
                }
                continue;
            };
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            if let Some(row) = stored.get(path) {
                // stat fast path: ~1µs, and it eliminates hashing for almost everything.
                if !forced_full.is_some()
                    && row.size_bytes == size as i64
                    && row.mtime_ns == Some(mtime)
                {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&abs) else {
                    continue;
                };
                let hash = walk::hash_bytes(&bytes);
                if hash == row.content_hash && forced_full.is_none() {
                    continue; // touched, not changed
                }
                changed.push(HashedFile {
                    path: path.clone(),
                    size_bytes: size,
                    mtime_ns: mtime,
                    content_hash: hash,
                    loc: bytes.iter().filter(|b| **b == b'\n').count() as u32 + 1,
                });
            } else {
                let Ok(bytes) = std::fs::read(&abs) else {
                    continue;
                };
                changed.push(HashedFile {
                    path: path.clone(),
                    size_bytes: size,
                    mtime_ns: mtime,
                    content_hash: walk::hash_bytes(&bytes),
                    loc: bytes.iter().filter(|b| **b == b'\n').count() as u32 + 1,
                });
            }
        }

        if exhaustive {
            let seen: BTreeSet<&String> = candidates.iter().collect();
            for path in stored.keys() {
                if !seen.contains(path) {
                    deleted_paths.insert(path.clone());
                }
            }
        }

        // Rename detection: a path that vanished and a path that appeared sharing a content
        // hash is a rename, not a delete plus an add. Without this, every package refactor
        // invents a repository full of "new" symbols.
        let renames = detect_renames(&stored, &changed, &mut deleted_paths);

        if changed.is_empty() && deleted_paths.is_empty() {
            return Ok(RescanReport {
                scan_uid: None,
                baseline: base_rev,
                current: cur_rev,
                unchanged: true,
                forced_full,
                files_changed: 0,
                files_deleted: 0,
                symbols_changed: 0,
                items: Vec::new(),
                files_failed: 0,
                health: Health::Ok,
                warnings,
                duration_ms: started.elapsed().as_millis(),
            });
        }

        // ── record the scan, then Tier 2 ──
        let mut tree: BTreeMap<String, String> = stored
            .iter()
            .map(|(p, f)| (p.clone(), f.content_hash.clone()))
            .collect();
        for c in &changed {
            tree.insert(c.path.clone(), c.content_hash.clone());
        }
        for d in &deleted_paths {
            tree.remove(d);
        }

        let (scan_id, scan_uid) = self.store.begin_scan(
            self.project_id,
            if forced_full.is_some() {
                ScanKind::Full
            } else {
                ScanKind::Incremental
            },
            Some(baseline.scan_id),
            commit.as_deref(),
            &walk::working_tree_hash(&tree),
            dirty,
            &self.tool_versions(),
        )?;

        let parsed = parse_all(&self.registry, &self.root, &changed);
        let mut items = Vec::new();
        let mut failed = 0usize;
        let mut symbols_changed = 0usize;
        // Appearances and disappearances are held until every changed file has been seen:
        // a rename is only visible from both halves at once, and they can be in different
        // files — which is exactly what a package move is.
        let mut appeared: Vec<SymbolDelta> = Vec::new();
        let mut vanished: Vec<SymbolDelta> = Vec::new();

        // Read every symbol set we will need *before* opening the transaction: rusqlite's
        // Transaction holds a mutable borrow of the connection for its entire lifetime.
        let mut old_by_path: BTreeMap<String, BTreeMap<String, nexus_store::SymbolRow>> =
            BTreeMap::new();
        for path in deleted_paths.iter().chain(changed.iter().map(|c| &c.path)) {
            if let Some(row) = stored.get(path) {
                let map = self
                    .store
                    .symbols_for_file(row.id)?
                    .into_iter()
                    .map(|s| (s.fqn.clone(), s))
                    .collect();
                old_by_path.insert(path.clone(), map);
            }
        }

        let tx = self.store.transaction()?;

        for path in &deleted_paths {
            if stored.contains_key(path) {
                for s in old_by_path
                    .get(path)
                    .cloned()
                    .unwrap_or_default()
                    .into_values()
                {
                    // A file that disappeared is the commonest source of a rename, so its
                    // symbols are held back with the rest rather than reported as deleted.
                    vanished.push(SymbolDelta {
                        fqn: s.fqn.clone(),
                        path: path.clone(),
                        name: s.name.clone(),
                        sig_hash: s.sig_hash.clone(),
                        body_hash: s.body_hash.clone(),
                        old_id: Some(s.id),
                    });
                }
            }
            let ct = if renames.contains_key(path) {
                ChangeType::Renamed
            } else {
                ChangeType::Deleted
            };
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "file",
                    entity_id: stored.get(path).map(|r| r.id),
                    path: Some(path.clone()),
                    fqn: None,
                    change_type: ct,
                    detail: None,
                    before_hash: stored.get(path).map(|r| r.content_hash.clone()),
                    after_hash: None,
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "file",
                change_type: ct.as_str(),
                kind: None,
                path: Some(path.clone()),
                fqn: None,
                from_fqn: None,
            });
            Store::mark_file_deleted(&tx, self.project_id, path, scan_id)?;
        }

        let mut pending_edges: Vec<(FileId, Vec<NewEdge>)> = Vec::new();
        for (file, outcome) in &parsed {
            let (status, error, symbols, edges) = classify(outcome);
            if !edges.is_empty() {
                pending_edges.push((0, edges));
            }
            if status == ParseStatus::Failed {
                failed += 1;
            }
            if let Some(e) = &error {
                warnings.push(format!("{}: {e}", file.path));
            }

            let was = stored.get(&file.path);
            let old_symbols = old_by_path.get(&file.path).cloned().unwrap_or_default();

            let lang = self
                .registry
                .language_for_path(&file.path)
                .map(|l| l.as_str());
            let file_id = Store::upsert_file(
                &tx,
                self.project_id,
                scan_id,
                &file.path,
                lang,
                &file.content_hash,
                file.size_bytes as i64,
                Some(file.loc as i64),
                Some(file.mtime_ns),
                status,
                error.as_deref(),
            )?;

            let file_ct = if was.is_some() {
                ChangeType::Modified
            } else {
                ChangeType::Added
            };
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "file",
                    entity_id: Some(file_id),
                    path: Some(file.path.clone()),
                    fqn: None,
                    change_type: file_ct,
                    detail: Some("content"),
                    before_hash: was.map(|r| r.content_hash.clone()),
                    after_hash: Some(file.content_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "file",
                change_type: file_ct.as_str(),
                kind: None,
                path: Some(file.path.clone()),
                fqn: None,
                from_fqn: None,
            });

            // ── Tier 2: symbol-level diff ──
            if let Some(new_symbols) = symbols {
                for s in &new_symbols {
                    let kind = match old_symbols.get(&s.fqn) {
                        None => {
                            // Held back: this may be half of a rename, and that can only be
                            // decided once every changed file has been seen — the other half
                            // is usually in a different file.
                            appeared.push(SymbolDelta {
                                fqn: s.fqn.clone(),
                                path: file.path.clone(),
                                name: s.name.clone(),
                                sig_hash: s.sig_hash.clone(),
                                body_hash: s.body_hash.clone(),
                                old_id: None,
                            });
                            continue;
                        }
                        Some(old) => symbol_change(old, s),
                    };
                    let Some(kind) = kind else { continue };
                    Store::insert_change(
                        &tx,
                        scan_id,
                        &ChangeRecord {
                            entity: "symbol",
                            entity_id: None,
                            path: Some(file.path.clone()),
                            fqn: Some(s.fqn.clone()),
                            change_type: kind.change_type(),
                            detail: kind.detail(),
                            before_hash: old_symbols.get(&s.fqn).map(|o| o.body_hash.clone()),
                            after_hash: Some(s.body_hash.clone()),
                            commit_sha: commit.clone(),
                        },
                    )?;
                    items.push(ChangeItem {
                        entity: "symbol",
                        change_type: kind.change_type().as_str(),
                        kind: Some(kind.as_str()),
                        path: Some(file.path.clone()),
                        fqn: Some(s.fqn.clone()),
                        from_fqn: None,
                    });
                    symbols_changed += 1;
                }
                let new_fqns: BTreeSet<&str> = new_symbols.iter().map(|s| s.fqn.as_str()).collect();
                for (fqn, old) in &old_symbols {
                    if new_fqns.contains(fqn.as_str()) {
                        continue;
                    }
                    vanished.push(SymbolDelta {
                        fqn: fqn.clone(),
                        path: file.path.clone(),
                        name: old.name.clone(),
                        sig_hash: old.sig_hash.clone(),
                        body_hash: old.body_hash.clone(),
                        old_id: Some(old.id),
                    });
                }
                Store::replace_symbols(&tx, self.project_id, file_id, scan_id, &new_symbols)?;
            }
            if let Some(last) = pending_edges.last_mut() {
                if last.0 == 0 {
                    last.0 = file_id;
                }
            }
        }
        // ── rename resolution ──
        //
        // Every symbol is now written, so an appearance can be matched to a disappearance
        // and the pair collapsed into one rename. Matching on (name, sig_hash, body_hash)
        // is what survives a package move: the FQN changes and nothing else does.
        let renamed = resolve_symbol_renames(&appeared, &vanished);

        for (old_idx, new_idx) in &renamed {
            let old = &vanished[*old_idx];
            let new = &appeared[*new_idx];
            if let Some(id) = Store::symbol_id_by_fqn(&tx, self.project_id, &new.fqn)? {
                Store::record_alias(&tx, self.project_id, &old.fqn, id, scan_id)?;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: None,
                    path: Some(new.path.clone()),
                    fqn: Some(new.fqn.clone()),
                    change_type: ChangeType::Renamed,
                    detail: None,
                    before_hash: Some(old.body_hash.clone()),
                    after_hash: Some(new.body_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "renamed",
                kind: Some(ChangeKind::Renamed.as_str()),
                path: Some(new.path.clone()),
                fqn: Some(new.fqn.clone()),
                from_fqn: Some(old.fqn.clone()),
            });
            symbols_changed += 1;
        }

        let matched_old: BTreeSet<usize> = renamed.iter().map(|(o, _)| *o).collect();
        let matched_new: BTreeSet<usize> = renamed.iter().map(|(_, n)| *n).collect();

        for (i, d) in vanished.iter().enumerate() {
            if matched_old.contains(&i) {
                continue;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: d.old_id,
                    path: Some(d.path.clone()),
                    fqn: Some(d.fqn.clone()),
                    change_type: ChangeType::Deleted,
                    detail: None,
                    before_hash: Some(d.body_hash.clone()),
                    after_hash: None,
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "deleted",
                kind: Some(ChangeKind::Deleted.as_str()),
                path: Some(d.path.clone()),
                fqn: Some(d.fqn.clone()),
                from_fqn: None,
            });
            symbols_changed += 1;
        }

        for (i, d) in appeared.iter().enumerate() {
            if matched_new.contains(&i) {
                continue;
            }
            Store::insert_change(
                &tx,
                scan_id,
                &ChangeRecord {
                    entity: "symbol",
                    entity_id: None,
                    path: Some(d.path.clone()),
                    fqn: Some(d.fqn.clone()),
                    change_type: ChangeType::Added,
                    detail: None,
                    before_hash: None,
                    after_hash: Some(d.body_hash.clone()),
                    commit_sha: commit.clone(),
                },
            )?;
            items.push(ChangeItem {
                entity: "symbol",
                change_type: "added",
                kind: Some(ChangeKind::Added.as_str()),
                path: Some(d.path.clone()),
                fqn: Some(d.fqn.clone()),
                from_fqn: None,
            });
            symbols_changed += 1;
        }

        for (file_id, edges) in &pending_edges {
            if *file_id != 0 {
                Store::replace_edges_for_file(&tx, self.project_id, *file_id, scan_id, edges)?;
            }
        }
        // Tier 3: an added or renamed symbol can resolve edges elsewhere without those
        // files changing, so resolution re-runs over the unresolved set every scan.
        Store::resolve_edges(&tx, self.project_id)?;
        tx.commit().map_err(nexus_store::StoreError::from)?;

        let (files, symbols) = self.store.index_counts(self.project_id)?;
        self.store
            .finish_scan(scan_id, ScanStatus::Ok, files, failed as i64, symbols, None)?;
        self.store.set_baseline(self.project_id, scan_id)?;

        Ok(RescanReport {
            scan_uid: Some(scan_uid),
            baseline: base_rev,
            current: cur_rev,
            unchanged: false,
            forced_full,
            files_changed: changed.len(),
            files_deleted: deleted_paths.len(),
            symbols_changed,
            items,
            files_failed: failed,
            health: if failed > 0 {
                Health::Degraded
            } else {
                Health::Ok
            },
            warnings,
            duration_ms: started.elapsed().as_millis(),
        })
    }

    /// Candidate paths for Tier 1, and whether the set is exhaustive (a full walk, from
    /// which deletions can be inferred) or a git delta (which reports deletions itself).
    fn candidates(
        &self,
        baseline: &nexus_store::Baseline,
        force_full: bool,
        warnings: &mut Vec<String>,
    ) -> (Vec<String>, BTreeSet<String>, bool) {
        // A baseline taken on a dirty tree indexed content that no commit describes, so
        // `git diff <baseline commit>` is not a sufficient candidate set: reverting a file
        // to its committed state leaves git reporting nothing while the index still holds
        // the uncommitted version. The only safe candidate set is then everything — and the
        // stat fast path makes that cheap.
        let dirty_baseline = baseline.dirty;
        if !force_full && !dirty_baseline {
            if let (Some(repo), Some(from)) = (&self.repo, &baseline.commit_sha) {
                match repo.changed_paths_since(from) {
                    Ok(d) => {
                        let changed = d
                            .changed
                            .into_iter()
                            .filter(|p| !walk::is_excluded(p))
                            .collect();
                        let deleted = d
                            .deleted
                            .into_iter()
                            .filter(|p| !walk::is_excluded(p))
                            .collect();
                        return (changed, deleted, false);
                    }
                    Err(VcsError::Unreachable(sha)) => {
                        // Falling back is correct; doing it silently is not. A wrong diff is
                        // far worse than a slow scan.
                        warnings.push(format!(
                            "baseline commit {} is unreachable (force-push, rebase or shallow clone) — fell back to a full walk",
                            Repo::short_sha(&sha)
                        ));
                    }
                    Err(e) => {
                        warnings.push(format!("git diff failed ({e}) — fell back to a full walk"))
                    }
                }
            }
        }
        let walked = walk::walk(&self.root, &[]);
        (
            walked.into_iter().map(|f| f.path).collect(),
            BTreeSet::new(),
            true,
        )
    }

    // ── status ───────────────────────────────────────────────

    pub fn status(&self) -> Result<StatusReport> {
        let (commit, dirty) = self.head();
        let baseline = self.store.baseline(self.project_id)?;
        let (files, symbols) = self.store.index_counts(self.project_id)?;
        let commits_behind = match (&self.repo, &baseline) {
            (Some(r), Some(b)) => b.commit_sha.as_ref().and_then(|s| r.commits_since(s)),
            _ => None,
        };
        let drifted = match &baseline {
            Some(b) => dirty || b.commit_sha != commit || commits_behind.unwrap_or(0) > 0,
            None => true,
        };
        Ok(StatusReport {
            project: self.name(),
            profile: self.load_profile()?,
            baseline: baseline.as_ref().map(|b| Revision {
                scan_uid: Some(b.scan_uid.clone()),
                commit: b.commit_sha.clone(),
                dirty: b.dirty,
            }),
            current: Revision {
                scan_uid: None,
                commit,
                dirty,
            },
            commits_behind,
            scans: self.store.scan_count(self.project_id)?,
            files,
            symbols,
            drifted,
        })
    }

    pub fn changes(&self, entity: Option<&str>) -> Result<Vec<nexus_store::ChangeRow>> {
        let Some(b) = self.store.baseline(self.project_id)? else {
            return Err(EngineError::NoBaseline);
        };
        Ok(self.store.changes_for_scan(b.scan_id, entity)?)
    }

    // ── bugs ─────────────────────────────────────────────────

    /// Run every deterministic detector and reconcile the results with what is already known.
    ///
    /// No model is asked here, so nothing this produces is subject to the 0.75 clamp that
    /// applies to a model's own confidence: both sides of every claim are in the index and
    /// comparing them is a query.
    /// Run one capability over a scope and reconcile its findings with what is known.
    ///
    /// Nexus owns identity, lifecycle and storage; the capability only says what is wrong.
    /// That split is what lets a second capability be a few hundred lines instead of a
    /// re-argument about when a finding is new, recurring, fixed or regressed.
    pub fn analyze(&mut self, capability_id: &str, scope: Scope) -> Result<AnalyzeReport> {
        let started = Instant::now();
        let (commit, _) = self.head();

        let capability =
            self.capabilities
                .get(capability_id)
                .ok_or_else(|| EngineError::UnknownCapability {
                    asked: capability_id.to_string(),
                    known: self.capabilities.ids().join(", "),
                })?;
        let cap_id = capability.id().to_string();
        let prefix = capability.finding_prefix().to_string();

        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        let (scan_uid, scan_id) = (baseline.scan_uid.clone(), baseline.scan_id);

        let symbols: Vec<SymbolFacts> = self
            .store
            .symbol_facts(self.project_id)?
            .into_iter()
            .map(|r| SymbolFacts {
                fqn: r.fqn,
                name: r.name,
                kind: r.kind,
                file: r.file,
                line: r.line,
                visibility: r.visibility,
                parent_fqn: r.parent_fqn,
                annotations: r
                    .annotations_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str(j).ok())
                    .unwrap_or_default(),
            })
            .collect();
        let edges: Vec<EdgeFacts> = self
            .store
            .edge_facts(self.project_id)?
            .into_iter()
            .map(|r| EdgeFacts {
                src_fqn: r.src_fqn,
                dst_fqn: r.dst_fqn,
                dst_hint: r.dst_hint,
                edge_type: r.edge_type,
                resolution: r.resolution,
                line: r.line,
            })
            .collect();
        let files: Vec<FileFacts> = self
            .store
            .file_facts(self.project_id)?
            .into_iter()
            .map(|r| FileFacts {
                path: r.path,
                lang: r.lang,
            })
            .collect();

        // What moved, so a `Changed` scope has something to narrow by. Read from the
        // ledger rather than recomputed: the rescan cascade already worked this out.
        let changed: Vec<ChangedSymbol> = match &scope {
            Scope::Changed { since_scan } => self
                .store
                .changes_for_scan(*since_scan, Some("symbol"))?
                .into_iter()
                // The kind is what a capability decides on — an API break ripples to every
                // caller where a body edit does not — and it is already in the ledger.
                // Hardcoding BODY_CHANGED here made every rule that asks "did the contract
                // move?" permanently unreachable, silently, with no test able to see it
                // because no capability asked until now.
                .filter_map(|(_, change_type, target, detail)| {
                    let fqn = target?;
                    let kind = ChangeKind::from_ledger(&change_type, detail.as_deref())?;
                    Some(ChangedSymbol {
                        path: symbols
                            .iter()
                            .find(|s| s.fqn == fqn)
                            .map(|s| s.file.clone())
                            .unwrap_or_default(),
                        fqn,
                        kind,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };

        // Loaded once and handed over: a capability that re-derived the profile would be
        // re-reading build files the platform already read.
        let profile = self.load_profile().ok().flatten();
        let ctx = ProjectContext::new(&self.root, &symbols, &edges, &files)
            .with_changes(&changed, commit.as_deref())
            .with_profile(profile.as_ref());

        let mut candidates = capability
            .analyze(&ctx, &scope)
            .map_err(|e| EngineError::Capability(e.to_string()))?;
        let examined = ctx.in_scope(&scope).len();

        // A candidate with no checkable evidence is rejected at the boundary rather than
        // down-ranked, whether it came from a rule or from a model. An assertion nobody can
        // verify is not a finding, and storing one lets the next reader mistake it for one.
        let before = candidates.len();
        candidates.retain(|c| !c.evidence.is_empty());
        let rejected = before - candidates.len();

        let mut alternates: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for c in &candidates {
            let Some(anchor) = c.anchor_fqn.as_deref() else {
                continue;
            };
            if alternates.contains_key(anchor) {
                continue;
            }
            alternates.insert(
                anchor.to_string(),
                self.store.old_fqns_for(self.project_id, anchor)?,
            );
        }

        let open_before = self.store.open_findings(self.project_id, &cap_id)?;
        let mut touched: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let (mut new_count, mut recurring, mut regressed) = (0usize, 0usize, 0usize);

        let tx = self.store.transaction()?;
        for c in &candidates {
            let fingerprint = c.fingerprint();
            let alt_fingerprints: Vec<String> = c
                .anchor_fqn
                .as_deref()
                .and_then(|a| alternates.get(a))
                .map(|olds| {
                    olds.iter()
                        .map(|old| {
                            let mut moved = c.clone();
                            moved.anchor_fqn = Some(old.clone());
                            moved.fingerprint()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let status = c.initial_status();
            let anchor_line = c.evidence.first().map(|e| e.line as i64);
            let anchor_file = c.evidence.first().map(|e| e.file.clone());

            let up = Store::upsert_finding(
                &tx,
                self.project_id,
                scan_id,
                &nexus_store::NewFinding {
                    capability: cap_id.clone(),
                    uid_prefix: prefix.clone(),
                    fingerprint,
                    alt_fingerprints,
                    slug: c.slug.clone(),
                    title: c.title.clone(),
                    finding_type: c.finding_type.as_str().to_string(),
                    component: c.component.clone(),
                    severity: c.severity.as_str().to_string(),
                    confidence: c.confidence,
                    status: status.as_str().to_string(),
                    detector: c.detector.clone(),
                    anchor_fqn: c.anchor_fqn.clone(),
                    commit: commit.clone(),
                    capability_data: c.capability_data.as_ref().map(|v| v.to_string()),
                },
            )?;
            touched.insert(up.id);
            if up.is_new {
                new_count += 1;
            } else if up.status == "REGRESSED" {
                regressed += 1;
            } else {
                recurring += 1;
            }

            Store::insert_occurrence(
                &tx,
                up.id,
                scan_id,
                &nexus_store::NewOccurrence {
                    file_path: anchor_file,
                    start_line: anchor_line,
                    status: up.status.clone(),
                    confidence: c.confidence,
                    evidence_json: serde_json::to_string(&c.evidence)?,
                    commit: commit.clone(),
                },
            )?;
        }

        // Closing a finding needs evidence. For a deterministic capability that evidence is:
        // the rule ran again over the same index and did not fire. A narrowed scope did not
        // look everywhere, so it may not close anything it did not examine.
        let mut fixed = 0usize;
        if scope == Scope::Everything {
            for open in &open_before {
                if touched.contains(&open.id) {
                    continue;
                }
                Store::mark_fixed(&tx, open.id, scan_id, commit.as_deref())?;
                fixed += 1;
            }
        }
        tx.commit().map_err(nexus_store::StoreError::from)?;

        let findings = self.findings(Some(&cap_id), None, None)?;
        Ok(AnalyzeReport {
            capability: cap_id,
            scope: scope.describe(),
            scan_uid: Some(scan_uid),
            symbols_examined: examined,
            found: candidates.len(),
            new: new_count,
            recurring,
            regressed,
            fixed,
            rejected,
            findings,
            duration_ms: started.elapsed().as_millis(),
        })
    }

    /// Record a finding produced outside this process.
    ///
    /// This is what LLM independence actually means. Until an agent can write a finding
    /// back, only code compiled into Nexus can produce one — and that, not the absence of
    /// HTTP clients, is what makes a system model-dependent. With this, any model is a
    /// provider and Nexus contains no provider-specific code at all.
    ///
    /// The same evidence rule applies as to a rule-produced finding: a candidate with no
    /// checkable `file:line` is rejected, not down-ranked. A model's confidence is clamped,
    /// because a model may not grade its own work.
    pub fn record_finding(&mut self, capability: &str, mut f: Finding) -> Result<RecordedFinding> {
        if f.evidence.is_empty() {
            return Err(EngineError::NoEvidence);
        }
        let known: Vec<String> = self
            .store
            .live_files(self.project_id)?
            .into_iter()
            .map(|r| r.path)
            .collect();
        // Evidence pointing at a file that is not in the index is not evidence. A model
        // describing a plausible problem in a file that does not exist produces no rows.
        if let Some(bad) = f
            .evidence
            .iter()
            .find(|e| !known.iter().any(|k| k == &e.file))
        {
            return Err(EngineError::UnknownEvidenceFile(bad.file.clone()));
        }
        f.confidence = f.confidence.min(MODEL_CONFIDENCE_CAP);

        let (commit, _) = self.head();
        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        let prefix = self
            .capabilities
            .get(capability)
            .map(|c| c.finding_prefix().to_string())
            .unwrap_or_else(|| capability.to_uppercase().chars().take(3).collect());

        let tx = self.store.transaction()?;
        let up = Store::upsert_finding(
            &tx,
            self.project_id,
            baseline.scan_id,
            &nexus_store::NewFinding {
                capability: capability.to_string(),
                uid_prefix: prefix,
                fingerprint: f.fingerprint(),
                alt_fingerprints: Vec::new(),
                slug: f.slug.clone(),
                title: f.title.clone(),
                finding_type: f.finding_type.as_str().to_string(),
                component: f.component.clone(),
                severity: f.severity.as_str().to_string(),
                confidence: f.confidence,
                status: f.initial_status().as_str().to_string(),
                detector: f.detector.clone(),
                anchor_fqn: f.anchor_fqn.clone(),
                commit: commit.clone(),
                // An agent-recorded finding may carry its own shape too — the write-back
                // path is not a lesser citizen than a compiled capability.
                capability_data: f.capability_data.as_ref().map(|v| v.to_string()),
            },
        )?;
        Store::insert_occurrence(
            &tx,
            up.id,
            baseline.scan_id,
            &nexus_store::NewOccurrence {
                file_path: f.evidence.first().map(|e| e.file.clone()),
                start_line: f.evidence.first().map(|e| e.line as i64),
                status: up.status.clone(),
                confidence: f.confidence,
                evidence_json: serde_json::to_string(&f.evidence)?,
                commit,
            },
        )?;
        tx.commit().map_err(nexus_store::StoreError::from)?;

        Ok(RecordedFinding {
            uid: up.uid,
            is_new: up.is_new,
            status: up.status,
        })
    }

    /// Findings attached to a file, a symbol or a component — "what do we already know
    /// about this code?"
    pub fn findings_for(&self, target: &str) -> Result<Vec<FindingSummary>> {
        Ok(self
            .store
            .findings_for(self.project_id, target)?
            .into_iter()
            .map(to_summary)
            .collect())
    }

    /// Remember something about this project that is not a symbol, an edge or a finding.
    pub fn record_fact(&mut self, f: FactInput) -> Result<()> {
        let baseline = self
            .store
            .baseline(self.project_id)?
            .ok_or(EngineError::NoBaseline)?;
        self.store.record_fact(
            self.project_id,
            baseline.scan_id,
            &nexus_store::NewFact {
                key: f.key,
                scope: f.scope,
                subject: f.subject,
                claim: f.claim,
                source: f.source,
                evidence_json: Some(serde_json::to_string(&f.evidence)?),
                confidence: f.confidence,
            },
        )?;
        Ok(())
    }

    pub fn facts(&self, subject: Option<&str>) -> Result<Vec<Fact>> {
        Ok(self
            .store
            .facts(self.project_id, subject)?
            .into_iter()
            .map(|r| Fact {
                key: r.key,
                scope: r.scope,
                subject: r.subject,
                claim: r.claim,
                source: r.source,
                confidence: r.confidence,
            })
            .collect())
    }

    /// The scan before the current baseline — what `--changed` is measured against.
    pub fn previous_scan_id(&self) -> Result<Option<i64>> {
        Ok(self.store.previous_scan_id(self.project_id)?)
    }

    pub fn capability_list(&self) -> Vec<CapabilityInfo> {
        self.capabilities
            .all()
            .map(|c| CapabilityInfo {
                id: c.id().to_string(),
                finding_prefix: c.finding_prefix().to_string(),
                describes: c.describe().to_string(),
            })
            .collect()
    }

    /// Findings, optionally narrowed to one capability.
    pub fn findings(
        &self,
        capability: Option<&str>,
        status: Option<&str>,
        severity: Option<&str>,
    ) -> Result<Vec<FindingSummary>> {
        Ok(self
            .store
            .findings(
                self.project_id,
                &nexus_store::FindingQuery {
                    status,
                    severity,
                    capability,
                },
            )?
            .into_iter()
            .map(to_summary)
            .collect())
    }

    pub fn finding(&self, uid: &str) -> Result<Option<FindingDetail>> {
        let Some(row) = self
            .store
            .findings(
                self.project_id,
                &nexus_store::FindingQuery {
                    capability: Some("bughunter"),
                    ..Default::default()
                },
            )?
            .into_iter()
            .find(|b| b.uid.eq_ignore_ascii_case(uid))
        else {
            return Ok(None);
        };
        let fingerprint = row.fingerprint.clone();
        let evidence: Vec<CodeRef> = self
            .store
            .finding_evidence(self.project_id, &row.uid)?
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let history = self
            .store
            .finding_history(self.project_id, &row.uid)?
            .into_iter()
            .map(|e| FindingEvent {
                scan_uid: e.scan_uid,
                commit: e.commit,
                status: e.status,
                confidence: e.confidence,
            })
            .collect();
        Ok(Some(FindingDetail {
            summary: to_summary(row),
            fingerprint,
            evidence,
            history,
        }))
    }

    /// Dismiss a finding. A human decision is sticky: a later scan will not re-open it.
    pub fn ignore_finding(&self, uid: &str) -> Result<bool> {
        Ok(self
            .store
            .set_finding_status(self.project_id, uid, "IGNORED")?)
    }

    // ── impact ───────────────────────────────────────────────

    /// Blast radius of a symbol, a file, or a bare name.
    ///
    /// Returns `Ambiguous` rather than picking one of several matches: in the face of
    /// ambiguity, refuse the temptation to guess. That is the same contract the
    /// clarification protocol formalizes for the investigation entry point.
    pub fn impact(&self, q: &ImpactQuery) -> Result<Resolved<ImpactReport>> {
        let matches = self.store.find_symbols(self.project_id, &q.target, 25)?;
        if matches.is_empty() {
            return Ok(Resolved::NotFound {
                target: q.target.clone(),
            });
        }
        // An exact FQN match, or every symbol in one file, is unambiguous.
        let exact: Vec<SymbolRef> = matches
            .iter()
            .filter(|m| m.fqn == q.target)
            .cloned()
            .collect();
        let same_file = matches.iter().all(|m| m.file_path == matches[0].file_path)
            && (q.target.contains('/') || q.target.ends_with(".java"));

        let seeds = if !exact.is_empty() {
            exact
        } else if matches.len() == 1 || same_file {
            matches.clone()
        } else {
            return Ok(Resolved::Ambiguous(
                matches
                    .into_iter()
                    .map(|m| SeedRef {
                        fqn: m.fqn,
                        kind: m.kind,
                        file: m.file_path,
                        line: m.start_line,
                    })
                    .collect(),
            ));
        };

        Ok(Resolved::One(impact::run(
            &self.store,
            self.project_id,
            &seeds,
            q,
        )?))
    }

    /// One symbol in detail, or the candidates when the target is ambiguous.
    pub fn symbol(&self, target: &str) -> Result<Resolved<SymbolDetail>> {
        let mut matches = self.store.find_symbols(self.project_id, target, 25)?;
        if matches.is_empty() {
            // A name that no longer exists may have moved. Consulting aliases before
            // declaring it unknown is what makes a rename survivable for a caller that
            // still knows the old name.
            if let Some(aliased) = self.store.resolve_alias(self.project_id, target)? {
                matches = vec![aliased];
            } else {
                return Ok(Resolved::NotFound {
                    target: target.to_string(),
                });
            }
        }
        let exact: Vec<_> = matches
            .iter()
            .filter(|m| m.fqn == target)
            .cloned()
            .collect();
        let chosen = if let Some(one) = exact.first() {
            one.clone()
        } else if matches.len() == 1 {
            matches[0].clone()
        } else {
            return Ok(Resolved::Ambiguous(
                matches
                    .into_iter()
                    .map(|m| SeedRef {
                        fqn: m.fqn,
                        kind: m.kind,
                        file: m.file_path,
                        line: m.start_line,
                    })
                    .collect(),
            ));
        };

        let source = self.read_lines(&chosen.file_path, chosen.start_line as usize, 80);
        Ok(Resolved::One(SymbolDetail {
            fqn: chosen.fqn.clone(),
            kind: chosen.kind,
            file: chosen.file_path,
            line: chosen.start_line,
            depends_on: self
                .store
                .edges_out(chosen.id)?
                .into_iter()
                .map(|n| Neighbourhood {
                    fqn: n.fqn,
                    edge: n.edge_type.as_str(),
                    resolution: n.resolution.as_str(),
                    confidence: n.confidence,
                })
                .collect(),
            depended_on_by: self
                .store
                .edges_into(chosen.id)?
                .into_iter()
                .map(|n| Neighbourhood {
                    fqn: n.fqn,
                    edge: n.edge_type.as_str(),
                    resolution: n.resolution.as_str(),
                    confidence: n.confidence,
                })
                .collect(),
            source,
        }))
    }

    /// A capped excerpt. Source is expensive context, so it is opt-in and bounded rather
    /// than attached to every result.
    fn read_lines(&self, rel: &str, start: usize, max: usize) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join(rel)).ok()?;
        let lines: Vec<&str> = text
            .lines()
            .skip(start.saturating_sub(1))
            .take(max)
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The questions a person or an agent actually has, each answered from the index.
    ///
    /// This lives here rather than in an adapter because every answer needs more than one
    /// query, and the rule is that a caller needing two `Engine` calls has found a missing
    /// `Engine` method.
    pub fn ask(&self, q: &Question) -> Result<Answer> {
        match q {
            Question::Changed => Ok(Answer::Changed {
                since: self.status()?.baseline.and_then(|b| b.scan_uid),
                symbols: self
                    .changes(Some("symbol"))?
                    .into_iter()
                    .filter_map(|(_, _, t, _)| t)
                    .collect(),
                files: self.changes(Some("file"))?.len(),
            }),

            // "What is affected by this change?" and "Where is this symbol used?" are the
            // same traversal asked from two directions, so they share an answer.
            Question::Affected(target) => {
                let query = ImpactQuery {
                    target: target.clone(),
                    direction: crate::impact::Direction::Reverse,
                    ..Default::default()
                };
                match self.impact(&query)? {
                    Resolved::One(r) => Ok(Answer::Affected {
                        target: target.clone(),
                        crossed_seam: r.crossed_seam,
                        symbols: r
                            .items
                            .into_iter()
                            .map(|i| Affected {
                                fqn: i.fqn,
                                score: i.score,
                                min_confidence: i.min_confidence,
                            })
                            .collect(),
                    }),
                    _ => Ok(Answer::Affected {
                        target: target.clone(),
                        symbols: Vec::new(),
                        crossed_seam: 0,
                    }),
                }
            }

            // "Have we seen this problem before?" — the question worth asking before changing
            // anything, and the one persistent knowledge exists to answer.
            Question::Known(target) => Ok(Answer::Known {
                findings: self.findings_for(target)?,
                facts: self.facts(Some(target))?,
                target: target.clone(),
            }),

            Question::Facts => Ok(Answer::Facts {
                facts: self.facts(None)?,
            }),

            Question::Next => Ok(Answer::Next {
                suggestions: self.suggest()?,
            }),
        }
    }

    /// What to look at next: changed symbols, ranked by how much they affect and by whether
    /// anything has gone wrong there before.
    ///
    /// Both halves are already indexed, so this is a ranking rather than an analysis — which
    /// is the point. Nexus does not need to think about what to examine; it already knows.
    ///
    /// **Two queries per candidate, up to forty candidates.** That is worth naming rather
    /// than leaving to be rediscovered. `impact` is inherently one traversal per seed and has
    /// no batched form. `findings_for` looks batchable and is not: it matches on five
    /// conditions — the occurrence's file path, an exact fqn, two `LIKE` forms over the fqn,
    /// and the component — so collapsing it into one query grouped by component would change
    /// which findings count, and reimplementing all five in Rust would leave two definitions
    /// of "a finding about this symbol" free to disagree. Batching it properly needs a store
    /// method that keeps the matching in SQL, where it already is.
    fn suggest(&self) -> Result<Vec<Suggestion>> {
        let changed: Vec<String> = self
            .changes(Some("symbol"))?
            .into_iter()
            .filter_map(|(_, _, target, _)| target)
            .take(40)
            .collect();

        let mut out = Vec::new();
        for fqn in changed {
            let reach = match self.impact(&ImpactQuery {
                target: fqn.clone(),
                ..Default::default()
            })? {
                Resolved::One(r) => r.items.len(),
                _ => 0,
            };
            let prior = self.findings_for(&fqn)?.len();
            // Reach is the cost of being wrong; prior findings are evidence that this code
            // has been wrong before. Neither alone is a good reason to look.
            let score = reach as f64 + prior as f64 * 3.0;
            if score <= 0.0 {
                continue;
            }
            out.push(Suggestion {
                why: match (reach, prior) {
                    (r, 0) => format!("changed, and {r} symbols depend on it"),
                    (0, p) => format!("changed, and {p} findings already exist here"),
                    (r, p) => {
                        format!("changed, {r} symbols depend on it, {p} findings already here")
                    }
                },
                target: fqn,
                score,
            });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(10);
        Ok(out)
    }

    pub fn graph(&self) -> Result<GraphReport> {
        let counts = self.store.edge_counts(self.project_id)?;
        Ok(GraphReport {
            edges_total: counts.total,
            edges_resolved: counts.resolved,
            edges_external: counts.external,
            edges_sibling: counts.sibling,
            by_resolution: self.store.edges_by_resolution(self.project_id)?,
        })
    }

    // ── doctor ───────────────────────────────────────────────

    pub fn doctor(&self) -> Result<Vec<Check>> {
        let mut checks = Vec::new();

        checks.push(match &self.repo {
            Some(r) => Check {
                name: "git",
                level: "ok",
                detail: match r.head_sha()? {
                    Some(sha) => format!("repository at HEAD {}", Repo::short_sha(&sha)),
                    None => "repository with no commits yet".into(),
                },
                remedy: None,
            },
            None => Check {
                name: "git",
                level: "warn",
                detail: "not a git repository — every rescan falls back to a full walk".into(),
                remedy: Some("git init".into()),
            },
        });

        let v = self.store.schema_version()?;
        checks.push(Check {
            name: "database",
            level: if v == nexus_store::SCHEMA_VERSION {
                "ok"
            } else {
                "error"
            },
            detail: format!(
                "{NEXUS_DIR}/{DB_FILE}, schema {v} (binary supports {})",
                nexus_store::SCHEMA_VERSION
            ),
            remedy: (v != nexus_store::SCHEMA_VERSION).then(|| "upgrade bughunter".into()),
        });

        let langs = self.registry.tool_versions();
        checks.push(Check {
            name: "languages",
            level: if self.registry.is_empty() {
                "error"
            } else {
                "ok"
            },
            detail: langs
                .iter()
                .map(|(k, v)| format!("{} ({v})", k.trim_start_matches("grammar:")))
                .collect::<Vec<_>>()
                .join(", "),
            remedy: None,
        });

        if let Some(p) = self.load_profile()? {
            checks.push(Check {
                name: "build system",
                level: if p.build_system.is_some() {
                    "ok"
                } else {
                    "warn"
                },
                detail: p
                    .build_system
                    .clone()
                    .unwrap_or_else(|| "not detected".into()),
                remedy: p
                    .build_system
                    .is_none()
                    .then(|| "set build_system in .nexus/config.toml".into()),
            });
            let unanalyzed: Vec<&str> = p
                .languages
                .iter()
                .filter(|l| !l.analyzed && l.files >= 5)
                .map(|l| l.lang.as_str())
                .collect();
            if !unanalyzed.is_empty() {
                checks.push(Check {
                    name: "coverage",
                    level: "warn",
                    detail: format!(
                        "{} present but not analyzed in this build",
                        unanalyzed.join(", ")
                    ),
                    remedy: Some("those analyzers land in V1 — see docs/roadmap.md".into()),
                });
            }
        }

        match self.store.baseline(self.project_id)? {
            None => checks.push(Check {
                name: "baseline",
                level: "warn",
                detail: "no baseline yet".into(),
                remedy: Some("bughunter scan".into()),
            }),
            Some(b) => {
                let behind = self
                    .repo
                    .as_ref()
                    .zip(b.commit_sha.as_ref())
                    .and_then(|(r, s)| r.commits_since(s))
                    .unwrap_or(0);
                checks.push(Check {
                    name: "baseline",
                    level: if behind > 0 { "warn" } else { "ok" },
                    detail: if behind > 0 {
                        format!("{} is {behind} commits behind HEAD", b.scan_uid)
                    } else {
                        format!("{} is current", b.scan_uid)
                    },
                    remedy: (behind > 0).then(|| "bughunter rescan".into()),
                });
            }
        }

        let bytes = dir_size(&self.root.join(NEXUS_DIR));
        checks.push(Check {
            name: "disk",
            level: "ok",
            detail: format!("{NEXUS_DIR}/ {}", human_bytes(bytes)),
            remedy: None,
        });
        Ok(checks)
    }
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
    }
}

/// Which hash moved, and therefore how far the change ripples. ADR-010.
fn symbol_change(old: &nexus_store::SymbolRow, new: &NewSymbol) -> Option<ChangeKind> {
    let sig_text_changed = old.signature.as_deref().unwrap_or_default()
        != new.signature.as_deref().unwrap_or_default();
    let old_anns: Vec<String> = old
        .annotations_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let ann_changed = old_anns != new.annotations;
    let body_changed = old.body_hash != new.body_hash;

    match (sig_text_changed, ann_changed, body_changed) {
        (false, false, false) => None,
        (true, _, true) => Some(ChangeKind::ApiAndBodyChanged),
        (true, _, false) => Some(ChangeKind::ApiChanged),
        // Annotations carry no signature diff and frequently matter more: removing
        // @Transactional changes nothing a compiler notices and everything about correctness.
        (false, true, _) => Some(ChangeKind::ContractChanged),
        (false, false, true) => Some(ChangeKind::BodyChanged),
    }
}

/// One symbol that appeared or disappeared during a rescan.
#[derive(Debug, Clone)]
struct SymbolDelta {
    fqn: String,
    path: String,
    name: String,
    sig_hash: String,
    body_hash: String,
    old_id: Option<SymbolId>,
}

/// Pair disappearances with appearances that are the same symbol under a new name.
///
/// The key is `(name, sig_hash, body_hash)`: a package move, a class move or a directory
/// rename changes the FQN and nothing else, so all three survive. A method whose body also
/// changed in the same commit will not match, which is the right call — that is a delete
/// and an add as far as identity goes, and guessing otherwise would attach an old bug
/// history to code that is no longer the same code.
///
/// Only unambiguous 1:1 matches count. Boilerplate accessors and generated equals/hashCode
/// collide on this key constantly, and carrying identity to an arbitrary one of five
/// candidates is worse than reporting a delete and an add.
fn resolve_symbol_renames(
    appeared: &[SymbolDelta],
    vanished: &[SymbolDelta],
) -> Vec<(usize, usize)> {
    let key = |d: &SymbolDelta| (d.name.clone(), d.sig_hash.clone(), d.body_hash.clone());

    let mut new_by_key: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
    for (i, d) in appeared.iter().enumerate() {
        new_by_key.entry(key(d)).or_default().push(i);
    }
    let mut old_by_key: BTreeMap<(String, String, String), Vec<usize>> = BTreeMap::new();
    for (i, d) in vanished.iter().enumerate() {
        old_by_key.entry(key(d)).or_default().push(i);
    }

    let mut out = Vec::new();
    for (k, olds) in &old_by_key {
        if olds.len() != 1 {
            continue;
        }
        if let Some(news) = new_by_key.get(k) {
            if news.len() == 1 && appeared[news[0]].fqn != vanished[olds[0]].fqn {
                out.push((olds[0], news[0]));
            }
        }
    }
    out
}

fn detect_renames(
    stored: &BTreeMap<String, nexus_store::FileRow>,
    added: &[HashedFile],
    deleted: &mut BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut by_hash: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for path in deleted.iter() {
        if let Some(row) = stored.get(path) {
            by_hash
                .entry(row.content_hash.as_str())
                .or_default()
                .push(path);
        }
    }
    let mut renames = BTreeMap::new();
    for a in added {
        if stored.contains_key(&a.path) {
            continue;
        }
        if let Some(olds) = by_hash.get(a.content_hash.as_str()) {
            // Only an unambiguous 1:1 match counts. Two identical files moving at once is
            // not a rename anyone can attribute, and guessing would carry symbol identity
            // to the wrong place.
            if olds.len() == 1 {
                renames.insert(olds[0].clone(), a.path.clone());
            }
        }
    }
    renames
}

fn to_summary(r: nexus_store::FindingRow) -> FindingSummary {
    FindingSummary {
        capability: r.capability,
        uid: r.uid,
        slug: r.slug,
        title: r.title,
        finding_type: r.finding_type,
        component: r.component,
        severity: r.severity,
        confidence: r.confidence,
        status: r.status,
        detector: r.detector,
        file: r.file,
        line: r.line,
        introduced_commit: r.introduced_commit,
        fixed_commit: r.fixed_commit,
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
