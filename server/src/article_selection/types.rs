//! Article Selection data types.
//!
//! Lifted from V4. RCL-related types (RclRule / RclDcPolicyRow /
//! RclConstraintRow) and the V4 watcher's `RclUpdate` are dropped — RCL data
//! now lives in the `rcl` crate via [`rcl::RuleStore`].

use serde::{Deserialize, Serialize, Serializer};

/// Serializer for fields that hold a JSON-array-as-text in the struct
/// (e.g. `sizes` = `["1060","1070",...]`). Emits the parsed JSON value
/// directly so downstream consumers see arrays, not quoted JSON strings.
/// On parse failure (or empty string), emits null (legacy v2 returns null
/// for empty arrays/objects in these columns).
fn ser_json_text<S: Serializer>(s: &String, ser: S) -> Result<S::Ok, S::Error> {
    if s.is_empty() {
        return ser.serialize_none();
    }
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => v.serialize(ser),
        Err(_) => ser.serialize_str(s),
    }
}

/// Same as [`ser_json_text`] but for `Option<String>` fields. `None` and
/// `Some("")` both emit null.
fn ser_json_text_opt<S: Serializer>(o: &Option<String>, ser: S) -> Result<S::Ok, S::Error> {
    match o {
        None => ser.serialize_none(),
        Some(s) if s.is_empty() => ser.serialize_none(),
        Some(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) => v.serialize(ser),
            Err(_) => ser.serialize_str(s),
        }
    }
}

/// One row in the Article Selection result set, keyed by `ph_code`.
/// Pre-computed from 8 pre-aggregated PG queries (asv2_* MVs) + RCL
/// resolution + store/DC/store-group expansion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleSelectionRow {
    // ── PH Master fields ──
    /// Numeric in PG (`bigint`); kept numeric here so the JSON wire format
    /// matches `inventory_smart.article_selection_list_v2` byte-for-byte.
    pub ph_code: i64,
    pub article: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub style_color_description: String,
    pub product_description: String,
    /// JSON-array text (`["1060","1070",...]`). Serialized as a real JSON
    /// array on the wire to match legacy v2's `sizes` (text[]).
    #[serde(serialize_with = "ser_json_text")]
    pub sizes: String,
    /// JSON-array text. Same wire-format treatment as `sizes` — legacy v2
    /// returns `upc` as `text[]`.
    #[serde(serialize_with = "ser_json_text")]
    pub upc: String,
    /// Empty string emits as null (legacy v2 returns null when the column
    /// has no value).
    #[serde(serialize_with = "ser_json_text")]
    pub product_life_cycle: String,
    pub article_status_tag: String,
    pub brand: String,
    /// JSON-array text. Legacy v2: `STRING_TO_ARRAY(ph.channel, ',')`.
    #[serde(serialize_with = "ser_json_text")]
    pub channel: String,

    // ── Inventory ──
    pub oh: i64,
    pub oo: i64,
    pub it: i64,
    pub reserve_quantity: i64,
    pub allocated_units: i64,
    pub net_available_inventory: i64,
    /// Per-size → per-DC nested JSON. Populated by Bucket 3 (sku_dc_*).
    /// `None` when the V7 pipeline hasn't computed it yet — emitting `null`
    /// (instead of `""`) is what legacy v2 returns when there's no data.
    #[serde(serialize_with = "ser_json_text_opt")]
    pub oh_map: Option<String>,
    #[serde(serialize_with = "ser_json_text_opt")]
    pub rq_map: Option<String>,
    #[serde(serialize_with = "ser_json_text_opt")]
    pub au_map: Option<String>,
    pub last_allocated: Option<String>,
    /// Always `None` for now — legacy v2 hardcodes `null` here. Kept as a
    /// real column to preserve the protocol shape; a future bucket can
    /// populate it from `dc_pack_inventory.pack_type_id` if needed.
    pub pack_type_id: Option<i64>,

    // ── Transaction metrics ──
    pub lw_units: i64,
    pub lw_margin: i64,
    pub lw_revenue: i64,
    /// `None` when there's no row in `asv2_txs_metrics` for this ph_code —
    /// matches legacy v2 which returns `null` instead of `0.0` defaults.
    pub price: Option<f64>,
    pub discount: Option<f64>,
    pub in_stock_perc: Option<f64>,

    // ── Constraints ──
    /// `None` when no RCL constraints resolve for this PH's product_codes.
    pub aps: Option<f64>,
    pub min_stock: i64,
    pub max_stock: i64,
    pub min_stock_validator: i64,
    pub max_stock_validator: i64,
    pub mapped_stores_count: i64,
    pub wos: i64,
    pub avg_max_mod: i64,
    pub min_woc: i64,
    pub max_woc: i64,

    // ── Config ──
    /// `None` when DC / SG / allocation-rule resolution finds nothing —
    /// legacy v2 returns `null`, not an empty array, when joins miss.
    #[serde(serialize_with = "ser_json_text_opt")]
    pub dcs: Option<String>,
    #[serde(serialize_with = "ser_json_text_opt")]
    pub store_groups: Option<String>,
    pub beginning_available_to_allocate_eaches: i64,
    pub beginning_available_to_allocate_packs: i64,
    #[serde(serialize_with = "ser_json_text_opt")]
    pub allocation_rules: Option<String>,

    // ── Bucket 3 fields ────────────────────────────────────────────────
    /// JSON array of `store_code`s mapped to this PH. Approximated from
    /// the SG-default expansion (compute_store_groups). Legacy v2 uses
    /// `constraints_resolved × psm_ph_store` per-PH, which V7 doesn't
    /// have — the SG-based list is close.
    #[serde(serialize_with = "ser_json_text_opt")]
    pub mapped_stores: Option<String>,
    /// `min_type` from the PH's allocation rule's `values->>min_type`,
    /// falling back to `dc_store_policy_user_rule WHERE rule_code=1 AND
    /// rule_type='dc-store-rule'`.
    pub min_type: Option<String>,
    /// JSON array of product profiles bound to the PH.
    #[serde(serialize_with = "ser_json_text_opt")]
    pub product_profiles: Option<String>,
    /// JSON array of size_names ordered by paf.ord.
    #[serde(serialize_with = "ser_json_text_opt")]
    pub size_names: Option<String>,
}

/// Intermediate: PH master row from `mv_asv2_ph_master`.
#[derive(Debug, Clone)]
pub struct PhMasterRow {
    pub ph_code: String,
    pub article: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub style_color_description: String,
    pub product_description: String,
    pub sizes: String,
    pub product_codes: String,
    pub product_life_cycle: String,
    pub article_status_tag: String,
    pub brand: String,
    pub channel: String,
}

/// Pre-aggregated transaction metrics per ph_code (from `mv_asv2_txs_metrics`).
#[derive(Debug, Clone, Default)]
pub struct TxsMetrics {
    pub lw_units: i64,
    pub lw_margin: i64,
    pub lw_revenue: i64,
    pub price: f64,
    pub discount: f64,
    pub in_stock_perc: f64,
}

/// Pre-aggregated inventory per ph_code (from `mv_asv2_inventory`).
#[derive(Debug, Clone, Default)]
pub struct InventoryAgg {
    pub oh: i64,
    pub oo: i64,
    pub it: i64,
    pub reserve_quantity: i64,
    pub allocated_units: i64,
}

/// Pre-aggregated WOC per ph_code (from `mv_asv2_woc`).
#[derive(Debug, Clone, Default)]
pub struct WocAgg {
    pub woc: f64,
    pub avg_max_mod: f64,
    pub min_woc: f64,
    pub max_woc: f64,
}

/// In-stock percentages per ph_code (from `mv_asv2_instock`).
#[derive(Debug, Clone, Default)]
pub struct InstockAgg {
    pub in_stock_perc: f64,
    pub dc_instock: f64,
}

/// Before-allocation per ph_code (from `mv_asv2_before_alloc`).
#[derive(Debug, Clone, Default)]
pub struct BeforeAllocAgg {
    pub eaches: i64,
    pub packs: i64,
}

/// Aggregated constraints per ph_code, computed from RCL constraint rows.
#[derive(Debug, Clone, Default)]
pub struct ConstraintsAgg {
    pub aps: f64,
    pub wos: f64,
    pub min_stock: f64,
    pub max_stock: f64,
    pub min_stock_validator: f64,
    pub max_stock_validator: f64,
    pub mapped_stores_count: i64,
    /// Sorted list of `store_code`s after PSA→store expansion + product
    /// eligibility filter. Surfaces as `ArticleSelectionRow.mapped_stores`
    /// to mirror legacy v2's `cd.mapped_stores`.
    pub mapped_stores: Vec<String>,
}

/// Raw row from `mv_asv2_paf` — a product's hierarchy attributes.
#[derive(Debug, Clone)]
pub struct PafRow {
    pub article: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub brand: String,
}
