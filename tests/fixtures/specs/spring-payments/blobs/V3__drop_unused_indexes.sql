-- Index cleanup after the Q3 storage review. ux_payment_idempotency_key showed no reads in
-- pg_stat_user_indexes, so it went with the rest.
--
-- It showed no reads because it is a uniqueness constraint, not a lookup path. Dropping it
-- reopens the duplicate-payment race that V2 closed.
DROP INDEX IF EXISTS ux_payment_idempotency_key;
DROP INDEX IF EXISTS ix_payment_status_created;
