# Per-DC OH Parse from `oh_map` — Documentation

## Prompt

> Parse oh_map across the entire catalog. For each DC: total OH it holds, distinct article count, average per-article OH at that DC. Top 20 DCs by total OH.

## SmartStudio MCP tool calls

A single MCP call was issued. The query expands `oh_map` JSON across 46K articles via two `json_each` lateral joins, which makes it the slowest query in this session by far (~10 s vs. typical 20–60 ms).

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
  "sql": "<two-level json_each expansion + DC aggregation — see below>",
  "limit": 20
}
```

**SQL executed**

```sql
WITH size_level AS (
  SELECT
    a.ph_code,
    a.article,
    je.key AS size_code,
    je.value AS dc_obj
  FROM article_selection a, json_each(a.oh_map) je
  WHERE a.oh_map IS NOT NULL AND a.oh_map <> '' AND a.oh_map <> '{}'
),
dc_level AS (
  SELECT
    s.article,
    je2.key AS dc_code,
    CAST(je2.value AS BIGINT) AS units
  FROM size_level s, json_each(s.dc_obj) je2
)
SELECT
  dc_code,
  SUM(units) AS total_oh,
  COUNT(DISTINCT article) AS distinct_articles,
  ROUND(SUM(units) * 1.0 / NULLIF(COUNT(DISTINCT article), 0), 2) AS avg_per_article
FROM dc_level
WHERE units > 0
GROUP BY dc_code
ORDER BY total_oh DESC
LIMIT 20
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **10,415 ms** (≈10.4 s) |
| Rows returned | 2 |

The latency is explained by:
- `oh_map` is a 2-level nested JSON object stored as VARCHAR
- Two `json_each` lateral joins explode it (size × DC per article)
- 46,610 articles × ~5 sizes × 1–2 DCs ≈ ~500K intermediate tuples
- Final `GROUP BY` + `COUNT(DISTINCT)` over those tuples

For comparison, all prior queries in this session ran in 20–86 ms because they aggregated directly over scalar columns.

---

## Response (verbatim)

Only **2 DCs** in the entire catalog (the V4 single-tenant Bealls instance is configured with two warehouses). The query is slow (10.4 s) because it expands `oh_map` JSON across 46K articles into roughly half a million size-DC tuples before aggregating.

### Per-DC inventory parsed from oh_map

| Rank | DC code | DC name | Total OH | Distinct articles | Avg OH / article |
|---:|---:|---|---:|---:|---:|
| 1 | **215** | CFC IA Physical WH | **280,981** | 35,109 | 8.00 |
| 2 | **214** | CFC IA Physical WH B | **109,473** | 14,095 | 7.77 |

**Total inventory across both DCs**: 390,454 units (matches the 390,454 total OH from the 3×3 segmentation cell-sum — sanity-check passes).

### Read

- **DC 215 is the primary warehouse**: holds ~72% of all units (280,981 / 390,454) and carries 75% of the unique-article footprint (35,109 / 46,610 articles).
- **DC 214 is the secondary**: 28% of units, 30% of articles.
- **Single-homing is the norm**: 35,109 + 14,095 = 49,204 article-DC pairings vs. 46,610 articles total. Overlap is at most ~2,594 — meaning **fewer than 5.6% of articles are stocked at both DCs**. Each article generally lives at one DC.
- **Similar average pile size**: 8.00 vs. 7.77 units per article — the DCs are similarly dense per-article; the size difference is in the breadth of assortment they cover, not in how much they hold per item.

This re-confirms the cross-DC imbalance pattern we saw on individual articles (ARLA THONG entirely at 214, HEY DUDE entirely at 215, Breeze Piper entirely at 215). It's not random — articles are deliberately single-homed, which means each DC's serviceable stores can only fulfill from the articles that DC carries. That's the structural reason behind the floor-stockout pattern across the assortment.

---

## Tool call summary

| Field | Value |
|---|---|
| Tool | `mcp__smartstudio__query_articles` |
| Number of calls | 1 |
| Server-reported duration | **10,415 ms** (slowest single query of the session) |
| Rows returned | 2 (only 2 DCs exist) |
| DC 215 | 280,981 units, 35,109 articles, ~72% of OH |
| DC 214 | 109,473 units, 14,095 articles, ~28% of OH |
| Cross-DC overlap | ≤ 2,594 articles (~5.6%) |
| Sanity check | 280,981 + 109,473 = 390,454 = total OH from the 3×3 segmentation ✓ |
| Data source | DuckDB `article_selection` table (46,610 rows), `oh_map` JSON column |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
