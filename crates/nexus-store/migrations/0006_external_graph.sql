-- Edges from an external graph (roadmap 2.12).
--
-- `graphify` can describe a language Nexus has no analyzer for. Those edges are real and
-- worth having, but they are a different kind of evidence from a parsed one: nobody resolved
-- a symbol table to produce them. So they get their own resolution value rather than being
-- laundered into `heuristic`, and the resolution rate excludes them for the same reason
-- ADR-017 excludes `external` — a denominator that quietly absorbs a weaker kind of evidence
-- stops measuring what it claims to.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. Same columns, same
-- indexes, one more permitted value.

PRAGMA foreign_keys = OFF;

CREATE TABLE symbol_edges_new (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  src_symbol_id     INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  dst_symbol_id     INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
  dst_fqn_hint      TEXT,
  edge_type         TEXT    NOT NULL CHECK (edge_type IN
                      ('calls','implements','extends','injects','routes','persists',
                       'reads','writes','emits','imports','tests','calls_http',
                       'calls_graphql','renders')),
  resolution        TEXT    NOT NULL CHECK (resolution IN
                      ('exact','framework','heuristic','contract','external','sibling',
                       'unresolved','external-graph')),
  confidence        REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  site_line         INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id)
);

INSERT INTO symbol_edges_new
  SELECT id, project_id, src_symbol_id, dst_symbol_id, dst_fqn_hint, edge_type,
         resolution, confidence, site_line, last_seen_scan_id
  FROM symbol_edges;

DROP TABLE symbol_edges;
ALTER TABLE symbol_edges_new RENAME TO symbol_edges;

CREATE INDEX idx_edges_src         ON symbol_edges(src_symbol_id);
CREATE INDEX idx_edges_dst         ON symbol_edges(dst_symbol_id);
CREATE INDEX idx_edges_unresolved  ON symbol_edges(project_id, dst_fqn_hint)
  WHERE dst_symbol_id IS NULL AND resolution = 'unresolved';

PRAGMA foreign_keys = ON;
