-- Per-store sales + stockout signal. Unpivots asv2_aid_per_store's
-- positional HUGEINT[] arrays via asv2_store_index into one row per
-- store_code. Backs the `aid_per_store` metric source on
-- bealls-inventory-graph and dv_store_group_performance.
CREATE OR REPLACE VIEW v_aid_per_store AS
SELECT
  idx.store_code,
  SUM(aid.lw_units[idx.idx + 1]::BIGINT)   AS lw_units,
  SUM(aid.lw_revenue[idx.idx + 1]::BIGINT) AS lw_revenue,
  SUM(aid.lw_margin[idx.idx + 1]::BIGINT)  AS lw_margin,
  SUM(aid.in_stock[idx.idx + 1]::BIGINT)   AS articles_in_stock,
  COUNT(*)                                  AS articles_total
FROM asv2_aid_per_store AS aid
CROSS JOIN asv2_store_index AS idx
GROUP BY idx.store_code;
