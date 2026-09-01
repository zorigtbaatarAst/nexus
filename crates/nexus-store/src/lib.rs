//! The SQLite knowledge store.
//!
//! Boundary rule: **this is the only crate in the workspace that contains SQL.** A schema
//! change therefore has exactly one blast radius. `nexus-lang-*` and `nexus-mcp` may not depend
//! on it at all; `tests/boundaries.rs` fails the build if they do.
//!
//! Callers read through the `live_*` views rather than the base tables. Soft-deletes mean
//! nearly every query needs `deleted = 0`, and forgetting it is a silent wrong answer —
//! so the filter lives in the schema, not in each call site.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use nexus_types::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 4;
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_graphql_seam.sql")),
    (3, include_str!("../migrations/0003_findings.sql")),
    (4, include_str!("../migrations/0004_sibling_resolution.sql")),
];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "database schema is version {found}, but this binary understands at most {max}. \
         Upgrade bughunter, or point at a different project."
    )]
    SchemaTooNew { found: u32, max: u32 },
    #[error("no project registered at {0}")]
    NoProject(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// `(entity, change_type, target, detail)` — one recorded change, denormalized for reading.
pub type ChangeRow = (String, String, Option<String>, Option<String>);
/// `(languages_json, frameworks_json, build_system, package_manager, databases_json, containers_json)`
pub type ProfileRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

// ─────────────────────────── row shapes ───────────────────────────

#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: FileId,
    pub path: String,
    pub lang: Option<String>,
    pub content_hash: String,
    pub size_bytes: i64,
    pub loc: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub parse_status: String,
}

#[derive(Debug, Clone)]
pub struct SymbolRow {
    pub id: SymbolId,
    pub kind: String,
    pub name: String,
    pub fqn: String,
    pub signature: Option<String>,
    pub start_line: i64,
    pub end_line: i64,
    pub sig_hash: String,
    pub body_hash: String,
    pub annotations_json: Option<String>,
}

/// A symbol as produced by an analyzer, before it has an id.
#[derive(Debug, Clone)]
pub struct NewSymbol {
    pub kind: SymbolKind,
    pub name: String,
    pub fqn: String,
    pub parent_fqn: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub sig_hash: String,
    pub body_hash: String,
    pub annotations: Vec<String>,
}

/// An edge as an analyzer produced it, before resolution.
#[derive(Debug, Clone)]
pub struct NewEdge {
    pub src_fqn: String,
    pub dst_hint: String,
    pub edge_type: EdgeType,
    pub site_line: u32,
}

/// One step of a graph traversal, with everything an impact report needs to explain itself.
#[derive(Debug, Clone)]
pub struct Neighbour {
    pub symbol_id: SymbolId,
    pub fqn: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: i64,
    pub edge_type: EdgeType,
    pub resolution: Resolution,
    pub confidence: f64,
    pub site_line: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct EdgeCounts {
    pub total: i64,
    pub resolved: i64,
    pub external: i64,
    pub sibling: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ResolveStats {
    pub total: usize,
    pub exact: usize,
    pub contract: usize,
    pub heuristic: usize,
    pub ambiguous: usize,
    /// Genuinely outside the index: a third-party library. Not a failure — counting it as
    /// one makes the resolution rate a lie. ADR-017.
    pub external: usize,
    /// Code this project owns that was not scanned — a sibling module of the same
    /// monorepo. Outside the index like `external`, but for a reason the caller can fix,
    /// and unlike a library it is code an edit here can actually break. Conflating the two
    /// is what lets an agent read "external" as "not my problem" about a module you own.
    pub sibling: usize,
    pub unresolved: usize,
    /// The package root the sibling classification was made against, so a caller can name
    /// it rather than telling someone their scan is incomplete without saying how.
    pub owner: Option<String>,
}

impl ResolveStats {
    /// Edges that could in principle have resolved, i.e. excluding external targets.
    pub fn in_scope(&self) -> usize {
        self.total - self.external
    }
    pub fn resolved(&self) -> usize {
        self.exact + self.contract + self.heuristic + self.ambiguous
    }
}

#[derive(Debug, Clone)]
pub struct SymbolRef {
    pub id: SymbolId,
    pub fqn: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: i64,
}

#[derive(Debug, Clone)]
pub struct SymbolFactRow {
    pub fqn: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub visibility: Option<String>,
    pub parent_fqn: Option<String>,
    pub annotations_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EdgeFactRow {
    pub src_fqn: String,
    pub dst_fqn: Option<String>,
    pub dst_hint: Option<String>,
    pub edge_type: String,
    pub resolution: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct FileFactRow {
    pub path: String,
    pub lang: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewFinding {
    /// Which capability produced this. Part of the uniqueness key: two capabilities may
    /// legitimately flag the same line for different reasons, and collapsing those would
    /// lose one of them.
    pub capability: String,
    /// Display-id prefix, e.g. `BUG`. A developer should never have to ask which subsystem
    /// a number came from.
    pub uid_prefix: String,
    pub fingerprint: String,
    /// Fingerprints this finding would have had under the anchor's previous names.
    pub alt_fingerprints: Vec<String>,
    pub slug: String,
    pub title: String,
    pub finding_type: String,
    pub component: String,
    pub severity: String,
    pub confidence: f64,
    pub status: String,
    pub detector: String,
    pub anchor_fqn: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewOccurrence {
    pub file_path: Option<String>,
    pub start_line: Option<i64>,
    pub status: String,
    pub confidence: f64,
    pub evidence_json: String,
    pub commit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FindingUpsert {
    pub id: i64,
    pub uid: String,
    pub is_new: bool,
    pub previous_status: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct OpenFindingRow {
    pub capability: String,
    pub id: i64,
    pub uid: String,
    pub fingerprint: String,
    pub detector: String,
    pub status: String,
    pub file_path: Option<String>,
}

/// Filters for listing findings. A struct rather than three positional options: the next
/// capability adds a filter and every call site would otherwise change.
#[derive(Debug, Clone, Default)]
pub struct FindingQuery<'a> {
    pub status: Option<&'a str>,
    pub severity: Option<&'a str>,
    pub capability: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct FindingRow {
    pub capability: String,
    pub uid: String,
    pub fingerprint: String,
    pub slug: String,
    pub title: String,
    pub finding_type: String,
    pub component: Option<String>,
    pub severity: String,
    pub confidence: f64,
    pub status: String,
    pub detector: String,
    pub introduced_commit: Option<String>,
    pub fixed_commit: Option<String>,
    pub file: Option<String>,
    pub line: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FindingEventRow {
    pub scan_uid: String,
    pub commit: Option<String>,
    pub status: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct NewFact {
    pub key: String,
    pub scope: String,
    pub subject: Option<String>,
    pub claim: String,
    pub source: String,
    pub evidence_json: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct FactRow {
    pub key: String,
    pub scope: String,
    pub subject: Option<String>,
    pub claim: String,
    pub source: String,
    pub confidence: f64,
    pub evidence_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Baseline {
    pub scan_id: ScanId,
    pub scan_uid: String,
    pub commit_sha: Option<String>,
    pub working_tree_hash: String,
    pub dirty: bool,
    pub set_at: String,
    pub tool_versions_json: String,
}

#[derive(Debug, Clone)]
pub struct ChangeRecord {
    pub entity: &'static str,
    pub entity_id: Option<i64>,
    pub path: Option<String>,
    pub fqn: Option<String>,
    pub change_type: ChangeType,
    pub detail: Option<&'static str>,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub commit_sha: Option<String>,
}

// ─────────────────────────── store ───────────────────────────

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// `foreign_keys` is per-connection, not persisted with the file — a fact that is very
    /// easy to forget and produces orphan rows that nothing complains about.
    fn configure(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "cache_size", -65536)?;
        conn.pragma_update(None, "mmap_size", 268_435_456i64)?;
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;

        // The version is read *inside* an immediate transaction, not before one.
        //
        // Reading it first and then opening a transaction lets two processes starting on the
        // same fresh project both see version 0 and both apply migration 1 — the second one
        // failing with "table projects already exists". SQLite serializes the writes but not
        // the decision to write, so the decision has to happen under the write lock.
        loop {
            let tx = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let current: u32 = tx.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )?;

            if current > SCHEMA_VERSION {
                // Refuse rather than guess at an unknown schema. Errors should never pass
                // silently, least of all in the store.
                return Err(StoreError::SchemaTooNew {
                    found: current,
                    max: SCHEMA_VERSION,
                });
            }

            let Some((version, sql)) = MIGRATIONS.iter().find(|(v, _)| *v > current) else {
                return Ok(());
            };
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, now()],
            )?;
            tx.commit()?;
        }
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?)
    }

    // ── project ──────────────────────────────────────────────

    pub fn ensure_project(&self, root: &str, name: &str, vcs: &str) -> Result<ProjectId> {
        self.conn.execute(
            "INSERT INTO projects (root_path, name, vcs, schema_version, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(root_path) DO UPDATE SET name = excluded.name, vcs = excluded.vcs",
            params![root, name, vcs, SCHEMA_VERSION, now()],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            params![root],
            |r| r.get(0),
        )?)
    }

    pub fn project_id(&self, root: &str) -> Result<ProjectId> {
        self.conn
            .query_row(
                "SELECT id FROM projects WHERE root_path = ?1",
                params![root],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NoProject(root.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_profile(
        &self,
        project_id: ProjectId,
        languages_json: &str,
        frameworks_json: &str,
        build_system: Option<&str>,
        package_manager: Option<&str>,
        databases_json: &str,
        containers_json: &str,
        entrypoints_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_profile
               (project_id, languages_json, frameworks_json, build_system, package_manager,
                databases_json, containers_json, entrypoints_json, detected_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(project_id) DO UPDATE SET
               languages_json = excluded.languages_json,
               frameworks_json = excluded.frameworks_json,
               build_system = excluded.build_system,
               package_manager = excluded.package_manager,
               databases_json = excluded.databases_json,
               containers_json = excluded.containers_json,
               entrypoints_json = excluded.entrypoints_json,
               detected_at = excluded.detected_at",
            params![
                project_id,
                languages_json,
                frameworks_json,
                build_system,
                package_manager,
                databases_json,
                containers_json,
                entrypoints_json,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn load_profile(&self, project_id: ProjectId) -> Result<Option<ProfileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT languages_json, frameworks_json, build_system, package_manager,
                        databases_json, containers_json
                 FROM project_profile WHERE project_id = ?1",
                params![project_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?)
    }

    // ── scans ────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn begin_scan(
        &self,
        project_id: ProjectId,
        kind: ScanKind,
        parent_scan_id: Option<ScanId>,
        commit_sha: Option<&str>,
        working_tree_hash: &str,
        dirty: bool,
        tool_versions_json: &str,
    ) -> Result<(ScanId, String)> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM scans WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?;
        let scan_uid = format!("scan-{:03}", n + 1);
        self.conn.execute(
            "INSERT INTO scans (project_id, scan_uid, kind, parent_scan_id, commit_sha,
                                working_tree_hash, dirty, status, tool_versions_json, started_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'running',?8,?9)",
            params![
                project_id,
                scan_uid,
                kind.as_str(),
                parent_scan_id,
                commit_sha,
                working_tree_hash,
                dirty as i64,
                tool_versions_json,
                now()
            ],
        )?;
        Ok((self.conn.last_insert_rowid(), scan_uid))
    }

    pub fn finish_scan(
        &self,
        scan_id: ScanId,
        status: ScanStatus,
        files_scanned: i64,
        files_failed: i64,
        symbols_indexed: i64,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET status=?2, files_scanned=?3, files_failed=?4,
                              symbols_indexed=?5, finished_at=?6, error=?7
             WHERE id = ?1",
            params![
                scan_id,
                status.as_str(),
                files_scanned,
                files_failed,
                symbols_indexed,
                now(),
                error
            ],
        )?;
        Ok(())
    }

    pub fn set_baseline(&self, project_id: ProjectId, scan_id: ScanId) -> Result<()> {
        self.conn.execute(
            "INSERT INTO baselines (project_id, scan_id, set_at) VALUES (?1,?2,?3)
             ON CONFLICT(project_id) DO UPDATE SET scan_id = excluded.scan_id, set_at = excluded.set_at",
            params![project_id, scan_id, now()],
        )?;
        Ok(())
    }

    pub fn baseline(&self, project_id: ProjectId) -> Result<Option<Baseline>> {
        Ok(self
            .conn
            .query_row(
                "SELECT s.id, s.scan_uid, s.commit_sha, s.working_tree_hash, s.dirty,
                        b.set_at, s.tool_versions_json
                 FROM baselines b JOIN scans s ON s.id = b.scan_id
                 WHERE b.project_id = ?1",
                params![project_id],
                |r| {
                    Ok(Baseline {
                        scan_id: r.get(0)?,
                        scan_uid: r.get(1)?,
                        commit_sha: r.get(2)?,
                        working_tree_hash: r.get(3)?,
                        dirty: r.get::<_, i64>(4)? != 0,
                        set_at: r.get(5)?,
                        tool_versions_json: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// The most recent completed scan. `--changed` narrows to what it recorded.
    pub fn previous_scan_id(&self, project_id: ProjectId) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM scans WHERE project_id = ?1 AND status = 'ok'
                 ORDER BY id DESC LIMIT 1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn scan_count(&self, project_id: ProjectId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM scans WHERE project_id = ?1 AND status = 'ok'",
            params![project_id],
            |r| r.get(0),
        )?)
    }

    // ── files ────────────────────────────────────────────────

    pub fn live_files(&self, project_id: ProjectId) -> Result<Vec<FileRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, lang, content_hash, size_bytes, loc, mtime_ns, parse_status
             FROM live_files WHERE project_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(FileRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    lang: r.get(2)?,
                    content_hash: r.get(3)?,
                    size_bytes: r.get(4)?,
                    loc: r.get(5)?,
                    mtime_ns: r.get(6)?,
                    parse_status: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }

    /// Insert or refresh one file row. Returns its id and whether the content changed.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_file(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        scan_id: ScanId,
        path: &str,
        lang: Option<&str>,
        content_hash: &str,
        size_bytes: i64,
        loc: Option<i64>,
        mtime_ns: Option<i64>,
        parse_status: ParseStatus,
        parse_error: Option<&str>,
    ) -> Result<FileId> {
        tx.execute(
            "INSERT INTO files (project_id, path, lang, content_hash, size_bytes, loc, mtime_ns,
                                parse_status, parse_error, first_seen_scan_id, last_seen_scan_id, deleted)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,0)
             ON CONFLICT(project_id, path) DO UPDATE SET
               lang=excluded.lang, content_hash=excluded.content_hash,
               size_bytes=excluded.size_bytes, loc=excluded.loc, mtime_ns=excluded.mtime_ns,
               parse_status=excluded.parse_status, parse_error=excluded.parse_error,
               last_seen_scan_id=excluded.last_seen_scan_id, deleted=0",
            params![
                project_id, path, lang, content_hash, size_bytes, loc, mtime_ns,
                parse_status.as_str(), parse_error, scan_id
            ],
        )?;
        Ok(tx.query_row(
            "SELECT id FROM files WHERE project_id = ?1 AND path = ?2",
            params![project_id, path],
            |r| r.get(0),
        )?)
    }

    /// Soft-delete. Rows are never removed: `changes` and `finding_occurrences` reference them,
    /// and history must not develop holes.
    pub fn mark_file_deleted(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        path: &str,
        scan_id: ScanId,
    ) -> Result<()> {
        tx.execute(
            "UPDATE files SET deleted = 1, last_seen_scan_id = ?3
             WHERE project_id = ?1 AND path = ?2",
            params![project_id, path, scan_id],
        )?;
        tx.execute(
            "UPDATE symbols SET deleted = 1, last_seen_scan_id = ?2
             WHERE file_id IN (SELECT id FROM files WHERE project_id = ?1 AND path = ?3)",
            params![project_id, scan_id, path],
        )?;
        Ok(())
    }

    // ── symbols ──────────────────────────────────────────────

    pub fn symbols_for_file(&self, file_id: FileId) -> Result<Vec<SymbolRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, name, fqn, signature, start_line, end_line,
                    sig_hash, body_hash, annotations_json
             FROM live_symbols WHERE file_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![file_id], |r| {
                Ok(SymbolRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    fqn: r.get(3)?,
                    signature: r.get(4)?,
                    start_line: r.get(5)?,
                    end_line: r.get(6)?,
                    sig_hash: r.get(7)?,
                    body_hash: r.get(8)?,
                    annotations_json: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Replace the symbol set of one file. Symbols absent from `symbols` are soft-deleted.
    pub fn replace_symbols(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        file_id: FileId,
        scan_id: ScanId,
        symbols: &[NewSymbol],
    ) -> Result<usize> {
        let keep: Vec<&str> = symbols.iter().map(|s| s.fqn.as_str()).collect();

        // Soft-delete what this file no longer defines.
        let mut existing = tx.prepare("SELECT id, fqn FROM live_symbols WHERE file_id = ?1")?;
        let gone: Vec<i64> = existing
            .query_map(params![file_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, fqn)| !keep.contains(&fqn.as_str()))
            .map(|(id, _)| id)
            .collect();
        drop(existing);
        for id in gone {
            tx.execute(
                "UPDATE symbols SET deleted = 1, last_seen_scan_id = ?2 WHERE id = ?1",
                params![id, scan_id],
            )?;
        }

        // Containers come first in source order, so a parent is always inserted before its
        // children and `parent_id` resolves in a single pass.
        for s in symbols {
            let parent_id: Option<i64> = match &s.parent_fqn {
                Some(p) => tx
                    .query_row(
                        "SELECT id FROM symbols WHERE project_id = ?1 AND fqn = ?2",
                        params![project_id, p],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let annotations = if s.annotations.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&s.annotations)?)
            };
            tx.execute(
                "INSERT INTO symbols (project_id, file_id, parent_id, kind, name, fqn, signature,
                                      visibility, start_line, end_line, sig_hash, body_hash,
                                      annotations_json, first_seen_scan_id, last_seen_scan_id, deleted)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14,0)
                 ON CONFLICT(project_id, fqn) DO UPDATE SET
                   file_id=excluded.file_id, parent_id=excluded.parent_id, kind=excluded.kind,
                   name=excluded.name, signature=excluded.signature, visibility=excluded.visibility,
                   start_line=excluded.start_line, end_line=excluded.end_line,
                   sig_hash=excluded.sig_hash, body_hash=excluded.body_hash,
                   annotations_json=excluded.annotations_json,
                   last_seen_scan_id=excluded.last_seen_scan_id, deleted=0",
                params![
                    project_id, file_id, parent_id, s.kind.as_str(), s.name, s.fqn, s.signature,
                    s.visibility, s.start_line as i64, s.end_line as i64, s.sig_hash, s.body_hash,
                    annotations, scan_id
                ],
            )?;
        }
        Ok(symbols.len())
    }

    // ── edges ────────────────────────────────────────────────

    /// Replace the outgoing edges of every symbol defined in one file.
    ///
    /// Edges are `DERIVED` (docs/data-model.md §2c): dropping and recomputing them is
    /// always safe, which is why an analyzer upgrade needs no data migration.
    pub fn replace_edges_for_file(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        file_id: FileId,
        scan_id: ScanId,
        edges: &[NewEdge],
    ) -> Result<usize> {
        tx.execute(
            "DELETE FROM symbol_edges
             WHERE src_symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
            params![file_id],
        )?;
        let mut written = 0usize;
        for e in edges {
            let src: Option<i64> = tx
                .query_row(
                    "SELECT id FROM symbols WHERE project_id = ?1 AND fqn = ?2 AND deleted = 0",
                    params![project_id, e.src_fqn],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(src) = src else { continue };
            tx.execute(
                "INSERT INTO symbol_edges (project_id, src_symbol_id, dst_symbol_id, dst_fqn_hint,
                                           edge_type, resolution, confidence, site_line, last_seen_scan_id)
                 VALUES (?1, ?2, NULL, ?3, ?4, 'unresolved', 0.0, ?5, ?6)",
                params![project_id, src, e.dst_hint, e.edge_type.as_str(), e.site_line as i64, scan_id],
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// Turn `dst_fqn_hint` into a symbol id.
    ///
    /// Runs once per scan, after every symbol is written — an analyzer cannot do this
    /// because it only ever sees one file. Each edge records which tier resolved it, so a
    /// three-hop heuristic chain is visibly a guess rather than silently a compiler fact.
    pub fn resolve_edges(tx: &Transaction<'_>, project_id: ProjectId) -> Result<ResolveStats> {
        let mut by_fqn: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let mut by_prefix: std::collections::HashMap<String, Vec<i64>> =
            std::collections::HashMap::new();
        // Simple `Type#member` and bare `Type`, for the last-resort tier below.
        let mut by_simple: std::collections::HashMap<String, Vec<i64>> =
            std::collections::HashMap::new();
        {
            let mut stmt = tx.prepare("SELECT id, fqn FROM live_symbols WHERE project_id = ?1")?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, fqn) = row?;
                // A method FQN carries its parameter types; a call site does not know them,
                // so both a full key and a `owner#name` key are needed.
                if let Some(paren) = fqn.find('(') {
                    by_prefix
                        .entry(fqn[..paren].to_string())
                        .or_default()
                        .push(id);
                }
                by_simple.entry(simple_key(&fqn)).or_default().push(id);
                by_fqn.insert(fqn, id);
            }
        }

        let unresolved: Vec<(i64, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, dst_fqn_hint, edge_type FROM symbol_edges
                 WHERE project_id = ?1 AND dst_symbol_id IS NULL AND dst_fqn_hint IS NOT NULL",
            )?;
            let rows = stmt
                .query_map(params![project_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        // Packages this project actually defines. A hint outside all of them points at a
        // library or an unscanned sibling module, which is a different thing from a hint
        // BugHunter looked for and could not find.
        let mut project_packages: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for fqn in by_fqn.keys() {
            let type_part = fqn.split('#').next().unwrap_or(fqn);
            if let Some((pkg, _)) = type_part.rsplit_once('.') {
                project_packages.insert(pkg.to_string());
            }
        }

        // Computed once: it is a property of the index, not of any one edge.
        let owner = package_root(&project_packages);

        // `extends`/`implements` hints are already qualified when the analyzer emits them,
        // so the chain is walkable before any of it has been resolved.
        let mut supertypes: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT s.fqn, e.dst_fqn_hint FROM symbol_edges e
                 JOIN live_symbols s ON s.id = e.src_symbol_id
                 WHERE e.project_id = ?1 AND e.edge_type IN ('extends','implements')
                   AND e.dst_fqn_hint IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (sub, sup) = row?;
                supertypes.entry(sub).or_default().push(sup);
            }
        }

        let mut stats = ResolveStats {
            total: unresolved.len(),
            owner: owner.clone(),
            ..Default::default()
        };
        for (edge_id, hint, edge_type) in unresolved {
            if let Some(&dst) = by_fqn.get(&hint) {
                // A GraphQL field is a real contract that both sides name identically —
                // an exact join, not a guess, so it is labelled `contract`.
                let (res, conf) = if hint.starts_with("graphql:") {
                    stats.contract += 1;
                    (Resolution::Contract, 0.95)
                } else {
                    stats.exact += 1;
                    (Resolution::Exact, 1.0)
                };
                tx.execute(
                    "UPDATE symbol_edges SET dst_symbol_id = ?2, resolution = ?3, confidence = ?4 WHERE id = ?1",
                    params![edge_id, dst, res.as_str(), conf],
                )?;
                continue;
            }

            match by_prefix.get(&hint).map(Vec::as_slice) {
                Some([only]) => {
                    stats.heuristic += 1;
                    tx.execute(
                        "UPDATE symbol_edges SET dst_symbol_id = ?2, resolution = 'heuristic', confidence = 0.9
                         WHERE id = ?1",
                        params![edge_id, only],
                    )?;
                }
                // Overloads. Every candidate gets an edge at reduced confidence: dropping
                // them costs recall, and picking one silently would be a confident guess.
                Some(many) if many.len() <= 4 => {
                    stats.ambiguous += 1;
                    let conf = 0.9 / many.len() as f64;
                    tx.execute(
                        "UPDATE symbol_edges SET dst_symbol_id = ?2, resolution = 'heuristic', confidence = ?3
                         WHERE id = ?1",
                        params![edge_id, many[0], conf],
                    )?;
                    for dst in &many[1..] {
                        tx.execute(
                            "INSERT INTO symbol_edges (project_id, src_symbol_id, dst_symbol_id, dst_fqn_hint,
                                                       edge_type, resolution, confidence, site_line, last_seen_scan_id)
                             SELECT project_id, src_symbol_id, ?2, dst_fqn_hint, ?3, 'heuristic', ?4,
                                    site_line, last_seen_scan_id
                             FROM symbol_edges WHERE id = ?1",
                            params![edge_id, dst, edge_type, conf],
                        )?;
                    }
                }
                _ => {
                    // An inherited member is declared on a supertype and called on the
                    // subtype, so `Issue#getId` names a method that exists — on
                    // `BaseEntity`. Tried before the simple-name tier below because a
                    // declared `extends` is evidence and a name collision is not.
                    if let Some(dst) = through_supertypes(&hint, &supertypes, &by_prefix, &by_fqn) {
                        stats.heuristic += 1;
                        tx.execute(
                            "UPDATE symbol_edges SET dst_symbol_id = ?2, resolution = 'heuristic', confidence = 0.85
                             WHERE id = ?1",
                            params![edge_id, dst],
                        )?;
                        continue;
                    }
                    // Last resort: match on the simple name alone, and only when it is
                    // unique across the project. A wildcard import or a nested type means
                    // the package qualification guessed wrong, but the simple name is still
                    // right — and if it is not unique, this tier declines rather than picks.
                    if let Some([only]) = by_simple.get(&simple_key(&hint)).map(Vec::as_slice) {
                        stats.heuristic += 1;
                        tx.execute(
                            "UPDATE symbol_edges SET dst_symbol_id = ?2, resolution = 'heuristic', confidence = 0.7
                             WHERE id = ?1",
                            params![edge_id, only],
                        )?;
                        continue;
                    }
                    let type_part = hint.split('#').next().unwrap_or(&hint);
                    let pkg = type_part.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
                    if !pkg.is_empty() && !project_packages.contains(pkg) {
                        // Outside the index either way — but a library and an unscanned
                        // module of this same project are different facts, and only one of
                        // them is something the caller can fix by widening the scan.
                        let resolution = if is_sibling(pkg, owner.as_deref()) {
                            stats.sibling += 1;
                            "sibling"
                        } else {
                            stats.external += 1;
                            "external"
                        };
                        tx.execute(
                            "UPDATE symbol_edges SET resolution = ?2 WHERE id = ?1",
                            params![edge_id, resolution],
                        )?;
                    } else {
                        // In a project package but no such symbol: BugHunter looked and
                        // failed. The hint is kept so a later scan can resolve it once the
                        // ambiguity goes away.
                        stats.unresolved += 1;
                    }
                }
            }
        }
        Ok(stats)
    }

    /// Four counts rather than a tuple, because `(total, resolved, external, sibling)` at
    /// a call site is four chances to transpose two of them.
    pub fn edge_counts(&self, project_id: ProjectId) -> Result<EdgeCounts> {
        let mut stmt = self.conn.prepare(
            "SELECT
               COUNT(*),
               SUM(dst_symbol_id IS NOT NULL),
               SUM(resolution = 'external'),
               SUM(resolution = 'sibling')
             FROM symbol_edges WHERE project_id = ?1",
        )?;
        let row = stmt.query_row(params![project_id], |r| {
            Ok(EdgeCounts {
                total: r.get::<_, i64>(0)?,
                resolved: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                external: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                sibling: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?;
        Ok(row)
    }

    pub fn edges_by_resolution(&self, project_id: ProjectId) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT resolution, COUNT(*) FROM symbol_edges WHERE project_id = ?1
             GROUP BY resolution ORDER BY 2 DESC",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── graph traversal ──────────────────────────────────────

    /// Who depends on this symbol. One indexed seek on `idx_edges_dst` per frontier node —
    /// this query is why that index exists.
    pub fn edges_into(&self, symbol_id: SymbolId) -> Result<Vec<Neighbour>> {
        self.neighbours(
            "SELECT s.id, s.fqn, s.kind, f.path, s.start_line,
                    e.edge_type, e.resolution, e.confidence, e.site_line
             FROM symbol_edges e
             JOIN symbols s ON s.id = e.src_symbol_id AND s.deleted = 0
             JOIN files   f ON f.id = s.file_id
             WHERE e.dst_symbol_id = ?1",
            symbol_id,
        )
    }

    /// What this symbol reaches.
    pub fn edges_out(&self, symbol_id: SymbolId) -> Result<Vec<Neighbour>> {
        self.neighbours(
            "SELECT s.id, s.fqn, s.kind, f.path, s.start_line,
                    e.edge_type, e.resolution, e.confidence, e.site_line
             FROM symbol_edges e
             JOIN symbols s ON s.id = e.dst_symbol_id AND s.deleted = 0
             JOIN files   f ON f.id = s.file_id
             WHERE e.src_symbol_id = ?1",
            symbol_id,
        )
    }

    fn neighbours(&self, sql: &str, symbol_id: SymbolId) -> Result<Vec<Neighbour>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![symbol_id], |r| {
                let edge_type: String = r.get(5)?;
                let resolution: String = r.get(6)?;
                Ok(Neighbour {
                    symbol_id: r.get(0)?,
                    fqn: r.get(1)?,
                    kind: r.get(2)?,
                    file_path: r.get(3)?,
                    start_line: r.get(4)?,
                    edge_type: EdgeType::parse(&edge_type).unwrap_or(EdgeType::Calls),
                    resolution: Resolution::parse(&resolution).unwrap_or(Resolution::Heuristic),
                    confidence: r.get(7)?,
                    site_line: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find the symbols a user's target string refers to: an exact FQN, an FQN suffix, a
    /// bare name, or every symbol defined in a file.
    pub fn find_symbols(
        &self,
        project_id: ProjectId,
        target: &str,
        limit: usize,
    ) -> Result<Vec<SymbolRef>> {
        let looks_like_path =
            target.contains('/') || target.ends_with(".java") || target.ends_with(".ts");
        let sql = if looks_like_path {
            "SELECT s.id, s.fqn, s.kind, f.path, s.start_line
             FROM live_symbols s JOIN files f ON f.id = s.file_id
             WHERE s.project_id = ?1 AND f.path = ?2
             ORDER BY s.start_line LIMIT ?3"
        } else {
            "SELECT s.id, s.fqn, s.kind, f.path, s.start_line
             FROM live_symbols s JOIN files f ON f.id = s.file_id
             WHERE s.project_id = ?1
               AND (s.fqn = ?2 OR s.fqn LIKE '%' || ?2 OR s.fqn LIKE '%' || ?2 || '(%'
                    OR s.name = ?2)
             ORDER BY LENGTH(s.fqn) LIMIT ?3"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params![project_id, target, limit as i64], |r| {
                Ok(SymbolRef {
                    id: r.get(0)?,
                    fqn: r.get(1)?,
                    kind: r.get(2)?,
                    file_path: r.get(3)?,
                    start_line: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── detector snapshot ────────────────────────────────────

    /// Everything a detector needs about symbols, in one pass.
    ///
    /// Detectors get a snapshot rather than the store: it keeps them pure and unit-testable,
    /// and it keeps SQL in this crate where boundary rule 3 says it belongs.
    pub fn symbol_facts(&self, project_id: ProjectId) -> Result<Vec<SymbolFactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.fqn, s.name, s.kind, f.path, s.start_line, s.visibility,
                    p.fqn, s.annotations_json
             FROM live_symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN symbols p ON p.id = s.parent_id
             WHERE s.project_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(SymbolFactRow {
                    fqn: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    file: r.get(3)?,
                    line: r.get::<_, i64>(4)? as u32,
                    visibility: r.get(5)?,
                    parent_fqn: r.get(6)?,
                    annotations_json: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn edge_facts(&self, project_id: ProjectId) -> Result<Vec<EdgeFactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT src.fqn, dst.fqn, e.dst_fqn_hint, e.edge_type, e.resolution, e.site_line
             FROM symbol_edges e
             JOIN live_symbols src ON src.id = e.src_symbol_id
             LEFT JOIN live_symbols dst ON dst.id = e.dst_symbol_id
             WHERE e.project_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(EdgeFactRow {
                    src_fqn: r.get(0)?,
                    dst_fqn: r.get(1)?,
                    dst_hint: r.get(2)?,
                    edge_type: r.get(3)?,
                    resolution: r.get(4)?,
                    line: r.get::<_, Option<i64>>(5)?.map(|v| v as u32),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn file_facts(&self, project_id: ProjectId) -> Result<Vec<FileFactRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, lang FROM live_files WHERE project_id = ?1")?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                Ok(FileFactRow {
                    path: r.get(0)?,
                    lang: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── bugs ─────────────────────────────────────────────────

    /// Record a finding, or recognize one already known.
    ///
    /// `UNIQUE(project_id, fingerprint)` is what makes deduplication a database guarantee
    /// rather than application discipline: a bug seen again is an `ON CONFLICT` that
    /// advances `last_seen_scan_id`, never a second row.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_finding(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        scan_id: ScanId,
        b: &NewFinding,
    ) -> Result<FindingUpsert> {
        // The primary fingerprint first, then any this finding would have had under a name
        // the symbol used to carry. Without the alternates a package move reports every
        // finding in it twice — once fixed, once new.
        let mut existing = None;
        for candidate in std::iter::once(&b.fingerprint).chain(b.alt_fingerprints.iter()) {
            existing = tx
                .query_row(
                    "SELECT id, finding_uid, status FROM findings
                     WHERE project_id = ?1 AND capability = ?3 AND fingerprint = ?2",
                    params![project_id, candidate, b.capability],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            if existing.is_some() {
                break;
            }
        }

        if let Some((id, uid, prev_status)) = existing {
            // A bug that had been fixed and is firing again is a regression, and that is the
            // strongest thing this product can say — so it is never silently re-opened as a
            // plain finding.
            let next = match prev_status.as_str() {
                "FIXED" => "REGRESSED",
                "IGNORED" => "IGNORED", // a human dismissal is sticky
                other => other,
            };
            // Identity migrates forward: matched through an alias, the row now carries the
            // fingerprint of the name the symbol has today, so the next scan matches directly.
            tx.execute(
                "UPDATE findings SET status = ?2, severity = ?3, confidence = ?4,
                                 title = ?5, last_seen_scan_id = ?6, fingerprint = ?7,
                                 component = ?8
                 WHERE id = ?1",
                params![
                    id,
                    next,
                    b.severity,
                    b.confidence,
                    b.title,
                    scan_id,
                    b.fingerprint,
                    b.component
                ],
            )?;
            let next = next.to_string();
            return Ok(FindingUpsert {
                id,
                uid,
                is_new: false,
                previous_status: Some(prev_status),
                status: next,
            });
        }

        let n: i64 = tx.query_row(
            "SELECT COALESCE(MAX(CAST(SUBSTR(finding_uid, ?3) AS INTEGER)), 0)
             FROM findings WHERE project_id = ?1 AND capability = ?2",
            params![project_id, b.capability, b.uid_prefix.len() as i64 + 2],
            |r| r.get(0),
        )?;
        let uid = format!("{}-{}", b.uid_prefix, n + 1);
        tx.execute(
            "INSERT INTO findings (project_id, capability, finding_uid, fingerprint, slug, title,
                                   finding_type, component, severity, confidence, status, detector,
                                   anchor_symbol_id, introduced_commit,
                                   first_seen_scan_id, last_seen_scan_id)
             VALUES (?1,?15,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,
                     (SELECT id FROM symbols WHERE project_id = ?1 AND fqn = ?12 AND deleted = 0),
                     ?13,?14,?14)",
            params![
                project_id,
                uid,
                b.fingerprint,
                b.slug,
                b.title,
                b.finding_type,
                b.component,
                b.severity,
                b.confidence,
                b.status,
                b.detector,
                b.anchor_fqn,
                b.commit,
                scan_id,
                b.capability,
            ],
        )?;
        Ok(FindingUpsert {
            id: tx.last_insert_rowid(),
            uid,
            is_new: true,
            previous_status: None,
            status: b.status.clone(),
        })
    }

    pub fn insert_occurrence(
        tx: &Transaction<'_>,
        finding_id: i64,
        scan_id: ScanId,
        o: &NewOccurrence,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO finding_occurrences (finding_id, scan_id, file_path, start_line,
                                          status_at_scan, confidence_at_scan, evidence_json, commit_sha)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(finding_id, scan_id) DO UPDATE SET
               status_at_scan = excluded.status_at_scan,
               confidence_at_scan = excluded.confidence_at_scan,
               evidence_json = excluded.evidence_json",
            params![
                finding_id, scan_id, o.file_path, o.start_line, o.status, o.confidence,
                o.evidence_json, o.commit
            ],
        )?;
        Ok(())
    }

    /// Close a bug whose detector no longer fires over code it actually re-examined.
    pub fn mark_fixed(
        tx: &Transaction<'_>,
        finding_id: i64,
        scan_id: ScanId,
        commit: Option<&str>,
    ) -> Result<()> {
        tx.execute(
            "UPDATE findings SET status = 'FIXED', fixed_commit = ?3, last_seen_scan_id = ?2
             WHERE id = ?1 AND status IN ('SUSPECTED','UNVERIFIED','VERIFIED','REGRESSED')",
            params![finding_id, scan_id, commit],
        )?;
        Ok(())
    }

    pub fn set_finding_status(
        &self,
        project_id: ProjectId,
        uid: &str,
        status: &str,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE findings SET status = ?3 WHERE project_id = ?1 AND finding_uid = ?2",
            params![project_id, uid, status],
        )?;
        Ok(n > 0)
    }

    /// Open bugs by detector, so a detector pass can tell "no longer fires" from
    /// "never looked at".
    /// Open findings belonging to one capability, so its sweep can tell "my rule no longer
    /// fires" from "another capability did not run this time".
    pub fn open_findings(
        &self,
        project_id: ProjectId,
        capability: &str,
    ) -> Result<Vec<OpenFindingRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.id, b.finding_uid, b.fingerprint, b.detector, b.status,
                    (SELECT file_path FROM finding_occurrences o WHERE o.finding_id = b.id
                     ORDER BY o.scan_id DESC LIMIT 1),
                    b.capability
             FROM findings b
             WHERE b.project_id = ?1 AND b.capability = ?2
               AND b.status IN ('SUSPECTED','UNVERIFIED','VERIFIED','REGRESSED')",
        )?;
        let rows = stmt
            .query_map(params![project_id, capability], |r| {
                Ok(OpenFindingRow {
                    id: r.get(0)?,
                    uid: r.get(1)?,
                    fingerprint: r.get(2)?,
                    detector: r.get(3)?,
                    status: r.get(4)?,
                    file_path: r.get(5)?,
                    capability: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn findings(&self, project_id: ProjectId, q: &FindingQuery<'_>) -> Result<Vec<FindingRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT finding_uid, fingerprint, slug, title, finding_type, component, severity,
                    confidence, status, detector, introduced_commit, fixed_commit,
                    (SELECT file_path FROM finding_occurrences o WHERE o.finding_id = findings.id
                     ORDER BY o.scan_id DESC LIMIT 1),
                    (SELECT start_line FROM finding_occurrences o WHERE o.finding_id = findings.id
                     ORDER BY o.scan_id DESC LIMIT 1),
                    capability
             FROM findings
             WHERE project_id = ?1
               AND (?2 IS NULL OR status = ?2)
               AND (?3 IS NULL OR severity = ?3)
               AND (?4 IS NULL OR capability = ?4)
             ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1
                                    WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END,
                      confidence DESC",
        )?;
        let rows = stmt
            .query_map(
                params![project_id, q.status, q.severity, q.capability],
                |r| {
                    Ok(FindingRow {
                        uid: r.get(0)?,
                        fingerprint: r.get(1)?,
                        slug: r.get(2)?,
                        title: r.get(3)?,
                        finding_type: r.get(4)?,
                        component: r.get(5)?,
                        severity: r.get(6)?,
                        confidence: r.get(7)?,
                        status: r.get(8)?,
                        detector: r.get(9)?,
                        introduced_commit: r.get(10)?,
                        fixed_commit: r.get(11)?,
                        file: r.get(12)?,
                        line: r.get(13)?,
                        capability: r.get(14)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn finding_evidence(&self, project_id: ProjectId, uid: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT o.evidence_json FROM finding_occurrences o
                 JOIN findings b ON b.id = o.finding_id
                 WHERE b.project_id = ?1 AND b.finding_uid = ?2
                 ORDER BY o.scan_id DESC LIMIT 1",
                params![project_id, uid],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every sighting of one bug, oldest first. This is the regression history.
    pub fn finding_history(
        &self,
        project_id: ProjectId,
        uid: &str,
    ) -> Result<Vec<FindingEventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.scan_uid, s.commit_sha, o.status_at_scan, o.confidence_at_scan
             FROM finding_occurrences o
             JOIN findings b ON b.id = o.finding_id
             JOIN scans s ON s.id = o.scan_id
             WHERE b.project_id = ?1 AND b.finding_uid = ?2
             ORDER BY o.scan_id",
        )?;
        let rows = stmt
            .query_map(params![project_id, uid], |r| {
                Ok(FindingEventRow {
                    scan_uid: r.get(0)?,
                    commit: r.get(1)?,
                    status: r.get(2)?,
                    confidence: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn finding_counts(&self, project_id: ProjectId) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT status, COUNT(*) FROM findings WHERE project_id = ?1 GROUP BY status",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Findings attached to a file or a symbol.
    ///
    /// Answers "what findings relate to this code?" — the question an agent asks when it is
    /// about to change something and wants to know what is already known about it.
    pub fn findings_for(&self, project_id: ProjectId, target: &str) -> Result<Vec<FindingRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT f.finding_uid, f.fingerprint, f.slug, f.title, f.finding_type,
                    f.component, f.severity, f.confidence, f.status, f.detector,
                    f.introduced_commit, f.fixed_commit, o.file_path, o.start_line, f.capability
             FROM findings f
             JOIN finding_occurrences o ON o.finding_id = f.id
             LEFT JOIN symbols s ON s.id = f.anchor_symbol_id
             WHERE f.project_id = ?1
               AND (o.file_path = ?2 OR s.fqn = ?2 OR s.fqn LIKE '%' || ?2
                    OR s.fqn LIKE '%' || ?2 || '(%' OR f.component = ?2)
             ORDER BY CASE f.severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1
                                      WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END",
        )?;
        let rows = stmt
            .query_map(params![project_id, target], |r| {
                Ok(FindingRow {
                    uid: r.get(0)?,
                    fingerprint: r.get(1)?,
                    slug: r.get(2)?,
                    title: r.get(3)?,
                    finding_type: r.get(4)?,
                    component: r.get(5)?,
                    severity: r.get(6)?,
                    confidence: r.get(7)?,
                    status: r.get(8)?,
                    detector: r.get(9)?,
                    introduced_commit: r.get(10)?,
                    fixed_commit: r.get(11)?,
                    file: r.get(12)?,
                    line: r.get(13)?,
                    capability: r.get(14)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── facts: what Nexus has learned ────────────────────────

    /// Record a fact, superseding any previous one under the same key.
    ///
    /// Facts are never edited — a newer row supersedes the old one — so project memory has
    /// an audit trail and you can always ask what Nexus believed at a given scan and what
    /// changed its mind.
    pub fn record_fact(
        &mut self,
        project_id: ProjectId,
        scan_id: ScanId,
        f: &NewFact,
    ) -> Result<i64> {
        // Insert first, then point the old rows at the new one. A sentinel in
        // `superseded_by` would violate its foreign key to `facts(id)` — the column means
        // "the fact that replaced this", and there is no such thing as fact -1.
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO facts (project_id, fact_key, scope, subject, claim, source,
                                evidence_json, confidence, created_scan_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(project_id, fact_key, created_scan_id) DO UPDATE SET
               claim = excluded.claim, confidence = excluded.confidence,
               evidence_json = excluded.evidence_json, superseded_by = NULL",
            params![
                project_id,
                f.key,
                f.scope,
                f.subject,
                f.claim,
                f.source,
                f.evidence_json,
                f.confidence,
                scan_id
            ],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM facts WHERE project_id = ?1 AND fact_key = ?2 AND created_scan_id = ?3",
            params![project_id, f.key, scan_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE facts SET superseded_by = ?3
             WHERE project_id = ?1 AND fact_key = ?2 AND id <> ?3 AND superseded_by IS NULL",
            params![project_id, f.key, id],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Current facts, most relevant first. Superseded and invalidated rows stay on disk but
    /// never surface: a fact about code that no longer exists is a trap.
    pub fn facts(&self, project_id: ProjectId, subject: Option<&str>) -> Result<Vec<FactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT fact_key, scope, subject, claim, source, confidence, evidence_json
             FROM facts
             WHERE project_id = ?1
               AND superseded_by IS NULL AND invalidated_at IS NULL
               AND (?2 IS NULL OR subject = ?2 OR subject LIKE ?2 || '%')
             ORDER BY CASE source WHEN 'human' THEN 0 WHEN 'deterministic' THEN 1 ELSE 2 END,
                      confidence DESC",
        )?;
        let rows = stmt
            .query_map(params![project_id, subject], |r| {
                Ok(FactRow {
                    key: r.get(0)?,
                    scope: r.get(1)?,
                    subject: r.get(2)?,
                    claim: r.get(3)?,
                    source: r.get(4)?,
                    confidence: r.get(5)?,
                    evidence_json: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── aliases ──────────────────────────────────────────────

    /// Record that `old_fqn` now lives at `symbol_id`.
    ///
    /// Without this a package rename reads as every symbol in it being deleted and a set of
    /// unrelated ones appearing — which, once bug detection exists, duplicates every finding
    /// in the moved package. That is the failure ADR-007 was written to prevent.
    pub fn record_alias(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        old_fqn: &str,
        symbol_id: SymbolId,
        scan_id: ScanId,
    ) -> Result<()> {
        tx.execute(
            "INSERT INTO symbol_aliases (project_id, old_fqn, symbol_id, scan_id)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, old_fqn) DO UPDATE SET
               symbol_id = excluded.symbol_id, scan_id = excluded.scan_id",
            params![project_id, old_fqn, symbol_id, scan_id],
        )?;
        Ok(())
    }

    /// Follow an old name to the symbol it became, if it moved.
    ///
    /// Consulted before a name is declared unknown, so a bug found under the old FQN and one
    /// found under the new one land on the same identity.
    pub fn resolve_alias(&self, project_id: ProjectId, fqn: &str) -> Result<Option<SymbolRef>> {
        Ok(self
            .conn
            .query_row(
                "SELECT s.id, s.fqn, s.kind, f.path, s.start_line
                 FROM symbol_aliases a
                 JOIN symbols s ON s.id = a.symbol_id AND s.deleted = 0
                 JOIN files   f ON f.id = s.file_id
                 WHERE a.project_id = ?1 AND a.old_fqn = ?2",
                params![project_id, fqn],
                |r| {
                    Ok(SymbolRef {
                        id: r.get(0)?,
                        fqn: r.get(1)?,
                        kind: r.get(2)?,
                        file_path: r.get(3)?,
                        start_line: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn symbol_id_by_fqn(
        tx: &Transaction<'_>,
        project_id: ProjectId,
        fqn: &str,
    ) -> Result<Option<SymbolId>> {
        Ok(tx
            .query_row(
                "SELECT id FROM symbols WHERE project_id = ?1 AND fqn = ?2 AND deleted = 0",
                params![project_id, fqn],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every name this symbol used to have.
    ///
    /// Consulted when a finding is recorded, so a bug on a symbol that moved is recognized
    /// rather than reported twice — ADR-007's "rename aliases are consulted before declaring
    /// a new bug", which is inert unless something actually consults them.
    pub fn old_fqns_for(&self, project_id: ProjectId, fqn: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.old_fqn FROM symbol_aliases a
             JOIN symbols s ON s.id = a.symbol_id
             WHERE a.project_id = ?1 AND s.fqn = ?2",
        )?;
        let rows = stmt
            .query_map(params![project_id, fqn], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn alias_count(&self, project_id: ProjectId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM symbol_aliases WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?)
    }

    // ── changes ──────────────────────────────────────────────

    pub fn insert_change(tx: &Transaction<'_>, scan_id: ScanId, c: &ChangeRecord) -> Result<()> {
        tx.execute(
            "INSERT INTO changes (scan_id, entity, entity_id, path, fqn, change_type, detail,
                                  before_hash, after_hash, commit_sha)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                scan_id,
                c.entity,
                c.entity_id,
                c.path,
                c.fqn,
                c.change_type.as_str(),
                c.detail,
                c.before_hash,
                c.after_hash,
                c.commit_sha
            ],
        )?;
        Ok(())
    }

    pub fn change_counts(&self, scan_id: ScanId) -> Result<(i64, i64)> {
        let files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE scan_id = ?1 AND entity = 'file'",
            params![scan_id],
            |r| r.get(0),
        )?;
        let symbols: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM changes WHERE scan_id = ?1 AND entity = 'symbol'",
            params![scan_id],
            |r| r.get(0),
        )?;
        Ok((files, symbols))
    }

    pub fn changes_for_scan(
        &self,
        scan_id: ScanId,
        entity: Option<&str>,
    ) -> Result<Vec<ChangeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT entity, change_type, COALESCE(fqn, path), detail
             FROM changes
             WHERE scan_id = ?1 AND (?2 IS NULL OR entity = ?2)
             ORDER BY entity, change_type",
        )?;
        let rows = stmt
            .query_map(params![scan_id, entity], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── counts ───────────────────────────────────────────────

    pub fn index_counts(&self, project_id: ProjectId) -> Result<(i64, i64)> {
        let files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM live_files WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?;
        let symbols: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM live_symbols WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?;
        Ok((files, symbols))
    }
}

/// `mn.pay.PaymentService#createPayment(String)` -> `PaymentService#createPayment`.
/// The organisation's package root, inferred from the packages this project defines.
///
/// Java and Kotlin packages are reverse-DNS, so the first two segments name the owner
/// (`mn.autoland`, `com.example`) and everything beneath belongs to that owner. A hint
/// under this root that the index does not contain is therefore **a module of this project
/// that was not scanned**, not a third-party library — and those are the two things
/// `external` has always conflated.
///
/// `None` when no root holds a majority. A project whose packages share no common owner
/// gives no signal, and inventing one would relabel every library as a sibling — which is
/// worse than the status quo, because it would understate what is genuinely outside.
fn package_root(project_packages: &std::collections::HashSet<String>) -> Option<String> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut considered = 0usize;
    for pkg in project_packages {
        // Two segments, by convention the owner. A one-segment package names no owner.
        let Some(end) = pkg
            .match_indices('.')
            .nth(1)
            .map(|(i, _)| i)
            .or_else(|| pkg.contains('.').then_some(pkg.len()))
        else {
            continue;
        };
        considered += 1;
        *counts.entry(&pkg[..end]).or_default() += 1;
    }
    let (root, hits) = counts.into_iter().max_by_key(|&(_, n)| n)?;
    // Strictly more than half, so a tie between two owners yields nothing rather than a
    // coin flip that silently mislabels one of them.
    (hits * 2 > considered).then(|| root.to_string())
}

/// Find `Type#member` on a supertype of `Type`.
///
/// An inherited method is declared once and called on every subtype, so the hint names a
/// method that genuinely exists — one level up. Without this walk a `@Data` base class makes
/// every `child.getId()` in the codebase unresolvable, which was 193 of the 214 distinct
/// unresolved accessors left after Lombok synthesis.
///
/// Breadth-first with a visited set, because `implements` makes the graph a lattice rather
/// than a chain and a cycle in bad input must not hang a scan. Depth is capped: a hierarchy
/// deeper than this is not a hierarchy, it is a mistake, and walking it forever to find out
/// costs a scan.
fn through_supertypes(
    hint: &str,
    supertypes: &std::collections::HashMap<String, Vec<String>>,
    by_prefix: &std::collections::HashMap<String, Vec<i64>>,
    by_fqn: &std::collections::HashMap<String, i64>,
) -> Option<i64> {
    const MAX_DEPTH: usize = 8;

    let (ty, member) = hint.split_once('#')?;
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    seen.insert(ty);
    let mut frontier: Vec<&str> = supertypes
        .get(ty)
        .map(|v| v.iter().map(String::as_str).collect())?;

    for _ in 0..MAX_DEPTH {
        let mut next: Vec<&str> = Vec::new();
        for sup in frontier {
            if !seen.insert(sup) {
                continue;
            }
            let candidate = format!("{sup}#{member}");
            if let Some(&id) = by_fqn.get(&candidate) {
                return Some(id);
            }
            // A method FQN carries its parameter types and a call site does not, so the
            // prefix map is what an inherited call actually matches. Ambiguity declines:
            // two overloads inherited from the same supertype cannot be told apart here,
            // and picking one would attribute the call to a method it may never reach.
            if let Some([only]) = by_prefix.get(&candidate).map(Vec::as_slice) {
                return Some(*only);
            }
            if let Some(more) = supertypes.get(sup) {
                next.extend(more.iter().map(String::as_str));
            }
        }
        if next.is_empty() {
            return None;
        }
        frontier = next;
    }
    None
}

/// Whether a package the index does not contain still belongs to this project's owner.
fn is_sibling(pkg: &str, root: Option<&str>) -> bool {
    let Some(root) = root else {
        return false;
    };
    // The dot matters: `mn.autolandia` must not match the root `mn.autoland`.
    pkg == root
        || pkg
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn simple_key(fqn: &str) -> String {
    let (type_part, member) = match fqn.split_once('#') {
        Some((t, m)) => (t, Some(m)),
        None => (fqn, None),
    };
    let simple_type = type_part.rsplit('.').next().unwrap_or(type_part);
    match member {
        Some(m) => format!("{simple_type}#{}", m.split('(').next().unwrap_or(m)),
        None => simple_type.to_string(),
    }
}

pub fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format_iso8601(d.as_secs() as i64)
}

/// ISO-8601 UTC without pulling in a date crate. Timestamps sort correctly as text and stay
/// readable in `sqlite3` at 2 a.m., which matters more than four bytes.
fn format_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // civil_from_days, Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_report_version() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), SCHEMA_VERSION);
    }

    #[test]
    fn all_twenty_one_tables_exist() {
        let s = Store::open_in_memory().expect("open");
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name NOT LIKE 'sqlite_%'
                   AND name NOT LIKE 'ui_strings_fts%' AND name <> 'schema_migrations'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            n, 21,
            "the schema in docs/data-model.md specifies 21 tables"
        );
    }

    #[test]
    fn live_views_hide_soft_deleted_rows() {
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/x", "x", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");
        let tx = s.transaction().expect("tx");
        Store::upsert_file(
            &tx,
            p,
            scan,
            "a.java",
            Some("java"),
            "h1",
            10,
            Some(1),
            None,
            ParseStatus::Ok,
            None,
        )
        .expect("upsert");
        tx.commit().expect("commit");
        assert_eq!(s.live_files(p).expect("files").len(), 1);

        let tx = s.transaction().expect("tx");
        Store::mark_file_deleted(&tx, p, "a.java", scan).expect("delete");
        tx.commit().expect("commit");
        assert_eq!(
            s.live_files(p).expect("files").len(),
            0,
            "soft-deleted rows must not surface"
        );
    }

    #[test]
    fn two_capabilities_may_flag_the_same_place_without_colliding() {
        // Uniqueness is (project, capability, fingerprint). A security finding and a review
        // finding on the same line are two different things, and collapsing them would lose
        // one — which is why the capability is part of identity rather than a label.
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/x", "x", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");

        let make = |cap: &str, prefix: &str| NewFinding {
            capability: cap.into(),
            uid_prefix: prefix.into(),
            fingerprint: "same-fingerprint".into(),
            slug: "x".into(),
            title: "t".into(),
            finding_type: "logic".into(),
            component: "C".into(),
            severity: "high".into(),
            confidence: 0.9,
            status: "UNVERIFIED".into(),
            detector: "d".into(),
            anchor_fqn: None,
            commit: None,
            alt_fingerprints: vec![],
        };

        let tx = s.transaction().expect("tx");
        let a = Store::upsert_finding(&tx, p, scan, &make("bughunter", "BUG")).expect("a");
        let b = Store::upsert_finding(&tx, p, scan, &make("security", "SEC")).expect("b");
        tx.commit().expect("commit");

        assert!(a.is_new && b.is_new, "both are new: {a:?} {b:?}");
        assert_ne!(a.id, b.id, "one row each, not a collision");
        assert_eq!(a.uid, "BUG-1");
        assert_eq!(
            b.uid, "SEC-1",
            "ids are numbered per capability with its own prefix"
        );

        let all = s.findings(p, &FindingQuery::default()).expect("all");
        assert_eq!(all.len(), 2);
        let only_bh = s
            .findings(
                p,
                &FindingQuery {
                    capability: Some("bughunter"),
                    ..Default::default()
                },
            )
            .expect("filtered");
        assert_eq!(only_bh.len(), 1);
        assert_eq!(only_bh[0].capability, "bughunter");
    }

    #[test]
    fn a_fact_is_superseded_rather_than_edited() {
        let mut s = Store::open_in_memory().expect("open");
        let p = s.ensure_project("/tmp/y", "y", "git").expect("project");
        let (scan, _) = s
            .begin_scan(p, ScanKind::Full, None, None, "h", false, "{}")
            .expect("scan");

        let fact = |claim: &str| NewFact {
            key: "arch.payment.idempotency".into(),
            scope: "module".into(),
            subject: Some("mn.pay".into()),
            claim: claim.into(),
            source: "human".into(),
            evidence_json: Some("[]".into()),
            confidence: 0.9,
        };
        s.record_fact(p, scan, &fact("enforced in the controller"))
            .expect("first");

        let (scan2, _) = s
            .begin_scan(p, ScanKind::Incremental, None, None, "h2", false, "{}")
            .expect("scan2");
        s.record_fact(p, scan2, &fact("moved to the service"))
            .expect("second");

        let current = s.facts(p, None).expect("facts");
        assert_eq!(
            current.len(),
            1,
            "only the current belief surfaces: {current:?}"
        );
        assert_eq!(current[0].claim, "moved to the service");

        // The old one is still on disk — memory has an audit trail, it is not overwritten.
        let total: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE project_id = ?1",
                params![p],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(total, 2, "the superseded belief is kept");
    }

    #[test]
    fn timestamps_are_iso8601() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_iso8601(1_756_600_000), "2025-08-31T00:26:40Z");
    }

    fn packages(of: &[&str]) -> std::collections::HashSet<String> {
        of.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_module_of_a_monorepo_still_names_its_owner() {
        // Every package sits under mn.autoland.sales, but the owner is mn.autoland — which
        // is what makes mn.autoland.model recognisable as a module rather than a library.
        let root = package_root(&packages(&[
            "mn.autoland.sales.vehicle",
            "mn.autoland.sales.order",
            "mn.autoland.sales.web.graphql",
        ]));
        assert_eq!(root.as_deref(), Some("mn.autoland"));
    }

    #[test]
    fn no_majority_owner_yields_no_root() {
        // Guessing here would relabel every library as a sibling module, which understates
        // what is genuinely outside the project — worse than saying nothing.
        assert_eq!(package_root(&packages(&["com.a.one", "org.b.two"])), None);
        assert_eq!(package_root(&packages(&["flat"])), None);
    }

    #[test]
    fn a_sibling_module_is_not_a_library() {
        let root = Some("mn.autoland");
        assert!(
            is_sibling("mn.autoland.model", root),
            "the shared model is code this project owns and did not scan"
        );
        assert!(is_sibling("mn.autoland", root), "the root itself counts");
        assert!(
            !is_sibling("org.springframework.stereotype", root),
            "Spring is correctly outside the index — ADR-017"
        );
        assert!(
            !is_sibling("mn.autolandia.thing", root),
            "a prefix match without the dot is a different owner"
        );
        assert!(
            !is_sibling("mn.autoland.model", None),
            "without a majority owner nothing may be claimed as a sibling"
        );
    }
}
