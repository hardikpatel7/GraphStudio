-- ============================================================================
-- Materialized views for Article Selection V4.
-- Run once in PG. Refresh periodically (REFRESH MATERIALIZED VIEW CONCURRENTLY).
-- The V4 extractor COPYs from these instead of the raw tables.
-- ============================================================================

-- 1. Transaction metrics: 27M aid rows → 43K rows grouped by ph_code
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_txs_metrics AS
SELECT
    ph.ph_code,
    CAST(ROUND(SUM(COALESCE(a.lw_units, 0))) AS INTEGER) AS lw_units,
    CAST(ROUND(SUM(COALESCE(a.lw_margin, 0))) AS INTEGER) AS lw_margin,
    CAST(ROUND(SUM(COALESCE(a.lw_revenue, 0))) AS INTEGER) AS lw_revenue,
    ROUND(COALESCE(SUM(a.lw_revenue) / NULLIF(SUM(a.lw_units), 0), 0)::DECIMAL, 2) AS price,
    ROUND(COALESCE(SUM(a.msrp * a.discount) / NULLIF(SUM(a.msrp), 0), 0)::DECIMAL, 2) AS discount,
    ROUND(CASE WHEN COUNT(*) != 0
          THEN COUNT(CASE WHEN a.in_stock = 1 THEN 1 END)::FLOAT / COUNT(*)
          ELSE 0 END::DECIMAL, 4) AS in_stock_perc
FROM inventory_smart.ph_master ph
JOIN inventory_smart.article_inventory_dashboard a USING (article)
GROUP BY ph.ph_code;

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_txs_metrics_pk ON inventory_smart.mv_asv2_txs_metrics (ph_code);

-- 2. WOC: 13M woc_master rows → 43K rows grouped by ph_code
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_woc AS
SELECT
    ph.ph_code,
    ROUND(AVG(wm.woc)::NUMERIC, 2) AS woc,
    ROUND(AVG(wm.max_mod)::NUMERIC, 2) AS avg_max_mod,
    ROUND(MIN(wm.woc)::NUMERIC, 2) AS min_woc,
    ROUND(MAX(wm.woc)::NUMERIC, 2) AS max_woc,
    COUNT(DISTINCT wm.store_code) AS woc_mapped_stores_count
FROM inventory_smart.ph_master ph
JOIN inventory_smart.woc_master wm ON wm.l4_name = ph.l4_name
WHERE wm.woc IS NOT NULL
GROUP BY ph.ph_code;

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_woc_pk ON inventory_smart.mv_asv2_woc (ph_code);

-- 3. Inventory: sku_dc_* joined through product_dc mapping → grouped by ph_code
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_inventory AS
WITH ph_products AS (
    SELECT ph.ph_code, unnest(ph.product_codes) AS product_code
    FROM inventory_smart.ph_master ph
),
product_dc AS (
    SELECT pp.ph_code, pp.product_code, pmpd.dc_code
    FROM ph_products pp
    JOIN global.product_mapping_product_dc pmpd ON pmpd.product_code = pp.product_code AND pmpd.is_active
    JOIN global.distribution_centres dc ON dc.dc_code = pmpd.dc_code AND dc.is_active AND NOT dc.is_deleted
    WHERE pmpd.dc_code IN (SELECT dc_code FROM global.product_mapping_store_dc WHERE is_active)
),
sda AS (
    SELECT product_code, dc_code, SUM(COALESCE(oh,0)) AS oh, SUM(COALESCE(oo,0)) AS oo, SUM(COALESCE(it,0)) AS it
    FROM inventory_smart.sku_dc_available_units GROUP BY 1,2
),
reserv AS (
    SELECT product_code, dc_code, SUM(COALESCE(quantity,0)) AS quantity
    FROM inventory_smart.sku_dc_reserved_units GROUP BY 1,2
)
SELECT
    pd.ph_code,
    COALESCE(SUM(s.oh), 0) AS oh,
    COALESCE(SUM(s.oo), 0) AS oo,
    COALESCE(SUM(s.it), 0) AS it,
    COALESCE(SUM(r.quantity), 0) AS reserve_quantity,
    0::bigint AS allocated_units
FROM product_dc pd
LEFT JOIN sda s ON s.product_code = pd.product_code AND s.dc_code = pd.dc_code
LEFT JOIN reserv r ON r.product_code = pd.product_code AND r.dc_code = pd.dc_code
GROUP BY pd.ph_code;

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_inventory_pk ON inventory_smart.mv_asv2_inventory (ph_code);

-- 4. In-stock: article_instock → grouped by ph_code
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_instock AS
SELECT
    ph.ph_code,
    ROUND(CASE WHEN SUM(total_count) != 0
          THEN SUM(in_stock_count)::FLOAT / SUM(total_count)::FLOAT
          ELSE 0 END::NUMERIC, 4) AS in_stock_perc,
    ROUND(CASE WHEN SUM(dc_instock_total_count) != 0
          THEN SUM(dc_instock_count)::FLOAT / SUM(dc_instock_total_count)::FLOAT
          ELSE 0 END::NUMERIC * 100, 2) AS dc_instock
FROM inventory_smart.article_instock ai
JOIN inventory_smart.ph_master ph USING (article)
GROUP BY ph.ph_code;

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_instock_pk ON inventory_smart.mv_asv2_instock (ph_code);

-- 5. Before-allocation: dc_pack → grouped by ph_code
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_before_alloc AS
SELECT
    ph.ph_code,
    COALESCE(SUM(CASE WHEN dpi.pack_type = 'eaches' THEN dpc.units_in_pack * dpi.oh_pack_qty ELSE 0 END), 0) AS eaches,
    COALESCE(SUM(CASE WHEN dpi.pack_type = 'packs' THEN dpc.units_in_pack * dpi.oh_pack_qty ELSE 0 END), 0) AS packs
FROM inventory_smart.ph_master ph
JOIN inventory_smart.dc_pack_inventory dpi ON dpi.article = ph.article
JOIN inventory_smart.dc_pack_configuration dpc
    ON dpc.pack_type_id = dpi.pack_type_id AND dpc.article = dpi.article AND dpc.pack_type = dpi.pack_type
WHERE dpi.dc_code IN (SELECT dc_code FROM global.product_mapping_store_dc WHERE is_active)
GROUP BY ph.ph_code;

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_before_alloc_pk ON inventory_smart.mv_asv2_before_alloc (ph_code);

-- 6. PH Master filtered (only articles in aid): 531K → 43K
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_ph_master AS
SELECT
    ph.ph_code, ph.article, ph.l0_name, ph.l1_name, ph.l2_name, ph.l3_name,
    ph.l4_name, ph.l5_name, ph.style_color_description, ph.product_description,
    ph.sizes, ph.product_codes, ph.product_lifecycle, ph.article_status_tag,
    ph.brand, ph.channel
FROM inventory_smart.ph_master ph
WHERE ph.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard);

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_ph_master_pk ON inventory_smart.mv_asv2_ph_master (ph_code);

-- 7. PAF filtered: 1.4M → only active products with articles in aid
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_paf AS
SELECT
    paf.product_code,
    paf.article,
    paf.l0_name,
    paf.l1_name,
    paf.l2_name,
    paf.l3_name,
    paf.l4_name,
    paf.l5_name,
    paf.brand
FROM global.product_attributes_filter paf
WHERE paf.active = true AND NOT paf.is_deleted
  AND paf.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard);

CREATE UNIQUE INDEX IF NOT EXISTS mv_asv2_paf_pk ON inventory_smart.mv_asv2_paf (product_code);

-- 8. Product-DC mapping: 10M → only active mappings for products in aid
CREATE MATERIALIZED VIEW IF NOT EXISTS inventory_smart.mv_asv2_product_dc AS
SELECT DISTINCT
    pmpd.product_code,
    pmpd.dc_code
FROM global.product_mapping_product_dc pmpd
JOIN global.distribution_centres dc
    ON dc.dc_code = pmpd.dc_code AND dc.is_active AND NOT dc.is_deleted
WHERE pmpd.is_active
  AND pmpd.product_code IN (
      SELECT unnest(ph.product_codes)
      FROM inventory_smart.ph_master ph
      WHERE ph.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard)
  );

CREATE INDEX IF NOT EXISTS mv_asv2_product_dc_pc ON inventory_smart.mv_asv2_product_dc (product_code);

-- ============================================================================
-- Refresh all views (run periodically, e.g., every 5 minutes):
--
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_txs_metrics;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_woc;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_inventory;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_instock;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_before_alloc;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_paf;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_product_dc;
--   REFRESH MATERIALIZED VIEW CONCURRENTLY inventory_smart.mv_asv2_ph_master;
--
-- CONCURRENTLY allows reads during refresh (requires unique index).
-- ============================================================================
