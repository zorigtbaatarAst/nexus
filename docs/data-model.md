# BugHunter — Data Model

Storage is a single SQLite file per project at `.nexus/nexus.db`.
Rationale: [ADR-002](architecture-decisions.md#adr-002-sqlite-as-the-knowledge-store).

---

## 1. Entities

```
projects ─┬─ project_profile        detected language / framework / build / DB / containers
          ├─ scans ── baselines     every run; exactly one is the current baseline
          ├─ commits                git history BugHunter has observed
          ├─ files ── symbols ── symbol_edges     the code index and dependency graph
          │              └── symbol_aliases       rename carry-over
          ├─ external_deps          third-party libraries
          ├─ tests ── test_coverage ── test_runs
          ├─ bugs ─┬─ bug_occurrences        one row per (bug, scan) sighting
          │        ├─ bug_verifications      one row per reproduction attempt
          │        └─ bug_relations          duplicate_of / regression_of / caused_by
          ├─ facts                  structured project memory
          └─ audit_events           every exec and every AI call
```

21 tables plus one FTS5 virtual table. `changes` hangs off `scans` rather than `projects` — a change is only meaningful
relative to the scan that observed it.

---

## 2. The immutability doctrine

This is the backbone of the design, and getting it wrong silently destroys the product's
main claim. Three classes of table:

### 2a. Immutable evidence ledger — append-only, never `UPDATE`d

`scans` · `changes` · `commits` · `bug_occurrences` · `bug_verifications` · `test_runs` ·
`audit_events`

These record **what was observed at a point in time**. A row is written once and never
edited. (`scans` has one exception: `status`, `finished_at` and the counters are written
exactly once at completion, transitioning `running` → `ok|failed|aborted`.)

Why this matters: the sentence "BUG-104 was fixed in c72aa11 and regressed in f0091ab" is
only sayable if the sightings at each of those scans are still on disk, unedited. The moment
you start updating occurrence rows in place, regression detection becomes a guess.

### 2b. Current state — upserted each scan, soft-deleted

`projects` · `project_profile` · `baselines` · `files` · `symbols` · `symbol_aliases` ·
`external_deps` · `tests` · `bugs`

These answer "what is true now". They carry `first_seen_scan_id` / `last_seen_scan_id` and
a `deleted` flag; rows are **never hard-deleted**, because `changes` and `bug_occurrences`
reference them and history must not develop holes. A file removed from the repo becomes
`deleted = 1`, keeping its id and its history.

`bugs` is the one mutable row with real churn: `status`, `severity`, `confidence`,
`last_seen_scan_id`, `fixed_commit`. Its *history* lives in the immutable
`bug_occurrences` / `bug_verifications` rows, so mutating the summary row loses nothing.

### 2c. Derived cache — droppable and recomputable

`symbol_edges` · `test_coverage` · `ui_strings` (+ its FTS index)

All three are functions of (source, index, analyzer version). `bughunter rescan --rebuild-graph`
truncates and recomputes them. They are stored rather than computed on demand purely for
query speed, and nothing in the ledger depends on them.

### 2d. `facts` — append plus supersede

Project memory is neither immutable nor mutable: a fact is never edited, it is **superseded**
by a newer row via `superseded_by`. This gives memory an audit trail — you can always ask
"what did BugHunter believe about `PaymentService` at scan 12, and what changed its mind?"
See [memory-model.md](memory-model.md).

---

## 3. Schema — DDL

Written for SQLite 3.35+. Timestamps are ISO-8601 UTC strings (`TEXT`), which sort
correctly and stay readable in `sqlite3` at 2 a.m., which matters more than four bytes.

### 3.1 Pragmas and conventions

```sql
PRAGMA journal_mode = WAL;          -- readers never block the scanner
PRAGMA synchronous  = NORMAL;       -- WAL makes FULL unnecessary for this workload
PRAGMA foreign_keys = ON;           -- must be set per connection, not once
PRAGMA busy_timeout = 5000;
PRAGMA mmap_size    = 268435456;    -- 256 MB
PRAGMA temp_store   = MEMORY;
PRAGMA cache_size   = -65536;       -- 64 MB page cache
```

Conventions: `id INTEGER PRIMARY KEY` (rowid alias) everywhere; paths are repo-relative and
`/`-normalized on all platforms; hashes are lowercase hex `blake3` truncated to 128 bits;
all `*_json` columns hold JSON validated on write.

### 3.2 Identity and runs

```sql
CREATE TABLE projects (
  id              INTEGER PRIMARY KEY,
  root_path       TEXT    NOT NULL UNIQUE,
  name            TEXT    NOT NULL,
  vcs             TEXT    NOT NULL DEFAULT 'git' CHECK (vcs IN ('git','none')),
  schema_version  INTEGER NOT NULL,
  created_at      TEXT    NOT NULL
);                                                        -- MUTABLE

CREATE TABLE project_profile (
  project_id      INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  languages_json  TEXT NOT NULL,   -- [{lang,files,loc,share}]
  frameworks_json TEXT NOT NULL,   -- [{name:"spring-boot",version:"3.5",evidence:[...]}]
  build_system    TEXT,            -- gradle | maven | npm | pnpm | poetry | cargo
  package_manager TEXT,
  databases_json  TEXT,            -- [{kind:"mongodb",evidence:"docker/compose.yml:12"}]
  containers_json TEXT,
  entrypoints_json TEXT,           -- main classes, server bootstraps
  detected_at     TEXT NOT NULL
);                                                        -- MUTABLE (replaced wholesale)

CREATE TABLE scans (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  scan_uid           TEXT    NOT NULL,          -- 'scan-001'
  kind               TEXT    NOT NULL CHECK (kind IN ('full','incremental')),
  parent_scan_id     INTEGER REFERENCES scans(id),   -- the baseline it diffed against
  commit_sha         TEXT,                      -- NULL when vcs='none'
  working_tree_hash  TEXT    NOT NULL,          -- merkle root over (path, content_hash)
  dirty              INTEGER NOT NULL DEFAULT 0,
  status             TEXT    NOT NULL CHECK (status IN ('running','ok','failed','aborted')),
  files_scanned      INTEGER,
  files_failed       INTEGER,
  symbols_indexed    INTEGER,
  tool_versions_json TEXT    NOT NULL,          -- grammar + analyzer + schema versions
  started_at         TEXT    NOT NULL,
  finished_at        TEXT,
  error              TEXT,
  UNIQUE (project_id, scan_uid)
);                                                        -- IMMUTABLE

CREATE TABLE baselines (
  project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  scan_id    INTEGER NOT NULL REFERENCES scans(id),
  set_at     TEXT    NOT NULL
);                                                        -- MUTABLE pointer
```

`baselines` is a one-row-per-project pointer table rather than a flag on `scans`. The
primary key makes "exactly one current baseline" a database invariant instead of application
discipline, and baseline *history* is already fully recoverable from `scans.parent_scan_id`.

The baseline the brief asks for is this join:

```json
{
  "commit": "a81f92c",
  "working_tree_hash": "9f2c…",
  "scan_id": "scan-001",
  "dirty": false,
  "set_at": "2026-08-31T09:14:22Z"
}
```

### 3.3 Code index

```sql
CREATE TABLE files (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  path               TEXT    NOT NULL,
  lang               TEXT,                      -- NULL = unknown or binary
  content_hash       TEXT    NOT NULL,
  size_bytes         INTEGER NOT NULL,
  loc                INTEGER,
  mtime_ns           INTEGER,                   -- stat fast path
  parse_status       TEXT    NOT NULL DEFAULT 'ok'
                     CHECK (parse_status IN ('ok','partial','failed','skipped')),
  parse_error        TEXT,
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  deleted            INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, path)
);                                                        -- MUTABLE, soft-delete

CREATE TABLE symbols (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id            INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  parent_id          INTEGER REFERENCES symbols(id),      -- class → method nesting
  kind               TEXT    NOT NULL,   -- module|package|class|interface|enum|trait
                                         -- |function|method|field|route|entity|config|bean
  name               TEXT    NOT NULL,
  fqn                TEXT    NOT NULL,   -- mn.pay.PaymentService#createPayment(String,Money)
  signature          TEXT,
  visibility         TEXT,
  start_line         INTEGER NOT NULL,
  end_line           INTEGER NOT NULL,
  sig_hash           TEXT    NOT NULL,   -- signature + annotations   → API change
  body_hash          TEXT    NOT NULL,   -- normalized body           → behaviour change
  annotations_json   TEXT,               -- ["@Transactional","@PostMapping(\"/pay\")"]
  authority          TEXT    NOT NULL DEFAULT 'declares', -- declares | implements
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  deleted            INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, fqn)
);                                                        -- MUTABLE, soft-delete
```

**`UNIQUE (project_id, fqn)` means two analyzers can contend for one row, and `authority`
says who wins.** A `.graphqls` file *declares* `Query.vehicles`; a Spring `@QueryMapping`
handler *implements* it. Both emit a symbol at that coordinate — it is the join key the
frontend also points at — so one of them owns the row. `Store::replace_symbols` applies the
precedence in the upsert's `WHERE`, and it has to be there rather than in the caller: on a
partial rescan the file that declares an FQN may not be in the scan at all, so only the
stored row can say who owns it.

| incoming | stored | outcome |
|---|---|---|
| `declares` | anything | wins |
| `implements` | `implements` | wins |
| `implements` | same file | wins — a file always owns its own updates |
| `implements` | deleted | wins — nothing live holds the name |
| `implements` | live `declares`, another file | **refused** |

A refused write is returned to the caller rather than swallowed, because it is not a change
and `changes` is append-only: recording one puts a permanent phantom in the ledger that
regression detection reads back. Yielding is not silence — a project that generates its
schema at build time has only the handler, and its `implements` row is the one that stands.
Nor is it permanent: deleting the declaration soft-deletes its row, and `rescan` re-parses
the files that supplied edges for it (`Store::edge_suppliers_for_file`) so the
implementation can take the coordinate back. Without that step the symbol stayed buried
until the next full scan, because the implementing file had not itself changed.

Two `declares` rows at one FQN are still last-writer-wins. That is not the seam case — it
means two files really do declare the same thing, which is a duplicate to fix rather than a
precedence to arbitrate — and nothing about `authority` makes it quieter.

Upgrading a pre-9 database records one phantom `ADDED` for a contested coordinate: rows
predating the column default to `declares`, so the schema's write is an ordinary overwrite
of a row the resolver happened to own, and the forced full rescan reports the move. The end
state is correct, and it happens once.

See `nexus_types::Authority` and [ADR-014](architecture-decisions.md#adr-014-join-the-stack-at-the-http-contract).

**Two hashes per symbol is the single most load-bearing decision in the schema.**
`sig_hash` covers the signature and annotations; `body_hash` covers the normalized body
(comments and formatting stripped, per-language via `LanguageAnalyzer::normalize_body`).
A `sig_hash` change is an API break and ripples to every caller. A `body_hash`-only change
ripples only along data and effect edges. Without the split, every edit fans out to the
whole reverse-reachable set and impact analysis is worthless on a real repo.
See [ADR-010](architecture-decisions.md#adr-010-two-hashes-per-symbol).

```sql
CREATE TABLE symbol_aliases (           -- rename / move carry-over
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  old_fqn     TEXT    NOT NULL,
  symbol_id   INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  scan_id     INTEGER NOT NULL REFERENCES scans(id),
  PRIMARY KEY (project_id, old_fqn)
);                                                        -- MUTABLE

CREATE TABLE symbol_edges (             -- the dependency graph
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  src_symbol_id     INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  dst_symbol_id     INTEGER REFERENCES symbols(id) ON DELETE CASCADE,   -- NULL = unresolved
  dst_fqn_hint      TEXT,                -- kept when unresolved, re-resolved later
  edge_type         TEXT    NOT NULL CHECK (edge_type IN
                      ('calls','implements','extends','injects','routes',
                       'persists','reads','writes','emits','imports','tests',
                       'calls_http','calls_graphql','renders')),
  -- 'external': a third-party library, correctly outside the index (ADR-017).
  -- 'sibling':  this project's own code, in a module that was not scanned. Outside the
  --             index like 'external', but an edit here can break it, and widening the
  --             scan resolves it — so it counts in the denominator and 'external' does not.
  resolution        TEXT    NOT NULL CHECK (resolution IN
                      ('exact','framework','contract','heuristic',
                       'external','sibling','unresolved')),
  confidence        REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  site_line         INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  file_id           INTEGER REFERENCES files(id),  -- the parse that produced it; NULL if none
);                                                        -- DERIVED
```

`calls_http` is the edge that crosses the frontend/backend seam, and `resolution='contract'`
marks an edge produced by matching a canonical `METHOD /path/:p` on both sides. It needs no
table of its own: a backend route is already a symbol with `kind='route'`, so a frontend call
site emits an edge with `dst_fqn_hint = 'GET /api/cart/:p'` and the existing Tier-3
unresolved sweep matches it. See [investigation.md](investigation.md) §3.

`file_id` is the edge's provenance — the file whose parse emitted it — and it is what
`replace_edges_for_file` deletes by. Deleting by the file that owns the *source symbol*
looks equivalent and is not: a symbol can be owned by one file while another supplies its
edges (see `authority` above), and the rescan of that other file then deleted nothing and
inserted a second copy, growing the edge every time an untouched file was rescanned. NULL
means no file parse produced it — the external dependency graph, which is not per-file and
must not be swept away by one.

Carrying `resolution` and `confidence` on every edge is what lets an impact result explain
*why* it believes something is affected, and lets a calling agent discount a chain that
went through three heuristic hops. It is the schema-level expression of constraint 14,
"prefer deterministic evidence over AI assumptions."

```sql
CREATE TABLE external_deps (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  ecosystem   TEXT    NOT NULL CHECK (ecosystem IN ('maven','npm','pypi','cargo','go','other')),
  name        TEXT    NOT NULL,
  version     TEXT,
  scope       TEXT,                       -- compile | test | dev | optional
  source_file TEXT,                       -- build.gradle · package.json · Cargo.toml
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  UNIQUE (project_id, ecosystem, name)
);                                                        -- MUTABLE
```

### 3.4 History and evidence

```sql
CREATE TABLE commits (
  id           INTEGER PRIMARY KEY,
  project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  sha          TEXT    NOT NULL,
  parent_shas  TEXT,                      -- space-separated
  author       TEXT,
  authored_at  TEXT,
  subject      TEXT,
  UNIQUE (project_id, sha)
);                                                        -- IMMUTABLE

CREATE TABLE changes (
  id           INTEGER PRIMARY KEY,
  scan_id      INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
  entity       TEXT    NOT NULL CHECK (entity IN ('file','symbol','dependency','config','test')),
  entity_id    INTEGER,                   -- files.id / symbols.id / …
  path         TEXT,                      -- denormalized on purpose
  fqn          TEXT,                      -- denormalized on purpose
  change_type  TEXT    NOT NULL CHECK (change_type IN
                 ('added','modified','deleted','renamed','moved')),
  detail       TEXT    CHECK (detail IN
                 ('signature','body','annotations','both','content',NULL)),
  before_hash  TEXT,
  after_hash   TEXT,
  commit_sha   TEXT
);                                                        -- IMMUTABLE
```

`changes.path` and `changes.fqn` duplicate data reachable through `entity_id`. That
denormalization is deliberate: the evidence must remain readable after the symbol is
deleted, and a historical record that resolves to `NULL` two refactors later is not a
record. Storage cost is trivial; the alternative is losing the history you built the
product to keep.

### 3.5 Tests

```sql
CREATE TABLE tests (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id           INTEGER REFERENCES files(id) ON DELETE SET NULL,
  framework         TEXT,                 -- junit5 | jest | vitest | pytest | cargo-test
  test_fqn          TEXT    NOT NULL,     -- mn.pay.PaymentServiceTest#createsOnce
  kind              TEXT    NOT NULL CHECK (kind IN ('unit','integration','e2e','generated')),
  origin            TEXT    NOT NULL DEFAULT 'project' CHECK (origin IN ('project','bughunter')),
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  deleted           INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, test_fqn)
);                                                        -- MUTABLE

CREATE TABLE test_coverage (
  test_id    INTEGER NOT NULL REFERENCES tests(id) ON DELETE CASCADE,
  symbol_id  INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  source     TEXT    NOT NULL CHECK (source IN ('runtime','static','naming')),
  confidence REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  PRIMARY KEY (test_id, symbol_id)
);                                                        -- DERIVED

CREATE TABLE test_runs (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  scan_id     INTEGER REFERENCES scans(id),
  revision    TEXT,                       -- commit actually executed against
  command     TEXT    NOT NULL,           -- fully expanded argv, JSON array
  sandbox     TEXT    NOT NULL CHECK (sandbox IN ('docker','host')),
  exit_code   INTEGER,
  duration_ms INTEGER,
  passed      INTEGER, failed INTEGER, skipped INTEGER,
  log_path    TEXT,
  started_at  TEXT    NOT NULL
);                                                        -- IMMUTABLE
```

`test_coverage.source` ranks how the link was established: `runtime` (a coverage report was
parsed) beats `static` (the test calls the symbol through the graph) beats `naming`
(`PaymentServiceTest` → `PaymentService`). Ranked test selection needs to know which it has.

### 3.6 UI surface index

```sql
CREATE TABLE ui_strings (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  symbol_id   INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  text        TEXT    NOT NULL,
  kind        TEXT    NOT NULL CHECK (kind IN
                ('literal','i18n_key','i18n_value','test_id','aria_label','placeholder')),
  locale      TEXT,                    -- for i18n_value rows
  line        INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id)
);                                                        -- DERIVED

CREATE VIRTUAL TABLE ui_strings_fts USING fts5(
  text, content='ui_strings', content_rowid='id', tokenize='unicode61'
);
```

Every user-visible string in the frontend: JSX text nodes, `aria-label`, `data-testid`,
`placeholder`, and i18n keys with their values in **every** locale. This is what turns a
label read off a screenshot into a component anchor.

Indexing every locale is the point, not thoroughness for its own sake: the screenshot may be
in Mongolian while the source holds an English i18n key. Matching the *value* reaches the
key, and the key reaches the component. Without the locale rows, a non-English UI is
unanchorable by text and the investigation entry point loses its strongest signal.

The table is `DERIVED` — droppable and rebuilt from source, like `symbol_edges`.

### 3.7 Bug intelligence

```sql
CREATE TABLE bugs (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  bug_uid            TEXT    NOT NULL,       -- 'BUG-104' — display id, monotonic per project
  fingerprint        TEXT    NOT NULL,       -- stable identity across scans
  slug               TEXT    NOT NULL,       -- 'payment-duplicate-concurrent-create'
  title              TEXT    NOT NULL,
  bug_type           TEXT    NOT NULL CHECK (bug_type IN
                       ('concurrency','transaction','null-safety','security','logic',
                        'performance','error-handling','data-consistency','api-contract',
                        'resource-leak','regression','ui-state')),
  component          TEXT,                   -- 'PaymentService'
  severity           TEXT    NOT NULL CHECK (severity IN ('critical','high','medium','low','info')),
  confidence         REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  status             TEXT    NOT NULL CHECK (status IN
                       ('SUSPECTED','UNVERIFIED','VERIFIED','FIXED','REGRESSED','IGNORED')),
  detector           TEXT    NOT NULL,       -- 'semgrep:java.lang.security.x' | 'ai:agent' | 'compiler'
  anchor_symbol_id   INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  introduced_commit  TEXT,
  fixed_commit       TEXT,
  first_seen_scan_id INTEGER NOT NULL REFERENCES scans(id),
  last_seen_scan_id  INTEGER NOT NULL REFERENCES scans(id),
  UNIQUE (project_id, fingerprint),
  UNIQUE (project_id, bug_uid)
);                                                        -- MUTABLE summary
```

`UNIQUE (project_id, fingerprint)` is where constraint 7 — "fingerprints must prevent
duplicate findings" — becomes a database guarantee. An insert of a bug already known is an
`ON CONFLICT DO UPDATE` that advances `last_seen_scan_id`, not a second row.

```sql
CREATE TABLE bug_occurrences (           -- one row per (bug, scan) sighting
  id                 INTEGER PRIMARY KEY,
  bug_id             INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  scan_id            INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
  symbol_id          INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
  file_path          TEXT,
  start_line         INTEGER, end_line INTEGER,
  snippet_hash       TEXT,
  status_at_scan     TEXT    NOT NULL,
  confidence_at_scan REAL    NOT NULL,
  evidence_json      TEXT    NOT NULL,   -- [{file,line,kind,excerpt_hash,note}]
  commit_sha         TEXT,
  UNIQUE (bug_id, scan_id)
);                                                        -- IMMUTABLE

CREATE TABLE bug_verifications (
  id               INTEGER PRIMARY KEY,
  bug_id           INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  scan_id          INTEGER NOT NULL REFERENCES scans(id),
  attempt          INTEGER NOT NULL,
  hypothesis       TEXT    NOT NULL,     -- the failure predicted, in one sentence
  test_id          INTEGER REFERENCES tests(id) ON DELETE SET NULL,
  test_path        TEXT,
  run_current_id   INTEGER REFERENCES test_runs(id),
  run_baseline_id  INTEGER REFERENCES test_runs(id),   -- same test, baseline revision
  outcome          TEXT    NOT NULL CHECK (outcome IN
                     ('reproduced','reproduced_preexisting','not_reproduced',
                      'flaky','inconclusive','error')),
  repetitions      INTEGER NOT NULL DEFAULT 1,
  failures         INTEGER NOT NULL DEFAULT 0,
  confidence_before REAL, confidence_after REAL,
  notes            TEXT,
  created_at       TEXT    NOT NULL,
  UNIQUE (bug_id, attempt)
);                                                        -- IMMUTABLE
```

`run_baseline_id` is not optional polish. Running the same generated test against the
baseline revision is the only way to distinguish "this change introduced a bug" from "this
suite was already red", and therefore the only honest way to move confidence from 71 % to
97 %. See [verification-engine.md](verification-engine.md) §4.

```sql
CREATE TABLE bug_relations (
  bug_id         INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  related_bug_id INTEGER NOT NULL REFERENCES bugs(id) ON DELETE CASCADE,
  relation       TEXT    NOT NULL CHECK (relation IN
                   ('duplicate_of','regression_of','caused_by','related')),
  created_scan_id INTEGER NOT NULL REFERENCES scans(id),
  PRIMARY KEY (bug_id, related_bug_id, relation),
  CHECK (bug_id <> related_bug_id)
);                                                        -- MUTABLE
```

### 3.8 Memory and audit

```sql
CREATE TABLE facts (
  id              INTEGER PRIMARY KEY,
  project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  fact_key        TEXT    NOT NULL,      -- 'arch.payment.idempotency'
  scope           TEXT    NOT NULL CHECK (scope IN ('project','module','file','symbol')),
  subject         TEXT,                  -- fqn or module path
  claim           TEXT    NOT NULL,      -- one sentence, the fact itself
  source          TEXT    NOT NULL CHECK (source IN ('deterministic','ai','human')),
  evidence_json   TEXT,                  -- [{file,line}] / commit shas
  confidence      REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  created_scan_id INTEGER NOT NULL REFERENCES scans(id),
  superseded_by   INTEGER REFERENCES facts(id),
  invalidated_at  TEXT,
  UNIQUE (project_id, fact_key, created_scan_id)
);                                                        -- APPEND + SUPERSEDE

CREATE TABLE audit_events (
  id          INTEGER PRIMARY KEY,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  at          TEXT    NOT NULL,
  actor       TEXT    NOT NULL,          -- 'cli' | 'mcp:claude-code' | 'ai:openai'
  action      TEXT    NOT NULL CHECK (action IN
                ('exec','ai_request','write_test','policy_override','db_migrate','export')),
  target      TEXT,
  outcome     TEXT,
  redactions  INTEGER NOT NULL DEFAULT 0,
  payload_hash TEXT,                     -- hash of the payload, never the payload
  detail_json TEXT
);                                                        -- IMMUTABLE
```

`audit_events` stores a hash of any AI payload, never the payload itself. Storing the prompt
would recreate, inside BugHunter's own database, exactly the secret-leak risk the redaction
pass exists to prevent. Rows are mirrored to `.nexus/audit.log` as JSONL so the log
survives a database reset and can be tailed.

---

## 4. Indexes

```sql
-- change detection
CREATE INDEX idx_files_project_hash  ON files(project_id, content_hash);
CREATE INDEX idx_files_lang          ON files(project_id, lang)        WHERE deleted = 0;
CREATE INDEX idx_symbols_file        ON symbols(file_id)               WHERE deleted = 0;
CREATE INDEX idx_symbols_name        ON symbols(project_id, name);
CREATE INDEX idx_symbols_parent      ON symbols(parent_id);

-- graph traversal
CREATE INDEX idx_edges_src           ON symbol_edges(src_symbol_id);
CREATE INDEX idx_edges_dst           ON symbol_edges(dst_symbol_id);
CREATE INDEX idx_edges_unresolved    ON symbol_edges(project_id, dst_fqn_hint)
                                       WHERE dst_symbol_id IS NULL;

-- history
CREATE INDEX idx_changes_scan        ON changes(scan_id, entity);
CREATE INDEX idx_changes_fqn         ON changes(fqn);
CREATE INDEX idx_scans_project       ON scans(project_id, id DESC);
CREATE INDEX idx_commits_sha         ON commits(project_id, sha);

-- tests
CREATE INDEX idx_cov_symbol          ON test_coverage(symbol_id);
CREATE INDEX idx_tests_origin        ON tests(project_id, origin) WHERE deleted = 0;

-- bugs
CREATE INDEX idx_bugs_status         ON bugs(project_id, status);
CREATE INDEX idx_bugs_component      ON bugs(project_id, component);
CREATE INDEX idx_occ_scan            ON bug_occurrences(scan_id);
CREATE INDEX idx_occ_bug             ON bug_occurrences(bug_id, scan_id DESC);
CREATE INDEX idx_verif_bug           ON bug_verifications(bug_id, attempt DESC);

-- ui surface
CREATE INDEX idx_ui_strings_file    ON ui_strings(file_id);
CREATE INDEX idx_ui_strings_symbol  ON ui_strings(symbol_id);
CREATE INDEX idx_ui_strings_kind    ON ui_strings(project_id, kind);

-- memory
CREATE INDEX idx_facts_subject       ON facts(project_id, subject) WHERE invalidated_at IS NULL;
CREATE INDEX idx_facts_key           ON facts(project_id, fact_key);
CREATE INDEX idx_audit_at            ON audit_events(project_id, at DESC);
```

Two of these carry unusual weight:

- **`idx_edges_dst`** is what makes reverse traversal cheap. Every "which functions depend on
  this function" query, and therefore the entire impact engine, is an index seek per frontier
  node instead of a table scan. Without it, impact analysis on a large repo is unusable.
- **`idx_edges_unresolved`** is a partial index over only the unresolved edges. Tier-3
  re-resolution after a rename becomes one indexed lookup per changed FQN rather than a
  full graph rebuild, which is the difference between a 200 ms rescan and a 40 s one.

---

## 5. Migrations

Forward-only numbered SQL files in `crates/nexus-store/migrations/`, applied in a transaction,
tracked in a `schema_migrations` table. `projects.schema_version` records what the database
was last migrated to.

A version mismatch is never repaired silently. `bughunter doctor` reports it, and any
command against a database newer than the binary refuses to run rather than guessing at an
unknown schema — errors should never pass silently, least of all in the store.

Because `symbol_edges` and `test_coverage` are derived, a migration that changes how edges
are extracted does not need a data migration at all: it bumps the analyzer version in
`tool_versions_json`, which forces re-parsing on the next scan. That is the practical payoff
of separating derived tables from the ledger.

---

## 6. Retention

Unbounded growth is a real risk on a busy monorepo — the ledger only ever appends.

| Table | Default retention | Notes |
|---|---|---|
| `scans` | last 200, plus every scan referenced by a live bug | never drop a baseline ancestor |
| `changes` | follows its scan (`ON DELETE CASCADE`) | |
| `bug_occurrences` | never dropped for non-`IGNORED` bugs | this is the regression evidence |
| `test_runs` | last 50 per bug; logs pruned at 30 days | logs live on disk, not in the DB |
| `audit_events` | 365 days in DB, unbounded in `audit.log` | |
| `facts` | superseded facts kept; `invalidated` pruned after 90 days | |

`bughunter prune --older-than 90d` performs it explicitly. Nothing is ever deleted
automatically during a scan: a scan that quietly discards history is worse than a large
database file.
