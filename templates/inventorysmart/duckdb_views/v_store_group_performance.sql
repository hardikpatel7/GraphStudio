-- Per-store-group rollup of sales + stockout signal, joining
-- raw_store_groups (active only) → raw_store_groups_mapping → v_aid_per_store.
-- LEFT JOIN on v_aid_per_store so groups whose stores are missing from
-- asv2_store_index still surface with 0s. Backs dv_store_group_performance.
CREATE OR REPLACE VIEW v_store_group_performance AS
SELECT
  g.name                                  AS store_group,
  g.sg_code,
  COUNT(DISTINCT m.store_code)            AS store_count,
  COALESCE(SUM(a.lw_units), 0)            AS lw_units,
  COALESCE(SUM(a.lw_revenue), 0)          AS lw_revenue,
  COALESCE(SUM(a.lw_margin), 0)           AS lw_margin,
  COALESCE(SUM(a.articles_in_stock), 0)   AS articles_in_stock,
  COALESCE(SUM(a.articles_total), 0)      AS articles_total
FROM raw_store_groups AS g
INNER JOIN raw_store_groups_mapping AS m ON m.sg_code = g.sg_code
LEFT JOIN  v_aid_per_store          AS a ON a.store_code = CAST(m.store_code AS VARCHAR)
WHERE g.is_deleted = FALSE
GROUP BY g.name, g.sg_code;
