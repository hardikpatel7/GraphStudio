//! Agent tool registry. Each Tool wraps a `crate::service::*` function and
//! exposes its argument shape as JSONSchema for the LLM.
//!
//! Tools are dispatched by Rig through the `rig::tool::Tool` trait. Metering,
//! SSE event emission, caching, timeouts, and the response-size cap all live
//! in the `PromptHook` (`agent/meter/hook.rs`) — that keeps Tool impls thin
//! and side-effect free.
//!
//! v1 ships Inventory-workspace tools only. Adding a workspace kind = add a
//! new module here + extend `for_kind` below.

use std::sync::Arc;

use rig::{completion::ToolDefinition, tool::Tool, tool::ToolDyn};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

/// Maximum size (in bytes of JSON-serialized output) a single tool call can
/// return to the model. Larger results are converted to a structured
/// truncation envelope — the JSON stays valid and the model can see how
/// many rows/items were elided.
///
/// Sized small enough that an entire 16-turn tool loop fits in
/// gpt-4o-mini's 128K window. The earlier 60K cap meant 4 fat
/// responses ≈ 60K tokens of tool history — enough to push the
/// agent into context-overflow loops where it kept calling tools
/// trying to make sense of truncated data. 16K bytes ≈ 4K tokens;
/// a model getting more than that should narrow with `limit` or
/// `group_by`, not get a raw firehose. For graph dataviews the
/// 10-row aggregated answer is usually <2K, so this only bites
/// the "I forgot a `limit`" path.
const MAX_TOOL_OUTPUT_BYTES: usize = 16_000;

/// Structurally truncate a tool's JSON output to `MAX_TOOL_OUTPUT_BYTES`.
/// Two strategies:
///   1. Top-level array (`list_*` shape) → keep the first N items that fit.
///   2. Object with a `rows` array (`dataview_read` / `*_query` shape) →
///      keep the first N rows, retain other fields.
///   3. Anything else over budget → stringify and head-truncate, wrapped in
///      a `{ truncated, original_bytes, head, note }` envelope.
fn truncate_for_model(v: Value) -> Value {
    let body_len = match serde_json::to_string(&v) {
        Ok(s)  => s.len(),
        Err(_) => return v,
    };
    if body_len <= MAX_TOOL_OUTPUT_BYTES { return v; }

    if let Value::Array(arr) = &v {
        let kept = trim_array(arr, MAX_TOOL_OUTPUT_BYTES);
        let returned = kept.len();
        let total    = arr.len();
        return json!({
            "truncated":       true,
            "original_count":  total,
            "returned_count":  returned,
            "original_bytes":  body_len,
            "items":           kept,
            "note": format!(
                "Returned {returned} of {total} items; remainder elided to fit the context window. \
                 Refine your query (filter / limit / group_by) for a tighter result."
            ),
        });
    }

    if let Value::Object(obj) = &v {
        if let Some(Value::Array(rows)) = obj.get("rows") {
            let kept = trim_array(rows, MAX_TOOL_OUTPUT_BYTES);
            let returned = kept.len();
            let total    = rows.len();
            let mut out = obj.clone();
            out.insert("rows".into(), Value::Array(kept));
            out.insert("truncated".into(), json!(true));
            out.insert("original_row_count".into(), json!(total));
            out.insert("returned_row_count".into(), json!(returned));
            out.insert(
                "truncation_note".into(),
                json!(format!(
                    "Returned {returned} of {total} rows; remainder elided. \
                     Refine your query (smaller limit / filter / group_by) for a tighter result."
                )),
            );
            return Value::Object(out);
        }
    }

    // Last resort — non-array, non-rows object. Head-truncate the JSON
    // string so the model at least sees the prefix.
    let s = serde_json::to_string(&v).unwrap_or_default();
    let head: String = s.chars().take(MAX_TOOL_OUTPUT_BYTES).collect();
    json!({
        "truncated":      true,
        "original_bytes": body_len,
        "head":           head,
        "note": "Tool output exceeded the per-call budget. Head-truncated (JSON tail elided).",
    })
}

fn trim_array(arr: &[Value], budget: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let mut size = 64; // approximate envelope overhead
    for item in arr {
        let s = serde_json::to_string(item).map(|s| s.len()).unwrap_or(0) + 2;
        if size + s > budget { break; }
        out.push(item.clone());
        size += s;
    }
    out
}

/// Wraps a boxed `ToolDyn` to enforce two cross-cutting concerns uniformly:
///
/// 1. **LRU cache lookup/store** — keyed on `(tool_name, args_hash)` with a
///    5 min TTL (`cache::DEFAULT_TTL`). Identical tool calls within the
///    same session return the cached JSON without re-executing. Stale data
///    risk is bounded by the TTL.
///
/// 2. **Output truncation** — every JSON result passes through
///    `truncate_for_model` before reaching the model, so a wide
///    `dataview_read` or `clickhouse_query` can't blow the context window.
///
/// Applied uniformly in `make_tool`. Individual `Tool::call` impls stay
/// simple; ONE place enforces the per-call budget + the cache contract.
struct AgentToolDyn {
    inner: Box<dyn ToolDyn>,
    cache: Arc<super::cache::ToolCache>,
}

impl ToolDyn for AgentToolDyn {
    fn name(&self) -> String { self.inner.name() }

    fn definition<'a>(
        &'a self,
        prompt: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, ToolDefinition> {
        self.inner.definition(prompt)
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, Result<String, rig::tool::ToolError>> {
        let name = self.name();
        let key = super::cache::ToolCache::key(&name, stable_hash(&args));
        Box::pin(async move {
            // Cache lookup. Stored values are already truncated, so we can
            // re-serialize and return directly without paying the truncation
            // cost again.
            if let Some(hit) = self.cache.get(&key, super::cache::DEFAULT_TTL) {
                return Ok(serde_json::to_string(&hit).unwrap_or_default());
            }
            let raw = self.inner.call(args).await?;
            // Parse → truncate → cache. If the underlying call produced
            // non-JSON output (shouldn't happen for our tools), pass it
            // through unchanged (and don't cache).
            let v: Value = match serde_json::from_str(&raw) {
                Ok(v)  => v,
                Err(_) => return Ok(raw),
            };
            let trimmed = truncate_for_model(v);
            // Only cache successful tool outputs. Error envelopes
            // (`{"error": "..."}`) shouldn't be served from cache on the
            // next attempt — the model often retries with a fix.
            let cacheable = !matches!(&trimmed,
                Value::Object(m) if m.contains_key("error") && m.len() == 1);
            if cacheable {
                self.cache.put(key, trimmed.clone());
            }
            Ok(serde_json::to_string(&trimmed).unwrap_or(raw))
        })
    }
}

fn stable_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn wrap(inner: Box<dyn ToolDyn>, cache: Arc<super::cache::ToolCache>) -> Box<dyn ToolDyn> {
    Box::new(AgentToolDyn { inner, cache })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceKind {
    Inventory,
    Item,
    Pricing,
    Assort,
    Plan,
}

impl WorkspaceKind {
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "inventory" => WorkspaceKind::Inventory,
            "item"      => WorkspaceKind::Item,
            "pricing"   => WorkspaceKind::Pricing,
            "assort"    => WorkspaceKind::Assort,
            "plan"      => WorkspaceKind::Plan,
            _           => return None,
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceKind::Inventory => "inventory",
            WorkspaceKind::Item      => "item",
            WorkspaceKind::Pricing   => "pricing",
            WorkspaceKind::Assort    => "assort",
            WorkspaceKind::Plan      => "plan",
        }
    }
}

/// Build the tool set for a given workspace kind by querying
/// `workspace_kind_tools` for allowed tool names and instantiating each via
/// the registry below. Unknown names (e.g. legacy rows for a tool that's
/// since been removed) log a warning and are skipped. Returns an empty
/// vec when no rows exist — the prompt route treats that as "backend not
/// yet configured" and rejects the prompt.
pub fn for_kind(state: Arc<AppState>, kind: WorkspaceKind) -> Vec<Box<dyn ToolDyn>> {
    let rows = state.agent.db.query(
        "SELECT tool_name FROM workspace_kind_tools WHERE kind = ? ORDER BY tool_name",
        &[&kind.as_str()],
    );
    let names: Vec<String> = match rows {
        Ok(rs) => rs
            .into_iter()
            .filter_map(|r| r.get("tool_name").and_then(|v| v.as_str()).map(String::from))
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "[agent] for_kind: workspace_kind_tools query failed");
            return Vec::new();
        }
    };
    names
        .iter()
        .filter_map(|n| {
            let t = make_tool(n, state.clone());
            if t.is_none() {
                tracing::warn!(tool = %n, kind = %kind.as_str(),
                    "[agent] workspace_kind_tools references unknown tool — skipping");
            }
            t
        })
        .collect()
}

/// Tool-name → instance factory. The single source of truth for what
/// tool names map to which struct; the `workspace_kind_tools` table just
/// selects from this set. Every returned tool is wrapped by
/// `TruncatingToolDyn` so oversize outputs (e.g. a wide `dataview_read`
/// result) can't blow the model's context window.
fn make_tool(name: &str, state: Arc<AppState>) -> Option<Box<dyn ToolDyn>> {
    let cache = state.agent.cache.clone();
    let inner: Box<dyn ToolDyn> = match name {
        "list_dataviews"        => Box::new(ListDataviews { state }),
        "describe_dataview"     => Box::new(DescribeDataview { state }),
        "introspect_dataview"   => Box::new(IntrospectDataview { state }),
        "dataview_read"         => Box::new(DataviewRead { state }),
        "list_graphs"           => Box::new(ListGraphs { state }),
        "describe_graph"        => Box::new(DescribeGraph { state }),
        "graph_node"            => Box::new(GraphNode { state }),
        "graph_traverse"        => Box::new(GraphTraverse { state }),
        "graph_cross_filter"    => Box::new(GraphCrossFilter { state }),
        "list_sources"          => Box::new(ListSources { state }),
        "describe_source"       => Box::new(DescribeSource { state }),
        "list_connections"      => Box::new(ListConnections { state }),
        "duckdb_query"          => Box::new(DuckdbQuery { state }),
        "clickhouse_query"      => Box::new(ClickhouseQuery { state }),
        "clickhouse_dictionary" => Box::new(ClickhouseDictionary { state }),
        "resolve_filter_values" => Box::new(ResolveFilterValues { state }),
        _ => return None,
    };
    Some(wrap(inner, cache))
}

// ── shared types ─────────────────────────────────────────────────────────

/// Catch-all error for Tool impls. We collapse `ServiceError` and anything
/// else into a single variant so Rig's `Tool::Error` is uniform; the
/// metering hook inspects the result string and records status accordingly.
#[derive(Debug)]
pub struct ToolError(pub String);
impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Prefix is load-bearing: `meter::hook::on_tool_result` decides
        // status="error" when the serialized tool result starts with
        // "ToolCallError" or "JsonError". Without the prefix every
        // ServiceError-wrapped 400 / 404 gets recorded as status="ok"
        // and the widget footer can't tell the agent looped on errors.
        write!(f, "ToolCallError: {}", self.0)
    }
}
impl std::error::Error for ToolError {}
impl From<crate::service::ServiceError> for ToolError {
    fn from(e: crate::service::ServiceError) -> Self { ToolError(e.to_string()) }
}
impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self { ToolError(e.to_string()) }
}

#[derive(Debug, Default, Deserialize)]
pub struct EmptyArgs {}

#[derive(Debug, Deserialize)]
pub struct IdArgs { pub id: String }

// ── DataViews ────────────────────────────────────────────────────────────

pub struct ListDataviews { state: Arc<AppState> }
impl Tool for ListDataviews {
    const NAME: &'static str = "list_dataviews";
    type Error = ToolError;
    type Args = EmptyArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List all DataViews in this GraphStudio tenant. Returns id, display_name and metadata for each. Use this first when the user asks about available data.".into(),
            parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        }
    }
    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        let rows = crate::service::dataviews::list(&self.state).await?;
        Ok(Value::Array(rows))
    }
}

pub struct DescribeDataview { state: Arc<AppState> }
impl Tool for DescribeDataview {
    const NAME: &'static str = "describe_dataview";
    type Error = ToolError;
    type Args = IdArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return full metadata for one DataView (columns, dimensions, source binding, sort). Call list_dataviews first to discover ids.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "DataView id" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::dataviews::describe(&self.state, &a.id).await?)
    }
}

pub struct IntrospectDataview { state: Arc<AppState> }
impl Tool for IntrospectDataview {
    const NAME: &'static str = "introspect_dataview";
    type Error = ToolError;
    type Args = IdArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Probe a DataView's source and return the column projection (name + declared type) along with the engine that resolved it (pg / duckdb / clickhouse / graph). Cheaper than dataview_read when you only need the schema. Call list_dataviews to discover ids.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "DataView id" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::dataviews::introspect(self.state.clone(), a.id).await?)
    }
}

/// `dataview_read` Args. Mirrors the typed `DataReq` accepted by the
/// underlying handler — all fields optional so the model only specifies
/// what it needs. `filters` / `group_by` / `aggregates` / `having` are
/// passed through as raw JSON (their nested shapes are documented in the
/// tool description below); the handler validates them at the boundary.
#[derive(Debug, Default, Deserialize, serde::Serialize)]
pub struct DataviewReadArgs {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_col: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_total: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregates: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub having: Option<Value>,
}

pub struct DataviewRead { state: Arc<AppState> }
impl Tool for DataviewRead {
    const NAME: &'static str = "dataview_read";
    type Error = ToolError;
    type Args = DataviewReadArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Read paginated rows from a DataView's source. Handles PG / DuckDB / ClickHouse / parquet / article-graph sources uniformly. Returns {rows, total, columns, sql}. Defaults: limit=100, offset=0, no sort, total computed (set skip_total=true to skip when paging). `filters` is a v1 FilterPayload-style array; `group_by`+`aggregates`+`having` apply server-side aggregation. Call list_dataviews/describe_dataview/introspect_dataview first to understand the schema.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":        { "type": "string", "description": "DataView id" },
                    "limit":     { "type": "integer", "minimum": 1 },
                    "offset":    { "type": "integer", "minimum": 0 },
                    "sort_col":  { "type": "string" },
                    "sort_dir":  { "type": "string", "enum": ["ASC", "DESC", "asc", "desc"] },
                    "skip_total":{ "type": "boolean", "description": "When true, returns total=0 (caller already has the count)." },
                    "filters":   { "type": "array",  "description": "Cross-filter selections (FilterPayload-style)." },
                    "rules":     { "type": "array",  "description": "Exception-rule names (stockout, overstock, ...) to AND-narrow the candidate set." },
                    "node_kind": { "type": "string", "description": "Override the source's node_kind (article-graph DataViews only)." },
                    "group_by":  { "type": "array",  "description": "Column names to GROUP BY." },
                    "aggregates":{ "type": "array",  "description": "[{column, op, alias?}] specs; ops: sum/avg/count/count_distinct/min/max." },
                    "having":    { "type": "array",  "description": "Post-group filter clauses referencing group_by columns or aggregate aliases." }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        let id = a.id.clone();
        // Re-serialize the args, drop `id` (it's a path param, not a body field),
        // hand the rest to the bridge as a JSON body. `serde_json::to_value` on
        // a struct with `skip_serializing_if = "Option::is_none"` omits the
        // never-set fields so the handler's `serde(default)` kicks in.
        let mut body = serde_json::to_value(&a)
            .map_err(|e| ToolError(format!("serialize args: {e}")))?;
        if let Some(obj) = body.as_object_mut() { obj.remove("id"); }
        Ok(crate::service::dataviews::read(self.state.clone(), id, body).await?)
    }
}

// ── Graphs ───────────────────────────────────────────────────────────────

pub struct ListGraphs { state: Arc<AppState> }
impl Tool for ListGraphs {
    const NAME: &'static str = "list_graphs";
    type Error = ToolError;
    type Args = EmptyArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List all graphs (hierarchies) defined for this tenant. Each graph captures dimensional relationships like product -> category -> department.".into(),
            parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        }
    }
    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(Value::Array(crate::service::graphs::list(&self.state).await?))
    }
}

pub struct DescribeGraph { state: Arc<AppState> }
impl Tool for DescribeGraph {
    const NAME: &'static str = "describe_graph";
    type Error = ToolError;
    type Args = IdArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return the TOML definition + last validation status for one graph. Use to understand the hierarchy levels and cross-edges before traversing.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::graphs::describe(&self.state, &a.id).await?)
    }
}

pub struct GraphNode { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct GraphNodeArgs { id: String, #[serde(flatten)] req: crate::service::graphs::NodeRequest }
impl Tool for GraphNode {
    const NAME: &'static str = "graph_node";
    type Error = ToolError;
    type Args = GraphNodeArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        // OpenAI strict-mode function calling requires every key in
        // `properties` to appear in `required`; optional fields use a
        // nullable type. Applies to every tool below too.
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Project a single node from a built graph snapshot by (kind, name). Returns the node row including metrics + ancestors when requested. Pass `project: null` for default projection.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":   { "type": "string", "description": "Graph id" },
                    "from": { "type": "object",
                              "properties": { "kind": {"type":"string"}, "name": {"type":"string"} },
                              "required": ["kind","name"],
                              "additionalProperties": false },
                    "project": { "type": ["object", "null"], "description": "Projection options. Use null for defaults." }
                },
                "required": ["id", "from", "project"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::graphs::node(&self.state, &a.id, a.req).await?)
    }
}

pub struct GraphTraverse { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct GraphTraverseArgs { id: String, #[serde(flatten)] req: crate::service::graphs::TraverseRequest }
impl Tool for GraphTraverse {
    const NAME: &'static str = "graph_traverse";
    type Error = ToolError;
    type Args = GraphTraverseArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Walk a graph edge from a starting (kind, name) node and project each visited node. Edges: \"children\" | \"parent\" | \"ancestors\" | {\"descendants_of_kind\":\"article\"} | {\"cross_edge\":\"name\"}. Pass project/offset/limit as null for defaults.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":   { "type": "string" },
                    "from": { "type": "object",
                              "properties": { "kind": {"type":"string"}, "name": {"type":"string"} },
                              "required": ["kind","name"],
                              "additionalProperties": false },
                    "edge":   { "description": "String for bare variants or {variant: value} for parameterized ones" },
                    "project":{ "type": ["object", "null"] },
                    "offset": { "type": ["integer", "null"], "minimum": 0 },
                    "limit":  { "type": ["integer", "null"], "minimum": 1 }
                },
                "required": ["id", "from", "edge", "project", "offset", "limit"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::graphs::traverse_fn(&self.state, &a.id, a.req).await?)
    }
}

pub struct GraphCrossFilter { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct GraphCrossFilterArgs {
    id: String,
    #[serde(default)]
    target_kind: Option<String>,
    #[serde(flatten)]
    payload: crate::cross_filter::model::FilterPayload,
}
impl Tool for GraphCrossFilter {
    const NAME: &'static str = "graph_cross_filter";
    type Error = ToolError;
    type Args = GraphCrossFilterArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Apply filters against a graph snapshot and return distinct attribute values. `target_kind` is the kind whose nodes are filtered (default 'article'). `attributes` lists which columns to project distincts for; `filters` restricts the candidate set. Set `is_urm_filter` true only if you have a user_code+acl_code; otherwise leave null.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":          { "type": "string" },
                    "target_kind": { "type": ["string", "null"], "description": "Kind whose nodes form the candidate set. Pass null to use the graph's leaf-level kind (the most granular node type)." },
                    "attributes":  { "type": "array", "items": { "type": "object" },
                                     "description": "List of attribute objects to project distinct values for. Each has at least an `attribute_name` field." },
                    "filters":     { "type": "array", "items": { "type": "object" },
                                     "description": "List of filter objects narrowing the candidate set. Each has at least an `attribute_name` and a value selector." },
                    "is_urm_filter": { "type": ["boolean", "null"] },
                    "user_code":     { "type": ["integer", "null"] },
                    "acl_code":      { "type": ["integer", "null"] }
                },
                "required": ["id", "target_kind", "attributes", "filters", "is_urm_filter", "user_code", "acl_code"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        let q = crate::service::graphs::CrossFilterQuery { target_kind: a.target_kind };
        Ok(crate::service::graphs::cross_filter(&self.state, &a.id, q, a.payload).await?)
    }
}

// ── Sources ──────────────────────────────────────────────────────────────

pub struct ListSources { state: Arc<AppState> }
impl Tool for ListSources {
    const NAME: &'static str = "list_sources";
    type Error = ToolError;
    type Args = EmptyArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List all Source rows (pg_query, duckdb_table, parquet_glob, cdc_pg, ...). A DataView binds to a Source via its `source` field.".into(),
            parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        }
    }
    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(Value::Array(crate::service::sources::list(&self.state).await?))
    }
}

pub struct DescribeSource { state: Arc<AppState> }
impl Tool for DescribeSource {
    const NAME: &'static str = "describe_source";
    type Error = ToolError;
    type Args = IdArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return full metadata for one Source: kind, config, target_table, partition columns, CDC state.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::sources::describe(&self.state, &a.id).await?)
    }
}

// ── Connections ──────────────────────────────────────────────────────────

pub struct ListConnections { state: Arc<AppState> }
impl Tool for ListConnections {
    const NAME: &'static str = "list_connections";
    type Error = ToolError;
    type Args = EmptyArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List all backend connections (PG, ClickHouse, BigQuery, DuckDB) configured on this tenant. Passwords are masked. Use this to find a `connection_ref` for clickhouse_query.".into(),
            parameters: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        }
    }
    async fn call(&self, _: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(Value::Array(crate::service::connections::list(&self.state).await?))
    }
}

// ── Queries ──────────────────────────────────────────────────────────────

pub struct DuckdbQuery { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct DuckdbQueryArgs {
    sql: String,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}
impl Tool for DuckdbQuery {
    const NAME: &'static str = "duckdb_query";
    type Error = ToolError;
    type Args = DuckdbQueryArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Run a SQL SELECT against the tenant DuckDB. Supports SHOW/DESCRIBE/PRAGMA/WITH/FROM-first too. Use `{PARQUET_HOME}` as a path placeholder. Pass limit/offset as null to use the defaults (limit=500, offset=0). Returns {columns, rows, total, duration_ms}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql":    { "type": "string" },
                    "limit":  { "type": ["integer", "null"] },
                    "offset": { "type": ["integer", "null"] }
                },
                "required": ["sql", "limit", "offset"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::query::duckdb(
            &self.state,
            crate::service::query::DuckdbQueryArgs { sql: a.sql, limit: a.limit, offset: a.offset },
        ).await?)
    }
}

pub struct ClickhouseQuery { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct ClickhouseQueryArgs {
    connection_id: String,
    sql: String,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}
impl Tool for ClickhouseQuery {
    const NAME: &'static str = "clickhouse_query";
    type Error = ToolError;
    type Args = ClickhouseQueryArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Run a SQL SELECT against a ClickHouse connection. Get the connection_id from list_connections (rows where type='clickhouse'). Pass limit/offset as null to use defaults (limit=100, offset=0).".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "connection_id": { "type": "string" },
                    "sql":           { "type": "string" },
                    "limit":         { "type": ["integer", "null"] },
                    "offset":        { "type": ["integer", "null"] }
                },
                "required": ["connection_id", "sql", "limit", "offset"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::connections::clickhouse_query(
            &self.state,
            &a.connection_id,
            crate::service::connections::ClickhouseQueryArgs { sql: a.sql, limit: a.limit, offset: a.offset },
        ).await?)
    }
}

pub struct ClickhouseDictionary { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct ClickhouseDictionaryArgs {
    connection_id: String,
    #[serde(default)]
    database: Option<String>,
}
impl Tool for ClickhouseDictionary {
    const NAME: &'static str = "clickhouse_dictionary";
    type Error = ToolError;
    type Args = ClickhouseDictionaryArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return a compact schema dictionary for a ClickHouse connection: databases -> tables -> columns. Pass `database` as null to use the connection's default_database, or a specific database name to filter.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "connection_id": { "type": "string" },
                    "database":      { "type": ["string", "null"], "description": "Optional database filter. Null = use the connection's default_database." }
                },
                "required": ["connection_id", "database"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::connections::clickhouse_dictionary(
            &self.state,
            &a.connection_id,
            crate::service::connections::DictionaryArgs { database: a.database },
        ).await?)
    }
}

// ── Filter configs ───────────────────────────────────────────────────────

pub struct ResolveFilterValues { state: Arc<AppState> }
#[derive(Deserialize)]
pub struct ResolveFilterValuesArgs {
    id: String,
    #[serde(default)]
    context: std::collections::HashMap<String, Vec<String>>,
}
impl Tool for ResolveFilterValues {
    const NAME: &'static str = "resolve_filter_values";
    type Error = ToolError;
    type Args = ResolveFilterValuesArgs;
    type Output = Value;
    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Resolve dropdown values for a named filter config. Pass `context` as a map of parent column -> selected values to apply cascading rules, or null for no narrowing. Returns {columns: {col_name: [values]}}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id":      { "type": "string" },
                    "context": { "type": ["object", "null"],
                                 "description": "Parent column -> selected values; cascading rules narrow each child column accordingly. Null = no narrowing." }
                },
                "required": ["id", "context"],
                "additionalProperties": false
            }),
        }
    }
    async fn call(&self, a: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(crate::service::filter_configs::resolve_values(
            &self.state,
            &a.id,
            crate::service::filter_configs::ResolveValuesArgs { context: a.context },
        ).await?)
    }
}
