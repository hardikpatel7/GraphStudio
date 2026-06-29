# Allocation Funnel — Documentation

## Prompt

> Discover the allocation funnel: step 1 — articles with OH>0; step 2 — of those, mapped_stores_count>0; step 3 — allocated_units>0; step 4 — lw_units>0; step 5 — in_stock_perc>50. For each step, give count, drop-rate %, and 3 sample articles that dropped there.

## SmartStudio MCP tool calls

Two MCP calls were issued (counts + sample drops).

### Call 1 — `mcp__smartstudio__query_articles` (funnel counts)

**Tool description** (from the tool schema):

> Run a single SELECT (or WITH/FROM-first) DuckDB statement against the
> `article_selection` table. This is the primary read tool — use it for any filtered,
> aggregated, or grouped query.
>
> **Guardrails**
> - SELECT/WITH/FROM only; no DDL/DML
> - One statement (no `;`)
> - Must reference the `article_selection` table
> - `limit` / `offset` are applied automatically by the server on top of any LIMIT inside the SQL
>
> **Input**: `{ sql, limit?, offset? }`
> **Returns**: `{ rows, columns, total, duration_ms, executed_sql }`

**Input arguments**

```json
{
  "sql": "<funnel-classify query — see below>",
  "limit": 10
}
```

**SQL executed**

```sql
WITH classified AS (
  SELECT
    CASE
      WHEN oh = 0 THEN 1
      WHEN mapped_stores_count = 0 THEN 2
      WHEN allocated_units = 0 THEN 3
      WHEN lw_units = 0 THEN 4
      WHEN in_stock_perc <= 0.5 THEN 5
      ELSE 0
    END AS dropped_at_step
  FROM article_selection
)
SELECT dropped_at_step, COUNT(*) AS n
FROM classified
GROUP BY dropped_at_step
ORDER BY dropped_at_step
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **18 ms** |
| Rows returned | 2 |

**Raw result**

| dropped_at_step | n |
|---:|---:|
| 2 | 3,840 |
| 3 | 42,770 |

(Steps 1, 4, 5 had zero drops — they're absent from the output.)

---

### Call 2 — `mcp__smartstudio__query_articles` (sample drops per step)

**Tool description**: same as Call 1.

**Input arguments**

```json
{
  "sql": "<ranked sample-drop query — see below>",
  "limit": 20
}
```

**SQL executed**

```sql
WITH classified AS (
  SELECT article, brand, l1_name, l2_name,
         oh, mapped_stores_count, allocated_units, lw_units, lw_revenue, in_stock_perc,
         CASE
           WHEN oh = 0 THEN 1
           WHEN mapped_stores_count = 0 THEN 2
           WHEN allocated_units = 0 THEN 3
           WHEN lw_units = 0 THEN 4
           WHEN in_stock_perc <= 0.5 THEN 5
           ELSE 0
         END AS dropped_at_step
  FROM article_selection
),
ranked AS (
  SELECT *, ROW_NUMBER() OVER (
    PARTITION BY dropped_at_step
    ORDER BY lw_revenue DESC NULLS LAST, oh DESC NULLS LAST, article
  ) AS rn
  FROM classified
  WHERE dropped_at_step > 0
)
SELECT dropped_at_step, article, brand, l1_name, l2_name,
       oh, mapped_stores_count, allocated_units, lw_units, lw_revenue,
       ROUND(in_stock_perc, 3) AS in_stock_perc
FROM ranked
WHERE rn <= 3
ORDER BY dropped_at_step, rn
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **28 ms** |
| Rows returned | 6 (3 per drop step × 2 active drop steps) |

---

## Response (verbatim)

### Allocation funnel — SmartStudio assortment

| Step | Filter | Articles passing | Articles dropped here | Drop rate (of step input) |
|---|---|---:|---:|---:|
| Start | (all articles) | 46,610 | — | — |
| **1** | `oh > 0` | **46,610** | 0 | **0.0%** |
| **2** | `mapped_stores_count > 0` | **42,770** | 3,840 | **8.2%** |
| **3** | `allocated_units > 0` | **0** | 42,770 | **100.0%** ❗ |
| **4** | `lw_units > 0` | — | — | — (no input) |
| **5** | `in_stock_perc > 0.5` | — | — | — (no input) |

**The funnel collapses entirely at step 3.** No article in the entire 46,610-row dataset has `allocated_units > 0`. Steps 4 and 5 cannot fire because nothing reaches them.

### Sample drops by step

**Step 1 (`oh > 0`) — 0 drops**
Every article has at least 1 unit on hand somewhere. Consistent with our earlier finding: zero strict stockouts in the dataset.

**Step 2 (`mapped_stores_count > 0`) — 3,840 drops** (the RCL-eligibility gap)

| Article | Brand | L1 / L2 | OH | LW units | LW rev | In-stk % |
|---|---|---|---:|---:|---:|---:|
| 108021922-460 | VINCE CAMUTO | MISSES BETTER SW / EC Btr Woven | 12 | 369 | $8,895 | 42.1% |
| 108020968-690 | CE CE BY CYNTHIA STEFFE | MISSES BETTER SW / EC Btr Woven | 12 | 222 | $8,615 | 5.7% |
| 108220955-110 | MAX STUDIO | MISSES BETTER SW / WC Btr Woven | 8 | 372 | $7,128 | 8.0% |

All three are top-revenue articles (#5, #6, #10 in last-week revenue overall) that drop here because they have zero mapped stores — they're selling somehow despite the RCL eligibility gap. Likely a master-data bug, not real ineligibility.

**Step 3 (`allocated_units > 0`) — 42,770 drops** (the universal allocator-not-running issue)

| Article | Brand | L1 / L2 | OH | LW units | LW rev | In-stk % |
|---|---|---|---:|---:|---:|---:|
| 108186673-110 | REEBOK | ATHLETIC FW / Ladies Mod Ath | 12 | 315 | $11,602 | 60.9% |
| 107997589-100 | DASH | MISSES SW / MS Jackets | 12 | 523 | $10,012 | 12.2% |
| 108185756-660 | CHENAULT | MISSES SW / WC Cas Knits | 6 | 390 | $9,264 | 7.4% |

These are the #1, #2, #3 revenue articles in the whole assortment. All three sold hundreds of units last week, have OH ≥ 6, are mapped to 600 stores — but show `allocated_units = 0`. They sold without being allocated through the system at all.

**Steps 4 & 5 — N/A** (no input population)

### Read

This funnel is a clean diagnostic for the systemic issue we surfaced earlier:

1. **Inventory exists** — every article has stock (step 1 passes 100%)
2. **Most have store eligibility** — 91.8% pass step 2; the 8.2% drop is the documented RCL eligibility gap (mostly in MISSES BETTER SW)
3. **The allocator is the choke point** — 100% of remaining articles drop at step 3 because `allocated_units = 0` is universal across the dataset

The pipeline from "we have stock at the DC" → "this article is allocated to stores" is **completely broken or completely empty**. Either the allocation engine hasn't run, the `allocated_units` feed isn't populated, or the column is being reset somewhere. Sales are happening (step 2's drops show $24K of LW revenue from articles that aren't even eligible), but they aren't being captured in `allocated_units`.

---

## Tool call summary

| # | Tool | Purpose | Duration |
|---:|---|---|---:|
| 1 | `mcp__smartstudio__query_articles` | Funnel counts (drops per step) | 18 ms |
| 2 | `mcp__smartstudio__query_articles` | 3 sample drops per active step | 28 ms |

| Field | Value |
|---|---|
| SmartStudio MCP calls | 2 |
| Total server-reported MCP time | 46 ms (18 + 28) |
| Active drop steps | 2 of 5 (step 2 and step 3) |
| Funnel terminates | Step 3 — `allocated_units > 0` |
| Data source | DuckDB `article_selection` table (46,610 rows) |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
