CREATE TABLE checkouts (
    id VARCHAR(64) NOT NULL PRIMARY KEY,
    tenant_id VARCHAR(64) NOT NULL,
    product_id BIGINT UNSIGNED NOT NULL,
    quantity BIGINT NOT NULL,
    reservation_id VARCHAR(64) NULL,
    status VARCHAR(32) NOT NULL,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    KEY idx_checkouts_tenant_created (tenant_id, created_at)
);
