CREATE TABLE inventory_items (
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
    tenant_id VARCHAR(64) NOT NULL,
    sku VARCHAR(64) NOT NULL,
    name VARCHAR(255) NOT NULL,
    quantity BIGINT NOT NULL DEFAULT 0,
    version BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uniq_inventory_tenant_sku (tenant_id, sku),
    KEY idx_inventory_tenant_name (tenant_id, name)
);
