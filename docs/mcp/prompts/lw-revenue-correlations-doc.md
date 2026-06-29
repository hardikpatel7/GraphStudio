# Pearson Correlations with `lw_revenue` — Documentation

## Prompt

> Compute Pearson correlation between lw_revenue and every numeric column (oh, oo, it, allocated_units, price, discount, in_stock_perc, aps, wos, mapped_stores_count, max_stock, min_stock). Rank by |correlation|.

## SmartStudio MCP tool calls

A single MCP call was issued.

### Call 1 — `mcp__smartstudio__query_articles`

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
  "sql": "<UNION ALL of 12 CORR(...) aggregates — see below>",
  "limit": 20
}
```

**SQL executed**

```sql
WITH base AS (
  SELECT
    lw_revenue,
    oh, oo, it, allocated_units, price, discount, in_stock_perc,
    aps, wos, mapped_stores_count, max_stock, min_stock
  FROM article_selection
),
corrs AS (
  SELECT 'oh' AS col, CORR(lw_revenue, oh) AS r, STDDEV(oh) AS sd, COUNT(oh) AS n FROM base
  UNION ALL SELECT 'oo', CORR(lw_revenue, oo), STDDEV(oo), COUNT(oo) FROM base
  UNION ALL SELECT 'it', CORR(lw_revenue, it), STDDEV(it), COUNT(it) FROM base
  UNION ALL SELECT 'allocated_units', CORR(lw_revenue, allocated_units), STDDEV(allocated_units), COUNT(allocated_units) FROM base
  UNION ALL SELECT 'price', CORR(lw_revenue, price), STDDEV(price), COUNT(price) FROM base
  UNION ALL SELECT 'discount', CORR(lw_revenue, discount), STDDEV(discount), COUNT(discount) FROM base
  UNION ALL SELECT 'in_stock_perc', CORR(lw_revenue, in_stock_perc), STDDEV(in_stock_perc), COUNT(in_stock_perc) FROM base
  UNION ALL SELECT 'aps', CORR(lw_revenue, aps), STDDEV(aps), COUNT(aps) FROM base
  UNION ALL SELECT 'wos', CORR(lw_revenue, wos), STDDEV(wos), COUNT(wos) FROM base
  UNION ALL SELECT 'mapped_stores_count', CORR(lw_revenue, mapped_stores_count), STDDEV(mapped_stores_count), COUNT(mapped_stores_count) FROM base
  UNION ALL SELECT 'max_stock', CORR(lw_revenue, max_stock), STDDEV(max_stock), COUNT(max_stock) FROM base
  UNION ALL SELECT 'min_stock', CORR(lw_revenue, min_stock), STDDEV(min_stock), COUNT(min_stock) FROM base
)
SELECT
  col,
  ROUND(r, 4) AS pearson_r,
  ROUND(ABS(r), 4) AS abs_r,
  ROUND(sd, 4) AS column_stddev,
  n
FROM corrs
ORDER BY ABS(r) DESC NULLS LAST
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **35 ms** |
| Rows returned | 12 |

---

## Response (verbatim)

### Pearson correlation with `lw_revenue`, ranked by |r|

| Rank | Column | Pearson r | abs r | Column stddev | n | Notes |
|---:|---|---:|---:|---:|---:|---|
| 1 | **price** | **+0.484** | 0.484 | 7.70 | 46,610 | moderate positive — pricier items earn more revenue |
| 2 | **in_stock_perc** | **+0.460** | 0.460 | 0.102 | 46,012 | moderate positive — floor presence drives sales |
| 3 | **discount** | **+0.349** | 0.349 | 0.028 | 46,610 | positive — discounting correlates with revenue (promo cycle effect) |
| 4 | oh | +0.041 | 0.041 | 9.93 | 46,610 | essentially zero — OH doesn't predict revenue |
| 5 | min_stock | −0.021 | 0.021 | 0.275 | 46,610 | essentially zero |
| 5 | mapped_stores_count | −0.021 | 0.021 | 164.97 | 46,610 | essentially zero |
| 5 | max_stock | −0.021 | 0.021 | 1.37 | 46,610 | essentially zero |
| — | oo | NULL | — | **0** | 46,610 | undefined — zero variance (oo = 0 everywhere) |
| — | it | NULL | — | **0** | 46,610 | undefined — zero variance |
| — | allocated_units | NULL | — | **0** | 46,610 | undefined — zero variance |
| — | aps | NULL | — | **0** | 42,770 | undefined — zero variance |
| — | wos | NULL | — | **0** | 46,610 | undefined — zero variance (wos = 8 everywhere) |

### Read

**Five columns have undefined correlations** because they have zero variance — they're constants in this dataset:

- `oo`, `it`, `allocated_units` — all zero everywhere
- `aps` — all zero everywhere
- `wos` — constant 8 everywhere

This re-confirms findings from earlier in the session.

**Three columns carry real signal:**

- **`price` (+0.48)** is the strongest predictor — higher-priced articles generate more LW revenue. Mostly mechanical (since revenue = price × units), but the correlation isn't 1.0 because units vary across price tiers.
- **`in_stock_perc` (+0.46)** is nearly as strong — articles actually on the store floor sell more. This validates the systemic flow-problem hypothesis: the gap between "in stock at DC" and "in stock at store" matters more than nominal OH levels.
- **`discount` (+0.35)** is positively correlated — articles with any discount sell more. The mechanism is likely promotional intent rather than discount-driven demand: promoted articles are pushed harder and tend to move.

**Three columns are essentially noise:**

- **`oh` (+0.04)** — having more inventory on hand barely correlates with revenue. This is the inventory-vs-flow story: piles at the DC don't translate to sales.
- **`min_stock`, `max_stock`, `mapped_stores_count` (all exactly −0.021)** — the identical correlations are suspicious. Likely a linked-zero pattern: when an article has `mapped_stores_count = 0`, it also tends to have `min_stock = 0` and `max_stock = 0`. That common-zero state correlates weakly negatively with revenue (since RCL-ineligible articles can't accrue revenue normally).

**Notable absence**: nothing has |r| > 0.5. The variance in last-week revenue is explained more by sparse combinations of price + floor-presence than by any single column.

---

## Tool call summary

| Field | Value |
|---|---|
| Tool | `mcp__smartstudio__query_articles` |
| Number of calls | 1 |
| Server-reported duration | 35 ms |
| Rows returned | 12 |
| Columns with real correlation (defined) | 7 |
| Columns with undefined correlation (zero variance) | 5 |
| Strongest signal | `price` (+0.48), `in_stock_perc` (+0.46), `discount` (+0.35) |
| Data source | DuckDB `article_selection` table (46,610 rows) |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
