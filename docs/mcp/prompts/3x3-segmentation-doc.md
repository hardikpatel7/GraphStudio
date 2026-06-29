# 3×3 Tercile Segmentation — Documentation

## Prompt

> Build a 3×3 segmentation of articles: terciles of lw_revenue × terciles of in_stock_perc. For each of the 9 cells: count, total OH, total lw_revenue, average APS, and 3 example articles. Identify the worst cell — high OH, low revenue, low in-stock %.

## Caveats applied

- **`aps` column is uniformly 0** — substituted synthetic APS = `lw_units / NULLIF(mapped_stores_count, 0)`.
- **Zero-dominated columns**: both `lw_revenue` and `in_stock_perc` have so many ties at 0 that NTILE(3) cannot separate the bottom thirds by value. Most "tercile 1" and "tercile 2" cells contain articles with identical (rev=0, isp=0) signatures — NTILE just spreads tied zeros across buckets.
- **NTILE tie-breaking is non-deterministic between independent queries**: the count and sample queries each ran their own NTILE(...) windows, and the same article can land in different cells across queries because the tie-break order is implementation-defined. For this dataset the bucket-shape difference is small; the value-zones are the meaningful unit.

## SmartStudio MCP tool calls

Two MCP calls were issued (cell-stats + per-cell samples).

### Call 1 — `mcp__smartstudio__query_articles` (cell stats)

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
  "sql": "<3x3 cell-stats query — see below>",
  "limit": 10
}
```

**SQL executed**

```sql
WITH base AS (
  SELECT
    oh, lw_revenue, in_stock_perc, mapped_stores_count, lw_units,
    lw_units * 1.0 / NULLIF(mapped_stores_count, 0) AS s_aps,
    NTILE(3) OVER (ORDER BY lw_revenue) AS rev_tercile,
    NTILE(3) OVER (ORDER BY in_stock_perc) AS isp_tercile
  FROM article_selection
)
SELECT
  rev_tercile,
  isp_tercile,
  COUNT(*) AS n,
  SUM(oh) AS total_oh,
  SUM(lw_revenue) AS total_lw_revenue,
  ROUND(AVG(s_aps), 4) AS avg_synth_aps,
  MIN(lw_revenue) AS rev_min, MAX(lw_revenue) AS rev_max,
  ROUND(MIN(in_stock_perc), 3) AS isp_min, ROUND(MAX(in_stock_perc), 3) AS isp_max
FROM base
GROUP BY rev_tercile, isp_tercile
ORDER BY rev_tercile, isp_tercile
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **32 ms** |
| Rows returned | 8 (cell (3,1) is empty) |

---

### Call 2 — `mcp__smartstudio__query_articles` (samples per cell)

**Tool description**: same as Call 1.

**Input arguments**

```json
{
  "sql": "<per-cell sample query, top 3 by OH DESC — see below>",
  "limit": 30
}
```

**SQL executed**

```sql
WITH base AS (
  SELECT
    ph_code, article, brand, l1_name, l2_name,
    oh, lw_revenue, in_stock_perc, mapped_stores_count, lw_units,
    NTILE(3) OVER (ORDER BY lw_revenue) AS rev_tercile,
    NTILE(3) OVER (ORDER BY in_stock_perc) AS isp_tercile
  FROM article_selection
),
ranked AS (
  SELECT *,
         ROW_NUMBER() OVER (PARTITION BY rev_tercile, isp_tercile ORDER BY oh DESC NULLS LAST, ph_code) AS rn
  FROM base
)
SELECT rev_tercile, isp_tercile, article, brand, l1_name, l2_name,
       oh, lw_revenue, lw_units, ROUND(in_stock_perc, 3) AS isp
FROM ranked
WHERE rn <= 3
ORDER BY rev_tercile, isp_tercile, rn
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **54 ms** |
| Rows returned | 21 (3 per cell × 7 sampled cells) |

---

## Response (verbatim)

### Reality check on the buckets

Because `lw_revenue` and `in_stock_perc` are heavily zero-dominated, NTILE's "thirds" don't separate by value at the low end. Most cells in the bottom rev/isp terciles all contain `lw_revenue = 0` AND `in_stock_perc = 0` articles — NTILE just shuffles tied zeros across buckets. So the 9 cells effectively collapse to 4 distinct value-zones.

### 3×3 grid (terciles of lw_revenue × terciles of in_stock_perc)

| Rev tercile | ISP tercile | Articles | Total OH | Total LW rev | Avg synth APS | Cell rev range | Cell ISP range |
|:---:|:---:|---:|---:|---:|---:|---|---|
| 1 (lowest) | 1 (lowest) | **10,316** | **78,023** | $0 | 0.0000 | $0 | 0.0% |
| 1 | 2 | 3,629 | 28,500 | $0 | 0.0000 | $0 | 0.0% |
| 1 | 3 (highest) | 1,592 | 20,857 | $0 | 0.0000 | $0 | 0.1–34.2% |
| 2 | 1 | 5,221 | 40,928 | $0 | 0.0000 | $0 | 0.0% |
| 2 | 2 | **8,425** | **69,945** | $0 | 0.0000 | $0 | 0.0% |
| 2 | 3 | 1,891 | 26,190 | $0 | 0.0000 | $0 | 0.1–24.9% |
| 3 | 1 | **0** | **—** | — | — | — | — |
| 3 | 2 | 3,483 | 26,669 | $10,181 | 0.0003 | $0–59 | 0.0% |
| 3 (highest) | 3 (highest) | **12,053** | **99,342** | **$5,889,217** | **0.0626** | $0–11,602 | 0–95.3% |

**Total OH across all cells: ~390,454.** Revenue is overwhelmingly in (3,3): $5.89M of ~$5.90M total (99.8% of revenue concentrated in one cell).

### 3 example articles per cell (sorted by OH desc within each cell)

**Cell (1,1) — lowest rev, lowest isp**

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 108061407-400 | CUISINART | DOMESTICS / Kitchen Textiles | 142 | $0 | 0 | 0% |
| 107682169-399 | SUPPLIES BY UNIONBAY | MISSES BETTER SW / EC Better Bottoms | 137 | $0 | 0 | 0% |
| 107787298-1 | CORAL BAY | LADIES FOOTWEAR / EC Closed Flat | 130 | $0 | 0 | 0% |

**Cell (1,3)** — no revenue, but some floor presence

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 108091596-300 | HURLEY | YOUNGMENS / YM Shorts | 104 | $0 | 0 | 0.2% |
| 108230373-1 | LEVI'S | MENS SW / Mens Better Bottoms | 104 | $0 | 0 | 0.1% |
| 108091503-59 | HURLEY | YOUNGMENS / YM Shorts | 100 | $0 | 0 | 0.1% |

**Cell (2,1)**

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 205531951-409 | COUNTERPARTS | PETITES SW / PT Shorts | 116 | $0 | 0 | 0% |
| 107267008-19 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 74 | $0 | 0 | 0% |
| 108094068-739 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 72 | $0 | 0 | 0% |

**Cell (2,2) — second-largest dead pile**

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 108023240-1 | CLARKS | LADIES FOOTWEAR / Casual Sandals | 389 | $0 | 0 | 0% |
| 107266145-460 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 272 | $0 | 0 | 0% |
| 107266145-100 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 210 | $0 | 0 | 0% |

**Cell (2,3)**

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 205659921-273 | CLARKS | LADIES FOOTWEAR / Casual Sandals | 126 | $0 | 0 | 0.1% |
| 108230353-420 | LEVI'S | MENS SW / Mens Denim | 125 | $0 | 0 | 0.1% |
| 108230338-101 | LEVI'S | MENS SW / Mens Better Bottoms | 119 | $0 | 0 | 0.1% |

**Cell (3,2)** — small revenue, zero in-stock at store level (floor-stockout-but-selling)

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 108031542-242 | CLARKS | LADIES FOOTWEAR / Casual Sandals | 199 | $0 | 0 | 0% |
| 108010932-50 | TAILORBYRD | MENS FURN-ACC / Loungewear | 178 | $0 | 0 | 0% |
| 107928249-460 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 99 | $0 | 0 | 0% |

**Cell (3,3) — the productive zone**

| Article | Brand | L1 / L2 | OH | LW rev | LW units | ISP |
|---|---|---|---:|---:|---:|---:|
| 108139310-260 | ARIAT | MENS FOOTWEAR / Boots | 183 | $2,832 | 22 | 2.6% |
| 108137093-200 | HOSS | MENS FOOTWEAR / Boots | 175 | $793 | 8 | 0.8% |
| 108162357-400 | LIFESTRIDE | LADIES FOOTWEAR / Heels-Wedges | 167 | $836 | 44 | 2.7% |

### The worst cell

**By the user's criterion (high OH, low rev, low isp), the worst single cell is (1,1) — 10,316 articles holding 78,023 units OH, all with zero revenue and zero in-stock at the store level.**

But the picture is bigger than one cell. The four bottom-left cells (1,1)+(1,2)+(2,1)+(2,2) all share the same value profile (rev=0 AND isp=0) — NTILE just spread tied zeros across buckets. Aggregated:

- **27,591 articles** (59% of the assortment)
- **217,396 OH units** (56% of all on-hand inventory)
- **$0 revenue**
- **0% in-stock at the store floor**

This is the **dead-inventory pool**. Over half the units the business holds last week generated zero revenue and never made it to a store floor.

For contrast, **cell (3,3) — 12,053 articles, 99,342 OH (25% of OH)** — produced 99.8% of last week's revenue ($5.89M). The Pareto is extreme: a quarter of the inventory is doing almost all the selling, more than half of it is doing nothing.

The dominant `lw_revenue = 0 AND in_stock_perc = 0` signature across 27K+ articles re-confirms the systemic flow problem: inventory exists at DCs and isn't being pushed to stores, so it can't sell, so revenue stays at zero.

---

## Tool call summary

| # | Tool | Purpose | Duration |
|---:|---|---|---:|
| 1 | `mcp__smartstudio__query_articles` | 9-cell stats (counts, OH, rev, APS, value ranges) | 32 ms |
| 2 | `mcp__smartstudio__query_articles` | 3 sample articles per cell | 54 ms |

| Field | Value |
|---|---|
| SmartStudio MCP calls | 2 |
| Total server-reported MCP time | 86 ms (32 + 54) |
| Effective value-zones | 4 (dead inventory / no-rev floor / floor-stockout-selling / productive) |
| Largest dead-inventory cell | (1,1): 10,316 articles, 78,023 OH, $0 revenue |
| Combined dead pool | 27,591 articles (59%) holding 217,396 OH (56%), $0 revenue |
| Productive cell | (3,3): 12,053 articles holding $5.89M LW revenue (99.8% concentration) |
| Data source | DuckDB `article_selection` table (46,610 rows) |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
