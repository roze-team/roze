CREATE TABLE IF NOT EXISTS roze_outbox (
    id VARCHAR(255) PRIMARY KEY,
    topic VARCHAR(255) NOT NULL,
    message_key VARCHAR(255),
    headers_json TEXT NOT NULL,
    idempotency_key VARCHAR(255) NOT NULL,
    payload_json LONGTEXT NOT NULL,
    status VARCHAR(16) NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    next_attempt_millis BIGINT,
    lease_until_millis BIGINT,
    last_error TEXT,
    INDEX roze_outbox_claim_idx
        (status, next_attempt_millis, lease_until_millis, id)
) ENGINE=InnoDB;
