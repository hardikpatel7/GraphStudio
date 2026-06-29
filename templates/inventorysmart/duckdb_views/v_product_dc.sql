-- Article ↔ DC bridge. Explodes asv2_product_dc's pipe-delimited dc_codes
-- column into one (product_code, dc_code) row per edge. Backs the
-- product_dc_bridge cross-edge on bealls-inventory-graph.
CREATE OR REPLACE VIEW v_product_dc AS
SELECT
  pd.product_code,
  CAST(TRIM(t.dc_str) AS BIGINT) AS dc_code
FROM asv2_product_dc AS pd,
     UNNEST(string_split(pd.dc_codes, '|')) AS t(dc_str)
WHERE TRIM(t.dc_str) <> '';
