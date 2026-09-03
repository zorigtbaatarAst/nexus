-- Fact lifecycle (roadmap 3.1).
--
-- A fact arrives as a candidate and earns its weight by surviving scans: the evidence anchor
-- still exists and the symbol at it has not moved. Three distinct scans, or a human author,
-- makes it durable. `06-memory.md` §3.
--
-- These are not edits to a belief. A belief is superseded, never updated — that rule is
-- unchanged. These columns record what a *scan observed* about a belief, which is the same
-- category as `invalidated_at`, written by the same pass, from the same anchors.
--
-- Added rather than rebuilt: SQLite can ALTER TABLE ADD COLUMN, and the CHECK constraints on
-- `facts` are untouched by this change.

ALTER TABLE facts ADD COLUMN validated_scan_id INTEGER REFERENCES scans(id);
-- Distinct scans this fact's evidence has survived. Not a count of passes: a re-run of one
-- scan must not promote anything, so the pass guards on `validated_scan_id`.
ALTER TABLE facts ADD COLUMN validated_count   INTEGER NOT NULL DEFAULT 0;
-- Highest retrieval weight. Set by three validations, or at insert for `source = 'human'`.
ALTER TABLE facts ADD COLUMN durable           INTEGER NOT NULL DEFAULT 0;

-- Every fact already on disk was written by a person or by an agent with checked evidence,
-- and none of them has been validated by this pass because it did not exist. They start as
-- candidates, except the human ones, which §3 makes durable by authorship rather than by
-- survival.
UPDATE facts SET durable = 1 WHERE source = 'human';

-- Retrieval reads state on every query. Partial, because invalidated and superseded rows are
-- never retrieved and there is no reason to carry them in the index.
CREATE INDEX idx_facts_state ON facts(project_id, durable, validated_count)
  WHERE superseded_by IS NULL AND invalidated_at IS NULL;
