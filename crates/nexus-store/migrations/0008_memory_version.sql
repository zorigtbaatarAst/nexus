-- Every fact or finding write bumps this. The context cache key reads it to decide whether
-- what the project remembers has changed since the package it cached.
--
-- It replaces `COUNT(*) + MAX(id)` over every live fact, which ran on every request: the
-- query deciding whether the cache was still valid cost what the cache existed to save.
ALTER TABLE projects ADD COLUMN memory_version INTEGER NOT NULL DEFAULT 0;
