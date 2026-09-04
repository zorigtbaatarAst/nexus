-- Who owns a symbol row, and which file's parse produced an edge.
--
-- One change, two columns, because they are the same problem: `symbols` is unique on FQN
-- and two analyzers can legitimately reach the same one. See docs/data-model.md §`symbols`
-- for the precedence rule and why it lives in the upsert.
ALTER TABLE symbols ADD COLUMN authority TEXT NOT NULL DEFAULT 'declares'
  CHECK (authority IN ('declares', 'implements'));

-- An edge's provenance is the file whose parse emitted it, which stopped being the same
-- thing as the file that owns its source symbol the moment a symbol could be owned by
-- another file. `replace_edges_for_file` deleted by the latter, so a rescan of a resolver
-- whose route symbol the schema owns never deleted its own `routes` edge and inserted
-- another: one edge became two, then three, on every rescan of an untouched file.
--
-- NULL means no file parse produced it — the external dependency graph, which is not
-- per-file and must not be swept away by one.
ALTER TABLE symbol_edges ADD COLUMN file_id INTEGER REFERENCES files(id) ON DELETE CASCADE;

-- Existing rows predate provenance, so nothing would ever delete them. Edges are DERIVED
-- (docs/data-model.md §2c) and `schema` is part of `tool_versions_json`, so bumping it
-- forces a full re-scan that rebuilds every one. Same reasoning as 0002 and 0004.
DELETE FROM symbol_edges;

CREATE INDEX idx_edges_file ON symbol_edges(file_id);
