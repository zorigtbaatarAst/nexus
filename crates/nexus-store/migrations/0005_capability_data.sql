-- ADR-018's deferred half, done before the second capability rather than after.
--
-- `findings` became a platform table so Code Review and Security would not each reimplement
-- a lifecycle subtle enough that a second implementation is a second set of bugs. What was
-- left open is that `finding_type` is a taxonomy shared across capabilities, and the first
-- capability that does not fit it forces a migration. Two of them arrive at once: Architect
-- reports "this stack has no MongoDB MCP server configured" and Review reports "this change
-- touches code nothing tests", and neither is a `concurrency` or a `resource-leak`.
--
-- The fix ADR-018 named is a JSON column rather than a second table, because the lifecycle
-- is the expensive part and it is precisely what must not fork. A capability puts its own
-- shape here; identity, recurrence, fixed and regressed stay the platform's.
--
-- `findings` is CURRENT, not DERIVED (docs/data-model.md §2), so this adds a column rather
-- than rebuilding the table: the rows carry history that a rebuild would discard.
--
-- Done now because it is nearly free now. `findings` holds almost nothing on any real
-- project yet, and every week this waits it becomes a migration of a populated database for
-- no additional benefit.

ALTER TABLE findings ADD COLUMN capability_data TEXT;

-- Deliberately not indexed and deliberately not constrained. Nothing queries by its
-- contents: it is carried, rendered and handed back to the capability that wrote it. An
-- index or a CHECK here would be the platform taking an interest in a shape it has promised
-- not to know about, which is the coupling the column exists to avoid.
