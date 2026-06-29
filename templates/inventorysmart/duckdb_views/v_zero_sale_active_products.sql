-- Active SKUs with zero last-week sales. Predicate is
-- `article_status_tag = 'Active' AND lw_units = 0`. When the multi-week
-- sales source lands, the predicate upgrades to `8wk_units = 0`. Backs
-- dv_zero_sale_active_products.
CREATE OR REPLACE VIEW v_zero_sale_active_products AS
SELECT
  m.article, m.ph_code,
  m.l1_name, m.l2_name, m.l3_name, m.brand, m.channel,
  m.article_status_tag,
  COALESCE(t.lw_units, 0)   AS lw_units,
  COALESCE(t.lw_revenue, 0) AS lw_revenue,
  COALESCE(t.lw_margin, 0)  AS lw_margin
FROM asv2_ph_master AS m
LEFT JOIN asv2_txs_metrics_by_article AS t ON t.article = m.article
WHERE m.article_status_tag = 'Active'
  AND COALESCE(t.lw_units, 0) = 0;
