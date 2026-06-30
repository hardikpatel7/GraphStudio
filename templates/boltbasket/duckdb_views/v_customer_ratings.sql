-- Aggregated customer ratings per SKU × dark store (last 30 days).
CREATE OR REPLACE VIEW v_customer_ratings AS
SELECT
  r.sku_code,
  s.product_name,
  s.category_l1,
  r.dark_store_id,
  d.dark_store_name,
  d.service_zone,
  AVG(r.rating)::FLOAT                               AS avg_rating,
  COUNT(*)                                           AS rating_count,
  SUM(CASE WHEN r.complaint THEN 1 ELSE 0 END)       AS complaint_count,
  ROUND(100.0 * SUM(CASE WHEN r.complaint THEN 1 ELSE 0 END) / COUNT(*), 2)
                                                     AS complaint_rate_pct,
  MAX(r.survey_date)                                 AS last_rating_date
FROM delivery_surveys r
JOIN sku_master s   ON s.sku_code      = r.sku_code
JOIN dark_stores d  ON d.dark_store_id = r.dark_store_id
WHERE r.survey_date >= CURRENT_DATE - 30
GROUP BY r.sku_code, s.product_name, s.category_l1, r.dark_store_id, d.dark_store_name, d.service_zone;
