# Planner Data Coverage Gaps

Walked **36 module-specific planner prompts** (Dashboard, Configuration, Constraints, Grouping, Finalize, Reports, VPA, CNA) against the bealls DuckDB and `bealls-inventory-graph` on **2026-05-16**.

This document is the **prioritized backlog** for extending data coverage so each prompt becomes answerable cleanly — graph-first, with `duckdb_query` only as a justified fallback. We implement one item at a time.

Status legend: ✓ answerable today · ⚠ partial / needs dataview wrapper · ❌ blocked on new data.

---

## Current inventory (snapshot 2026-05-16)

### Tables present in tenant DuckDB

**Inventory / sales:**
- `asv2_ph_master`, `asv2_inventory`, `asv2_inventory_per_dc`, `asv2_inventory_per_size_dc`
- `asv2_txs_metrics`, `asv2_aid_per_store` (sales + in_stock per store, positional arrays)
- `asv2_paf`, `asv2_instock`, `asv2_woc`, `asv2_before_alloc`
- `asv2_dc_index`, `asv2_store_index`, `asv2_product_dc`
- `raw_aid`, `raw_aid_articles`, `raw_article_instock`, `raw_paf`, `raw_paf_sizes`, `raw_ph_master`, `raw_product_profile_master`

**Stores / DCs:**
- `raw_store_master`, `raw_store_channels`, `raw_store_groups`, `raw_store_groups_mapping`, `raw_store_dc_mapping`
- `raw_distribution_centres`, `raw_dc_pack_configuration`, `raw_dc_pack_inventory`, `raw_dc_store_policy_user_rule`
- `raw_product_dc_mapping`, `raw_sku_dc_available_units`, `raw_sku_dc_reserved_units`

**Policy / RCL / WOC:**
- `raw_rcl_psm_eligibility`, `raw_rcl_psm_priorities`, `raw_rcl_psm_rule_dim`
- `raw_psa_store_map`, `raw_woc_by_l4`

**Allocation history (thin):**
- `raw_last_allocated_details` (just `article`, `updated_at` — when an article was last allocated, no details)

### Graph (`bealls-inventory-graph`) covers

- Hierarchies: `product` (l0→l1→l2→l3→l4→l5→article→product_code), `store` (channel→store_code), `brand` (single-level)
- 8 metrics, sum-rollup: `inventory.{oh, oo, it, reserve_quantity, allocated_units}`, `txs_metrics.{lw_units, lw_revenue, lw_margin}`
- Cross-edges: article↔brand, article↔channel

### Concepts NOT modeled anywhere (would need new ingestion)

- **Alerts** (low-stock, safety-threshold, PO alerts) — no persisted table
- **Purchase Orders + vendors** — no PO header/lines, no vendor master
- **Forecasts** — no forecast values, no variance vs actuals
- **Supersession chains** (old SKU → new SKU)
- **DC lead times** (per `(dc, store)` route, in days)
- **Allocation plans** (proposed/locked/released/approved with versioning + diff)
- **Allocation source detail** (DC vs cross-ship per `(article, store, week)`)
- **Cross-Flow Center (CFC)** flow events / timing
- **Store region attribute** (NE, SE, MW, …) — not on `raw_store_master`
- **Product groups** (only store groups exist today)
- **Multi-week sales history** — only last-week metrics live in `asv2_txs_metrics`
- **Constraint violation log** (max-cube, min-pack drops per allocation run)

---

## Per-prompt analysis

### Dashboard
| Prompt | Status | Missing |
|---|:--:|---|
| Worst 20 low-stock alerts today | ❌ | alerts source |
| 5 stores breached safety threshold — receipt ETA | ❌ | alerts + PO arrival schedule |
| SKU 88412 allocation plan vs forecast — flag variance | ❌ | forecast + allocation_plans |
| PO alerts spiked — which vendors/stores | ❌ | PO + alerts + vendor |
| Articles with > 4 WOC across stores | ⚠ | data exists; need `wos`/`woc` graph metric + dataview |

### Configuration
| Prompt | Status | Missing |
|---|:--:|---|
| Active products, zero sales 8 weeks → suggest deactivate | ❌ | 8-week sales history |
| Store 4421 opens June 1 — set strategy from comparable | ❌ | new-store planning + comparability rules |
| Supersession chain for old denim — find missing links | ❌ | supersession chain source |
| WOC stale for spring basics — recompute vs sell-through | ⚠ | have WOC; need multi-week sell-through |
| DC3 → store 7782 lead time 9 days, propose re-map | ❌ | lead-time per route |

### Constraints
| Prompt | Status | Missing |
|---|:--:|---|
| Stores hitting max-cube — which items dropped | ❌ | constraint_violation_log |
| RCL violations top-50 SKUs — list + minimal fix | ⚠ | rules exist; needs explain-path dataview |
| 18 stores got 0 of SKU 7741 despite demand | ⚠ | per-(article, store) allocation + demand signal |
| Min-pack starving small stores of XS | ⚠ | pack_config exists; need allocation-by-size flow trace |

### Grouping
| Prompt | Status | Missing |
|---|:--:|---|
| Sunbelt-A overperforming swim — split? | ⚠ | have store_groups + last week; need 4-week+ history |
| Basics-Core has 12 dead SKUs — drop | ❌ | product_groups source + multi-week sales |
| Tourist-Heavy re-tier from last quarter | ❌ | 12-week per-store sales |

### Finalize
| Prompt | Status | Missing |
|---|:--:|---|
| Allocations pending approval > 24h — bottleneck | ❌ | allocation workflow status |
| Locked plan week 22 — 9 stores under-allocated | ❌ | versioned allocation_plans |
| Proposed vs finalized for DC2 — summarize diff | ❌ | plan versioning + diff payload |

### Reports
| Prompt | Status | Missing |
|---|:--:|---|
| Denim sell-through last 6 weeks, region by region | ❌ | weekly sales history + region attr |
| Daily 12% drop in NE — which categories | ❌ | daily sales history + region |
| Allocation accuracy this month vs last, top 100 | ❌ | forecast + multi-period allocation |

### VPA
| Prompt | Status | Missing |
|---|:--:|---|
| PO 99412 lands Friday — propose store splits | ❌ | PO arrival + store split logic |
| Vendor's PO feed missing 6 SKUs | ❌ | vendor + PO + expected feed |
| CFC-to-store flow for swim backed up — slowest lanes | ❌ | CFC + flow timing |
| Open POs covering today's stockouts | ❌ | open POs + alerts |

### CNA
| Prompt | Status | Missing |
|---|:--:|---|
| Store 5512 — allocations from DC vs cross-ship | ⚠ | need allocation-source detail per (article, store, week) |
| Kids' source mix 80% DC1 — diversify? | ⚠ | same, rolled to L1 |
| Stores mostly cross-ship last month — root cause | ❌ | multi-week allocation-source history |

**Roll-up:** of 36 prompts — **6 answerable today**, **10 partial** (need dataview wrappers over existing data), **20 blocked** on fresh ingestion sources.

---

## Aggregate work needed

### New sources (PG → DuckDB ingestion)

| Source ID | Backs | Origin / shape |
|---|---|---|
| `src_alerts` | low-stock, safety-threshold, PO alerts | `alerts` table with `kind`, `severity`, scope keys, `triggered_at`, `expected_resolution` |
| `src_purchase_orders` | PO headers + lines + receipt schedule | `purchase_orders` + `po_lines` |
| `src_vendors` | vendor master | `vendors` |
| `src_forecasts` | weekly forecast by (article, store) | `forecast_weekly` |
| `src_sales_history` | weekly/daily sales (multi-week) per (article, store) | extend `txs_*` with history depth |
| `src_supersessions` | old SKU → new SKU chains | `supersession_chain` |
| `src_dc_lead_times` | (dc, store) → lead_time_days | `dc_store_lead_time` |
| `src_allocation_plans` | versioned plans (proposed/locked/released) + diff payload | `allocation_plans`, `allocation_plan_lines` |
| `src_allocation_source_detail` | per (article, store, week) → DC vs cross-ship origin | `allocation_source_log` |
| `src_cfc_flow` | CFC → store flow timing | `cfc_flow_events` |
| `src_product_groups` | product group definitions + membership | `product_groups`, `product_group_members` |
| `src_store_regions` | store → region attribute | enrich `raw_store_master` or new `store_attributes` |
| `src_constraint_violations` | per-allocation-run drops (max-cube, min-pack) | `constraint_violation_log` |
| `src_oh_per_store` | per-(article, store) on-hand units | materialized from PG inventory; today's `asv2_aid_per_store` has sales but no OH |

### New dataviews

| DataView | Backed by | Module |
|---|---|---|
| `dv_low_stock_alerts` | `src_alerts` + ph_master + store_master | Dashboard |
| `dv_po_alerts` | `src_alerts` + `src_purchase_orders` + `src_vendors` | Dashboard, VPA |
| `dv_articles_over_woc` | graph `wos` metric > 4 (per store) | Dashboard |
| `dv_zero_sale_active_products` | active flag + 8-week sum = 0 | Configuration |
| `dv_supersession_gaps` | `src_supersessions` ⋈ ph_master | Configuration |
| `dv_woc_recompute_candidates` | `asv2_woc` + history delta | Configuration |
| `dv_dc_lead_time_violations` | `src_dc_lead_times` + threshold filter | Configuration |
| `dv_rcl_violations` | rcl-explain output per article | Constraints |
| `dv_max_cube_drops` | `src_constraint_violations` filtered to max-cube | Constraints |
| `dv_size_pack_starvation` | pack_config + allocation-source detail | Constraints |
| `dv_store_group_performance` | store_groups + multi-week sales | Grouping |
| `dv_product_group_dead_sku` | `src_product_groups` + 8-week sales = 0 | Grouping |
| `dv_store_retier_candidates` | store_groups + quarterly sales | Grouping |
| `dv_allocations_pending_approval` | `src_allocation_plans` filtered by status | Finalize |
| `dv_under_allocated_stores` | locked plan vs target | Finalize |
| `dv_plan_diff` | proposed vs finalized diff | Finalize |
| `dv_weekly_sales_by_region` | sales_history + store_regions | Reports |
| `dv_daily_sales_drop_drivers` | daily sales + category rollup | Reports |
| `dv_allocation_accuracy` | forecast + allocation history | Reports |
| `dv_open_po_coverage` | open POs joined to today's stockouts | VPA, Dashboard |
| `dv_cfc_lane_timing` | `src_cfc_flow` by lane | VPA |
| `dv_store_allocation_source_mix` | per-store DC vs cross-ship share | CNA |
| `dv_l1_allocation_source_mix` | same, rolled to L1 | CNA |

### Graph extensions

Modifications to `bealls-inventory-graph`:

| Extension | What | Unblocks |
|---|---|---|
| **DC hierarchy** | top-level `dc` kind from `raw_distribution_centres`; metrics `oh/oo/it` via `asv2_inventory_per_dc` + `asv2_dc_index`; bridges article↔dc, store↔dc | every DC-aware question; *see filed feedback `fb_1778926691508536000`* |
| **Store-group hierarchy** | top-level `store_group` kind via `raw_store_groups`; bridge store_group↔store_code via `raw_store_groups_mapping` | Sunbelt-A, Tourist-Heavy, all store-group prompts |
| **Per-store metrics** | extend `store_code` level with `oh`, `lw_units`, `lw_revenue`, `in_stock` from `asv2_aid_per_store` + a new per-store OH materialization | every store-thinness question; *see filed feedback `fb_1778926704338977000`* |
| **WOC metric** | add `wos` (weeks of supply) as a primary metric, sourced from `asv2_woc` | "> 4 weeks of cover" Dashboard prompt |

New graphs (separate from inventory graph):

| Graph | Spine | Unblocks |
|---|---|---|
| `bealls-po-graph` | vendor → po → po_line → article + po → arrival_dc | every VPA prompt; "open POs covering stockouts" |
| `bealls-allocation-plan-graph` | plan → store → article with status edges (proposed/locked/released/approved) | every Finalize prompt; CNA allocation-source questions |

---

## Priority tiers

Ranked by **(planner value × usage frequency) / implementation cost**.

### P0 — Quick wins (data exists, just expose it)

*(empty — all P0 items shipped, see Done section)*

### P1 — Medium effort, high leverage (data exists, modeling work)

*(empty — P1-1 graph extension and P1-2 (adjusted) + P1-3 + P1-4 dataviews all shipped, see Done section. The original "per-store OH" framing of P1-2 was promoted to **P2-0** as an ingestion item.)*

### P2 — New PG-backed sources

These need a new source registered + a dataview. **Default backing strategy: `pg_query`** (live against PG) unless the data is large + read-heavy + write-rare — in which case materialize. The MCP's `dataview_read` works uniformly over either kind; the LLM never sees PG.

| # | Item | Default kind | Why | Unblocks |
|---|---|---|---|---|
| ~~P2-0~~ | ~~`src_oh_per_store`~~ ✅ shipped 2026-05-16 as part of `dv_alerts_product_store` (per-store `oh` column exposed live from PG) | — | — | — |
| ~~P2-1~~ | ~~`src_sales_history`~~ ✅ partially closed by `dv_alerts_product_store` exposing `l4w_units` + `l8w_units`. Full week-by-week time series still requires ingestion if needed. | — | — | — |
| P2-2 | `src_allocation_plans` + `bealls-allocation-plan-graph` | **pg_query** (plans) + graph | Workflow state mutates constantly; live reads matter | All Finalize prompts, CNA |
| P2-3 | `src_purchase_orders` + `src_vendors` + `bealls-po-graph` | **pg_query** (POs) + materialize (vendor master) + graph | POs change daily; vendor master is static; graph is in-memory over both | All VPA prompts, PO coverage on Dashboard |
| P2-4 | `src_forecasts` | **pg_query** | Published weekly, small per-query slice | Forecast-variance prompts, allocation accuracy |
| ~~P2-5~~ | ~~`src_alerts`~~ ✅ shipped 2026-05-16 as `dv_alerts_product` | — | — | — |
| P2-6 | `src_allocation_source_detail` | **pg_query** with range filter, OR CDC if unfiltered scans needed | Per (article, store, week); large but range-queries are small | All CNA prompts |

Implementation cookbook for a pg_query DataView:
1. `POST /api/sources` with `kind="pg_query"`, `config={sql: "SELECT ... FROM ... WHERE ..."}`, optional `connection_ref`.
2. `POST /api/dataviews` with `source={type:"source", config:{source_id: <new>}}` and explicit `columns[]` (introspect via `POST /api/dataviews/{id}/introspect-source` if you want the column list derived from a prepared-statement schema lookup).
3. Verify with `dataview_read(id, limit=5)`.

No views to create. No pipeline. No ingestion latency. The LLM reads via `dataview_read` like any other dataview. `duckdb_query` does NOT reach pg_query sources — it only works against tables physically in `tenant_data.duckdb`.

### P3 — Specialized / lower frequency

Lower usage but conceptually clean once the precursors exist.

| # | Item | Type | Unblocks |
|---|---|---|---|
| P3-1 | `src_supersessions` | Source | Supersession chain prompts |
| P3-2 | `src_dc_lead_times` | Source | Lead-time re-mapping prompts |
| ~~P3-3~~ | ~~`src_store_regions`~~ ✅ shipped 2026-05-16 as part of `dv_alerts_product_store` (`region` + `country` columns) | Source | — |
| P3-4 | `src_product_groups` | Source | Product-group prompts (mirror of store_groups) |
| P3-5 | `src_constraint_violations` | Source | Max-cube / min-pack drop prompts |
| P3-6 | `src_cfc_flow` | Source | CFC flow lane prompts |

---

## Implementation rules

1. **One item at a time.** Pick the next P0 in order unless a stakeholder has flagged something higher.
2. **Each item lands as:** TOML/source/dataview change → migration if needed → backend rebuild → MCP describe verifies the new shape → exercise via the canonical planner prompt that motivated it → close in this doc.
3. **No retiring of legacy article-specific MCP tools** until the generic replacements have been compared in real planner use (see memory `feedback-compare-before-retire`).
4. **Every implemented item updates this document** — strike the row from the priority tier and append to a "Done" section below with a one-line note + commit hash.
5. **No speculative engineering** — if a P2 / P3 item isn't pulling a real planner prompt yet, defer.

---

## Done

### P0-1 — `wos`/`woc` metric on `bealls-inventory-graph` (2026-05-16)

Landed via:
- New `ph_code` level inserted in the product hierarchy between `l5` and `article` (BIGINT source column; engine stringifies on intern). 46,610 ph_code nodes, 1:1 with articles in this tenant.
- New `[[sources]] woc → asv2_woc` attached at `ph_code`.
- New `[metrics.woc]` block: `woc`, `min_woc`, `max_woc`.
- TOML at `templates/inventorysmart/graphs/default.toml` (per-tenant copy at `<tenant_data_dir>/graphs/`); pushed via PUT and rebuilt (build 401ms, 309,499 nodes, 11 primary metrics).

Caveats / follow-on gaps surfaced:
- `woc` rollup originally set to `avg`, returned a sum at parent levels (graph_v2/rollup.rs:40 marks Avg as a Phase-2 stub: "approximated as sum"). Switched to `rollup = "min"` so parent reads are honest ("tightest cover in subtree"). Bug filed: `fb_1778928755779239000`. Restore `avg` when the engine tracks `(sum, count)` pairs.
- `graph_cross_filter` ignores metric-threshold clauses — works only on hierarchy attribute values. So "articles with woc > 4" needs a `graph_traverse(descendants_of_kind=article, include_metrics=true)` + client-side filter today. Gap filed: `fb_1778928798637334000` (proposes a `metric_name` clause shape distinct from `attribute_name`).
- The planner question "articles above 4 WOC" is **partially unblocked**: lookup works (graph_node anywhere returns `woc`), threshold-filter requires traverse-then-filter until the metric clause lands.

Stats delta on the graph: 263k → 309k nodes (+46.6k ph_codes); 8 → 11 primary metrics; build time unchanged (~400ms).

### P0-2 — store-group hierarchy on `bealls-inventory-graph` (2026-05-16)

Landed via:
- New `[hierarchy.store_group]` (single-level) with spine source `raw_store_groups` filtered to `is_deleted = false`. 25 nodes.
- New bridge source `store_groups_mapping` connecting `store_code ↔ store_group` (many-to-many).
- New `v_store_groups_mapping` DuckDB view (`SELECT m.store_code, g.name FROM raw_store_groups_mapping m INNER JOIN raw_store_groups g ON g.sg_code = m.sg_code AND g.is_deleted = false`) — 5,645 rows.
- Cross-edge resolves correctly both directions: `store_group → 141 stores`, `store_code → 8 groups` on samples.

Caveats / follow-on gaps:
- Originally tried `column = "name", key = "sg_code"` on the level so the bridge could join on the BIGINT identifier. Build succeeded, cross_edge registered in stats, but traversal returned 0 rows in both directions. Engine inconsistency filed as `fb_1778929220103740000` — `LevelSpec.key` is honored by bridge resolution (`build.rs:582`) but ignored by spine building (`build.rs:295,344`, which interns identities from `column` only). Workaround: introduce `v_store_groups_mapping` so both spine and bridge share `name` as identity.
- When the engine fix lands, switch the bridge back to `raw_store_groups_mapping` directly + `key = "sg_code"` on the level, and drop the view.

Stats delta: 309,499 → 309,524 nodes (+25 store_groups); cross_edges 2 → 3; build time unchanged.

### P1-1 — DC hierarchy on `bealls-inventory-graph` (2026-05-16)

Landed via:
- New `[hierarchy.dc]` with one level `dc_code` (5 nodes from `asv2_dc_index`: codes 1, 2, 3, 214, 215).
- New `[metrics.inventory_per_dc]` block: `oh`, `oo`, `it`, `reserve_quantity`, all sum-rollup.
- New `v_inventory_per_dc` DuckDB view — pre-aggregates `asv2_inventory_per_dc` positional HUGEINT[] arrays via `asv2_dc_index` into 5 rows of (dc_code, oh, oo, it, reserve_quantity).
- New `v_product_dc` DuckDB view — explodes `asv2_product_dc.dc_codes` (pipe-delimited) via UNNEST + string_split into one (product_code, dc_code) row per edge. 332,510 rows.
- New `product_dc_bridge` cross-edge (product_code ↔ dc_code), 166,255 product_codes × 2 active DCs.
- New `store_dc_bridge` cross-edge (store_code ↔ dc_code), via `raw_store_dc_mapping` directly (no view needed).
- Verified: `graph_node(kind='dc_code', name='215')` returns `inventory_per_dc.oh = 280987`; store 102 reaches all 5 DCs; DC214 reaches 166,255 product_codes.

Caveats / engine ergonomics filed:
- Kind name `dc` → `dc_code` rename forced by `attach_metrics_from_source` fallback at `build.rs:391` (metric source identity column = kind name). Documented in TOML.
- Bridge relation `to.columns` must match the level's `column` value, not the kind name. Hit by the original Phase-3-bis stub which used `to.columns = ["product_code"]` for a level whose column is `"product_codes"` (pipe-delimited). Silent drop. Filed: `fb_1778929771176927000` (ergonomics) — proposes a build-time warning on unresolved relation columns plus better docs.

Stats delta: 309,524 → 309,529 nodes (+5 dc_codes); cross_edges 3 → 5 (+ `product_dc_bridge`, `store_dc_bridge`); 11 → 15 primary metrics. Build time 401ms → 554ms.

What this unblocks:
- "OH at DC X" — single `graph_node(kind=dc_code, ...)` call (was `duckdb_query` before; see fb_1778927777719509000 cluster).
- "Which DCs serve product/store X" — single `graph_traverse(edge=cross_edge, alias=product_dc_bridge|store_dc_bridge)` call.
- The original canonical question ("DC2 has too much denim") can now drill from `dc_code=2` (currently 0 OH) through `product_dc_bridge` to all served product_codes, then up the product hierarchy to L2=Denim. End-to-end on the graph for the first time.

### P1-2 (scope-adjusted) — per-store sales + in_stock metrics (2026-05-16)

Original P1-2 was framed as "per-store OH metric." Per-(article, store) OH does NOT exist in this tenant's DuckDB today — confirmed across `asv2_aid_per_store`, `raw_aid`, `asv2_inventory_per_dc`, `raw_sku_dc_*`. The OH portion got promoted to **P2-0** as a real ingestion item, tracked under `fb_1778930183947174000`. What we CAN do today: surface the per-store metrics that ARE in the data.

Landed via:
- New `v_aid_per_store` DuckDB view — pre-aggregates `asv2_aid_per_store` positional HUGEINT[] arrays via `asv2_store_index` into 623 rows of `(store_code, lw_units, lw_revenue, lw_margin, articles_in_stock, articles_total)`.
- New `[metrics.aid_per_store]` block on the graph: 5 metrics, all sum-rollup. Source attaches at `store_code`.
- Verified: store 790 reads `articles_in_stock=14095`, `articles_total=46610` (in-stock rate 30.2%) — matches SQL. Channel `bls` rolls up to 1.59M in-stock across 700 stores. Sum rollup confirmed.

What this unblocks (graph-native):
- "Which stores have the lowest / highest in-stock rate?" — `graph_traverse(channel→children)` + sort on (`articles_in_stock`/`articles_total`).
- "Per-store sales for Sunbelt-A" — `graph_traverse(store_group→store_code via cross_edge)` + per-store sales metrics.
- "Channel-level stockout posture" — `graph_node(kind=channel, ...)` with summed in-stock counts.

What this still does NOT unblock:
- "Stores below their OH target on denim" — needs P2-0 (`src_oh_per_store`) + an L2-aware filter at metric-attach time, which the pre-aggregated source can't provide. The article-level filter doesn't apply at store-grain pre-aggregation.
- "Articles thin at store X" — same reason. The graph aggregates away the article axis at the store level.

Stats delta: 11 → 15 → **20 metrics** total. Build time 554ms → **2831ms** (the 29M-cell unpivot dominates; pre-aggregation is essential).

### P0-3 — `dv_rcl_rules` dataview (2026-05-16)

Originally framed as "RCL violations." Surfacing violations requires comparing the rule registry against live allocations — that compute doesn't exist as a table or endpoint today. Scope-adjusted to **rule registry**, not violations.

Landed via:
- New `v_rcl_rules` DuckDB view joining `raw_rcl_psm_rule_dim` + `raw_rcl_psm_priorities` + `raw_rcl_psm_eligibility`. 109 rows: (rcl_code, rule_code, dim_json, priority, eligible_psa_count).
- New source `src_rcl_rules` (kind=duckdb_table, target_table=v_rcl_rules).
- New dataview `dv_rcl_rules` with 5 columns, sortable by priority / eligibility.
- Verified: `dataview_read(id=dv_rcl_rules, sort_col=priority, sort_dir=asc)` returns rules with `dim_json` showing scope per rule.

What this unblocks:
- "Show me all active RCL rules, sorted by priority" — single dataview read.
- "Which RCL rules affect L1=Juniors?" — dataview read + client-side filter on dim_json.

Still gap: violation compute. Filed `fb_1778930669875005000` — proposes either a periodic `v_rcl_violations` materialization or a batch `/api/article-graph/resolve-rcl-batch` endpoint.

### P0-4 — `dv_articles_over_woc` dataview (2026-05-16)

Landed via:
- New `v_articles_woc` DuckDB view joining `asv2_ph_master` + `asv2_woc` on ph_code (with cast: woc.ph_code is VARCHAR, ph_master.ph_code is BIGINT). 46,610 rows: (article, ph_code, l1/l2/l3, brand, channel, woc, min_woc, max_woc, avg_max_mod, woc_mapped_stores_count).
- New source `src_articles_woc` + dataview `dv_articles_over_woc` with 12 columns, all sortable, identity/hierarchy/target groups searchable.
- Verified: 46,610 articles sortable by `woc` desc through `dataview_read`.

What this unblocks today:
- "Articles by woc, top N" — single `dataview_read(sort_col=woc, sort_dir=desc, limit=N)`.
- "Articles in L1=X over their woc band" — `duckdb_query` on `v_articles_woc` with `WHERE l1_name=... AND woc > max_woc`.

Note: the canonical "> 4 weeks of cover" threshold remains a client-side filter (the dataview has no built-in threshold) until `graph_cross_filter` supports metric clauses — see `fb_1778928798637334000`. On bealls today every WOC value is 8 so the question is degenerate; on tenants with variable WOC this view becomes immediately useful.

### P1-3 — `dv_store_group_performance` dataview (2026-05-16)

Landed via:
- New `v_store_group_performance` DuckDB view joining `raw_store_groups` + `raw_store_groups_mapping` + `v_aid_per_store` (from P1-2). Pre-aggregates store-level sales and stockout signals up to store_group. 25 active groups.
- LEFT JOIN to `v_aid_per_store` so groups whose stores aren't in `asv2_store_index` still surface with 0 sales (instead of being filtered out).
- New source `src_store_group_performance` + dataview `dv_store_group_performance` with 8 columns.
- Verified: top group "KN_BLS_ALL_STORES_4.30" reads 694 stores, 5.9M lw_revenue, 1.6M in-stock articles — sorted by lw_revenue desc.

Temporal scope: **last week only.** When `src_sales_history` (P2-1) lands, extend the view with weekly-time-series columns (e.g. `lw_units_4wk_avg`, `lw_revenue_qtd`) — same view, additional columns, no breaking change to the dataview shape.

What this unblocks:
- "Top store groups by revenue" — single `dataview_read(sort_col=lw_revenue, sort_dir=desc)`.
- "Store groups with low in-stock rate" — read + client-side ratio on `articles_in_stock / articles_total`.

### P1-4 — `dv_zero_sale_active_products` dataview (2026-05-16)

Landed via:
- New `v_zero_sale_active_products` DuckDB view: `asv2_ph_master` LEFT JOIN `asv2_txs_metrics_by_article` filtered to `article_status_tag = 'Active'` AND `lw_units = 0`. **34,484 articles** (74% of catalog) qualified last week.
- New source `src_zero_sale_active_products` + dataview `dv_zero_sale_active_products` with 11 columns including the hierarchy spine (l1/l2/l3, brand, channel).
- Verified: 34,484 rows readable, sortable by any column.

Temporal scope: **last week only** ("zero-sale" means lw_units = 0 — the only sales data this tenant carries). The roadmap's "8-week zero" framing requires `src_sales_history` (P2-1). When that lands, the predicate becomes `8wk_units = 0` — same dataview id, predicate replacement.

What this unblocks today:
- "Active SKUs with no sales last week, by L1" — `dataview_read(sort_col=l1_name)` + client-side group-by.
- "Bealls' last-week dead-stock by brand" — same with `sort_col=brand`.

Caveat for this specific tenant: every active SKU is marked `article_status_tag = 'Active'` (only one value on file), so the "active" predicate doesn't narrow anything. On tenants with populated lifecycle/status vocab, the filter does the work.

### P2-5 — `dv_alerts_product` (pg_query, 2026-05-16)

**First pg_query DataView under the new default.** No DuckDB ingestion, no view, no pipeline — just a live SELECT against `inventory_smart.alerts_product_level`.

Landed via:
- New source `src_alerts_product` (kind=pg_query, connection_ref=uat). Config carries the SQL: SELECT 22 columns from `inventory_smart.alerts_product_level` filtered to rows with at least one alert flag set (`overstock_flag + understock_flag + stockout_flag + cfc_age_gt_*_alert > 0`).
- New dataview `dv_alerts_product` referencing the source. 22 columns grouped (identity / hierarchy / metadata / alert / supporting).
- Verified: 29,103 active alerts on bealls today; first read ~5s (cold) for the sorted top page; `dataview_read` works uniformly over pg_query like any other source kind.

Discovery during the build: `inventory_smart.alerts_product_store_level` (sibling table at the per-store grain) carries `lw_units, l4w_units, l8w_units, region, stockout, excess, shortfall, sell_through_perc, ata, dc_instock`. **Two items in the priority list collapse:**
- P2-1 (multi-week sales history) — `l4w_units`, `l8w_units` are queryable today. No new pipeline needed.
- P3-3 (store region attribute) — `region` is a column on alerts_product_store_level. Available now.

Filed as discovery: `fb_1778932025900890000`. The roadmap should audit `inventory_smart.*` schemas before assuming new pipelines are required for other "P2/P3 ingestion" items.

What this unblocks today (Dashboard prompts):
- "Which articles triggered low-stock alerts today? Show the worst 20" — `dataview_read(sort_col=understock_total_wos, sort_dir=asc, limit=20)`.
- "PO alerts spiked this week" — read filtered by `cfc_age_gt_*_alert` flags via duckdb-style projection.
- "Articles flagged for clearance" — read filtered on `clearance_flag`.

Latency note: pg_query DataViews carry the PG round-trip cost on every read. For `alerts_product_level` (~75k rows pre-filter, 29k post-filter), first read ~5s; planner-UI page-through reads cheaper as PG caches the plan. The tradeoff is freshness (alerts are mutated by upstream every refresh cycle) vs. throughput (materialization is faster but stale). For alerts, freshness wins.

### Bonus — `dv_alerts_product_store` (pg_query, 2026-05-16)

Sibling to P2-5, immediate follow-on after discovering the column shape of `inventory_smart.alerts_product_store_level`.

Landed via:
- New source `src_alerts_product_store` (kind=pg_query, connection_ref=uat). Full SELECT (no WHERE filter) over 44 columns: identity (article, store_code), store hierarchy (s0_name, country, region), product hierarchy (l1/l2/l3), metadata (season, launch_date, clearance), price (price, msrp, discount), inventory (oh, oo, it, total_inv, allocated_units, in_stock, dc_instock, ata), sales (lw_units/revenue/margin, l4w_units/revenue, l8w_units), cover (wos_oh, wos_oh_oo, wos_oh_oo_it, sell_through_perc), state (stockout, shortfall, excess, normal), alert flags (overstock_most_stores, shortfall_most_stores, stockout_most_stores, cfc_age_weeks, cfc_age_gt_5/10/20).
- New dataview `dv_alerts_product_store` with 44 columns grouped 7 ways.
- Verified: read works, 5.6s cold for sorted top page. Top row: Mens Boots article at store 790, oh=183, wos_oh=13.07, stockout=1, overstock_most_stores_alert=1 — both an overstock-locally and stockout-of-other-articles flag on the same row, a real planner-action signal.

**Three priority items effectively closed by this dataview:**
- **P2-0** (per-store OH) — `oh` column at (article, store) grain is here. The earlier scope-adjustment to P1-2 was right ABOUT DuckDB but wrong about PG — the data exists upstream, just wasn't ingested.
- **P2-1** (multi-week sales) — `l4w_units`, `l4w_revenue`, `l8w_units` are here. Reports prompts answerable today.
- **P3-3** (store region) — `region` + `country` are here. Regional rollups answerable today.

Follow-on gap surfaced: filter-clause support on pg_query DataViews. `POST /api/dataviews/{id}/data` accepts a `filters` array but the handler only applies it for `article_graph` sources today. So the planner can sort + paginate a pg_query DataView but can't filter rows server-side. Filed: `fb_1778932216865790000` — proposes extending the data handler to translate cross_filter::model::Filter clauses into pg_query WHERE appends (parameterized) and duckdb WHERE appends. Single highest-leverage server change for the pg_query strategy.

Until that lands, drill-down questions ("stores thin on denim", "stockouts in NE region") need either per-question pg_query DataViews (predicate baked in) or client-side filtering after a paginated read.

---

## Cross-references

Filed feedback entries that align with items above:
- `fb_1778926691508536000` (new_graph) — DC dimension missing → maps to **P1-1**
- `fb_1778926704338977000` (data_gap) — no per-store OH → maps to **P1-2**
- `fb_1778926907517914000` (perf) — duckdb fallback floor → resolves implicitly as P1-1 + P1-2 land
- `fb_1778927777719509000` (new_graph) — DC215 question, same root cause → P1-1
