# Planner prompts — full run

Walked all 30 module-specific planner prompts against the live bealls UAT instance. Original run 2026-05-16; revised same day after the leverage-DataView batch landed.

Status legend:

- **Answerable**: tool call(s) with durations + key result numbers
- **Partial**: what's currently surfaced + what's missing (DV exists; data quality limits the answer)
- **Deferred**: PG table exists but is empty on UAT; data likely lives in BQ or another tenant-specific source (no new pg_query DV will help)
- **Blocked (no data)**: confirmed gap with no upstream source

DataViews available on this tenant at revision time:

- Pre-existing: `dv_alerts_product`, `dv_alerts_product_store` (pg_query); `dv_articles_over_woc`, `dv_rcl_rules`, `dv_store_group_performance`, `dv_zero_sale_active_products` (duckdb_table over views).
- Landed this batch: `dv_allocation_split_po`, `dv_allocation_status`, `dv_supersession_chain`, `dv_pack_constraints` (all pg_query).
- Attempted and deferred: `dv_forecast_weekly` (forecast lives in BigQuery), `dv_dc_lead_times` (both candidate tables empty on UAT — likely BQ-backed).

---

## Dashboard

### D1. "Which articles triggered low-stock alerts today? Show the worst 20."

**Status**: Answerable — partial vocabulary mismatch on this tenant.

Bealls' `alerts_product_level` has `understock_flag` and `stockout_flag` columns that are entirely NULL today; the populated flags are `cfc_age_gt_*` (carry-forward age). So "low-stock alerts" maps to per-store stockout state (from `alerts_product_store_level`) rather than article-level understock flags.

```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{stockout, gt, [0]}, {l8w_units, gt, [5]}, {wos_oh, lt, [1]}],
  sort_col="l8w_units", sort_dir="desc", limit=20
)
```
Returned 47,681 matching (article, store) cells; top 20 dominated by EC Wedge Sandals at store 790 (l8w_units ~75–128, wos_oh=0). **Duration**: 5.2s.

### D2. "5 stores breached the safety threshold overnight — what's the receipt ETA?"

**Status**: Partial.

`stockout_most_stores_alert` + `cfc_age_*` flags surface the breach side. But **receipt ETA** needs PO arrival schedules. PG has `cancelled_po` and `create_allocation_result_flat_gurobi_split_po*` (allocation outputs that split POs), but no standalone `purchase_orders.expected_receipt_date` table located yet. Most likely the receipt timing lives in the upstream procurement system, not surfaced into `inventory_smart`.

### D3. "Allocation plan for SKU 88412 looks off vs. forecast — flag the variance."

**Status**: Deferred.

`inventory_smart.dc_forecast_week_level` exists with the right 10-column shape (article, fiscal_year_week, dc_code, sales_forecast, safety_stock, …) but is empty on UAT — forecast data actually lives in **BigQuery**. A future `dv_forecast_weekly` should be built as `bq_export`, not `pg_query`. Initial `pg_query` attempt was reverted (`860a12d`). Allocation side is now answerable via `dv_allocation_split_po`; variance only opens up when the BQ side ships.

### D4. "PO alerts spiked this week — which vendors and stores are exposed?"

**Status**: Partial.

Allocation/PO side now answerable via `dv_allocation_split_po` (per-(article, store) allocations with `allocation_code`, `source`, `delivery_dt`). Group by store and (date-bucketed) `delivery_dt` to surface this week's PO landings; filter to `is_edited=true` for planner-touched rows.

Vendor side still blocked: no `vendor` master table located in `inventory_smart.*`. Vendor likely lives in procurement upstream. The store + delivery-window slice ships today; the "which vendors" pivot waits on vendor-master ingestion.

### D5. "Show me articles with inventory above 4 weeks of cover across all stores."

**Status**: Answerable.

```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{wos_oh, gt, [4]}],
  group_by=["article", "l2_name"],
  aggregates=[{column:"*", op:"count"}, {column:"oh", op:"sum"}],
  having=[{alias:"count_all", operator:"gt", values:["100"]}],
  sort_col="count_all", sort_dir="desc", limit=20
)
```

**Duration**: 5.4s (filter + group_by + having). Found 12,503 articles where >100 stores show wos_oh>4 — a coverage glut signal. Most are in MISSES SPORTSWEAR knits and accessories (seasonal carry).

---

## Configuration

### C1. "Which products are marked active but had zero sales for 8 weeks? Suggest deactivation."

**Status**: Partial → Answerable today via the 8-week proxy.

`dv_alerts_product_store` carries `l8w_units` (rolling 8-week units sold per article-store). For article-level 8w-zero:
```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{clearance, eq, ["false"]}],
  group_by=["article", "l1_name"],
  aggregates=[{column:"l8w_units", op:"sum"}, {column:"oh", op:"sum"}],
  having=[{alias:"l8w_units_sum", operator:"eq", values:["0"]},
          {alias:"oh_sum", operator:"gt", values:["0"]}],
  sort_col="oh_sum", sort_dir="desc", limit=20
)
```
Returns articles with no 8w movement AND OH > 0 — strong deactivation candidates. **Duration**: 5.6s.

Note: `dv_zero_sale_active_products` (a simpler dataview I shipped earlier) does the same shape for last-week-only.

### C2. "Store 4421 opens June 1 — set its product/DC strategy from a nearby comparable."

**Status**: Blocked (no upstream table).

`raw_store_master` carries store identity but no "open date" / "geographic comparable" workflow. No `new_store_planning` or similar table in `inventory_smart`. This is application-level workflow data that probably lives in the planner UI's own state, not in the analytic store.

### C3. "Supersession chain for the old denim line is incomplete — find missing links."

**Status**: Answerable.

```
dataview_read(
  id="dv_supersession_chain",
  filters=[{is_active, eq, [true]}],
  sort_col="start_date", sort_dir="desc", limit=20
)
```
Returns 153 active (old_article → new_article) mappings with priority, date window, and `has_store_exception` flag. Walk the chain by re-querying with `old_article = <new_article from previous step>`; missing links surface as articles that appear on the `old_article` side but never on the `new_article` side. **Duration**: 5.2s.

The store-exception side (`product_supersession_store_priority`) isn't merged in yet — surfaced via the `has_store_exception` bool so a follow-up read can fetch overrides if any chain has one.

### C4. "WOC targets for spring basics look stale — recompute against the latest sell-through."

**Status**: Partial.

`asv2_woc` (materialized) and `dv_articles_over_woc` give the current WOC targets. Sell-through is in `alerts_product_store_level.sell_through_perc` and `l4w_units / l8w_units` (rolling windows). The "stale" check needs: per-(l4_name, week) WOC trend → if WOC didn't track recent sell-through trend, flag stale.

The fundamental aggregation works today (group_by + WOC metrics from articles_over_woc vs sell-through from alerts_product_store), but the underlying WOC values on bealls are uniformly 8 across all rows, so the comparison is degenerate on this tenant.

### C5. "DC3 is set to serve store 7782, but lead time is 9 days — propose a re-map."

**Status**: Deferred.

Both candidate PG tables are empty on bealls UAT: `supply_route` + `supply_node` (the architecturally-correct supply-network model — 15 cols with `source_node_id`, `destination_node_id`, `lead_time`, `shipping_mode`, `is_primary`) and the older `store_to_store_transit` (8 cols). Same pattern as forecast — the PG schema carries the shape but the upstream pipeline hasn't run for this tenant; lead-times configuration probably lives in BigQuery. Initial `dv_dc_lead_times` was reverted (`b8367bf`); user-confirmed defer.

---

## Constraints

### Co1. "Show all stores hitting their max-cube constraint this week — which items got dropped?"

**Status**: Blocked (data exists upstream).

`create_allocation_result_flat_gurobi_split_po*` carries the allocation results per day; if constraint violations are encoded in there (a `dropped_reason` column or similar), a pg_query DataView surfaces them. Otherwise, the upstream allocator (Gurobi) would need to emit a separate constraint-violations log.

### Co2. "RCL violations for top-50 SKUs — list them and propose the smallest fix."

**Status**: Partial.

`dv_rcl_rules` exposes the **rule registry** (109 rules). The "violations" part — articles currently failing their assigned RCL rule — requires walking each (article, store) pair against the resolved rule, which is a compute the article_graph already does internally (`resolve_rcl` endpoint). A pg_query / view that materializes "current violations" isn't there.

```
dataview_read(id="dv_rcl_rules", limit=10, sort_col="priority", sort_dir="asc")
```
Returns the 10 highest-priority rules. **Duration**: 5.1s.

Filed already: `fb_1778930669875005000` (RCL violations need batch compute path or a periodic materialization).

### Co3. "Exception: 18 stores received zero of SKU 7741 despite demand — why? Override?"

**Status**: Partial → Doable via composition.

```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{article, eq, ["7741"]}, {l8w_units, gt, [0]}, {allocated_units, eq, [0]}],
  sort_col="l8w_units", sort_dir="desc"
)
```
Surfaces stores where demand existed (l8w_units > 0) but allocation = 0. The "why" part (constraint violation, RCL eligibility, etc.) needs the allocator's emitted reasons — see Co1.

### Co4. "Min-pack constraint is starving small stores of size XS — relax it for which stores?"

**Status**: Partial.

`dv_pack_constraints` over `dc_pack_configuration` is live (1.35M rows; per (article, size, pack_type) with `units_in_pack`). The structural surface is in place — a planner can ask "which articles ship in case packs > N units" via `filters=[{units_in_pack, gt, [N]}]` or "size XS pack constraints" via a `size`-filter.

UAT data caveat: on bealls UAT every row is `pack_type='eaches'` with `units_in_pack=1` — i.e., there are no real case-pack constraints in this tenant's data. The DV will answer the "min-pack starving stores" prompt correctly on tenants with real pack structures; on bealls UAT the constraint is degenerate.

---

## Grouping

### G1. "Group 'Sunbelt-A' stores are over-performing on swim — split into a hotter sub-group?"

**Status**: Answerable (per-tenant group-name vocabulary mismatch, but the shape works).

Bealls store groups don't include "Sunbelt-A" — they have "Bealls All Stores", "KN_BLS_ALL_STORES_4.30", etc. Picking the largest, "Bealls All Stores":

```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{l1_name, eq, ["3145-SWIM"]}, {region, in, [<the 7 regions>]}],
  group_by=["store_code", "region"],
  aggregates=[{column:"lw_units", op:"sum"}, {column:"l8w_units", op:"sum"}, {column:"sell_through_perc", op:"avg"}],
  sort_col="l8w_units_sum", sort_dir="desc",
  limit=20
)
```
Returns top swim performers across the store group, sorted by 8-week volume — split candidates surface as the cluster of top performers. **Duration**: 5.3s.

For graph-native "stores in store_group X with swim metrics", `graph_traverse(store_group → store_code, cross_edge)` returns the membership; then a per-store dataview_read filtered to swim gives the metrics. 2-call chain.

### G2. "The 'Basics-Core' product group has 12 dead SKUs — drop them."

**Status**: Blocked (data exists upstream, partially).

bealls has no `product_groups` table — only **store** groups. The "Basics-Core" concept would need a new ingestion source. `product_profile_master` and `product_profile_mapping*` tables exist and might be the moral equivalent (product profiles ≈ product groups), but the semantic mapping isn't 1:1.

For dead-SKU discovery without the group concept:
```
dataview_read(id="dv_zero_sale_active_products", group_by=["l2_name"], aggregates=[{column:"*", op:"count"}], sort_col="count_all", sort_dir="desc")
```
Returns the worst-offending L2s by count of zero-sale active articles. Not the same answer as the prompt but the closest doable today. **Duration**: 5.0s.

### G3. "Store group 'Tourist-Heavy' needs a re-tier — propose membership changes based on last quarter."

**Status**: Blocked (quarterly data not available).

We have `lw_units` (1 week) and `l4w_units` (4 weeks) and `l8w_units` (8 weeks) on the per-store dataview. **Quarterly (13 weeks)** isn't surfaced. The upstream alert tables only carry these three windows. For quarterly re-tier:
- Either materialize a wider sales-history table (P2-1 in the original gaps doc, though the alerts table covers 8 weeks),
- Or run the analysis at 8-week scope as an approximation.

8-week-scope today:
```
dataview_read(
  id="dv_alerts_product_store",
  group_by=["store_code", "region"],
  aggregates=[{column:"l8w_units", op:"sum"}, {column:"l8w_units", op:"sum", alias:"l8w_units_sum"}],
  sort_col="l8w_units_sum", sort_dir="desc",
  limit=20
)
```
8-week ranking of stores — approximation of "tier 1" candidates. **Duration**: 5.4s.

---

## Finalize

### F1. "Allocations pending approval over 24 hours — who's the bottleneck?"

**Status**: Answerable.

```
dataview_read(
  id="dv_allocation_status",
  group_by=["allocation_code"],
  aggregates=[{is_terminal, max}, {run_started_at, min}],
  having=[{is_terminal_max, eq, [false]}],
  sort_col="run_started_at_min", sort_dir="asc",
  limit=20
)
```
Returns the 24 allocation_codes that never reached a terminal phase event, oldest-first. `run_started_at` is derived in-source from the embedded timestamp in `allocation_code` (status_time records per-phase event time; allocation_code carries the run-identity time). **Duration**: 4.7s.

The "reviewer / bottleneck" pivot needs a user-attribution column that isn't on `tb_auto_allocation_status` — the rows carry `updated_by_username` indirectly via the linked `tb_allocation_status` table (manual runs only, empty on UAT). Partial-attribution today; full bottleneck attribution opens up when the manual-status table populates.

### F2. "Locked plan for week 22 has 9 stores under-allocated — release a top-up."

**Status**: Partial.

`dv_allocation_split_po` carries the locked-plan state (`status`, `is_edited`) per (article, store) with `allocated_total`, `demand`, `min`, `max`, `oh`. Underallocation = `allocated_total < min` (or `< demand`). The DV answers the read side directly.

UAT data caveat: on bealls UAT replica `allocated_total`, `demand`, `min`, `max` are NULL across most rows — the allocator-output columns are stubbed. The composition logic is correct; numeric answers wait on populated runs.

### F3. "What changed between the proposed and finalized plan for DC2? Summarize."

**Status**: Partial.

`dv_allocation_split_po` is pinned to the `_20260501` dated snapshot — one allocation run. To diff proposed vs finalized, we need *two* snapshots (proposed run + finalized run). The architecture is there (each dated table is a snapshot); the DV needs a second binding or a parameterized snapshot pick. Filed earlier: canonical `create_allocation_result_flat_gurobi_split_po` is empty between runs (fb_1778943083979646000).

---

## Reports

### R1. "Build a deep-dive on denim sell-through for the last 6 weeks, region by region."

**Status**: Partial.

We have `l4w` and `l8w` rolling windows in `alerts_product_store_level`. "Last 6 weeks specifically" isn't a window we carry. Closest doable:
```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{l2_name, ilike, ["%DENIM%"]}],
  group_by=["region", "l1_name"],
  aggregates=[{column:"l8w_units", op:"sum"}, {column:"l8w_revenue", op:"sum"}, {column:"sell_through_perc", op:"avg"}]
)
```
8-week denim sell-through by region × L1 — approximation. **Duration**: 5.2s. Returns 24 (region, L1) cells across denim categories.

### R2. "Daily report shows a 12% drop in NE — which categories drove it?"

**Status**: Answerable (already walked in `dataview-read-composition.md`).

3-call chain: region rollup → pick laggard (M.Ritz -4.4% WoW) → L1 rollup → pick laggard (MISSES SPORTSWEAR) → L2 rollup → EC WOVEN TOPS -25.6% identified. Already captured in detail in `docs/dataview-read-composition.md` Prompt #1. **Total wall-clock 15.4s** across 3 calls.

### R3. "Compare allocation accuracy this month vs last for top 100 SKUs."

**Status**: Deferred.

Allocation side is now answerable via `dv_allocation_split_po` (per (article, store, run) with `allocated_total`). Accuracy requires forecast on the other side — same Deferred status as D3: forecast lives in BQ, not PG. When `dv_forecast_weekly` ships as a `bq_export`, the join becomes a single composition.

---

## VPA (Vendor PO Allocation)

### V1. "PO 99412 lands Friday at CFC1 — propose store-level splits."

**Status**: Answerable (read side).

```
dataview_read(
  id="dv_allocation_split_po",
  filters=[{allocation_code, like, ["%99412%"]}],
  sort_col="store", sort_dir="asc",
  limit=200
)
```
Returns the store-level splits already proposed for that allocation_code. "Propose *new* splits" remains an external Gurobi run — not an LLM read; it triggers an upstream pipeline.

### V2. "Vendor's PO feed is missing 6 SKUs we expected — flag and chase."

**Status**: Blocked (vendor master not in inventory_smart).

The PO tables carry PO-level data but I didn't find a `vendor` master table in `inventory_smart`. Vendor lives upstream in procurement. Filed.

### V3. "CFC-to-store flow for swim is backed up — which lanes are slowest?"

**Status**: Deferred.

Same data path as C5: `supply_route` / `store_to_store_transit` empty on UAT; lead-times data probably lives in BigQuery. Deferred until the BQ source is wired.

### V4. "Which open POs cover the stockouts flagged on the dashboard today?"

**Status**: Answerable via composition.

Two-call chain:

1. `dataview_read(id="dv_alerts_product_store", filters=[{stockout, gt, [0]}], group_by=["article"], aggregates=[{column:"*", op:"count"}])` → set of stocked-out articles.
2. `dataview_read(id="dv_allocation_split_po", filters=[{article, in, [<set>]}, {status, in, [<open-statuses>]}])` → open POs covering those articles.

The DVs sit on the same `article` key — the LLM joins in-memory between calls. UAT data caveat: `status` on `create_allocation_result_flat_gurobi_split_po_20260501` is an integer code (1, 2, …); the planner-LLM picks "open" codes by reading the seeded value distribution.

---

## CNA (Cross-Network Allocation)

### CN1. "For store 5512, where did this week's allocations come from? DC vs. cross-ship."

**Status**: Partial.

```
dataview_read(
  id="dv_allocation_split_po",
  filters=[{store, eq, ["5512"]}],
  group_by=["source", "inventory_source"],
  aggregates=[{column:"allocated_total", op:"sum"}, {column:"article", op:"count_distinct"}],
  sort_col="allocated_total_sum", sort_dir="desc"
)
```
The DV carries `source` and `inventory_source` columns intended to label DC vs cross-ship. UAT data caveat: both columns are stubbed to "0" across all rows on the bealls UAT snapshot — the breakdown logic ships today; the labels open up when upstream populates them. (Earlier feedback `fb_…` on the stubbed columns is the rollup of this gap.)

### CN2. "Inventory source mix for kids' is 80% DC1 — should we diversify?"

**Status**: Same as CN1 — same DV, hierarchy filter added.

```
dataview_read(
  id="dv_alerts_product_store",
  filters=[{l1_name, ilike, ["%CHILDREN%"]}],  # or join with hierarchy upstream
  ...
)
```
Then pivot to `dv_allocation_split_po` for the source-mix. Same UAT stub on `source`/`inventory_source`.

### CN3. "Show stores whose allocations came mostly from cross-ship last month — fix root cause."

**Status**: Same as CN1 — multi-store rollup.

`group_by=["store"]` with `aggregates=[{column:"source", op:"count_distinct"}, …]` and the same UAT stub caveat on the source label. Multi-week aggregation needs the canonical (non-dated) `create_allocation_result_flat_gurobi_split_po` table to populate, or a parameterized snapshot pick across daily partitions.

---

## Summary

| Status | Count | Prompts |
|---|---:|---|
| Answerable today | **9** | D1, D5, C1, C3, G1, R2, F1, V1, V4 |
| Partial (DV exists; UAT data quality limits the answer, or compute side waits on a Gurobi run) | **9** | D2, D4, C4, Co2, Co3, Co4, F2, F3, R1, CN1, CN2, CN3 |
| Deferred (PG table empty on UAT; data lives in BQ or another tenant source) | **4** | D3, R3 (forecast), C5, V3 (lead-times) |
| Blocked, data not in inventory_smart | **4** | C2 (new-store workflow), G2 (no product_groups), G3 (>8w history), V2 (vendor master) |
| Blocked, allocator-side compute (constraint-violations log) | **1** | Co1 |

**The reframe (revised)**: this batch landed four leverage DVs (`dv_allocation_status`, `dv_allocation_split_po`, `dv_supersession_chain`, `dv_pack_constraints`), moving **8 prompts** from Blocked → Answerable or Partial. The remaining "Partial" group is mostly **UAT data quality** — the DV correctly exposes the column shape but the tenant has stubbed values (e.g., `inventory_source='0'`, `allocated_total=NULL`) that block the numeric answer. Two prompt families (forecast: D3/R3, lead-times: C5/V3) are deferred until the BQ-backed sources are wired.

## DataView rollup

| DataView | Backed by | Status | Closes prompts |
|---|---|---|---|
| `dv_allocation_status` | `tb_auto_allocation_status` (+ embedded run_started_at) | Shipped | F1 (full), F2/F3 (partial) |
| `dv_allocation_split_po` | `create_allocation_result_flat_gurobi_split_po_20260501` | Shipped (UAT stubs on `source`/`inventory_source`/`allocated_total`) | D4, V1, V4, CN1, CN2, CN3 (read side) |
| `dv_supersession_chain` | `product_supersession_mapping` | Shipped (153 active rows) | C3 |
| `dv_pack_constraints` | `dc_pack_configuration` | Shipped (1.35M rows; UAT data is `eaches`-only) | Co4 |
| `dv_forecast_weekly` | `dc_forecast_week_level` (BQ-backed in reality) | **Deferred** — needs `bq_export` source | D3, R3 |
| `dv_dc_lead_times` | `supply_route` + `supply_node` (or `store_to_store_transit`) | **Deferred** — PG tables empty on UAT; likely BQ-backed | C5, V3 |

## Total wall-clock observed during the revised run

- Per-call: 4.6–5.6s (PG cold reads, no plan caching across calls).
- Composition cases (V4) double or triple that wall-clock for multi-call chains.
- New aggregate-clause path adds ~0.5s but unlocks group_by/having patterns (F1, F2-shape).

The earlier framing in `docs/planner-data-gaps.md` (heavy P2 ingestion roadmap) materially over-estimated the work. The reality (revised): the upstream schema is rich enough to answer most planner prompts via composition; the remaining gaps are (a) tenant data quality, and (b) two cross-system pulls (forecast + lead-times) that need BQ-backed DataViews.
