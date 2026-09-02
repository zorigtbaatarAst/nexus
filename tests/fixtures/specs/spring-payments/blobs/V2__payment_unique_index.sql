-- The database half of the idempotency fix. The application check races; this does not.
CREATE UNIQUE INDEX ux_payment_idempotency_key ON payment (idempotency_key);

ALTER TABLE payment ADD COLUMN version BIGINT NOT NULL DEFAULT 0;
