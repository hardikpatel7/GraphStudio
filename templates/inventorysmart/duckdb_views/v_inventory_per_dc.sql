-- Per-DC inventory totals. Unpivots asv2_inventory_per_dc's positional
-- HUGEINT[] arrays via asv2_dc_index into one row per dc_code. Backs the
-- `inventory_per_dc` metric source on bealls-inventory-graph.
CREATE OR REPLACE VIEW v_inventory_per_dc AS
SELECT
  idx.dc_code,
  SUM(inv.oh[idx.idx + 1]::BIGINT)                AS oh,
  SUM(inv.oo[idx.idx + 1]::BIGINT)                AS oo,
  SUM(inv.it[idx.idx + 1]::BIGINT)                AS it,
  SUM(inv.reserve_quantity[idx.idx + 1]::BIGINT)  AS reserve_quantity
FROM asv2_inventory_per_dc AS inv
CROSS JOIN asv2_dc_index AS idx
GROUP BY idx.dc_code;
