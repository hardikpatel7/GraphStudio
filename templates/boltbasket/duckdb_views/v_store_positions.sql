-- Pre-aggregated inventory position view.
-- Joins raw position data with 7-day velocity and threshold config.
CREATE OR REPLACE VIEW v_store_positions AS
SELECT
  p.sku_code,
  s.product_name,
  s.category_l1,
  s.category_l2,
  s.brand,
  s.upc,
  s.unit_size,
  s.delivery_type,
  p.dark_store_id,
  d.dark_store_name,
  d.service_zone,
  p.on_hand_qty                                   AS on_hand_units,
  COALESCE(p.reserved_qty, 0)                     AS reserved_units,
  COALESCE(p.on_order_qty, 0)                     AS on_order_units,
  GREATEST(p.on_hand_qty - COALESCE(p.reserved_qty, 0), 0)
                                                   AS available_units,
  (p.on_hand_qty - COALESCE(p.reserved_qty, 0)) > 0
                                                   AS in_stock,
  COALESCE(v.daily_units, 0)                      AS daily_velocity,
  COALESCE(v.weekly_units, 0)                     AS weekly_velocity,
  COALESCE(v.fill_rate_pct, 100.0)                AS fill_rate_pct,
  CASE
    WHEN COALESCE(v.daily_units, 0) = 0 THEN NULL
    ELSE ROUND(p.on_hand_qty::FLOAT / v.daily_units, 1)
  END                                              AS days_of_supply,
  COALESCE(t.min_stock, 0)                         AS min_stock,
  COALESCE(t.max_stock, 9999)                      AS max_stock,
  COALESCE(t.reorder_qty, 10)                      AS reorder_qty,
  p.last_received_date,
  p.last_po_qty,
  COALESCE(p.pending_deliveries, 0)               AS pending_deliveries,
  COALESCE(r.avg_rating, 0.0)                     AS avg_customer_rating
FROM inventory_positions p
JOIN sku_master s          ON s.sku_code      = p.sku_code
JOIN dark_stores d          ON d.dark_store_id = p.dark_store_id
LEFT JOIN velocity_7d v    ON v.sku_code = p.sku_code AND v.dark_store_id = p.dark_store_id
LEFT JOIN stock_thresholds t ON t.sku_code = p.sku_code AND t.dark_store_id = p.dark_store_id
LEFT JOIN sku_store_ratings r ON r.sku_code = p.sku_code AND r.dark_store_id = p.dark_store_id
WHERE s.is_active = true;
