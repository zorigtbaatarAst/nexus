-- BugHunter schema, version 1.
-- Table classification per docs/data-model.md §2:
--   LEDGER  append-only, never UPDATEd   — the evidence
--   CURRENT upserted, soft-deleted       — what is true now
--   DERIVED droppable, recomputable      — caches

-- ─────────────────────────── identity and runs ───────────────────────────

CREATE TABLE projects (
  id              INTEGER PRIMARY KEY,
  root_path       TEXT    NOT NULL UNIQUE,
  name            TEXT    NOT NULL,
  vcs             TEXT    NOT NULL DEFAULT 'git' CHECK (vcs IN ('git','none')),
  schema_version  INTEGER NOT NULL,
  created_at      TEXT    NOT NULL
);                                                              -- CURRENT

CREATE TABLE project_profile (
  project_id       INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  languages_json   TEXT NOT NULL,
  frameworks_json  TEXT NOT NULL,
  build_system     TEXT,
  package_manager  TEXT,
  databases_json   TEXT,
  containers_json  TEXT,
  entrypoints_json TEXT,
  detected_at      TEXT NOT NULL
);                                                              -- CURRENT

CREATE TABLE scans (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  scan_uid           TEXT    NOT NULL,
  kind               TEXT    NOT NULL CHECK (kind IN ('full','incremental')),
  parent_scan_id     INTEGER REFERENCES scans(id),
  commit_sha         TEXT,
  working_tree_hash  TEXT    NOT NULL,
  dirty              INTEGER NOT NULL DEFAULT 0,
  status             TEXT    NOT NULL CHECK (status IN ('running','ok','failed','aborted')),
  files_scanned      INTEGER,
  files_failed       INTEGER,
  symbols_indexed    INTEGER,
  tool_versions_json TEXT    NOT NULL,
  started_at         TEXT    NOT NULL,
  finished_at        TEXT,
  error              TEXT,
  UNIQUE (project_id, scan_uid)
);                                                              -- LEDGER

CREATE TABLE baselines (
  project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  scan_id    INTEGER NOT NULL REFERENCES scans(id),
  set_at     TEXT    NOT NULL
);                                                              -- CURRENT (pointer)

-- ─────────────────────────── code index ───────────────────────────

CREATE TABLE files (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  path               TEXT    NOT NULL,
  lang               TEXT,
  content_hash       TEXT    NOT NULL,
  size_bytes         INTEGER NOT NULL,
  loc                INTEGER,
  mtime_ns           INTEGER,
  parse_status       TEXT    NOT NULL DEFAULT 'ok'
                     CHECK (parse_status IN ('ok','partial','failed','skipped')),
  parse_error        TEXT,
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  deleted            INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, path)
);                                                              -- CURRENT

CREATE TABLE symbols (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id            INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  parent_id          INTEGER REFERENCES symbols(id),
  kind               TEXT    NOT NULL,
  name               TEXT    NOT NULL,
  fqn                TEXT    NOT NULL,
  signature          TEXT,
  visibility         TEXT,
  start_line         INTEGER NOT NULL,
  end_line           INTEGER NOT NULL,
  sig_hash           TEXT    NOT NULL,
  body_hash          TEXT    NOT NULL,
  annotations_json   TEXT,
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  deleted            INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, fqn)
);                                                              -- CURRENT

CREATE TABLE symbol_aliases (
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  old_fqn    TEXT    NOT NULL,
  symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  scan_id    INTEGER NOT NULL REFERENCES scans(id),
  PRIMARY KEY (project_id, old_fqn)
);                                                              -- CURRENT

CREATE TABLE symbol_edges (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  src_symbol_id     INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  dst_symbol_id     INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
  dst_fqn_hint      TEXT,
  edge_type         TEXT    NOT NULL CHECK (edge_type IN
                      ('calls','implements','extends','injects','routes','persists',
                       'reads','writes','emits','imports','tests','calls_http','renders')),
  resolution        TEXT    NOT NULL CHECK (resolution IN
                      ('exact','framework','heuristic','contract','unresolved')),
  confidence        REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  site_line         INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id)
);                                                              -- DERIVED

CREATE TABLE external_deps (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  ecosystem         TEXT    NOT NULL CHECK (ecosystem IN ('maven','npm','pypi','cargo','go','other')),
  name              TEXT    NOT NULL,
  version           TEXT,
  scope             TEXT,
  source_file       TEXT,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  UNIQUE (project_id, ecosystem, name)
);                                                              -- CURRENT

-- ─────────────────────────── history and evidence ───────────────────────────

CREATE TABLE commits (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  sha         TEXT    NOT NULL,
  parent_shas TEXT,
  author      TEXT,
  authored_at TEXT,
  subject     TEXT,
  UNIQUE (project_id, sha)
);                                                              -- LEDGER

CREATE TABLE changes (
  id          INTEGER PRIMARY KEY,
  scan_id     INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
  entity      TEXT    NOT NULL CHECK (entity IN ('file','symbol','dependency','config','test')),
  entity_id   INTEGER,
  path        TEXT,
  fqn         TEXT,
  change_type TEXT    NOT NULL CHECK (change_type IN
                ('added','modified','deleted','renamed','moved')),
  detail      TEXT    CHECK (detail IN ('signature','body','annotations','both','content') OR detail IS NULL),
  before_hash TEXT,
  after_hash  TEXT,
  commit_sha  TEXT
);                                                              -- LEDGER

-- ─────────────────────────── tests ───────────────────────────

CREATE TABLE tests (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id           INTEGER REFERENCES files(id) ON DELETE SET NULL,
  framework         TEXT,
  test_fqn          TEXT    NOT NULL,
  kind              TEXT    NOT NULL CHECK (kind IN ('unit','integration','e2e','generated')),
  origin            TEXT    NOT NULL DEFAULT 'project' CHECK (origin IN ('project','bughunter')),
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  deleted           INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, test_fqn)
);                                                              -- CURRENT

CREATE TABLE test_coverage (
  test_id    INTEGER NOT NULL REFERENCES tests(id) ON DELETE CASCADE,
  symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  source     TEXT    NOT NULL CHECK (source IN ('runtime','static','naming')),
  confidence REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  PRIMARY KEY (test_id, symbol_id)
);                                                              -- DERIVED

CREATE TABLE test_runs (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  scan_id     INTEGER REFERENCES scans(id),
  revision    TEXT,
  command     TEXT    NOT NULL,
  sandbox     TEXT    NOT NULL CHECK (sandbox IN ('docker','host')),
  exit_code   INTEGER,
  duration_ms INTEGER,
  passed      INTEGER, failed INTEGER, skipped INTEGER,
  log_path    TEXT,
  started_at  TEXT    NOT NULL
);                                                              -- LEDGER

-- ─────────────────────────── UI surface index ───────────────────────────

CREATE TABLE ui_strings (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id           INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  symbol_id         INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  text              TEXT    NOT NULL,
  kind              TEXT    NOT NULL CHECK (kind IN
                      ('literal','i18n_key','i18n_value','test_id','aria_label','placeholder')),
  locale            TEXT,
  line              INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id)
);                                                              -- DERIVED

CREATE VIRTUAL TABLE ui_strings_fts USING fts5(
  text, content='ui_strings', content_rowid='id', tokenize='unicode61'
);

-- ─────────────────────────── bug intelligence ───────────────────────────

CREATE TABLE bugs (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  bug_uid            TEXT    NOT NULL,
  fingerprint        TEXT    NOT NULL,
  slug               TEXT    NOT NULL,
  title              TEXT    NOT NULL,
  bug_type           TEXT    NOT NULL CHECK (bug_type IN
                       ('concurrency','transaction','null-safety','security','logic',
                        'performance','error-handling','data-consistency','api-contract',
                        'resource-leak','regression','ui-state')),
  component          TEXT,
  severity           TEXT    NOT NULL CHECK (severity IN ('critical','high','medium','low','info')),
  confidence         REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  status             TEXT    NOT NULL CHECK (status IN
                       ('SUSPECTED','UNVERIFIED','VERIFIED','FIXED','REGRESSED','IGNORED')),
  detector           TEXT    NOT NULL,
  anchor_symbol_id   INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  introduced_commit  TEXT,
  fixed_commit       TEXT,
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  UNIQUE (project_id, fingerprint),
  UNIQUE (project_id, bug_uid)
);                                                              -- CURRENT

CREATE TABLE bug_occurrences (
  id                 INTEGER PRIMARY KEY,
  bug_id             INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  scan_id            INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
  symbol_id          INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  file_path          TEXT,
  start_line         INTEGER, end_line INTEGER,
  snippet_hash       TEXT,
  status_at_scan     TEXT    NOT NULL,
  confidence_at_scan REAL    NOT NULL,
  evidence_json      TEXT    NOT NULL,
  commit_sha         TEXT,
  UNIQUE (bug_id, scan_id)
);                                                              -- LEDGER

CREATE TABLE bug_verifications (
  id                INTEGER PRIMARY KEY,
  bug_id            INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  scan_id           INTEGER NOT NULL REFERENCES scans(id),
  attempt           INTEGER NOT NULL,
  hypothesis        TEXT    NOT NULL,
  test_id           INTEGER REFERENCES tests(id) ON DELETE SET NULL,
  test_path         TEXT,
  run_current_id    INTEGER REFERENCES test_runs(id),
  run_baseline_id   INTEGER REFERENCES test_runs(id),
  outcome           TEXT    NOT NULL CHECK (outcome IN
                      ('reproduced','reproduced_preexisting','not_reproduced',
                       'flaky','inconclusive','error')),
  repetitions       INTEGER NOT NULL DEFAULT 1,
  failures          INTEGER NOT NULL DEFAULT 0,
  confidence_before REAL, confidence_after REAL,
  notes             TEXT,
  created_at        TEXT    NOT NULL,
  UNIQUE (bug_id, attempt)
);                                                              -- LEDGER

CREATE TABLE bug_relations (
  bug_id          INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  related_bug_id  INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  relation        TEXT    NOT NULL CHECK (relation IN
                    ('duplicate_of','regression_of','caused_by','related')),
  created_scan_id INTEGER NOT NULL REFERENCES scans(id),
  PRIMARY KEY (bug_id, related_bug_id, relation),
  CHECK (bug_id <> related_bug_id)
);                                                              -- CURRENT

-- ─────────────────────────── memory and audit ───────────────────────────

CREATE TABLE facts (
  id              INTEGER PRIMARY KEY,
  project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  fact_key        TEXT    NOT NULL,
  scope           TEXT    NOT NULL CHECK (scope IN ('project','module','file','symbol')),
  subject         TEXT,
  claim           TEXT    NOT NULL,
  source          TEXT    NOT NULL CHECK (source IN ('deterministic','ai','human')),
  evidence_json   TEXT,
  confidence      REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  created_scan_id INTEGER NOT NULL REFERENCES scans(id),
  superseded_by   INTEGER REFERENCES facts(id),
  invalidated_at  TEXT,
  UNIQUE (project_id, fact_key, created_scan_id)
);                                                              -- APPEND + SUPERSEDE

CREATE TABLE audit_events (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  at           TEXT    NOT NULL,
  actor        TEXT    NOT NULL,
  action       TEXT    NOT NULL CHECK (action IN
                 ('exec','ai_request','write_test','policy_override','db_migrate','export')),
  target       TEXT,
  outcome      TEXT,
  redactions   INTEGER NOT NULL DEFAULT 0,
  payload_hash TEXT,
  detail_json  TEXT
);                                                              -- LEDGER

-- ─────────────────────────── indexes ───────────────────────────

CREATE INDEX idx_files_project_hash ON files(project_id, content_hash);
CREATE INDEX idx_files_lang         ON files(project_id, lang)   WHERE deleted = 0;
CREATE INDEX idx_symbols_file       ON symbols(file_id)          WHERE deleted = 0;
CREATE INDEX idx_symbols_name       ON symbols(project_id, name);
CREATE INDEX idx_symbols_parent     ON symbols(parent_id);

-- The index that makes reverse traversal a seek instead of a scan.
CREATE INDEX idx_edges_src          ON symbol_edges(src_symbol_id);
CREATE INDEX idx_edges_dst          ON symbol_edges(dst_symbol_id);
-- Partial: Tier-3 re-resolution touches only unresolved edges, ~2-5% of the table.
CREATE INDEX idx_edges_unresolved   ON symbol_edges(project_id, dst_fqn_hint)
                                      WHERE dst_symbol_id IS NULL;

CREATE INDEX idx_changes_scan       ON changes(scan_id, entity);
CREATE INDEX idx_changes_fqn        ON changes(fqn);
CREATE INDEX idx_scans_project      ON scans(project_id, id DESC);
CREATE INDEX idx_commits_sha        ON commits(project_id, sha);

CREATE INDEX idx_cov_symbol         ON test_coverage(symbol_id);
CREATE INDEX idx_tests_origin       ON tests(project_id, origin) WHERE deleted = 0;

CREATE INDEX idx_ui_strings_file    ON ui_strings(file_id);
CREATE INDEX idx_ui_strings_symbol  ON ui_strings(symbol_id);
CREATE INDEX idx_ui_strings_kind    ON ui_strings(project_id, kind);

CREATE INDEX idx_bugs_status        ON bugs(project_id, status);
CREATE INDEX idx_bugs_component     ON bugs(project_id, component);
CREATE INDEX idx_occ_scan           ON bug_occurrences(scan_id);
CREATE INDEX idx_occ_bug            ON bug_occurrences(bug_id, scan_id DESC);
CREATE INDEX idx_verif_bug          ON bug_verifications(bug_id, attempt DESC);

CREATE INDEX idx_facts_subject      ON facts(project_id, subject) WHERE invalidated_at IS NULL;
CREATE INDEX idx_facts_key          ON facts(project_id, fact_key);
CREATE INDEX idx_audit_at           ON audit_events(project_id, at DESC);

-- ─────────────────────────── views ───────────────────────────
-- Soft-deletes mean nearly every query needs `deleted = 0`, and forgetting it is silent.
-- Callers read these views; only the indexer touches the base tables.

CREATE VIEW live_files   AS SELECT * FROM files   WHERE deleted = 0;
CREATE VIEW live_symbols AS SELECT * FROM symbols WHERE deleted = 0;
CREATE VIEW live_tests   AS SELECT * FROM tests   WHERE deleted = 0;
