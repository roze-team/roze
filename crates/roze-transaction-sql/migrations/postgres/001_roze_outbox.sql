CREATE TABLE IF NOT EXISTS roze_outbox (
    id VARCHAR(255) PRIMARY KEY,
    topic VARCHAR(255) NOT NULL,
    message_key VARCHAR(255),
    headers_json TEXT NOT NULL,
    idempotency_key VARCHAR(255) NOT NULL,
    payload_json TEXT NOT NULL,
    status VARCHAR(16) NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_millis BIGINT,
    lease_until_millis BIGINT,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS roze_outbox_claim_idx
    ON roze_outbox (status, next_attempt_millis, lease_until_millis, id);
