# DataView read composition — walkthrough

Captured 2026-05-16 on the bealls UAT instance. Demonstrates the `dataview_read` MCP tool's filter + group_by + aggregate surface against a single broad PG-backed DataView (`dv_alerts_product_store`), with one DuckDB-materialized comparison (`dv_articles_over_woc`).

The point of the surface: **don't proliferate per-question DataViews**. Compose at read time. The same broad DataView answers every "slice × group × aggregate" analytical question the planner asks, because the read primitive carries the slice/group/aggregate as arguments.

## What shipped

```
dataview_read(
  id          : <DataView id>,
  filters     : [{ attribute_name, operator, values }, ...],   // AND-ed WHERE
  group_by    : [col, ...],                                    // optional GROUP BY
  aggregates  : [{ column, op, alias? }, ...],                 // SELECT projection
  sort_col    : <col or aggregate alias>,
  sort_dir    : asc | desc,
  limit       : <n>,
  offset      : <n>,
  skip_total  : <bool>,
)
```

- Operators: `in`, `not_in`, `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `like`, `ilike`.
- Aggregate ops: `sum`, `avg`, `count` (incl. `count(*)`), `min`, `max`. Optional `alias`; default `<column>_<op>`.
- `attribute_name` and `group_by` columns validated against the DataView's declared `columns` array (strict allowlist).
- Numeric operators (`gt|lt|gte|lte`) parse the value as `f64` and reject non-numerics. String values single-quote-escape.
- Applies to all three SQL source kinds: `pg_query`, `duckdb_table`, `duckdb_query`. Article-graph sources have their own filter pipeline (unchanged).
- Server: `build_where_clause` + `build_aggregate_clauses` in `server/src/handlers/dataview_source.rs`. Both nested under the standard `SELECT ... FROM (<source SQL>) AS _q ...` pattern.
- MCP: `mcp-server/src/tools/dataview_read.ts`.

The DataView holding the action: **`dv_alerts_product_store`** — a pg_query DataView over `inventory_smart.alerts_product_store_level`, 44 columns including per-store `oh / oo / it`, multi-week sales (`lw / l4w / l8w`), `region`, `country`, full WOS triangle, sell-through, and 7 alert flags.

---

## The 8 prompts

Each prompt is an **initial broad question** that an LLM (or planner) asks. The "drill" calls use values from the prior response — no SQL composed by the LLM, just argument composition against the same tool.

### 1. "Where is sales drag this week?"  (3 calls, 15.4s)

Drills: network → region → L1 → L2.

```
dataview_read(group_by=['region'],
              aggregates=[{lw_units, sum}, {l4w_units, sum}, ...],
              sort_col='lw_units_sum', sort_dir='desc')              → 7 regions, 5.3s
```

Picked the worst region (M.Ritz, REGION3019, -4.4% WoW), then:

```
dataview_read(filters=[{region eq REGION3019-BLS-M.Ritz}],
              group_by=['l1_name'],
              aggregates=[{lw_units, sum}, {l4w_units, sum}, ...])   → 37 L1s, 5.0s
```

Picked MISSES SPORTSWEAR (-5.6%, -1,030 units), then:

```
dataview_read(filters=[{region eq ...}, {l1_name eq 3110-MISSES SPORTSWEAR}],
              group_by=['l2_name'],
              aggregates=[{lw_units, sum}, {l4w_units, sum}])        → 16 L2s, 5.1s
```

**Finding**: EC WOVEN TOPS -25.6% WoW; WC CAS SS-SL KNITS +13.8%. Weather-shift from wovens to knits inside M.Ritz's territory, not demand collapse.

### 2. "Which brands are most exposed to stockouts?"  (1 call, 5.0s)

```
dataview_read(filters=[{stockout, gt, [0]}],
              group_by=['l1_name'],
              aggregates=[{*, count}, {oh, sum}, {lw_units, sum}],
              sort_col='count_all', sort_dir='desc')
```

**Finding**: Beauty (321k stockout cells), Misses Sportswear (213k). **Gap**: `brand` column lives on the article-level alerts table, not this DataView. Brand-cross-stockout drill would need either column addition on this DV or a join DataView.

### 3. "Apparel categories discounting hardest?"  (1 call, 5.2s)

```
dataview_read(filters=[{l1_name, ilike, ['%APPAREL%']}],
              group_by=['l1_name'],
              aggregates=[{discount, avg}, {lw_revenue, sum}, {lw_margin, sum}, {lw_units, sum}],
              sort_col='discount_avg', sort_dir='desc')
```

**Finding**: Only 2 L1s match `%APPAREL%` — Girls/Boys at ~3.4% avg discount, ~50% margin rate. No erosion.

### 4. "Inventory rotting on shelf — which categories?"  (1 call, 4.9s)

```
dataview_read(filters=[{cfc_age_gt_20_alert, eq, ['1']}],
              group_by=['l1_name'],
              aggregates=[{*, count}, {oh, sum}, {lw_units, sum}],
              sort_col='oh_sum', sort_dir='desc')
```

**Finding**: MISSES SPORTSWEAR sits on 10k aged units with 284 weekly turnover (3% sell rate). HOUSEWARES has 4k aged units, 20 weekly turnover (0.5% — dead stock candidate). Ladies Footwear keeps moving despite age.

### 5. "Stores with high stock, low sell-through?"  (2 calls, 9.9s)

First attempt with `oh > 100` filter applied **per-row** (WHERE not HAVING) — only Store 790 qualified because the filter is per-(article, store) cell, not the store-level sum. Re-cast without the filter:

```
dataview_read(group_by=['store_code', 'region'],
              aggregates=[{oh, sum}, {lw_units, sum}, {sell_through_perc, avg}],
              sort_col='sell_through_perc_avg', sort_dir='asc')
```

**Finding**: Store 969 (REGION3999 / ECom) holds 6,436 OH with zero last-week sales. Mostly closed/inactive ECom-classed stores sitting on dead inventory.

**Gap surfaced**: no `HAVING` (post-group filter). To express "stores where `sum(oh) > N`" today we either drop the filter and sort, or build a more specific DataView with the predicate baked in.

### 6. "Categories outside their WOC target band?"  (1 call, **25ms**)

```
dataview_read(id='dv_articles_over_woc',                    // duckdb_table, materialized
              group_by=['l1_name'],
              aggregates=[{woc, avg}, {min_woc, min}, {max_woc, max}, {*, count}])
```

**Finding**: Degenerate — all WOC = 8 on bealls today. Real story: **25ms for 37 group rows, ~200× faster than the pg_query path's ~5s**. The materialization tradeoff in numbers.

### 7. "Compare physical vs ECom across categories"  (1 call, 5.3s)

```
dataview_read(
  filters=[{region, in, [REGION3999, REGION3019-BLS-M.Ritz]},
           {l1_name, in, [3110-MISSES SPORTSWEAR, 3200-BEAUTY, 3510-LADIES FOOTWEAR, 3145-SWIM]}],
  group_by=['region', 'l1_name'],
  aggregates=[{lw_units, sum}, {lw_revenue, sum}, {oh, sum}],
  sort_col='lw_revenue_sum', sort_dir='desc')
```

**Finding**: ECom Swim ASP is $26.6 (+37% vs physical's $19.4) and 2× the cover ratio. Real channel-mix insight from one call.

### 8. "Where are stockouts hitting proven-velocity items?"  (1 call, 5.2s)

```
dataview_read(filters=[{stockout, gt, [0]},
                       {l8w_units, gt, [5]},
                       {wos_oh, lt, [1]}],
              group_by=['l2_name'],
              aggregates=[{*, count}, {l8w_units, sum}, {lw_units, sum}],
              sort_col='count_all', sort_dir='desc')
```

**Finding**: MS SHORTS dominates (3,269 stockout cells across 26k 8-week units sold). Summer wardrobe demand outpacing supply. 7 of the top 8 L2s are seasonal apparel; the surprise inclusion is DRINKS & MIXES (consumables).

---

## Capability × performance matrix

| # | Prompt | DataView kind | Calls | Capabilities | Wall time |
|---|---|---|---:|---|---:|
| 1 | Sales drag | pg_query | 3 | filter, group_by, aggregate, iterative drill | 15.4s |
| 2 | Brand audit | pg_query | 1 | filter + group_by; **brand col missing** | 5.0s |
| 3 | Margin erosion | pg_query | 1 | ilike + group_by + multi-aggregate (avg+sum) | 5.2s |
| 4 | Aging inventory | pg_query | 1 | flag eq + group_by + aggregate | 4.9s |
| 5 | Footprint efficiency | pg_query | 2 | 2-col group_by + multi-aggregate; **HAVING gap** | 9.9s |
| 6 | WOC band | duckdb_table | 1 | group_by + avg/min/max | **25ms** |
| 7 | Channel × L1 | pg_query | 1 | 2-clause IN + 2-col group_by | 5.3s |
| 8 | Stockouts+velocity | pg_query | 1 | 3-clause AND (gt/gt/lt) + group_by + count(*) | 5.2s |

**Aggregate**: 11 tool calls, ~50s wall-clock. Mean ~4.5s per call against PG; one materialized read at 25ms.

## SmartStudio capabilities leveraged

| Capability | Where it lives | Used |
|---|---|---|
| `pg_query` source kind, live PG via SmartStudio data handler | `server/src/handlers/dataview_source.rs` | every prompt except #6 |
| `duckdb_table` source kind, materialized | same | #6 only |
| Connection registry, default = `uat` | `/api/connections` | every pg_query call |
| Per-DataView columns allowlist | `dataviews.columns` JSON column | all filter/group_by validation |
| WHERE clause builder (filter) | `build_where_clause` | #2, #3, #4, #5, #7, #8 |
| GROUP BY + aggregate builder | `build_aggregate_clauses` | #1–#8 |
| Server sort/page (`ORDER BY/LIMIT`) | `build_outer_select` | every call |
| Count-of-groups for grouped reads | nested `count_sql` | when `skip_total=false` |
| Tool-response `duration_ms` envelope | MCP `dataview_read` tool | the perf column above |

## Gaps surfaced

Three filed as MCP feedback during the run:

| Gap | Trigger | Resolution |
|---|---|---|
| `HAVING` clause (post-group filter) | #5 | Extend `build_aggregate_clauses` with optional `having: [{column, op, value}]` array; append after `GROUP BY`. |
| `brand` column missing on `dv_alerts_product_store` | #2 | Either extend the DataView's `columns` list (and the source SQL projection) OR ship a per-(article, store, brand) DataView that joins the article-level table. |
| `BETWEEN` operator | implied across `cfc_age_weeks` drills | Add to `FILTER_OPERATORS`; trivial server change. |

Two retired during the run by being closed by `dv_alerts_product_store` itself:

| Originally framed as | Reality |
|---|---|
| P2-0 (`src_oh_per_store` needs ingestion) | `oh` already in `alerts_product_store_level` — pg_query exposes it. |
| P2-1 (`src_sales_history` needs ingestion for multi-week) | `lw_units`, `l4w_units`, `l8w_units` already in the same table. |
| P3-3 (`src_store_regions` needs ingestion) | `region`, `country` already in the same table. |

## Lessons

1. **One broad DataView + composition beats many narrow DataViews + per-question SQL.** All 8 prompts hit the same `dv_alerts_product_store` and differ only in arguments. Adding a 9th prompt doesn't require adding a 9th DataView.

2. **pg_query DataViews are interactive-acceptable but not free.** ~5s per call is fine for analytic chains of 2-3 calls; ~50s for an 8-question batch is fine for batch workloads. For UI-paginated reads the planner will feel the latency; for "what's the story" workflows it's tolerable.

3. **Materialization wins 200× when it applies.** The DuckDB-materialized `dv_articles_over_woc` answered in 25ms vs pg_query's 5000ms. The decision rule (`feedback-pg-query-default` memory): default pg_query unless read volume × infrequent writes justifies the materialization cost. Sales history, allocation plans, alerts → pg_query. Master tables, RCL rules, pre-aggregated metrics → materialize.

4. **Per-tenant vocabulary trumps prompt-literal vocabulary.** Generic-retail prompts assume `region=NE` or `group=Sunbelt-A`; this tenant uses `REGION3019-BLS-M.Ritz` and internal label names. The robust LLM workflow enumerates actual values first (via a `dataview_read` with `group_by` on the target column), then drills. Don't trust prompt-literal names.

5. **Filter-clause + group_by + aggregate together is the full OLAP read pattern.** Adding `HAVING` (and arguably window functions / pivot) would close the remaining shape gap. The composition was deliberately built minimal-first — extend when a real prompt forces it.
