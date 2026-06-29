# Per-Cell MAD Anomaly Hunt — Documentation

## Prompt

> For each (brand, L1) cell with ≥ 20 articles, compute median APS, median in_stock_perc, median wos. Then find articles whose individual metric is > 3 MADs from the cell median on any of the three. List top 100 anomalies by combined deviation score.

## Caveats applied before running

- **`aps` column is uniformly 0** in this dataset → substituted synthetic APS = `lw_units / NULLIF(mapped_stores_count, 0)`. Same proxy used throughout this session.
- **`wos` is constant 8** across all 46,610 rows → MAD(wos) = 0 in every cell, so the wos channel cannot fire. Effective analysis is 2-dimensional (synthetic APS + in_stock_perc).
- MAD threshold uses the literal definition `|x - median| > 3 × MAD` (no 1.4826 normality scaling).

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
  "sql": "<MAD CTE pipeline — see below>",
  "limit": 100
}
```

**SQL executed**

```sql
WITH base AS (
  SELECT
    ph_code, article, brand, l1_name, l2_name,
    UPPER(brand) AS brand_uc,
    lw_units, mapped_stores_count,
    lw_units * 1.0 / NULLIF(mapped_stores_count, 0) AS s_aps,
    in_stock_perc AS isp,
    wos, oh
  FROM article_selection
  WHERE brand IS NOT NULL AND brand <> ''
),
cell_size AS (
  SELECT brand_uc, l1_name, COUNT(*) AS n
  FROM base
  GROUP BY brand_uc, l1_name
  HAVING COUNT(*) >= 20
),
filtered AS (
  SELECT b.* FROM base b JOIN cell_size cs USING (brand_uc, l1_name)
),
medians AS (
  SELECT brand_uc, l1_name,
         MEDIAN(s_aps) AS med_aps,
         MEDIAN(isp)   AS med_isp,
         MEDIAN(wos)   AS med_wos
  FROM filtered GROUP BY brand_uc, l1_name
),
deviations AS (
  SELECT f.*, m.med_aps, m.med_isp, m.med_wos,
         ABS(f.s_aps - m.med_aps) AS dev_aps,
         ABS(f.isp   - m.med_isp) AS dev_isp,
         ABS(f.wos   - m.med_wos) AS dev_wos
  FROM filtered f JOIN medians m USING (brand_uc, l1_name)
),
mads AS (
  SELECT brand_uc, l1_name,
         MEDIAN(dev_aps) AS mad_aps,
         MEDIAN(dev_isp) AS mad_isp,
         MEDIAN(dev_wos) AS mad_wos
  FROM deviations GROUP BY brand_uc, l1_name
),
scored AS (
  SELECT d.ph_code, d.article, d.brand, d.l1_name, d.l2_name,
         d.lw_units, d.mapped_stores_count, d.oh,
         d.s_aps, d.isp, d.wos,
         d.med_aps, d.med_isp, d.med_wos,
         m.mad_aps, m.mad_isp, m.mad_wos,
         CASE WHEN m.mad_aps > 0 AND d.s_aps IS NOT NULL THEN d.dev_aps / m.mad_aps ELSE 0 END AS z_aps,
         CASE WHEN m.mad_isp > 0 THEN d.dev_isp / m.mad_isp ELSE 0 END AS z_isp,
         CASE WHEN m.mad_wos > 0 THEN d.dev_wos / m.mad_wos ELSE 0 END AS z_wos
  FROM deviations d JOIN mads m USING (brand_uc, l1_name)
)
SELECT
  article, brand, l1_name, l2_name,
  oh, lw_units, mapped_stores_count,
  ROUND(s_aps, 4) AS aps_synth,
  ROUND(isp, 3) AS isp,
  wos,
  ROUND(med_aps, 4) AS cell_med_aps,
  ROUND(med_isp, 3) AS cell_med_isp,
  ROUND(z_aps, 2) AS z_aps,
  ROUND(z_isp, 2) AS z_isp,
  ROUND(z_wos, 2) AS z_wos,
  ROUND(z_aps + z_isp + z_wos, 2) AS combined_score
FROM scored
WHERE z_aps > 3 OR z_isp > 3 OR z_wos > 3
ORDER BY combined_score DESC
LIMIT 100
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **64 ms** |
| Rows returned | 100 |
| Columns returned | 16 |

---

## Response (verbatim)

**Key framing**: combined scores reach 917 because in nearly every (brand × L1) cell, the median in_stock_perc is ~0.001 and the MAD is similarly tiny. Any article actually selling at >10% in-stock blows past 3 MADs by a huge margin. **The "anomalies" here are inverted: they're the rare articles actually performing in cells where the norm is dormancy.**

### Top 25 anomalies

| # | Article | Brand | L1 / L2 | OH | LW units | aps_synth | isp | cell med isp | z_isp | z_aps | combined |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108192036-960 | STITCH | CHILDRENS ACCESS / Beauty | 8 | 61 | 0.102 | 0.459 | 0.001 | 917 | 0 | 917 |
| 2 | 108193990-640 | SECONDARY | CHILDRENS ACCESS / Q-Line | 8 | 370 | 0.617 | 0.902 | 0.001 | 901 | 0 | 901 |
| 3 | 108192130-960 | STITCH | CHILDRENS ACCESS / Beauty | 8 | 55 | 0.092 | 0.432 | 0.001 | 863 | 0 | 863 |
| 4 | 108133307-960 | STITCH | CHILDRENS ACCESS / Girls Socks | 6 | 211 | 0.352 | 0.415 | 0.001 | 829 | 0 | 829 |
| 5 | 108133305-960 | STITCH | CHILDRENS ACCESS / Girls Socks | 12 | 118 | 0.197 | 0.414 | 0.001 | 827 | 0 | 827 |
| 6 | 108192129-960 | STITCH | CHILDRENS ACCESS / Beauty | 8 | 68 | 0.113 | 0.409 | 0.001 | 817 | 0 | 817 |
| 7 | 108112566-0 | NAUTICA | PET / Dog Beds | 10 | 85 | 0.142 | 0.796 | 0.001 | 795 | 0 | 795 |
| 8 | 108193980-640 | SECONDARY | CHILDRENS ACCESS / Q-Line | 8 | 58 | 0.097 | 0.793 | 0.001 | 792 | 0 | 792 |
| 9 | 108193981-460 | SECONDARY | CHILDRENS ACCESS / Q-Line | 8 | 78 | 0.130 | 0.793 | 0.001 | 792 | 0 | 792 |
| 10 | 108222022-650 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 8 | 123 | 0.205 | 0.790 | 0.001 | 789 | 0 | 789 |
| 11 | 108222021-650 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 8 | 249 | 0.415 | 0.785 | 0.001 | 784 | 0 | 784 |
| 12 | 108222138-650 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 194 | 0.323 | 0.783 | 0.001 | 782 | 0 | 782 |
| 13 | 108133302-960 | STITCH | CHILDRENS ACCESS / Girls Socks | 6 | 49 | 0.082 | 0.391 | 0.001 | 781 | 0 | 781 |
| 14 | 108193989-640 | SECONDARY | CHILDRENS ACCESS / Q-Line | 8 | 276 | 0.460 | 0.777 | 0.001 | 776 | 0 | 776 |
| 15 | 108112568-0 | NAUTICA | PET / Dog Beds | 27 | 95 | 0.158 | 0.775 | 0.001 | 774 | 0 | 774 |
| 16 | 108032663-960 | SECONDARY | CHILDRENS ACCESS / Boys Fash Acc | 6 | 54 | 0.090 | 0.745 | 0.001 | 744 | 0 | 744 |
| 17 | 108133301-960 | STITCH | CHILDRENS ACCESS / Girls Socks | 6 | 49 | 0.082 | 0.363 | 0.001 | 725 | 0 | 725 |
| 18 | 108170490-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 7 | 40 | 0.067 | 0.644 | 0.001 | 643 | 0 | 643 |
| 19 | 108204681-960 | STITCH | CHILDRENS ACCESS / Beauty | 12 | 86 | 0.143 | 0.316 | 0.001 | 631 | 0 | 631 |
| 20 | 108222137-500 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 131 | 0.218 | 0.623 | 0.001 | 622 | 0 | 622 |
| 21 | 108193693-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 12 | 122 | 0.203 | 0.615 | 0.001 | 614 | 0 | 614 |
| 22 | 108222018-400 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 8 | 81 | 0.135 | 0.613 | 0.001 | 612 | 0 | 612 |
| 23 | 108225093-110 | GURUNANDA | BEAUTY / Wellness | 10 | 116 | 0.193 | 0.892 | 0.002 | 593.67 | 0 | 593.67 |
| 24 | 108112532-0 | NAUTICA | PET / Dog Beds | 10 | 43 | 0.072 | 0.587 | 0.001 | 586 | 0 | 586 |
| 25 | 108107592-960 | STITCH | CHILDRENS ACCESS / Girls Socks | 6 | 86 | 0.143 | 0.290 | 0.001 | 579 | 0 | 579 |

### Anomalies 26–100

| # | Article | Brand | L1 / L2 | OH | LW units | aps_synth | isp | cell med isp | z_isp | z_aps | combined |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 26 | 108222136-271 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 130 | 0.217 | 0.578 | 0.001 | 577 | 0 | 577 |
| 27 | 108140702-960 | STITCH | CHILDRENS ACCESS / Kids Gifts-Tech | 7 | 36 | 0.060 | 0.287 | 0.001 | 573 | 0 | 573 |
| 28 | 108112565-0 | NAUTICA | PET / Dog Beds | 10 | 54 | 0.090 | 0.570 | 0.001 | 569 | 0 | 569 |
| 29 | 108225089-110 | GURUNANDA | BEAUTY / Wellness | 10 | 142 | 0.237 | 0.854 | 0.002 | 568.33 | 0 | 568.33 |
| 30 | 108193694-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 12 | 158 | 0.263 | 0.568 | 0.001 | 567 | 0 | 567 |
| 31 | 108152066-400 | STITCH | CHILDRENS ACCESS / Girls Fashion Acc | 5 | 17 | 0.028 | 0.283 | 0.001 | 565 | 0 | 565 |
| 32 | 108225092-110 | GURUNANDA | BEAUTY / Wellness | 10 | 134 | 0.223 | 0.849 | 0.002 | 565 | 0 | 565 |
| 33 | 108173326-1 | SECONDARY | MENS FURN-ACC / Mens Small Leather | 6 | 83 | 0.138 | 0.565 | 0.001 | 564 | 0 | 564 |
| 34 | 108139331-650 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 6 | 153 | 0.255 | 0.549 | 0.001 | 548 | 0 | 548 |
| 35 | 108225090-110 | GURUNANDA | BEAUTY / Wellness | 10 | 211 | 0.352 | 0.811 | 0.002 | 539.67 | 0 | 539.67 |
| 36 | 108112567-0 | NAUTICA | PET / Dog Beds | 10 | 46 | 0.077 | 0.538 | 0.001 | 537 | 0 | 537 |
| 37 | 108191873-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 103 | 0.172 | 0.536 | 0.001 | 535 | 0 | 535 |
| 38 | 108225094-110 | GURUNANDA | BEAUTY / Wellness | 10 | 214 | 0.357 | 0.778 | 0.002 | 517.67 | 0 | 517.67 |
| 39 | 108193691-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 12 | 141 | 0.235 | 0.506 | 0.001 | 505 | 0 | 505 |
| 40 | 108152068-460 | STITCH | CHILDRENS ACCESS / Girls Fashion Acc | 1 | 45 | 0.075 | 0.249 | 0.001 | 497 | 0 | 497 |
| 41 | 108222134-500 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 97 | 0.162 | 0.494 | 0.001 | 493 | 0 | 493 |
| 42 | 108146209-960 | SECONDARY | MENS FURN-ACC / Mens Headwear | 7 | 54 | 0.090 | 0.469 | 0.001 | 468 | 0 | 468 |
| 43 | 108116779-0 | GURUNANDA | BEAUTY / Wellness | 10 | 60 | 0.100 | 0.701 | 0.002 | 466.33 | 0 | 466.33 |
| 44 | 108139328-110 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 6 | 142 | 0.237 | 0.451 | 0.001 | 450 | 0 | 450 |
| 45 | 108171655-0 | GURUNANDA | BEAUTY / Wellness | 20 | 91 | 0.152 | 0.674 | 0.002 | 448.33 | 0 | 448.33 |
| 46 | 108158880-960 | SECONDARY | CHILDRENS ACCESS / Beauty | 6 | 25 | 0.042 | 0.446 | 0.001 | 445 | 0 | 445 |
| 47 | 108140595-400 | SPIDERMAN | CHILDRENS ACCESS / Boys Fash Acc | 4 | 51 | 0.085 | 0.430 | 0.001 | 429 | 0 | 429 |
| 48 | 108167769-650 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 4 | 32 | 0.053 | 0.406 | 0.001 | 405 | 0 | 405 |
| 49 | 108167765-200 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 5 | 36 | 0.060 | 0.393 | 0.001 | 392 | 0 | 392 |
| 50 | 108193711-960 | SECONDARY | CHILDRENS ACCESS / Beauty | 8 | 122 | 0.203 | 0.392 | 0.001 | 391 | 0 | 391 |
| 51 | 108146208-1 | SECONDARY | MENS FURN-ACC / Mens Headwear | 7 | 42 | 0.070 | 0.390 | 0.001 | 389 | 0 | 389 |
| 52 | 108146205-301 | SECONDARY | MENS FURN-ACC / Mens Headwear | 1 | 42 | 0.070 | 0.387 | 0.001 | 386 | 0 | 386 |
| 53 | 108116775-0 | GURUNANDA | BEAUTY / Wellness | 20 | 134 | 0.223 | 0.575 | 0.002 | 382.33 | 0 | 382.33 |
| 54 | 108167562-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 6 | 35 | 0.058 | 0.382 | 0.001 | 381 | 0 | 381 |
| 55 | 108197701-960 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 6 | 120 | 0.200 | 0.369 | 0.001 | 368 | 0 | 368 |
| 56 | 108193442-960 | SECONDARY | CHILDRENS ACCESS / Backpacks | 12 | 69 | 0.115 | 0.363 | 0.001 | 362 | 0 | 362 |
| 57 | 108158873-690 | SECONDARY | CHILDRENS ACCESS / Beauty | 6 | 28 | 0.047 | 0.360 | 0.001 | 359 | 0 | 359 |
| 58 | 108116774-0 | GURUNANDA | BEAUTY / Wellness | 26 | 121 | 0.202 | 0.538 | 0.002 | 357.67 | 0 | 357.67 |
| 59 | 108139330-400 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 6 | 49 | 0.082 | 0.358 | 0.001 | 357 | 0 | 357 |
| 60 | 108198295-0 | SECONDARY | TOYS / Boys Toys | 6 | 107 | 0.178 | 0.711 | 0.002 | 354.5 | 0 | 354.5 |
| 61 | 108171369-0 | WAHL | BEAUTY / Mens Electric | 9 | 132 | 0.220 | 0.767 | 0.009 | 84.22 | 263 | 347.22 |
| 62 | 108224657-400 | JACLYN | LINGERIE / MS Sleepwear | 12 | 112 | 0.187 | 0.347 | 0.001 | 346 | 0.71 | 346.71 |
| 63 | 108126362-690 | SECONDARY | CHILDRENS ACCESS / Girls Socks | 6 | 74 | 0.123 | 0.343 | 0.001 | 342 | 0 | 342 |
| 64 | 108193447-690 | SECONDARY | CHILDRENS ACCESS / Backpacks | 12 | 62 | 0.103 | 0.342 | 0.001 | 341 | 0 | 341 |
| 65 | 108116776-0 | GURUNANDA | BEAUTY / Wellness | 21 | 155 | 0.258 | 0.510 | 0.002 | 339 | 0 | 339 |
| 66 | 108198299-0 | SECONDARY | TOYS / Boys Toys | 6 | 75 | 0.125 | 0.679 | 0.002 | 338.5 | 0 | 338.5 |
| 67 | 108197695-960 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 6 | 95 | 0.158 | 0.339 | 0.001 | 338 | 0 | 338 |
| 68 | 108193437-960 | SECONDARY | CHILDRENS ACCESS / Backpacks | 8 | 65 | 0.108 | 0.337 | 0.001 | 336 | 0 | 336 |
| 69 | 108205517-1 | YOGALICIOUS | LADIES SPORTS / Plus | 2 | 166 | 0.277 | 0.167 | 0.001 | 333 | 0 | 333 |
| 70 | 108034201-0 | SECONDARY | TOYS / Summer Toys | 5 | 80 | 0.133 | 0.665 | 0.002 | 331.5 | 0 | 331.5 |
| 71 | 108033166-690 | SECONDARY | CHILDRENS ACCESS / Girls Fashion Acc | 2 | 22 | 0.037 | 0.326 | 0.001 | 325 | 0 | 325 |
| 72 | 108140609-1 | SPIDERMAN | CHILDRENS ACCESS / Boys Fash Acc | 6 | 40 | 0.067 | 0.326 | 0.001 | 325 | 0 | 325 |
| 73 | 108171653-0 | GURUNANDA | BEAUTY / Wellness | 14 | 15 | 0.025 | 0.488 | 0.002 | 324.33 | 0 | 324.33 |
| 74 | 108193433-110 | SECONDARY | CHILDRENS ACCESS / Backpacks | 6 | 36 | 0.060 | 0.324 | 0.001 | 323 | 0 | 323 |
| 75 | 108167768-400 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 2 | 15 | 0.025 | 0.323 | 0.001 | 322 | 0 | 322 |
| 76 | 108167600-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 10 | 44 | 0.073 | 0.319 | 0.001 | 318 | 0 | 318 |
| 77 | 108154375-200 | ADRIANNA PAPELL | MISSES BETTER SW / EC Better Bottoms | 12 | 92 | null | 0.319 | 0.001 | 318 | 0 | 318 |
| 78 | 108180783-49 | SECONDARY | MENS FURN-ACC / Mens Accessories | 8 | 106 | 0.177 | 0.315 | 0.001 | 314 | 0 | 314 |
| 79 | 108101975-1 | NUBY | BABY / Layette Hardlines | 1 | 74 | 0.123 | 0.620 | 0.002 | 309 | 0 | 309 |
| 80 | 108146205-200 | SECONDARY | MENS FURN-ACC / Mens Headwear | 1 | 30 | 0.050 | 0.307 | 0.001 | 306 | 0 | 306 |
| 81 | 108167599-650 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 8 | 40 | 0.067 | 0.302 | 0.001 | 301 | 0 | 301 |
| 82 | 108193436-110 | SECONDARY | CHILDRENS ACCESS / Backpacks | 8 | 47 | 0.078 | 0.291 | 0.001 | 290 | 0 | 290 |
| 83 | 108145176-270 | INSPIRED HEARTS | JUNIORS / WC JR Dresses | 6 | 57 | 0.095 | 0.291 | 0.001 | 290 | 0 | 290 |
| 84 | 108152067-460 | STITCH | CHILDRENS ACCESS / Girls Fashion Acc | 3 | 11 | 0.018 | 0.138 | 0.001 | 275 | 0 | 275 |
| 85 | 108167598-960 | SECONDARY | CHILDRENS ACCESS / Kids Gifts-Tech | 5 | 33 | 0.055 | 0.270 | 0.001 | 269 | 0 | 269 |
| 86 | 108034200-0 | SECONDARY | TOYS / Summer Toys | 6 | 48 | 0.080 | 0.528 | 0.002 | 263 | 0 | 263 |
| 87 | 108171370-0 | WAHL | BEAUTY / Mens Electric | 12 | 92 | 0.153 | 0.714 | 0.009 | 78.33 | 183 | 261.33 |
| 88 | 108171656-0 | GURUNANDA | BEAUTY / Wellness | 8 | 27 | 0.045 | 0.393 | 0.002 | 261 | 0 | 261 |
| 89 | 108075838-690 | SECONDARY | CHILDRENS ACCESS / Girls Socks | 1 | 19 | 0.032 | 0.242 | 0.001 | 241 | 0 | 241 |
| 90 | 108177372-0 | SECONDARY | TOYS / Summer Toys | 6 | 63 | 0.105 | 0.482 | 0.002 | 240 | 0 | 240 |
| 91 | 108116777-0 | GURUNANDA | BEAUTY / Wellness | 20 | 126 | 0.210 | 0.358 | 0.002 | 237.67 | 0 | 237.67 |
| 92 | 108204632-960 | SECONDARY | CHILDRENS ACCESS / Backpacks | 8 | 61 | 0.102 | 0.234 | 0.001 | 233 | 0 | 233 |
| 93 | 108198297-0 | SECONDARY | TOYS / Boys Toys | 6 | 124 | 0.207 | 0.467 | 0.002 | 232.5 | 0 | 232.5 |
| 94 | 108107335-960 | SECONDARY | CHILDRENS ACCESS / Boys Socks | 2 | 18 | 0.030 | 0.231 | 0.001 | 230 | 0 | 230 |
| 95 | 108034197-0 | SECONDARY | TOYS / Summer Toys | 3 | 54 | 0.090 | 0.451 | 0.002 | 224.5 | 0 | 224.5 |
| 96 | 108034198-0 | SECONDARY | TOYS / Summer Toys | 4 | 44 | 0.073 | 0.449 | 0.002 | 223.5 | 0 | 223.5 |
| 97 | 108115434-0 | KENRA | BEAUTY / Hair Care | 31 | 212 | 0.353 | 0.443 | 0.002 | 220.5 | 0 | 220.5 |
| 98 | 108204631-960 | SECONDARY | CHILDRENS ACCESS / Backpacks | 8 | 32 | 0.053 | 0.210 | 0.001 | 209 | 0 | 209 |
| 99 | 108057844-0 | SECONDARY | TOYS / Summer Toys | 5 | 25 | 0.042 | 0.408 | 0.002 | 203 | 0 | 203 |
| 100 | 108098272-0 | SECONDARY | TOYS / Children Books | 3 | 36 | 0.060 | 0.393 | 0.002 | 195.5 | 0 | 195.5 |

### Distribution of anomalies across L1s (top 100)

| L1 | # in top 100 |
|---|---:|
| 3430-CHILDRENS ACCESS | ~60 (dominant) |
| 3200-BEAUTY | ~10 |
| 3725-TOYS | ~7 |
| 3340-MENS FURN-ACC | ~5 |
| 3740-PET | 4 |
| Others (3115, 3130, 3140, 3150, 3420) | 1–2 each |

### What channels triggered

- **z_isp** triggered all 100 anomalies — the only metric with meaningful variance per cell.
- **z_aps** triggered on only **2 rows** (both WAHL Mens Electric in Beauty L1) — z_aps of 263 and 183. Wahl's synthetic APS of 0.22/0.15 vs a cell median of 0.0008 is the only place APS variance was strong enough to fire.
- **z_wos** triggered **zero times** — `wos = 8` everywhere, MAD = 0 in every cell.

### Read

- **Inverted anomaly direction**: these are not "broken" articles — they are the few SKUs in each cell that are actually on the floor. The cells themselves are unhealthy (median in-stock ≈ 0.1%), so any working article looks anomalous.
- **CHILDRENS ACCESS dominates** — kids' socks, kids' gifts/tech, kids' beauty, backpacks. The "STITCH" and "SECONDARY" brand labels suggest licensed/multi-licensor packs that move briskly relative to their L1 average.
- **NAUTICA dog beds** (PET L1) are clear outliers at 58–80% in-stock — they're stocked at meaningful levels while everything else in PET sits at zero.
- **GURUNANDA wellness** (BEAUTY) is the other notable cluster — many SKUs anomalously in-stock and selling (50–89% in-stock) vs. a cell median around 0.2%.
- **WAHL** is the lone APS standout — Mens Electric grooming, selling at a per-store rate ~263× the cell median.
- One degenerate row: ADRIANNA PAPELL (MISSES BETTER SW) has `mapped_stores_count = 0` (the RCL eligibility gap we've documented), so its synthetic APS is null, but its in_stock_perc anomaly still fired (z=318).

---

## Tool call summary

| Field | Value |
|---|---|
| Tool | `mcp__smartstudio__query_articles` |
| Number of calls | 1 |
| Server-reported duration | 64 ms |
| Rows returned | 100 |
| Columns returned | 16 |
| Cells evaluated | (brand × L1) with ≥ 20 articles |
| Channels fired | z_isp: 100, z_aps: 2, z_wos: 0 |
| Data source | DuckDB `article_selection` table (46,610 rows) |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
