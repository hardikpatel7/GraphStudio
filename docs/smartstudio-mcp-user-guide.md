# SmartStudio MCP — End User Guide

A guide to using the SmartStudio MCP from Claude Code (or any LLM client that
speaks the Model Context Protocol). Written for the **planner** — the retail
analyst making inventory decisions today — not for the engineer maintaining
the platform.

If you've ever wanted to ask your data layer questions like *"Which stores are
thin on denim?"* without writing SQL, this is for you.

---

## What is SmartStudio?

SmartStudio is a **metadata-driven app generation platform** for retail
inventory management. Behind every screen in your inventory-smart app is
metadata that describes:

- **Apps** — one per retail client (e.g. `bealls-inventory-smart-app`).
- **Dimensions** — Product (8 levels), Store (6 levels), DC.
- **DataViews** — server-side tabular datasets the planner reads from:
  alerts, allocations, supersession chains, pack constraints, and so on.
- **Modules** — Dashboard, Configuration, Constraints, Grouping, Finalize,
  Reports, VPA (Vendor PO Allocation), CNA (Cross-Network Allocation).

The catalog is the foundation. The UI renders from it. The MCP exposes it.

---

## What is the SmartStudio MCP?

The MCP server is a thin **describer + reader** that lets an LLM (Claude
Code, etc.) ask SmartStudio for data conversationally. It does three things:

1. **Describes the catalog** — what apps, DataViews, sources, and graphs
   exist on this tenant.
2. **Reads from the catalog** — filtered, grouped, aggregated, sorted slices
   of any DataView; targeted graph queries for rolled-up metrics; raw
   DuckDB for the long tail.
3. **Captures feedback** — when the LLM hits a gap, it files a structured
   request via `submit_feedback` so the platform team can prioritise.

The MCP itself is **dumb on purpose**. It does not pick which layer to hit,
does not reformulate questions, does not maintain its own cache. The LLM
makes those decisions; the MCP just forwards the calls.

---

## What this means for you

You can ask SmartStudio questions like:

- *"DC2 has too much denim — which stores are thin? Propose transfers."*
- *"Which articles triggered low-stock alerts today? Show the worst 20."*
- *"Allocations pending approval over 24 hours — who's the bottleneck?"*
- *"Supersession chain for the old denim line — find missing links."*
- *"Show stores with on-hand below 1 week of cover, grouped by L2 category."*
- *"Which open POs cover the stockouts flagged today?"*

You don't write SQL. You don't pick a table. You describe a goal, and the
LLM composes one or more tool calls to land on an answer. When it can't,
it says so honestly — and often files feedback so the gap shows up in the
Feedback tab.

The model isn't "ask one question, get one chart." It's a **conversation**.
You iterate: narrow the slice, pivot on a different dimension, drill into a
candidate row. Each turn is one or two MCP calls, each ~3–6 seconds against
live data.

---

## How it works — the layered architecture

SmartStudio sits on **four data layers**. The MCP describes all of them and
lets the LLM pick which one to hit for a given question.

```
                                    ┌─────────────────────┐
  ┌───────────────────┐             │  Service layer      │
  │  PG catalog       │             │  (gRPC, in-process) │
  │  (live, source-   │             │  • RCL resolver     │
  │   of-truth)       │             │  • Cross-filter     │
  └────────┬──────────┘             │  • Article graph    │
           │                        │  • Article selection│
           │ pg_query               └──────────▲──────────┘
           ▼                                   │
  ┌───────────────────┐                        │
  │  DataViews        │  composition surface   │
  │  (filter / group  ├────────────────────────┘
  │   / aggregate /   │                        ▲
  │   sort / having)  │                        │
  └────────▲──────────┘             ┌──────────┴──────────┐
           │                        │  Derived layer      │
           │ duckdb_table           │  (DuckDB +          │
           │                        │   parquet snapshots,│
           │                        │   refreshed by      │
           │                        │   pipelines)        │
           │                        └─────────────────────┘
  ┌────────┴──────────┐
  │  BQ catalog       │  bq_export (planned for
  │  (analytics)      │  forecast + lead-times)
  └───────────────────┘
```

**Why four layers**: each one is good at something different.

- **PG** is source-of-truth for transactional state (allocations, alerts,
  workflow status). Live; no staleness.
- **DuckDB / parquet** materialises heavy reads as snapshots. Cheaper for
  big aggregations than going back to PG every time.
- **BQ** holds analytics-grade data the warehouse team produces — forecast
  weeks, supplier-route lead-times. Tenant-specific.
- **gRPC services** handle compute that doesn't fit a SQL query — graph
  cross-filtering, RCL rule resolution, article-selection materialisation.

The **article graph** is the backbone. It threads articles under a product
hierarchy (l0 → l1 → l2 → l3 → l4 → l5 → article), pinned to stores and DCs.
Rolled-up metrics live at every node, so "OH by L2" is a graph read, not a
table scan.

**The LLM picks the layer.** The MCP doesn't route. If a question fits the
graph (rolled-up metric at a node), the LLM uses `graph_node`. If it fits a
DataView (filtered rows over a known table), `dataview_read`. If it falls off
both, `duckdb_query` — but that's a signal: the LLM is expected to file
feedback when this happens, so the platform learns where to add structure.

---

## What you can ask today

Categories of prompts that work right now against the bealls UAT tenant:

**Inventory state** — across articles, stores, DCs:
- Stockouts, overstocks, on-hand by L2/region/store-group.
- Article alerts (clearance flag, age buckets, sell-through windows).
- Per-(article, store) flat view: filter, group, aggregate freely.

**Allocation workflow**:
- Pending approvals over 24 hours (with `run_started_at` parsed from the
  embedded timestamp in the allocation_code).
- Per-(article, store) split-PO results — who got what from where.
- Phase lifecycle: which runs reached terminal, which stalled.

**Catalog navigation**:
- "What DataViews exist?" — `list_dataviews`.
- "What columns does dv_X have?" — `describe_dataview`.
- "What distinct regions exist on this tenant?" — `resolve_filter_values`.
- "What does the article graph look like at l3?" — `describe_graph` +
  `graph_node`.

**Cross-cuts**:
- Supersession chains (old → new article mappings with priority + window).
- Pack constraints (units_in_pack per article × size).
- Store group performance.

**What's deferred** (data lives in BigQuery, not PG, on this tenant):

- *Forecast vs allocation variance.* `dc_forecast_week_level` is empty on
  PG by design; the forecast is in BQ. Until a `bq_export` source is wired,
  `dv_forecast_weekly` will return zero rows.
- *Lead-times / lane analysis.* `supply_route` and `store_to_store_transit`
  are both empty on PG. Same story — likely BQ-backed.
- *Vendor master.* No vendor-master table in `inventory_smart`; vendor
  identity probably lives in procurement upstream.

When you ask one of these, the LLM should tell you it can't, and usually it
files feedback automatically so the gap appears in the Feedback tab.

---

## The MCP tool surface

Twenty tools, grouped by what they do. You'll rarely think about them
directly — the LLM picks. But knowing they exist helps you understand
*why* the LLM is sometimes saying "let me look that up" before answering.

### Discovery — "what's in the catalog?"

| Tool | What it returns |
|---|---|
| `list_dataviews` | All DataViews on this tenant, with id + display name. |
| `describe_dataview` | Full schema for one DataView: columns, source binding, dimensions. |
| `introspect_dataview` | Live shape of a DataView when columns are pipeline-driven (empty metadata → introspect first row). |
| `list_sources` | Sources registered in the catalog (pg_query / pg_sp / bq_export / duckdb_table). |
| `describe_source` | The actual SQL or config behind a source. |
| `list_graphs` | All graph specs on this tenant. |
| `describe_graph` | Hierarchies, levels, metrics, and cross-edges in a graph. |
| `glossary` | Tenant-specific vocabulary (region names, store-group conventions, hierarchy meanings). |

### Reading — "give me the data"

| Tool | What it does |
|---|---|
| `dataview_read` | The workhorse. Filter / group_by / aggregate / having / sort / limit over any DataView. Returns rows + the SQL it ran (for transparency). |
| `duckdb_query` | Raw SQL against the derived DuckDB layer. Last-resort fallback — the LLM should file feedback when it lands here. |

### Article graph — "rolled-up metrics at a hierarchy node"

| Tool | What it does |
|---|---|
| `graph_node` | Read a metric at a specific (hierarchy, level, node). E.g. "OH for L2=DENIM in region M.Ritz." |
| `graph_traverse` | Walk a hierarchy or cross-edge (e.g. "all stores in store_group X"). |
| `graph_cross_filter` | Constrain one dimension by filters on another (e.g. "stores carrying article N"). |

### Article selection — multi-step planner workflows

| Tool | What it does |
|---|---|
| `list_articles` / `query_articles` | Browse and search the article master. |
| `article_detail` | Full record for one article — hierarchy path, attributes, pack info. |
| `describe_article_selection` / `materialize_article_selection` / `article_selection_status` | Manage a planner-scoped working set of articles (used by some Configuration workflows). |

### Plumbing

| Tool | What it does |
|---|---|
| `resolve_filter_values` | Distinct values for a column — populates dropdowns; helpful when prompts mention vocabulary like "Sunbelt-A" that may not exist on this tenant. |
| `submit_feedback` | File a capability gap or bug. Persisted in the Feedback tab with a status lifecycle. |

---

## Design decisions worth knowing

A few choices that shape how the system behaves.

### The MCP is a thin describer, not a router

The MCP doesn't decide which data layer to hit. It exposes the layers, the
shapes, and the readers. The LLM (with memory, see below) makes the call.

**Why**: the alternative — a smart MCP that routes — locks the routing
logic inside the proxy. Then improving the planner experience means
shipping MCP releases. With a thin MCP, the LLM gets smarter via memory and
prompting, not via redeploys.

### The LLM thinks; the agent learns

The MCP itself has no memory. The agent (Claude Code, etc.) holds the
memory: which tool fits which question, what workarounds previously worked,
what tenant-specific vocabulary means. Memory persists across conversations
in the agent's storage; the MCP stays stateless.

**Why**: feedback that says "graph_cross_filter doesn't do metric thresholds"
should become a memory entry the agent consults next time — not a state
hidden in the proxy.

### Pure proxy — no direct PG from the LLM

The LLM never writes raw PG SQL. It composes against the DataView surface
(`dataview_read` with filter/group_by/aggregate) or hits the graph layer.
The only raw-SQL escape is `duckdb_query` against the derived layer.

**Why**: PG is the system of record. Giving the LLM raw write/read access
would couple the planner experience to schema choices that change for
operational reasons (snapshots get dated, columns get renamed). The
DataView surface is the **stable contract**; underneath, the source SQL can
evolve without breaking the LLM.

### Source-controlled DataViews

DataView and source starter shapes live as TOML files in
`templates/<app_type>/{dataviews,sources}/`. At `is_new = true`
bootstrap they are copied into the tenant data dir, and the boot-time
seed loaders upsert them into SQLite from there on every start.
Adding a new DataView to the product line is a PR; per-tenant
divergence is just an edit in the tenant's own data dir.

**Why**: pre-source-control, DataViews were created at runtime and lost on
re-seed. Catalog drift between tenants was invisible. With files, every
DataView change is reviewable; tenants stay coherent.

### Feedback as a first-class loop

When the LLM falls back to `duckdb_query` for a question that *should*
have fit a graph or DataView, it's expected to `submit_feedback` with the
prompt as `example_question`. That entry shows up in the Feedback tab with
a status (pending / partial / addressed). The platform team triages from
that feed.

**Why**: the planner's actual questions are the highest-signal input. A
feedback entry tagged with the real prompt that motivated it is far more
useful than a generic "graph needs DC dimension" wishlist item.

### Graph describes itself

The article graph isn't hardcoded in the MCP. The MCP calls
`describe_graph` against SmartStudio's HTTP API and exposes the result.
Adding a new dimension (e.g. a brand axis) makes it visible to the LLM
without any MCP code change.

**Why**: the graph evolves per-tenant. A new client may have a different
hierarchy depth or extra dimensions; the LLM should pick those up from the
catalog, not from MCP releases.

### Bias toward composition over more endpoints

When a new prompt type lands, the first question is *"can the existing
DataView composition surface answer this?"* Most of the time the answer is
yes — add a column, add a derived bool, write a richer filter chain. Only
when composition genuinely can't reach a shape (e.g. multi-snapshot diff,
window functions, OR-grouped filters) does a new endpoint earn its keep.

**Why**: each new endpoint expands the LLM's decision surface. Fewer,
sharper primitives are easier to memorise and easier to reason about.

---

## The feedback loop in practice

The Feedback tab in the SmartStudio UI lists every entry the LLM has filed,
newest-first. Each entry has:

- **Category** — `missing_endpoint`, `data_gap`, `ergonomics`, `perf`,
  `new_graph`, `bug`.
- **Summary** — one line.
- **Example question** — the planner prompt that triggered the entry.
- **Optional details** — what was painful, the workaround the LLM used,
  proposed solution, the tool path tried (`graph_node → graph_cross_filter
  → duckdb_query`).
- **Status** — `pending` / `partial` / `addressed`. Click the chip to cycle.

Filter pills at the top let you show any combination of statuses (or none —
the empty state is intentional). Counts are full-set totals, so they stay
stable while you triage.

The status lifecycle is intentionally tiny — three values, no assignee, no
due date. The point is the feed itself; richer workflow lives in whatever
issue tracker the team uses for follow-through.

---

## Working in practice — a planner's session

A representative turn:

1. **You ask**: *"DC2 has too much denim — which stores are thin?"*
2. The LLM calls `describe_graph` (already cached most turns) to see what
   hierarchies exist, then `graph_cross_filter` to find stores carrying
   denim articles.
3. It then calls `dataview_read` against `dv_alerts_product_store`, filtered
   to `l2_name LIKE '%DENIM%'`, grouped by `store_code`, sorted by `wos_oh`
   ascending (lowest cover first).
4. Two tool calls, ~6–10 seconds, candidate stores returned with their cover
   ratios.
5. **You follow up**: *"Of the top 10, which have a DC2 supply lane?"*
6. Deferred — `dv_dc_lead_times` is in the BQ-backed bucket on this tenant.
   The LLM tells you so and files a feedback entry if it hasn't already.

You stay in the conversation; the LLM picks the shape; the data layer
underneath is invisible until you want to know about it.

---

## Glossary

A few terms you'll see when the LLM explains what it just did.

- **DataView** — a server-side tabular dataset. The composition surface
  you read against: columns + filters + group_by + aggregates + sort.
- **Source** — the SQL or config that defines a DataView's rows. Source
  kinds: `pg_query` (live PG), `pg_sp` (stored procedure), `bq_export`
  (BigQuery), `duckdb_table` (materialised view).
- **ViewPort** — a stateful, server-side filtered window into a DataView.
  Used by the UI; not directly exposed in the MCP.
- **Dimension** — Product, Store, or DC, each with named levels.
- **Hierarchy** — the named tree under a dimension (e.g. Product:
  l0 → l1 → … → l5 → article).
- **Article** — the unit of allocation. Roughly a SKU; SmartStudio's
  hierarchy lives above it.
- **Allocation code** — the run identifier produced by the optimizer.
  Carries an embedded timestamp suffix (`..._YYYYMMDDHHMMSS`) which
  SmartStudio parses out as `run_started_at`.
- **RCL** — Retail Constraint Language. Per-(article, store) eligibility
  rules the allocator must respect. Resolved by a gRPC service in-process.
- **Article graph** — the indexed hierarchy + cross-edges the MCP queries
  via `graph_node` / `graph_traverse` / `graph_cross_filter`.
- **Feedback** — a structured capability request filed via
  `submit_feedback` and visible in the Feedback tab.

---

## Where this is heading

Two anchors keep this honest:

- **Designed for today's planner.** Prompts come from real retail analysts
  doing real inventory decisions; the catalog grows when those prompts hit
  a gap.
- **The MCP stays light.** As the LLM gets smarter (via memory and
  prompting), the MCP shouldn't have to. The architectural goal is *more
  capable LLM × stable thin proxy*, not *thicker proxy × dumber LLM*.

When that balance shifts — when a memory pattern emerges that's universal
enough to deserve being a primitive — the right place to bake it in is the
DataView composition surface, not the MCP itself.
