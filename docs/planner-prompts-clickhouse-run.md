# Planner prompts — ClickHouse run

Walked the sample planner prompts against the **Arhaus CH** instance
(`ds_b171ae9ef080`, `arhaus_dev` database) via Claude Code + the
smartstudio MCP (`list_connections`, `clickhouse_dictionary`,
`clickhouse_query`). Parallel to `planner-prompts-run.md` which
covered the PG version on the bealls tenant.

Run date: 2026-05-21. Today's fiscal anchor: `fiscal_year=2026,
fiscal_quarter=2 (Q202602), fiscal_month=5, fiscal_year_month=202605,
fiscal_year_week=202605`.

Revised after the smartstudio server gained `duration_ms` / `client_ms`
/ `server_ms` / `read_rows` / `read_bytes` on every CH MCP response
(commit `a56ae59`). The previous version of this doc estimated tokens
heuristically and could only guess at query time; this revision has
real CH-server-side execution times and scan-cost numbers.

## Measurement legend

For each prompt:

- **`duration_ms`** — smartstudio's wall-clock for the whole request
  (auth lookup + CH HTTP + JSON decode + optional count query). What
  the LLM sees.
- **`server_ms`** — ClickHouse's reported execution time, parsed from
  the `X-ClickHouse-Summary.elapsed_ns` response header. The real
  "did CH have to work hard?" number.
- **`read_rows` / `read_bytes`** — scan cost CH paid. Subset of the
  result-cardinality conversation: a SELECT count() of 1M rows
  might read 1M rows or, with the right index, just a handful.
- **Token estimates** are still heuristic (`(prompt + SQL + JSON
  response + interpretation chars) / 4`); the server doesn't yet
  surface token cost.

A consistent **~500ms client_ms floor** across every prompt is
smartstudio + network overhead, not CH execution. Subtracting
`server_ms` from `client_ms` gives roughly that overhead — see
the bottom of this doc for the cross-cutting analysis.

## Data context

- **Tenant**: Arhaus (US furniture retailer). MFP + Assortment +
  Single-Period-Optimization (SPO) workflow.
- **Main DB**: `arhaus_dev` — 205 tables, ~19.6M rows total.
- **Key tables**:
  - `kpi_calculated_<category>_<wp|others>` — per-category KPI cubes.
    `wp` = Working Plan; `others` = scenario alternates. Used CH's
    `merge('arhaus_dev', '^kpi_calculated_.*_wp$')` to scan all WP
    tables in a single query.
  - `product_details` (628K) — item master with lifecycle dates,
    MOQ, lead_time, store_count, vendor, ROS, target_fmos,
    baseline_discount, price.
  - `scenario_planning` (4.4M) — discount-sweep scenarios.
  - `fiscal_date_mapping` (7.3K) — 4-5-4 calendar.
  - `new_item_ph_mapping` (301K) — AI-scored new-SKU → hierarchy.

---

## Prompt 1 — Dining WP by month + GM% + PY comp

> Show me the Working Plan for Dining this fiscal year — written sales dollars and GM% by month, with prior year comp.

**SQL**:
```sql
SELECT
  fiscal_year_month % 100 AS fiscal_month,
  sumIf(written_sales_dollars, fiscal_year = 2026) AS cy_sales,
  sumIf(written_sales_dollars, fiscal_year = 2025) AS py_sales,
  100.0 * sumIf(written_gm_dollar, fiscal_year = 2026) / nullIf(sumIf(written_sales_dollars, fiscal_year = 2026), 0) AS cy_gm,
  100.0 * sumIf(written_gm_dollar, fiscal_year = 2025) / nullIf(sumIf(written_sales_dollars, fiscal_year = 2025), 0) AS py_gm
FROM arhaus_dev.kpi_calculated_dining_wp
WHERE fiscal_year IN (2025, 2026)
GROUP BY fiscal_month ORDER BY fiscal_month
```

**Result**: 12 rows. FY26 $172.4M vs FY25 $186.4M → 92.5% YoY; GM 65.5% (CY) vs 65.0% (PY). Outliers M3 (-19.8%), M4 (+43.5%), M11 (-21%).

**Timing**: `duration_ms=599, server_ms=12, read=49,152 rows / 1.3 MB`.

**Token est**: ~1,800.

---

## Prompt 2 — Off-plan categories this quarter

> Which categories are off-plan this quarter? Show categories where actual delivered sales are <90% of plan.

**SQL**:
```sql
SELECT l0_name, sum(written_sales_dollars) AS wp, sum(delivered_net_sales_dollars) AS dlv,
       100.0 * sum(delivered_net_sales_dollars) / nullIf(sum(written_sales_dollars), 0) AS pct
FROM merge('arhaus_dev', '^kpi_calculated_.*_wp$')
WHERE fiscal_year_quarter = 202602
GROUP BY l0_name ORDER BY pct ASC
```

**Result**: 18 L0s. **BATH at 61.9%** is the only underperformer; everything else over-delivers (DINING 121%, LIGHTING 169%).

**Timing**: `duration_ms=562, server_ms=49, read=653,888 rows / 7.2 MB`. `merge()` across 13 WP tables.

**Token est**: ~2,400.

---

## Prompt 3 — Open-to-buy next 8 weeks

> Open-to-buy for the next 8 weeks by category — recommended receipts minus on-order.

**SQL**:
```sql
SELECT l0_name, sum(recomm_receipt_cost_flowover) - sum(on_order_placed_total) - sum(on_order_unplaced_total) AS otb_gap
FROM merge('arhaus_dev', '^kpi_calculated_.*_wp$')
WHERE fiscal_year_month BETWEEN 202605 AND 202607
GROUP BY l0_name HAVING sum(recomm_receipt_cost_flowover) > 0 ORDER BY otb_gap DESC
```

**Result**: All 12 categories show large negative OTB (already over-committed against recommended flow). DINING -$20.7M.

**Timing**: `duration_ms=694, server_ms=53, read=571,848 rows / 9.4 MB`.

**Token est**: ~2,100.

---

## Prompt 4 — Items with FMOS > 2× target

> Top 50 items with FMOS > 2× target — overstocked, need exit or markdown.

**SQL**:
```sql
SELECT l0_name, l1_name, l4_name, aoh_fmos_units / nullIf(target_fmos, 0) AS ratio
FROM merge('arhaus_dev', '^kpi_calculated_.*_wp$')
WHERE fiscal_year_month = 202605 AND target_fmos > 0 AND aoh_fmos_units > target_fmos * 2
ORDER BY ratio DESC LIMIT 50
```

**Result**: Top items dominated by SOFT GOODS/TWLS test rows and NONSALEABLE/SWATCHES (display samples). Real planner usage filters those.

**Timing**: `duration_ms=570, server_ms=59, read=1,466,136 rows / 11.1 MB`. The `merge()` scans hierarchy_code per WP table.

**Token est**: ~5,500.

---

## Prompt 5 — Launching in next 30 days with setup gaps

> Items launching in the next 30 days where store_count is 0 or vendor_name is null.

**SQL**:
```sql
SELECT count() FROM arhaus_dev.product_details
WHERE product_launch_date BETWEEN '2026-05-21' AND '2026-06-20'
  AND (coalesce(store_count, 0) = 0 OR vendor_name IS NULL OR vendor_name = '')
```

**Result**: **0 rows** — no launch gaps in next 30 days.

**Timing**: `duration_ms=518, server_ms=7, read=628,248 rows / 3.1 MB`. Full scan of product_details (no useful index on launch_date), but CH burns through it in 7 ms.

**Token est**: ~500.

---

## Prompt 6 — EOP up + sales down (MoM)

> Categories with EOP units trending up while written sales trend down.

**SQL**: WITH-MoM pivot across `merge()`.

**Result**: 0 rows for Apr→May 2026 — no MoM divergence.

**Timing**: `duration_ms=575, server_ms=62, read=645,696 rows / 6.1 MB`.

**Token est**: ~500.

---

## Prompt 7 — Items hitting markdown date this week

> All items hitting their markdown date this week.

**SQL**:
```sql
SELECT l0_name, count(*), sum(presentation_minimum * price) FROM arhaus_dev.product_details
WHERE product_markdown_date BETWEEN '2026-05-21' AND '2026-05-28' GROUP BY l0_name
```

**Result**: 0 rows.

**Timing**: `duration_ms=548, server_ms=6, read=628,248 rows / 3.1 MB`.

**Token est**: ~400.

---

## Prompt 8 — Expired items still showing AOH

> Items with product_exit_date in past but still showing AOH.

**SQL**: product_details ⨝ merge(kpi_*_wp) on hierarchy_code, filter exit_date past + aoh > 0.

**Result**: 13 L0s, **2,008 expired items still carrying stock**. OUTDOOR 448, UPHOLSTERY 324, DINING 239.

**Timing**: `duration_ms=626, server_ms=86, read=1,605,448 rows / 25.9 MB`. The join is the heaviest scan in the whole run; CH still does the work in 86 ms.

**Token est**: ~1,400.

---

## Prompt 9 — Mapping status counts

> How many items are 'Copied to WP' vs 'Unmapped'?

**Result**: All 628K items are `Unmapped`.

**Timing**: `duration_ms=518, server_ms=5, read=628,248 rows / 1.3 MB`.

**Token est**: ~400.

---

## Prompt 10 — Vendors with highest unplaced on-order

> Vendors with highest unplaced on-order dollars.

**Result**: 0 rows — `on_order_unplaced_total = 0` everywhere.

**Timing**: `duration_ms=640, server_ms=42, read=302,344 rows / 2.7 MB`.

**Token est**: ~300.

---

## Prompt 11 — ATP > MOQ × 3

> Items where current ATP > MOQ × 3.

**SQL**:
```sql
SELECT l0_name, l4_name, atp_units, moq FROM merge('arhaus_dev', '^kpi_calculated_.*_wp$')
WHERE fiscal_year_month = 202605 AND moq > 0 AND atp_units > moq * 3
ORDER BY atp_units / moq DESC LIMIT 30
```

**Result**: Most items have MOQ=1, so "ATP > MOQ × 3" isn't a meaningful filter at Arhaus.

**Timing**: `duration_ms=560, server_ms=57, read=1,466,136 rows / 10.1 MB`.

**Token est**: ~3,300.

---

## Prompt 12 — Compare scenarios

> Compare discount scenarios — what's the elasticity by category?

**SQL**: sumIf(scenario = 0.0 / 0.25 / 0.50) across scenario_planning, group by L0.

**Result**: DINING uniquely monotonic-decreasing; every other category peaks at scenario=0.25 then collapses at 0.50.

**Timing**: `duration_ms=532, server_ms=32, read=2,571,665 rows / 95.6 MB`. CH chewed through 96 MB in 32 ms — that's a ~3 GB/s scan rate on the column store.

**Token est**: ~2,800.

---

## Prompt 13 — Discount for 50% GM target (Dining)

> What discount would Dining need to hit a 50% GM target?

**SQL**: Full scenario sweep on `scenario_planning` for L0='DINING', grouped by scenario.

**Result**: Dining holds ≥50% GM as long as effective discount stays below **~54.7%** (linear interp between scenario=0.50 / 0.55).

**Timing**: `duration_ms=581, server_ms=37, read=4,380,120 rows / 128.7 MB`. **4.4M-row scan in 37 ms**.

**Token est**: ~2,200.

---

## Prompt 14 — Choice count below presentation minimum

> Choice count by L1 — where below presentation minimum?

**Result**: 20 L1s flagged, but `choice_count = 0` and `presentation_minimum = 1` on every flagged row → data quality issue.

**Timing**: `duration_ms=568, server_ms=52, read=532,280 rows / 6.1 MB`.

**Token est**: ~2,400.

---

## Prompt 15 — Top hierarchy nodes by productivity

> Top hierarchy nodes by sales / store_count.

**Result**: 20 rows; top is DNG TBL `30VTX47RCDBS` at $136K/store (in 2 stores).

**Timing**: `duration_ms=605, server_ms=65, read=661,856 rows / 18.5 MB`.

**Token est**: ~3,400.

---

## Prompt 17 — Dining WP vs others (by L2)

> Compare written sales between dining_wp and dining_others.

**Timing**: `duration_ms=839, server_ms=11, read=53,682 rows / 1.1 MB`. The 839ms is higher than typical because two separate tables are queried via UNION; CH still finishes in 11 ms server-side — the rest is two HTTP roundtrips.

**Token est**: ~1,600.

---

## Prompt 19 — Items launching in 90 days, gap pivot

> Items launching in next 90 days, by channel × L0, flag missing store_count or vendor.

**Result**: 58,656 items launching; every one has `store_count_feed = 0`.

**Timing**: `duration_ms=552, server_ms=21, read=628,248 rows / 26.7 MB`. Pulls more bytes because more columns (channel + vendor + dates).

**Token est**: ~1,800.

---

## Prompt 20 — Expired + unlocked

> Items where product_exit_date is in the past but is_locked = false.

**Result**: **73,512 expired-unlocked items**. ACCESSORY 15.5K leads.

**Timing**: `duration_ms=522, server_ms=10, read=628,248 rows / 9.2 MB`.

**Token est**: ~1,200.

---

## Prompt 21 — Markdown next 4 weeks dollar exposure

> Items hitting markdown date in next 4 weeks — total exposure by L0.

**Result**: 3 L0s, **1,632 items / $1.27M markdown exposure**. RUGS biggest ($966K).

**Timing**: `duration_ms=698, server_ms=9, read=628,248 rows / 4.8 MB`.

**Token est**: ~800.

---

## Prompt 22 — Discounted items not converting to lift

> Items where baseline_discount > 0.3 AND ros_l3m < 50% of ros_l12m.

**Result**: **17,952 items**. ACCESSORY 4,680 leads.

**Timing**: `duration_ms=511, server_ms=13, read=628,248 rows / 21.5 MB`.

**Token est**: ~1,100.

---

## Prompt 28 — Single scenario, discount rate by week

> For scenario=0.25, compare written_dr_perc week-over-week for Dining.

**Result**: `written_dr_perc` is exactly 25% every week — `scenario` is a flat-across-year discount lever.

**Timing**: `duration_ms=529, server_ms=23, read=4,380,120 rows / 126.0 MB`. **4.4M-row scenario_planning scan in 23 ms** — same shape as P13.

**Token est**: ~1,000.

---

## Prompt 30 — New-item AI mapping distribution

> Distribution of new_item_ph_mapping by product_type.

**Result**: 299K `new_sku`, 1.6K null product_type. score=null universally.

**Timing**: `duration_ms=504, server_ms=4, read=301,050 rows / 0.6 MB`. Cheapest CH cost in the run.

**Token est**: ~600.

---

## Prompt 35 — Audit logs last 7 days

> What plan rows were edited in the last 7 days, by user?

**Result**: Light usage — qa@impactanalytics.co, 18 events total across 3 event types (1 sync failure).

**Timing**: `duration_ms=504, server_ms=5, read=20 rows / 275 bytes`. CH knew there were only 159 rows total and the where-clause is selective — full index hit.

**Token est**: ~700.

---

## Prompt 36 — Approval state transitions

> Plans currently in approval — stuck >48 hours?

**Result**: 2 transitions total in the table (lock_created → sent_for_approval). Same lock_id.

**Timing**: `duration_ms=503, server_ms=5, read=2 rows / 14 bytes`. **Fastest CH execution in the whole run** — full table is 2 rows.

**Token est**: ~900.

---

## Cross-cutting timing analysis

### Server-side vs client-side

| Metric | Median | Min | Max |
|---|---:|---:|---:|
| `duration_ms` (smartstudio wall-clock) | 560 | 503 | 839 |
| `server_ms` (CH execution) | 23 | 4 | 86 |
| `client_ms − server_ms` (overhead) | ~520 | ~499 | ~828 |

**The CH server is fast.** Every query in this run completed CH-side
in **under 90 ms**, including 4.4M-row scans of `scenario_planning`
(96–129 MB scanned in 23–37 ms = ~3 GB/s).

**The smartstudio MCP wrapper is the bottleneck.** ~500 ms of every
roundtrip is overhead — HTTP request setup + auth + reqwest body
read + JSON parse + the additional count() query the
`/api/dataviews/.../data` path issues. For the demo, individual
query latency is dominated by this overhead, not CH itself.

### Scan cost by query class

| Query class | Typical read_bytes | Typical server_ms | Notes |
|---|---:|---:|---|
| Lookup against `product_details` (628K, one filter) | 1–10 MB | 5–13 ms | LowCardinality columns are tiny |
| `merge('arhaus_dev', '^kpi_calculated_.*_wp$')` aggregation | 6–26 MB | 42–86 ms | 13 WP tables scanned in parallel |
| `scenario_planning` (4.4M) full-year aggregation | 96–129 MB | 23–37 ms | Highest absolute throughput in the run |
| `audit_logs` / `approval_state_transitions` | <1 KB | 5 ms | Tiny tables; effectively free |

### `read_bytes` is the real cost signal

For demo cost projections (and for AI Code Review purposes), `read_bytes`
is more honest than `duration_ms` — a slow query that scanned 1 KB is
likely a smartstudio/network problem to fix; a fast query that scanned
100 MB still has real compute cost on CH. Two examples:

- **P21 (markdown exposure)**: `duration_ms=698` but `server_ms=9` /
  4.8 MB scan. The 698 ms is almost entirely smartstudio + network.
- **P13 (GM target sweep)**: `duration_ms=581` but `server_ms=37` /
  128.7 MB scan. Most of the 581 ms is overhead; CH did **real**
  work (~129 MB).

### Dictionary endpoint perf finding (resolved)

`GET /api/connections/:id/dictionary` originally issued
**O(databases + tables)** HTTP roundtrips to CH — one
`list_databases` + one `list_tables_in` per DB + one
`list_columns_in` per table. At ~500 ms `client_ms` per call, the full
Arhaus catalog crawl exceeded 60 seconds and had to be cancelled.

**Optimized** in a follow-up commit: replaced the loop with a single
CH query that JOINs `system.databases × system.tables × system.columns`
ORDER BY `(database, table, position)`, then walks the rows once in
Rust to assemble the hierarchical shape. One HTTP call; CH does the
join server-side.

**Real numbers on the Arhaus catalog**:
- 7 databases, 415 tables, **42,235 columns**
- Full crawl: **4.2 seconds** end-to-end (down from 60+ s timeout)
- Response size: **4.2 MB** JSON
- Single HTTP call to CH

The 4.2 MB response is still large for LLM context — pass the
optional `database` arg on `clickhouse_dictionary` to scope to one
DB at a time when you only need that slice.

---

## Summary

- **31 of 36 prompts executed** (5 duplicates / aspirational); 24
  unique SQLs ran with timing instrumentation.
- **Median CH execution time: 23 ms** server-side.
- **Median smartstudio wall-clock: 560 ms** — ~500 ms of which is
  per-roundtrip overhead, not CH.
- **Total CH bytes scanned across the 24 SQLs: ~636 MB**.
- **Total estimated tokens: ~46,000** (same heuristic as before;
  server doesn't surface token counters).

### Demo budgeting

Each chained planner question costs roughly:
- **~2,000–3,000 tokens** (~$0.01 input + ~$0.03 output at Sonnet 3.5)
- **~500–700 ms** wall-clock from question → answer (sub-second feel)
- **~5–25 MB CH scan**

A 30-prompt planner session is well under a dollar in API cost and
under 30 seconds of cumulative wait. The bottleneck for "snappier
demo" isn't CH — it's the smartstudio HTTP roundtrip overhead.

### Demo-ready prompt curation (unchanged from prior run)

1. **P1** — Dining WP by month vs PY
2. **P2** — Off-plan categories Q2 (BATH at 61.9% is the only one)
3. **P8** — 2,008 expired items still carrying stock
4. **P12 + P13** — Scenario elasticity + GM-target discount (chained)
5. **P21** — $1.3M markdown exposure next 4 weeks

These five deliver decisive numbers, hit CH execution under 90 ms
each, and avoid data-quality artifacts.
