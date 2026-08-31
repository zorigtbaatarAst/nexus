-- The seam this codebase actually has.
--
-- ADR-014 chose HTTP path matching as the cross-stack join because most codebases have no
-- shared IDL. A Spring-for-GraphQL backend does: the .graphqls schema is the contract, and
-- the join key is a schema field rather than a URL path. That is the revisit trigger the
-- ADR named, and this is it firing.

-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. symbol_edges is DERIVED
-- (docs/data-model.md §2c) — droppable and recomputed by the next scan — so there is no
-- data to migrate and no history to lose. That is the payoff of classifying tables.
DROP TABLE IF EXISTS symbol_edges;

CREATE TABLE symbol_edges (
  id                INTEGER PRIMARY KEY,
  project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  src_symbol_id     INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
  dst_symbol_id     INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
  dst_fqn_hint      TEXT,
  edge_type         TEXT    NOT NULL CHECK (edge_type IN
                      ('calls','implements','extends','injects','routes','persists',
                       'reads','writes','emits','imports','tests',
                       'calls_http','calls_graphql','renders')),
  resolution        TEXT    NOT NULL CHECK (resolution IN
                      ('exact','framework','contract','heuristic','external','unresolved')),
  confidence        REAL    NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  site_line         INTEGER,
  last_seen_scan_id INTEGER NOT NULL REFERENCES scans(id)
);

CREATE INDEX idx_edges_src        ON symbol_edges(src_symbol_id);
-- The index that makes reverse traversal a seek per frontier node instead of a table scan.
CREATE INDEX idx_edges_dst        ON symbol_edges(dst_symbol_id);
-- Partial: Tier-3 re-resolution touches only unresolved edges, a few percent of the table.
CREATE INDEX idx_edges_unresolved ON symbol_edges(project_id, dst_fqn_hint)
                                    WHERE dst_symbol_id IS NULL;

-- A GraphQL operation is a symbol like any other: kind='route', fqn 'graphql:Query.vehicles'.
-- Both sides of the seam point at it, so the join needs no table of its own.
CREATE INDEX idx_symbols_fqn_kind ON symbols(project_id, kind) WHERE deleted = 0;
