-- `external` conflated two facts that call for opposite reactions.
--
-- The only test at nexus-store resolve_edges was `!project_packages.contains(pkg)`, so an
-- edge to org.springframework and an edge to a sibling module of the same monorepo were
-- recorded identically. One is correctly outside the index; the other is code this project
-- owns, that an edit here can break, and that a wider scan would resolve. An agent reading
-- `external` concludes "not my problem" — right about the library, wrong about the module.
--
-- Measured on a six-service Gradle monorepo: scanning one module classified 9,514 edges as
-- external, and `impact` on the base class every entity inherits from answered "no symbol
-- matches". Scanning from the root found 768 affected symbols across all six services.
--
-- ADR-017 stands: `external` is still a resolution outcome, not a failure. This splits the
-- outcome in two rather than reclassifying it as one.

-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. symbol_edges is DERIVED
-- (docs/data-model.md §2c) — droppable and recomputed by the next scan — so there is no
-- data to migrate and no history to lose. Same reasoning as 0002.
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
  -- 'sibling': outside the index like 'external', but code this project owns. Kept
  -- distinct so the resolution rate can count it and the CLI can say how to fix it.
  resolution        TEXT    NOT NULL CHECK (resolution IN
                      ('exact','framework','contract','heuristic',
                       'external','sibling','unresolved')),
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
