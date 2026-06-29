// Static data for the article_selection DataView. Mirrors:
//   - server/src/handlers/article_selection.rs (46-column projection)
//   - smartstudio.db dataviews/dv_article_selection_v7
//   - smartstudio.db filter_configs/fc_877c14152bf9
//   - smartstudio.db dimensions/product

export const DATAVIEW_ID = "dv_article_selection_v7";
export const SOURCE_ID = "src_article_selection";
export const DUCKDB_TABLE = "article_selection";
export const FILTER_CONFIG_ID = "fc_877c14152bf9";
export const DIMENSION_ID = "product";

export type ColumnGroup =
  | "hierarchy"
  | "position"
  | "per_node_map"
  | "performance"
  | "policy"
  | "allocation";

export interface ColumnSpec {
  name: string;
  type: "VARCHAR" | "BIGINT" | "DOUBLE";
  group: ColumnGroup;
  description: string;
}

export const COLUMNS: ColumnSpec[] = [
  // hierarchy / identity
  { name: "ph_code", type: "VARCHAR", group: "hierarchy", description: "Product hierarchy code (style-color level identifier)." },
  { name: "article", type: "VARCHAR", group: "hierarchy", description: "Article (SKU) identifier." },
  { name: "l0_name", type: "VARCHAR", group: "hierarchy", description: "Top of product hierarchy (broadest)." },
  { name: "l1_name", type: "VARCHAR", group: "hierarchy", description: "Hierarchy level 1 (e.g., 'Women', 'Men')." },
  { name: "l2_name", type: "VARCHAR", group: "hierarchy", description: "Hierarchy level 2." },
  { name: "l3_name", type: "VARCHAR", group: "hierarchy", description: "Hierarchy level 3." },
  { name: "l4_name", type: "VARCHAR", group: "hierarchy", description: "Hierarchy level 4." },
  { name: "l5_name", type: "VARCHAR", group: "hierarchy", description: "Hierarchy level 5 (most specific)." },
  { name: "style_color_description", type: "VARCHAR", group: "hierarchy", description: "Human-readable style/color." },
  { name: "product_description", type: "VARCHAR", group: "hierarchy", description: "Human-readable product description." },
  { name: "sizes", type: "VARCHAR", group: "hierarchy", description: "Sizes carried (often a comma list or JSON-ish string)." },
  { name: "upc", type: "VARCHAR", group: "hierarchy", description: "UPC / barcode." },
  { name: "product_life_cycle", type: "VARCHAR", group: "hierarchy", description: "Lifecycle stage (e.g., Active, Discontinued)." },
  { name: "article_status_tag", type: "VARCHAR", group: "hierarchy", description: "Article-level status tag." },
  { name: "brand", type: "VARCHAR", group: "hierarchy", description: "Brand name (e.g., 'FILA')." },
  { name: "channel", type: "VARCHAR", group: "hierarchy", description: "Sales channel." },

  // inventory position
  { name: "oh", type: "BIGINT", group: "position", description: "On-hand units across all nodes." },
  { name: "oo", type: "BIGINT", group: "position", description: "On-order units (PO not yet received)." },
  { name: "it", type: "BIGINT", group: "position", description: "In-transit units." },
  { name: "reserve_quantity", type: "BIGINT", group: "position", description: "Units reserved (held back from allocation)." },
  { name: "allocated_units", type: "BIGINT", group: "position", description: "Units already allocated to stores." },
  { name: "net_available_inventory", type: "BIGINT", group: "position", description: "OH + OO + IT - reserve - allocated." },

  // per-node maps (JSON-ish strings)
  { name: "oh_map", type: "VARCHAR", group: "per_node_map", description: "Per-DC/node on-hand breakdown (JSON-ish)." },
  { name: "rq_map", type: "VARCHAR", group: "per_node_map", description: "Per-node reserve-quantity breakdown." },
  { name: "au_map", type: "VARCHAR", group: "per_node_map", description: "Per-store allocated-units breakdown." },
  { name: "last_allocated", type: "VARCHAR", group: "per_node_map", description: "Most-recent allocation timestamp/marker." },
  { name: "dcs", type: "VARCHAR", group: "per_node_map", description: "DCs holding this article." },
  { name: "store_groups", type: "VARCHAR", group: "per_node_map", description: "Store groups carrying this article." },

  // performance
  { name: "lw_units", type: "BIGINT", group: "performance", description: "Last-week units sold." },
  { name: "lw_margin", type: "BIGINT", group: "performance", description: "Last-week margin." },
  { name: "lw_revenue", type: "BIGINT", group: "performance", description: "Last-week revenue." },
  { name: "price", type: "DOUBLE", group: "performance", description: "Current ticket price." },
  { name: "discount", type: "DOUBLE", group: "performance", description: "Applied discount %." },
  { name: "in_stock_perc", type: "DOUBLE", group: "performance", description: "% of eligible stores with stock." },
  { name: "aps", type: "DOUBLE", group: "performance", description: "Average per-store sales rate." },

  // policy / RCL-resolved
  { name: "min_stock", type: "BIGINT", group: "policy", description: "Min stock policy threshold." },
  { name: "max_stock", type: "BIGINT", group: "policy", description: "Max stock policy threshold." },
  { name: "min_stock_validator", type: "BIGINT", group: "policy", description: "Validation flag/value for min_stock." },
  { name: "max_stock_validator", type: "BIGINT", group: "policy", description: "Validation flag/value for max_stock." },
  { name: "mapped_stores_count", type: "BIGINT", group: "policy", description: "Number of stores eligible to carry this article via RCL." },
  { name: "wos", type: "BIGINT", group: "policy", description: "Weeks of supply (current cover)." },
  { name: "avg_max_mod", type: "BIGINT", group: "policy", description: "Average/max modulation parameter." },
  { name: "min_woc", type: "BIGINT", group: "policy", description: "Lower bound for weeks-of-cover target." },
  { name: "max_woc", type: "BIGINT", group: "policy", description: "Upper bound for weeks-of-cover target." },

  // allocation inputs
  { name: "beginning_available_to_allocate_eaches", type: "BIGINT", group: "allocation", description: "Eaches available to allocate at run start." },
  { name: "beginning_available_to_allocate_packs", type: "BIGINT", group: "allocation", description: "Packs available to allocate at run start." },
  { name: "allocation_rules", type: "VARCHAR", group: "allocation", description: "Resolved allocation rules JSON-ish string." },
];

export const COLUMN_NAMES: Set<string> = new Set(COLUMNS.map((c) => c.name));

export const FILTER_CONFIG = {
  id: FILTER_CONFIG_ID,
  display_name: "Default Product Filter Configuration",
  dimension_ref: "product",
  mandatory_columns: ["l1_name"],
  cascading_rules: [
    { trigger: "l1_name", affects: ["l2_name", "l3_name", "l4_name"], type: "forward" },
  ],
  filter_columns: [
    { column: "l1_name", display_order: 0, single_select: true },
    { column: "l2_name", display_order: 1, single_select: false },
    { column: "l3_name", display_order: 2, single_select: false },
    { column: "l4_name", display_order: 3, single_select: false },
    { column: "l5_name", display_order: 4, single_select: false },
    { column: "brand", display_order: 5, single_select: false },
    { column: "article", display_order: 6, single_select: false },
    { column: "product_code", display_order: 7, single_select: false },
  ],
} as const;

export const PRODUCT_DIMENSION = {
  id: "product",
  display_name: "Product",
  master_table: "global.product_attributes_filter",
  levels: ["l0_name", "l1_name", "l2_name", "l3_name", "l4_name", "l5_name", "l6_name"],
  additional_filter_cols: ["product_code", "article", "style_color_id", "brand"],
} as const;
