CREATE TABLE competitive_inbox (
    event_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    sequence BIGINT NOT NULL UNIQUE CHECK (sequence BETWEEN 1 AND 100000),
    payload TEXT NOT NULL CHECK (char_length(payload) = 1024),
    received_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE competitive_outbox (
    event_id TEXT PRIMARY KEY REFERENCES competitive_inbox(event_id),
    topic TEXT NOT NULL CHECK (topic = 'competitive.confirmed.v1'),
    payload TEXT NOT NULL CHECK (char_length(payload) = 1024),
    state TEXT NOT NULL CHECK (state IN ('pending', 'published')),
    created_at TIMESTAMPTZ NOT NULL,
    published_at TIMESTAMPTZ
);

CREATE TABLE competitive_effects (
    event_id TEXT PRIMARY KEY REFERENCES competitive_inbox(event_id),
    sequence BIGINT NOT NULL UNIQUE CHECK (sequence BETWEEN 1 AND 100000),
    confirmed_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX competitive_outbox_pending_idx
    ON competitive_outbox (created_at, event_id)
    WHERE state = 'pending';
