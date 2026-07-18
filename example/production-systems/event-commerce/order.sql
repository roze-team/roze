CREATE TABLE orders (
    id VARCHAR(64) NOT NULL PRIMARY KEY,
    tenant_id VARCHAR(64) NOT NULL,
    product_id BIGINT UNSIGNED NOT NULL,
    quantity BIGINT NOT NULL,
    amount_minor BIGINT NOT NULL,
    status VARCHAR(32) NOT NULL,
    payment_status VARCHAR(32) NOT NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    KEY idx_orders_tenant_created (tenant_id, created_at)
);

CREATE TABLE outbox_events (
    id VARCHAR(64) NOT NULL PRIMARY KEY,
    aggregate_id VARCHAR(64) NOT NULL,
    event_type VARCHAR(128) NOT NULL,
    payload JSON NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    available_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    published_at TIMESTAMP NULL,
    KEY idx_outbox_delivery (published_at, available_at)
);

CREATE TABLE inbox_events (
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
    consumer VARCHAR(128) NOT NULL,
    event_id VARCHAR(64) NOT NULL,
    processed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uniq_inbox_consumer_event (consumer, event_id)
);
