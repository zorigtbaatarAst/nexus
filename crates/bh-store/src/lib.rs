//! The SQLite knowledge store.
//!
//! Boundary rule: **this is the only crate in the workspace that contains SQL.** A schema
//! change therefore has exactly one blast radius. `bh-lang-*` and `bh-mcp` may not depend
//! on it at all; `tests/boundaries.rs` fails the build if they do.
//!
//! Callers read through the `live_*` views rather than the base tables. Soft-deletes mean
//! nearly every query needs `deleted = 0`, and forgetting it is a silent wrong answer —
//! so the filter lives in the schema, not in each call site.

#![forbid(unsafe_code)]
// A panic in a scan loses the whole run; an error loses one file. Tests are exempt:
// an assertion that cannot unwrap is not an assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use bh_types::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;
const MIGRATIONS: &[(u32, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

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
        let current: u32 = self.conn.query_row(
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

        for (version, sql) in MIGRATIONS {
            if *version <= current {
                continue;
            }
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, now()],
            )?;
            tx.commit()?;
        }
        Ok(())
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

    /// Soft-delete. Rows are never removed: `changes` and `bug_occurrences` reference them,
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
    fn timestamps_are_iso8601() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_iso8601(1_756_600_000), "2025-08-31T00:26:40Z");
    }
}
