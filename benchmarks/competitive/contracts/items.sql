CREATE TABLE items (
    id BIGINT PRIMARY KEY,
    version BIGINT NOT NULL,
    name TEXT NOT NULL,
    payload TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

INSERT INTO items (id, version, name, payload, updated_at)
SELECT
    id,
    1,
    'item-' || id::text,
    repeat(chr(97 + (id % 26)::integer), 1024),
    TIMESTAMPTZ '2026-07-18 00:00:00+00'
FROM generate_series(1, 100000) AS id;

ANALYZE items;
