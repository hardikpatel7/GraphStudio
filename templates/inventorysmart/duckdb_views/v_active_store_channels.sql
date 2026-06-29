-- Active store-channel pairs. Filters to non-deleted active stores AND
-- active store-channel mappings. Backs the store hierarchy spine in
-- bealls-inventory-graph (alias = "store_channels").
CREATE OR REPLACE VIEW v_active_store_channels AS
SELECT sc.channel, sc.store_code
FROM raw_store_channels AS sc
INNER JOIN raw_store_master AS sm ON sm.store_code = sc.store_code
WHERE sc.active = TRUE AND sm.active = TRUE AND sm.is_deleted = FALSE;
