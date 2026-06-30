-- Open distribution/replenishment orders enriched with dark store info.
CREATE OR REPLACE VIEW v_distribution_orders AS
SELECT
  o.po_number,
  o.sku_code,
  s.product_name,
  o.dark_store_id,
  d.dark_store_name,
  d.service_zone,
  o.warehouse_id,
  o.ordered_qty,
  COALESCE(o.received_qty, 0)   AS received_qty,
  o.ordered_qty - COALESCE(o.received_qty, 0)
                                AS outstanding_qty,
  o.eta_date,
  o.status,
  o.created_at
FROM replenishment_orders o
JOIN sku_master s          ON s.sku_code      = o.sku_code
JOIN dark_stores d          ON d.dark_store_id = o.dark_store_id
WHERE o.status NOT IN ('cancelled', 'closed');
