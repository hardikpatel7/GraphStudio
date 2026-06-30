-- Exception alerts derived from store positions and thresholds.
-- alert_type: 'stockout' | 'low_stock' | 'overstock' | 'freshness_risk'
-- severity:   'critical' | 'high' | 'medium' | 'low'
CREATE OR REPLACE VIEW v_exception_alerts AS
SELECT
  CASE
    WHEN p.available_units = 0       THEN 'stockout'
    WHEN p.available_units <= p.min_stock THEN 'low_stock'
    WHEN p.on_hand_units > p.max_stock    THEN 'overstock'
  END AS alert_type,
  CASE
    WHEN p.available_units = 0       THEN 'critical'
    WHEN p.available_units <= p.min_stock AND p.days_of_supply < 1 THEN 'critical'
    WHEN p.available_units <= p.min_stock THEN 'high'
    WHEN p.on_hand_units > p.max_stock * 1.5 THEN 'high'
    ELSE 'medium'
  END AS severity,
  p.sku_code,
  p.product_name,
  p.category_l1,
  p.dark_store_id,
  p.dark_store_name,
  p.available_units,
  p.on_hand_units,
  p.days_of_supply,
  p.min_stock,
  p.max_stock,
  CASE
    WHEN p.available_units = 0
      THEN 'Out of stock — no units available to fulfil orders'
    WHEN p.available_units <= p.min_stock
      THEN FORMAT('Low stock — {} units available, min is {}; replenish now', p.available_units, p.min_stock)
    WHEN p.on_hand_units > p.max_stock
      THEN FORMAT('Overstock — {} units on hand, max is {}', p.on_hand_units, p.max_stock)
  END AS alert_message
FROM v_store_positions p
WHERE
  p.available_units = 0
  OR p.available_units <= p.min_stock
  OR p.on_hand_units > p.max_stock;
