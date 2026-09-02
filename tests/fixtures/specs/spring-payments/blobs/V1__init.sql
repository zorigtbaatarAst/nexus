CREATE TABLE payment (
  id              VARCHAR(36) PRIMARY KEY,
  idempotency_key VARCHAR(64)    NOT NULL,
  amount          DECIMAL(19, 2) NOT NULL,
  status          VARCHAR(16)    NOT NULL
);
