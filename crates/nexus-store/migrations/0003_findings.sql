-- Findings are a platform concept, not BugHunter's.
--
-- Stable identity across scans, an open/fixed/regressed/ignored lifecycle, file:line
-- evidence, per-scan occurrence history and survival across renames are what Code Review,
-- Security and Dependency Analysis each need, and nothing more. Keeping that machinery
-- under a table called `bugs` would mean the second capability either stores a security
-- vulnerability in it or duplicates the whole lifecycle.
--
-- The change is a rename plus one column. The lifecycle logic itself is untouched.

ALTER TABLE bugs              RENAME TO findings;
ALTER TABLE bug_occurrences   RENAME TO finding_occurrences;
ALTER TABLE bug_verifications RENAME TO finding_verifications;
ALTER TABLE bug_relations     RENAME TO finding_relations;

ALTER TABLE findings              RENAME COLUMN bug_uid  TO finding_uid;
ALTER TABLE findings              RENAME COLUMN bug_type TO finding_type;
ALTER TABLE finding_occurrences   RENAME COLUMN bug_id   TO finding_id;
ALTER TABLE finding_verifications RENAME COLUMN bug_id   TO finding_id;
ALTER TABLE finding_relations     RENAME COLUMN bug_id   TO finding_id;
ALTER TABLE finding_relations     RENAME COLUMN related_bug_id TO related_finding_id;

-- Which capability produced this. Display ids stay per-capability -- BUG-104, SEC-7,
-- REV-12 -- so a developer never has to ask which subsystem a number came from.
ALTER TABLE findings ADD COLUMN capability TEXT NOT NULL DEFAULT 'bughunter';

-- The finding-type CHECK moves with the rename and would still read as bug-specific.
-- SQLite cannot alter a constraint, and findings is a CURRENT-state table whose history
-- lives in finding_occurrences, so it is rebuilt rather than migrated in place.
CREATE TABLE findings_new (
  id                 INTEGER PRIMARY KEY,
  project_id         INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  capability         TEXT    NOT NULL,
  finding_uid        TEXT    NOT NULL,
  fingerprint        TEXT    NOT NULL,
  slug               TEXT    NOT NULL,
  title              TEXT    NOT NULL,
  finding_type       TEXT    NOT NULL,
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
  -- Deduplication stays a database guarantee rather than application discipline. The
  -- capability is part of it: two capabilities may legitimately flag the same line for
  -- different reasons, and collapsing those would lose one of them.
  UNIQUE (project_id, capability, fingerprint),
  UNIQUE (project_id, finding_uid)
);

INSERT INTO findings_new
  (id, project_id, capability, finding_uid, fingerprint, slug, title, finding_type,
   component, severity, confidence, status, detector, anchor_symbol_id,
   introduced_commit, fixed_commit, first_seen_scan_id, last_seen_scan_id)
SELECT id, project_id, capability, finding_uid, fingerprint, slug, title, finding_type,
       component, severity, confidence, status, detector, anchor_symbol_id,
       introduced_commit, fixed_commit, first_seen_scan_id, last_seen_scan_id
FROM findings;

DROP TABLE findings;
ALTER TABLE findings_new RENAME TO findings;

DROP INDEX IF EXISTS idx_bugs_status;
DROP INDEX IF EXISTS idx_bugs_component;
DROP INDEX IF EXISTS idx_occ_scan;
DROP INDEX IF EXISTS idx_occ_bug;
DROP INDEX IF EXISTS idx_verif_bug;

CREATE INDEX idx_findings_status     ON findings(project_id, status);
CREATE INDEX idx_findings_component  ON findings(project_id, component);
CREATE INDEX idx_findings_capability ON findings(project_id, capability, status);
CREATE INDEX idx_findings_anchor     ON findings(anchor_symbol_id);
CREATE INDEX idx_occ_scan            ON finding_occurrences(scan_id);
CREATE INDEX idx_occ_finding         ON finding_occurrences(finding_id, scan_id DESC);
CREATE INDEX idx_verif_finding       ON finding_verifications(finding_id, attempt DESC);
-- "What findings relate to this code?" is a join on the occurrence's file path.
CREATE INDEX idx_occ_file            ON finding_occurrences(file_path);
