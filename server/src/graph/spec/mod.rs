//! Serde shape of a graph TOML definition.
//!
//! Mirrors the locked authoring format (Decisions 18-36 in the
//! `analyse-the-code-for-velvet-dragon` plan). The structs here are the
//! *only* on-disk shape; the runtime engine (Phase 2) lowers `GraphSpec`
//! into typed `KindId` / `MetricId` registries.
//!
//! Order matters in two places — top-to-leaf hierarchy levels, and metric
//! columns under a source — so we use `IndexMap` everywhere keys are part
//! of the data, paired with `toml`'s `preserve_order` feature.

pub mod parse;
pub mod validate;

pub use parse::from_toml;
pub use validate::{Severity, ValidationIssue, validate};

use indexmap::IndexMap;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Top-level graph definition. One TOML file = one `GraphSpec`.
///
/// Layout follows Decision 20: `id`/`display_name` at the top, then four
/// section groups — `[[sources]]`, `[[relation]]` (singular, repeated),
/// `[hierarchy.<name>]` (one or more), and `[metrics.<source_alias>]`
/// (one block per metric source).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphSpec {
    pub id: String,
    pub display_name: String,

    /// All sources used by this graph. Role (metric vs. bridge) is inferred
    /// from `attaches_at` + relations (Decision 36).
    #[serde(default)]
    pub sources: Vec<SourceSpec>,

    /// Join paths. TOML key is `[[relation]]` (singular), as locked in
    /// Decision 20. Required only when source columns don't already match
    /// the attach kind's identifying column (Decision 33).
    #[serde(default, rename = "relation")]
    pub relations: Vec<RelationSpec>,

    /// `[hierarchy.<name>]` blocks, keyed by hierarchy name (= SmartStudio
    /// dimension reference, Decision 31). `IndexMap` preserves declaration
    /// order so the first hierarchy listed is treated as primary by default.
    pub hierarchy: IndexMap<String, HierarchySpec>,

    /// `[metrics.<source_alias>]` — metrics grouped by their source. Keys
    /// in the inner map are metric ids; `MetricSpec::column` defaults to
    /// the key when absent.
    #[serde(default)]
    pub metrics: IndexMap<String, IndexMap<String, MetricSpec>>,
}

// ──────────────────────────────────────────────────────────────────────────
// Sources
// ──────────────────────────────────────────────────────────────────────────

/// One `[[sources]]` entry — a DuckDB table plus optional metadata for how
/// it attaches into hierarchies.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SourceSpec {
    /// Stable handle referenced by hierarchies, relations, and metric blocks.
    pub alias: String,

    /// DuckDB table name. Schema-qualified names are allowed.
    pub table: String,

    /// `attaches_at = "<kind>"` (single, e.g. `"article"`) or
    /// `attaches_at = ["<primary>", "<aux>", …]` (composite, Decision 28).
    /// Absent → bridge source (Decision 36); the engine materializes a
    /// cross-edge between the two hierarchies the source bridges.
    #[serde(default)]
    pub attaches_at: Option<AttachesAt>,

    /// Optional WHERE clause body (no `WHERE` keyword). Free-form SQL,
    /// validated as a sub-grammar at validate time (Decision 19).
    #[serde(default)]
    pub filter: Option<String>,
}

/// `attaches_at` accepts either a bare string (single attach kind) or
/// an array (composite attach across multiple kinds; exactly one must be
/// from the primary hierarchy, per Decision 28).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AttachesAt {
    Single(String),
    Composite(Vec<String>),
}

impl AttachesAt {
    /// Yields each attach kind in declaration order — single attach
    /// becomes a one-element iterator, composite yields each in turn.
    /// Lets callers handle both shapes uniformly.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        match self {
            AttachesAt::Single(s) => std::slice::from_ref(s).iter().map(String::as_str),
            AttachesAt::Composite(v) => v[..].iter().map(String::as_str),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Relations
// ──────────────────────────────────────────────────────────────────────────

/// One `[[relation]]` block — a directed join path with per-side
/// cardinality (Decision 24) and positional column pairing (Decision 25).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationSpec {
    pub from: RelationSide,
    pub to: RelationSide,
}

/// One side of a relation. Columns at position `i` on `from.columns`
/// pair with position `i` on `to.columns` (Decision 25). When column
/// names differ across sides, validate emits a hint (not an error).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationSide {
    /// Source alias (must match a `[[sources]]` entry).
    pub alias: String,
    /// Join columns on this side, in pairing order.
    pub columns: Vec<String>,
    /// `"1"` or `"*"`. `from="*", to="*"` is rejected at validate time
    /// (Decision 24) — bridge sources should be used instead.
    pub cardinality: Cardinality,
}

/// Per-side cardinality. Serializes as the TOML strings `"1"` / `"*"`
/// to match Decision 24's wire vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Cardinality {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "*")]
    Many,
}

// ──────────────────────────────────────────────────────────────────────────
// Hierarchy
// ──────────────────────────────────────────────────────────────────────────

/// `[hierarchy.<name>]` — one hierarchy plus its ordered levels.
///
/// The level sub-tables (`[hierarchy.<name>.<level_id>]`) live at the
/// same TOML depth as `source`. We custom-implement (de)serialization
/// instead of `#[serde(flatten)]` because flatten routes unknown keys
/// through serde's internal collector, which buckets them by hash and
/// destroys declaration order — and order is load-bearing for the
/// spine walker (top-to-leaf threading articles under their l5/...).
/// The custom impl visits keys in the order the deserializer feeds
/// them, which (with `toml`'s `preserve_order` feature) is declaration
/// order.
#[derive(Debug, Clone)]
pub struct HierarchySpec {
    /// Source alias whose rows define the hierarchy spine. Per Decision
    /// 22, MVP supports a single source per hierarchy.
    pub source: String,

    /// Levels keyed by level id, in top-to-leaf declaration order.
    pub levels: IndexMap<String, LevelSpec>,
}

impl<'de> Deserialize<'de> for HierarchySpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct HierarchyVisitor;
        impl<'de> Visitor<'de> for HierarchyVisitor {
            type Value = HierarchySpec;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a hierarchy table with `source` plus level sub-tables")
            }
            fn visit_map<M>(self, mut map: M) -> Result<HierarchySpec, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut source: Option<String> = None;
                let mut levels: IndexMap<String, LevelSpec> = IndexMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "source" {
                        if source.is_some() {
                            return Err(de::Error::duplicate_field("source"));
                        }
                        source = Some(map.next_value()?);
                    } else {
                        if levels.contains_key(&key) {
                            return Err(de::Error::custom(format!(
                                "duplicate level `{key}` in hierarchy"
                            )));
                        }
                        let spec: LevelSpec = map.next_value()?;
                        levels.insert(key, spec);
                    }
                }
                let source = source.ok_or_else(|| de::Error::missing_field("source"))?;
                Ok(HierarchySpec { source, levels })
            }
        }
        deserializer.deserialize_map(HierarchyVisitor)
    }
}

impl Serialize for HierarchySpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Emit `source` first so the rendered TOML stays consistent
        // with the authoring convention; levels follow in IndexMap
        // order (= the order they were parsed).
        let mut map = serializer.serialize_map(Some(1 + self.levels.len()))?;
        map.serialize_entry("source", &self.source)?;
        for (k, v) in &self.levels {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// One level within a hierarchy — a column whose distinct values become
/// nodes at that depth.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LevelSpec {
    /// Source column whose value is read for this level's node.
    pub column: String,

    /// Optional identity column used when `column` is a display value
    /// distinct from the join key. Defaults to `column` when omitted.
    #[serde(default)]
    pub key: Option<String>,

    /// Multi-value column delimiter for legacy VARCHAR storage
    /// (Decision 21). Mutually exclusive with `unnest`.
    #[serde(default)]
    pub split: Option<String>,

    /// `unnest = true` when `column` is a native DuckDB `LIST<…>`.
    /// Mutually exclusive with `split` (Decision 21).
    #[serde(default)]
    pub unnest: Option<bool>,
}

// ──────────────────────────────────────────────────────────────────────────
// Metrics
// ──────────────────────────────────────────────────────────────────────────

/// One metric entry inside `[metrics.<source_alias>]`. The TOML key
/// becomes the metric id; `column` defaults to the key when absent
/// (Decision 20).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricSpec {
    /// Source column read for this metric. Defaults to the metric's
    /// TOML key (= map key) when omitted.
    #[serde(default)]
    pub column: Option<String>,

    /// Aggregation function. Applied identically at attach-time (when
    /// multiple source rows map to the same node) and at parent rollup
    /// (Decision 29). Must therefore be associative.
    pub rollup: Rollup,

    /// Optional SQL expression overriding the column reference (e.g.
    /// `"oh + it"` for derived metrics). Validate at definition time
    /// (Decision 19) — engine substitutes verbatim into the source SQL.
    #[serde(default)]
    pub expr: Option<String>,
}

/// Fixed aggregation vocabulary (Decision 13). New entries require a
/// runtime implementation in `graph/rollup.rs`. Operator must be
/// associative — `first`/`last`/percentiles are out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rollup {
    Sum,
    Min,
    Max,
    Count,
    CountDistinct,
    /// Deduplicated collection (preserves no duplicates).
    Set,
    /// Collection-with-duplicates (preserves all values).
    List,
    /// Nice-to-have (Decision 13). Engine tracks sum + count internally.
    Avg,
    /// Nice-to-have. Returns `true` iff any child is truthy.
    Any,
    /// Nice-to-have. Returns `true` iff all children are truthy.
    All,
}

impl Rollup {
    /// Whether this aggregation produces a scalar (`MetricValue::Scalar`)
    /// or a collection (`MetricValue::Set` / `List`). Used by validate to
    /// type-check column compatibility (e.g., `sum` over text columns is
    /// rejected) and by the engine to size the metric slot.
    pub fn produces_collection(self) -> bool {
        matches!(self, Rollup::Set | Rollup::List)
    }
}
