// Static data for the Bolt Basket store_positions DataView. Mirrors:
//   - server/src/handlers/store_positions.rs (26-column projection)
//   - smartstudio.db dataviews/dv_store_positions
//   - smartstudio.db filter_configs/fc_store_positions
//   - smartstudio.db dimensions/product

export const DATAVIEW_ID = "dv_store_positions";
export const SOURCE_ID = "src_store_positions";
export const DUCKDB_TABLE = "store_positions";
export const FILTER_CONFIG_ID = "fc_store_positions";
export const DIMENSION_ID = "product";

export type ColumnGroup =
  | "identity"
  | "hierarchy"
  | "location"
  | "position"
  | "performance"
  | "config"
  | "distribution";

export interface ColumnSpec {
  name: string;
  type: "VARCHAR" | "INTEGER" | "BOOLEAN" | "DOUBLE" | "DATE";
  group: ColumnGroup;
  visible: boolean;
  sortable: boolean;
  description: string;
}

// Bolt Basket — dark_store_positions DataView columns
export const COLUMNS: ColumnSpec[] = [
  // --- Identity / hierarchy ---
  { name: "sku_code", type: "VARCHAR", group: "identity", visible: true, sortable: true, description: "Unique SKU identifier" },
  { name: "product_name", type: "VARCHAR", group: "identity", visible: true, sortable: true, description: "Product display name" },
  { name: "category_l1", type: "VARCHAR", group: "hierarchy", visible: true, sortable: true, description: "Top-level product department (e.g. Dairy, Bakery, Produce)" },
  { name: "category_l2", type: "VARCHAR", group: "hierarchy", visible: true, sortable: true, description: "Product category within department" },
  { name: "brand", type: "VARCHAR", group: "hierarchy", visible: true, sortable: true, description: "Brand name" },
  { name: "upc", type: "VARCHAR", group: "identity", visible: false, sortable: false, description: "Universal product code / barcode" },
  { name: "unit_size", type: "VARCHAR", group: "identity", visible: true, sortable: false, description: "Pack size description (e.g. 500ml, 1kg, 6-pack)" },
  { name: "delivery_type", type: "VARCHAR", group: "identity", visible: true, sortable: true, description: "Delivery speed tier (express / standard / cold-chain)" },
  // --- Location ---
  { name: "dark_store_id", type: "VARCHAR", group: "location", visible: true, sortable: true, description: "Dark store / micro-fulfilment center identifier" },
  { name: "dark_store_name", type: "VARCHAR", group: "location", visible: true, sortable: true, description: "Dark store display name (city + zone)" },
  { name: "service_zone", type: "VARCHAR", group: "location", visible: true, sortable: true, description: "Delivery service zone served by this dark store" },
  // --- Inventory position ---
  { name: "on_hand_units", type: "INTEGER", group: "position", visible: true, sortable: true, description: "Units physically in the dark store (OHU)" },
  { name: "reserved_units", type: "INTEGER", group: "position", visible: true, sortable: true, description: "Units committed to in-flight orders, not yet picked" },
  { name: "on_order_units", type: "INTEGER", group: "position", visible: true, sortable: true, description: "Units on inbound replenishment orders (OOU)" },
  { name: "available_units", type: "INTEGER", group: "position", visible: true, sortable: true, description: "Pickable units = on_hand - reserved" },
  { name: "in_stock", type: "BOOLEAN", group: "position", visible: true, sortable: true, description: "True when available_units > 0" },
  // --- Performance ---
  { name: "daily_velocity", type: "DOUBLE", group: "performance", visible: true, sortable: true, description: "Average daily unit sales (rolling 7d)" },
  { name: "weekly_velocity", type: "DOUBLE", group: "performance", visible: true, sortable: true, description: "Total units sold last 7 days" },
  { name: "fill_rate_pct", type: "DOUBLE", group: "performance", visible: true, sortable: true, description: "Order-line fill rate for last 7 days (%)" },
  { name: "days_of_supply", type: "DOUBLE", group: "performance", visible: true, sortable: true, description: "Days of inventory at current daily velocity (DOS)" },
  { name: "avg_customer_rating", type: "DOUBLE", group: "performance", visible: true, sortable: true, description: "Average customer rating for this SKU at this dark store (1–5)" },
  // --- Thresholds / config ---
  { name: "min_stock", type: "INTEGER", group: "config", visible: true, sortable: true, description: "Replenishment trigger level (reorder when available_units <= min_stock)" },
  { name: "max_stock", type: "INTEGER", group: "config", visible: true, sortable: true, description: "Maximum stocking level for this dark store / SKU combination" },
  { name: "reorder_qty", type: "INTEGER", group: "config", visible: false, sortable: false, description: "Standard replenishment quantity per order" },
  // --- Distribution / replenishment ---
  { name: "last_received_date", type: "DATE", group: "distribution", visible: true, sortable: true, description: "Date of last inbound receipt from warehouse" },
  { name: "last_po_qty", type: "INTEGER", group: "distribution", visible: false, sortable: false, description: "Units in the last closed purchase order" },
  { name: "pending_deliveries", type: "INTEGER", group: "distribution", visible: true, sortable: true, description: "Number of open replenishment orders in transit" },
];

export const COLUMN_NAMES: Set<string> = new Set(COLUMNS.map((c) => c.name));

export const FILTER_CONFIG = {
  id: FILTER_CONFIG_ID,
  display_name: "Bolt Basket Store Positions Filter Configuration",
  dimension_ref: "product",
  mandatory_columns: ["category_l1"],
  cascading_rules: [
    { trigger: "category_l1", affects: ["category_l2"], type: "forward" },
  ],
  filter_columns: [
    { column: "category_l1", display_order: 0, single_select: true },
    { column: "category_l2", display_order: 1, single_select: false },
    { column: "brand", display_order: 2, single_select: false },
    { column: "sku_code", display_order: 3, single_select: false },
    { column: "delivery_type", display_order: 4, single_select: false },
    { column: "dark_store_name", display_order: 5, single_select: false },
  ],
} as const;

export const PRODUCT_DIMENSION = {
  id: "product",
  display_name: "Product",
  master_table: "bold_basket.product_catalog",
  levels: ["category_l1", "category_l2"],
  additional_filter_cols: ["sku_code", "product_name", "brand", "delivery_type"],
} as const;
