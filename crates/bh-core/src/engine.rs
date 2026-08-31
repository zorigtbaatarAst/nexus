//! `Engine` — the single public API of BugHunter.
//!
//! Every CLI command and (from V1) every MCP tool is one call into this facade. Boundary
//! rule: this crate must not depend on `bh-mcp`, `bh-cli`, or any concrete AI provider.
//! `tests/boundaries.rs` fails the build otherwise.

use crate::detect::Detector;
use crate::impact::{self, ImpactQuery};
use crate::report::*;
use crate::walk::{self, HashedFile};
use bh_lang::{ParsedFile, Registry, SourceFile};
use bh_lang_java::JavaAnalyzer;
use bh_lang_ts::TypeScriptAnalyzer;
use bh_store::{ChangeRecord, NewEdge, NewSymbol, Store, SymbolRef};
use bh_types::*;
use bh_vcs::{Repo, VcsError};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] bh_store::StoreError),
    #[error(transparent)]
    Vcs(#[from] bh_vcs::VcsError),
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
    #[error("no baseline for this project\n  run `bughunter scan` first")]
    NoBaseline,
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub const BH_DIR: &str = ".bughunter";

pub struct Engine {
    root: PathBuf,
    store: Store,
    repo: Option<Repo>,
    registry: Registry,
    project_id: ProjectId,
}

impl Engine {
    /// Create `.bughunter/`, migrate the database, and record what this project is.
    pub fn init(root: &Path) -> Result<(Self, Profile)> {
        let root = canonical(root);
        let dir = root.join(BH_DIR);
        std::fs::create_dir_all(dir.join("cache")).map_err(|e| EngineError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;

        // Self-managing: the store, caches, generated tests and audit log are local and
        // disposable; config and policy are committed team intent.
        write_if_absent(
            &dir.join(".gitignore"),
            "bughunter.db\nbughunter.db-wal\nbughunter.db-shm\ncache/\ngenerated-tests/\naudit.log\n",
        )?;
        write_if_absent(&dir.join("config.toml"), DEFAULT_CONFIG)?;
        write_if_absent(&dir.join("policy.toml"), DEFAULT_POLICY)?;

        let mut engine = Self::open_at(&root)?;
        let profile = engine.detect()?;
        engine.save_profile(&profile)?;
        Ok((engine, profile))
    }

    pub fn open(root: &Path) -> Result<Self> {
        let root = canonical(root);
        if !root.join(BH_DIR).join("bughunter.db").exists() {
            return Err(EngineError::NotInitialized(root.display().to_string()));
        }
        Self::open_at(&root)
    }

    fn open_at(root: &Path) -> Result<Self> {
        let store = Store::open(&root.join(BH_DIR).join("bughunter.db"))?;
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
            .register(Box::new(TypeScriptAnalyzer::new()));

        Ok(Engine {
            root: root.to_path_buf(),
            store,
            repo,
            registry,
            project_id,
        })
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
        map.insert("schema".into(), bh_store::SCHEMA_VERSION.to_string());
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
        tx.commit().map_err(bh_store::StoreError::from)?;
        if resolve.unresolved > 0 {
            warnings.push(format!(
                "{} edges point inside the project but matched no symbol (overloads, inherited methods)",
                resolve.unresolved
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
        let stored: BTreeMap<String, bh_store::FileRow> = self
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

        // Read every symbol set we will need *before* opening the transaction: rusqlite's
        // Transaction holds a mutable borrow of the connection for its entire lifetime.
        let mut old_by_path: BTreeMap<String, BTreeMap<String, bh_store::SymbolRow>> =
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
                    Store::insert_change(
                        &tx,
                        scan_id,
                        &ChangeRecord {
                            entity: "symbol",
                            entity_id: Some(s.id),
                            path: Some(path.clone()),
                            fqn: Some(s.fqn.clone()),
                            change_type: ChangeType::Deleted,
                            detail: None,
                            before_hash: Some(s.body_hash.clone()),
                            after_hash: None,
                            commit_sha: commit.clone(),
                        },
                    )?;
                    items.push(ChangeItem {
                        entity: "symbol",
                        change_type: "deleted",
                        kind: Some(ChangeKind::Deleted.as_str()),
                        path: Some(path.clone()),
                        fqn: Some(s.fqn),
                    });
                    symbols_changed += 1;
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
            });

            // ── Tier 2: symbol-level diff ──
            if let Some(new_symbols) = symbols {
                for s in &new_symbols {
                    let kind = match old_symbols.get(&s.fqn) {
                        None => Some(ChangeKind::Added),
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
                    });
                    symbols_changed += 1;
                }
                let new_fqns: BTreeSet<&str> = new_symbols.iter().map(|s| s.fqn.as_str()).collect();
                for (fqn, old) in &old_symbols {
                    if new_fqns.contains(fqn.as_str()) {
                        continue;
                    }
                    Store::insert_change(
                        &tx,
                        scan_id,
                        &ChangeRecord {
                            entity: "symbol",
                            entity_id: Some(old.id),
                            path: Some(file.path.clone()),
                            fqn: Some(fqn.clone()),
                            change_type: ChangeType::Deleted,
                            detail: None,
                            before_hash: Some(old.body_hash.clone()),
                            after_hash: None,
                            commit_sha: commit.clone(),
                        },
                    )?;
                    items.push(ChangeItem {
                        entity: "symbol",
                        change_type: "deleted",
                        kind: Some(ChangeKind::Deleted.as_str()),
                        path: Some(file.path.clone()),
                        fqn: Some(fqn.clone()),
                    });
                    symbols_changed += 1;
                }
                Store::replace_symbols(&tx, self.project_id, file_id, scan_id, &new_symbols)?;
            }
            if let Some(last) = pending_edges.last_mut() {
                if last.0 == 0 {
                    last.0 = file_id;
                }
            }
        }
        for (file_id, edges) in &pending_edges {
            if *file_id != 0 {
                Store::replace_edges_for_file(&tx, self.project_id, *file_id, scan_id, edges)?;
            }
        }
        // Tier 3: an added or renamed symbol can resolve edges elsewhere without those
        // files changing, so resolution re-runs over the unresolved set every scan.
        Store::resolve_edges(&tx, self.project_id)?;
        tx.commit().map_err(bh_store::StoreError::from)?;

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
        baseline: &bh_store::Baseline,
        force_full: bool,
        warnings: &mut Vec<String>,
    ) -> (Vec<String>, BTreeSet<String>, bool) {
        if !force_full {
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

    pub fn changes(&self, entity: Option<&str>) -> Result<Vec<bh_store::ChangeRow>> {
        let Some(b) = self.store.baseline(self.project_id)? else {
            return Err(EngineError::NoBaseline);
        };
        Ok(self.store.changes_for_scan(b.scan_id, entity)?)
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
            return Ok(Resolved::NotFound(q.target.clone()));
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

    pub fn graph(&self) -> Result<GraphReport> {
        let (total, resolved, external) = self.store.edge_counts(self.project_id)?;
        Ok(GraphReport {
            edges_total: total,
            edges_resolved: resolved,
            edges_external: external,
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
            level: if v == bh_store::SCHEMA_VERSION {
                "ok"
            } else {
                "error"
            },
            detail: format!(
                "{BH_DIR}/bughunter.db, schema {v} (binary supports {})",
                bh_store::SCHEMA_VERSION
            ),
            remedy: (v != bh_store::SCHEMA_VERSION).then(|| "upgrade bughunter".into()),
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
                    .then(|| "set build_system in .bughunter/config.toml".into()),
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

        let bytes = dir_size(&self.root.join(BH_DIR));
        checks.push(Check {
            name: "disk",
            level: "ok",
            detail: format!("{BH_DIR}/ {}", human_bytes(bytes)),
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

fn to_new_edge(e: &bh_lang::RawEdge) -> NewEdge {
    NewEdge {
        src_fqn: e.src_fqn.clone(),
        dst_hint: e.dst_hint.clone(),
        edge_type: e.edge_type,
        site_line: e.site_line,
    }
}

fn to_new_symbol(s: &bh_lang::RawSymbol) -> NewSymbol {
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
fn symbol_change(old: &bh_store::SymbolRow, new: &NewSymbol) -> Option<ChangeKind> {
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

fn detect_renames(
    stored: &BTreeMap<String, bh_store::FileRow>,
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
