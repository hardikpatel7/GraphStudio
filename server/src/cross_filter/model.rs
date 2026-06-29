//! Wire types for `POST /api/cross-filter-v2`.
//!
//! These match `inventory-smart-rust`'s
//! `impact_core::core::filters::models` shapes verbatim so the Rust
//! service can swap backends behind an env flag. We deliberately
//! deserialize with `#[serde(rename_all = "snake_case")]` /
//! `#[serde(rename_all = "lowercase")]` per the upstream conventions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Filter values can arrive as a JSON array or a single string. We
/// normalize into a Vec<String> for downstream processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Values {
    List(Vec<Value>),
    Single(String),
}

impl Values {
    /// Flatten to `Vec<String>` for membership checks. Numeric values
    /// are stringified.
    pub fn as_strings(&self) -> Vec<String> {
        match self {
            Values::List(arr) => arr
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect(),
            Values::Single(s) => vec![s.clone()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operator {
    In,
    Eq,
    Gt,
    Lt,
    Gte,
    Lte,
    Ne,
    IsEq,
    IsNot,
    #[serde(rename = "in")]
    InEq,
    NotIn,
    Like,
    ILike,
    /// Inclusive range. Takes exactly two values: `[lo, hi]`. SQL emits
    /// `col BETWEEN lo AND hi`. Both values are parsed as f64.
    Between,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Others,
    Custom,
    Vendor,
    Sales,
    Dc,
    Season,
    Ticketing,
    Store,
    Product,
    ProductStore,
    UserHierarchy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterType {
    Cascaded,
    NonCascaded,
}

impl Default for FilterType {
    fn default() -> Self {
        FilterType::NonCascaded
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribute {
    pub attribute_name: String,
    pub dimension: Dimension,
    #[serde(default)]
    pub filter_type: FilterType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    #[serde(default)]
    pub filter_id: Option<String>,
    // Aliases tolerate the simpler `{column, value, op}` shape the LLM
    // agent emits naturally — the canonical filter shape uses
    // `{attribute_name, values, operator}` but the model keeps
    // shortening it. Accepting both at the serde layer keeps the
    // graph + dataview filter paths from silently dropping the field.
    #[serde(alias = "column", alias = "attribute")]
    pub attribute_name: String,
    #[serde(default)]
    pub dimension: Option<Dimension>,
    #[serde(alias = "value")]
    pub values: Values,
    #[serde(default = "default_operator", alias = "op")]
    pub operator: Operator,
    #[serde(default)]
    pub extra: Option<Value>,
}

fn default_operator() -> Operator {
    Operator::In
}

/// Top-level request body. Matches `FilterPayload` upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterPayload {
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub filter_type: FilterType,
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub is_urm_filter: bool,
    #[serde(default)]
    pub screen_name: Option<String>,
    #[serde(default)]
    pub application_code: Option<i32>,
    /// Caller's user identifier. Optional for service-to-service
    /// callers; required when `is_urm_filter` is true.
    #[serde(default)]
    pub user_code: Option<i32>,
    /// Combined with `user_code` to look up the entitled set in the
    /// UAM store. Mirrors the upstream `(user_code, acl_code)` unique
    /// constraint on `global.user_access_hierarchy_mapping`.
    #[serde(default)]
    pub acl_code: Option<i32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FilterResponse {
    pub total: Option<i32>,
    pub page: Option<i32>,
    pub count: i32,
    pub status: bool,
    /// `attribute_name → distinct values` after applying filters and
    /// (optionally) the UAM entitled set. Keys come from the
    /// `attributes` array on the request.
    pub data: HashMap<String, Vec<String>>,
    pub message: String,
}
