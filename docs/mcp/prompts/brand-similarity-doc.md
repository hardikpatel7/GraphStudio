# Brand-Pair Similarity within L1 — Documentation

## Prompt

> Find the 50 most-similar (brand, brand) pairs within each L1 by inventory profile. Similarity = inverse Euclidean distance across (normalized) total_oh, total_lw_units, avg_price, avg_wos. Globally rank the 200 most-similar pairs across all L1s.

## Caveats applied

- **`avg_wos` is constant 8 across every brand × L1 cell** — within-L1 stddev is 0, so the wos dimension contributes nothing to the distance. Similarity is effectively 3D (OH, LW units, price).
- **Filter applied**: brand-L1 cells with `< 5 articles` were excluded to keep noise down.
- **First run produced degenerate ties**: 50+ pairs had distance = 0 because many dormant brand-L1 cells share identical profiles (small OH, zero LW units, null avg_price). A second refined run requires `SUM(lw_units) > 0` AND `AVG(NULLIF(price, 0)) IS NOT NULL` per brand to surface analytically meaningful pairs.

## Tool calls

Four calls were issued: two MCP queries (top 25 each, for inline display) + two DuckDB CLI exports (full top 200 each, to CSV).

### Call 1 — `mcp__smartstudio__query_articles` (literal top 25, includes dormant ties)

**Tool description** (from the tool schema):

> Run a single SELECT (or WITH/FROM-first) DuckDB statement against the
> `article_selection` table. … `{ sql, limit?, offset? }` → `{ rows, columns, total, duration_ms, executed_sql }`

**Input arguments**

```json
{
  "sql": "<brand-similarity literal pipeline — see SQL section>",
  "limit": 25
}
```

**SQL executed** (literal version)

```sql
WITH brand_l1 AS (
  SELECT l1_name, UPPER(brand) AS brand_uc,
         COUNT(*) AS n_articles,
         SUM(oh) AS total_oh,
         SUM(lw_units) AS total_lw_units,
         AVG(NULLIF(price, 0)) AS avg_price,
         AVG(wos) AS avg_wos
  FROM article_selection
  WHERE brand IS NOT NULL AND brand <> ''
  GROUP BY l1_name, UPPER(brand)
  HAVING COUNT(*) >= 5
),
norm AS (
  SELECT l1_name, brand_uc, total_oh, total_lw_units, avg_price, n_articles,
    (total_oh - AVG(total_oh) OVER (PARTITION BY l1_name))
      / NULLIF(STDDEV(total_oh) OVER (PARTITION BY l1_name), 0) AS z_oh,
    (total_lw_units - AVG(total_lw_units) OVER (PARTITION BY l1_name))
      / NULLIF(STDDEV(total_lw_units) OVER (PARTITION BY l1_name), 0) AS z_lwu,
    (COALESCE(avg_price, 0) - AVG(COALESCE(avg_price, 0)) OVER (PARTITION BY l1_name))
      / NULLIF(STDDEV(COALESCE(avg_price, 0)) OVER (PARTITION BY l1_name), 0) AS z_price
  FROM brand_l1
),
pairs AS (
  SELECT a.l1_name, a.brand_uc AS brand_a, b.brand_uc AS brand_b,
    sqrt(COALESCE(power(a.z_oh-b.z_oh,2),0)+COALESCE(power(a.z_lwu-b.z_lwu,2),0)+COALESCE(power(a.z_price-b.z_price,2),0)) AS euclid_dist,
    a.total_oh AS oh_a, b.total_oh AS oh_b,
    a.total_lw_units AS lwu_a, b.total_lw_units AS lwu_b,
    a.avg_price AS price_a, b.avg_price AS price_b
  FROM norm a JOIN norm b ON a.l1_name=b.l1_name AND a.brand_uc<b.brand_uc
),
ranked_in_l1 AS (
  SELECT *, ROW_NUMBER() OVER (PARTITION BY l1_name ORDER BY euclid_dist) AS rn_in_l1
  FROM pairs
)
SELECT l1_name, brand_a, brand_b,
       ROUND(euclid_dist, 4) AS distance,
       ROUND(1.0/(1.0+euclid_dist), 4) AS similarity,
       oh_a, oh_b, lwu_a, lwu_b,
       ROUND(price_a, 2) AS price_a, ROUND(price_b, 2) AS price_b
FROM ranked_in_l1
WHERE rn_in_l1 <= 50
ORDER BY euclid_dist
LIMIT 25
```

**Note**: an earlier first attempt requested `LIMIT 200` and overflowed the inline-result token cap. DuckDB processed the query and saved the output to a file. I then downsized to `LIMIT 25` for inline display and used the CLI export below for the full 200.

**Server response timing (LIMIT 25 inline call)**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **54 ms** |
| Rows returned | 25 |

---

### Call 2 — DuckDB CLI export (literal full top 200 → CSV)

**Tool used**: `Bash` invoking `duckdb -readonly` CLI (bypasses the MCP inline-row cap).

**Command** (same SQL as Call 1 but `LIMIT 200`, redirected to CSV)

```bash
duckdb -readonly /Users/karthickpachiappan/smartstudio/bealls-inventorysmart-uat-replica-2/data/tenant_data.duckdb -c "COPY (<literal SQL>) TO '/Users/karthickpachiappan/bb/smartstudio/brand-similarity-top200.csv' (HEADER, DELIMITER ',')"
```

**Result**

```
201 /Users/karthickpachiappan/bb/smartstudio/brand-similarity-top200.csv
```

(1 header + 200 rows)

---

### Call 3 — `mcp__smartstudio__query_articles` (refined top 25)

**Input arguments**

```json
{
  "sql": "<refined SQL with HAVING SUM(lw_units) > 0 AND AVG(NULLIF(price, 0)) IS NOT NULL>",
  "limit": 25
}
```

**SQL executed** (refined version — only the `HAVING` clause and the `norm` z_price expression differ from Call 1)

```sql
HAVING COUNT(*) >= 5 AND SUM(lw_units) > 0 AND AVG(NULLIF(price, 0)) IS NOT NULL
…
(avg_price - AVG(avg_price) OVER (PARTITION BY l1_name))
  / NULLIF(STDDEV(avg_price) OVER (PARTITION BY l1_name), 0) AS z_price
```

**Server response timing**

| Metric | Value |
|---|---:|
| `duration_ms` (server-reported) | **52 ms** |
| Rows returned | 25 |

---

### Call 4 — DuckDB CLI export (refined full top 200 → CSV)

**Command** (refined SQL, `LIMIT 200`, redirected to CSV)

```bash
duckdb -readonly … -c "COPY (<refined SQL>) TO '/Users/karthickpachiappan/bb/smartstudio/brand-similarity-active-top200.csv' (HEADER, DELIMITER ',')"
```

**Result**

```
201 /Users/karthickpachiappan/bb/smartstudio/brand-similarity-active-top200.csv
```

---

## Response (verbatim)

Two views are available — the literal one (lots of zero-distance ties from dormant-vs-dormant brands) and a refined one filtered to brands that actually sold something (analytically useful). Both 200-row sets are on disk.

| File | Description | Lines |
|---|---|---:|
| `brand-similarity-top200.csv` | Literal top 200 (mostly distance=0 ties between dormant brands) | 201 |
| `brand-similarity-active-top200.csv` | Refined: requires LW units > 0 AND non-null avg_price per brand-L1 | 201 |

### Literal top 25 — all degenerate ties

Every pair has distance=0, similarity=1.0. They share identical profiles: tiny OH (9–132 units), zero LW units, null avg_price. Examples include `LUCKY BRAND ↔ REBECCA MINKOFF` in Handbags, `MINNIE MOUSE ↔ STITCH` in Accessories, `BLISS ↔ MEOW & ROLL` in Pet. **These aren't meaningfully "similar" — they're equally dormant.**

### Refined top 25 — meaningful pairs (active brands only)

| # | L1 | Brand A | Brand B | Distance | Similarity | OH A / B | LW units A / B | Price A / B |
|---:|---|---|---|---:|---:|---|---|---|
| 1 | MISSES SW | BADGLEY MISCHKA | HARPER 241 | 0.003 | 0.997 | 78 / 76 | 1 / 1 | $14.99 / $14.99 |
| 2 | MISSES BETTER SW | VINTAGE HAVANA | WORKSHOP | 0.017 | 0.983 | 37 / 45 | 1 / 1 | $16.99 / $16.99 |
| 3 | MISSES SW | KINGS ROAD | UNIQUE SPECTRUM | 0.019 | 0.981 | 39 / 50 | 90 / 100 | $14.43 / $14.43 |
| 4 | PETITES SW | ADRIENNE VITTADINI | CASEY KEY | 0.020 | 0.981 | 28 / 28 | 100 / 98 | $12.39 / $12.34 |
| 5 | LINGERIE | BLISS | POPPY & CLAY | 0.024 | 0.977 | 60 / 54 | 1 / 1 | $24.99 / $24.99 |
| 6 | BEAUTY | FREIDA & JOE | LA L'AMOUR | 0.026 | 0.974 | 72 / 75 | 102 / 99 | $14.82 / $14.87 |
| 7 | SWIM | SHOW ME YOUR MUMU | STELLA PARKER | 0.038 | 0.963 | 59 / 60 | 4 / 7 | $28.93 / $28.74 |
| 8 | MISSES SW | REMI JAMES | TEEZ-HER | 0.041 | 0.960 | 61 / 77 | 134 / 158 | $12.46 / $12.53 |
| 9 | HANDBAGS | AMERICAN LEATHER CO | THE SAK | 0.044 | 0.958 | 107 / 102 | 4 / 8 | $68.64 / $68.97 |
| 10 | BEAUTY | CARIBBEAN JOE | COSRX | 0.047 | 0.955 | 78 / 73 | 21 / 29 | $12.21 / $12.10 |
| 11 | BEAUTY | CHLOE | MOSCHINO | 0.049 | 0.954 | 31 / 36 | 19 / 8 | $48.51 / $48.52 |
| 12 | MISSES BETTER SW | KENSIE | SOLITAIRE | 0.049 | 0.954 | 243 / 241 | 54 / 47 | $21.35 / $21.14 |
| 13 | CHILDRENS ACCESS | MARIO BROTHERS | PUMA | 0.051 | 0.952 | 46 / 36 | 332 / 313 | $8.85 / $8.74 |
| 14 | MISSES BETTER SW | PARKER | RAGA | 0.052 | 0.951 | 31 / 42 | 33 / 27 | $19.36 / $19.57 |
| 15 | LADIES SPORTS | COURT HALEY | UNDER ARMOUR | 0.052 | 0.951 | 249 / 262 | 10 / 36 | $18.97 / $18.95 |
| 16 | YOUNGMENS | AEROPOSTALE | ECKO | 0.054 | 0.949 | 90 / 71 | 729 / 734 | $14.34 / $14.17 |
| 17 | BEAUTY | LIFESTYLE | SCENTUALS | 0.056 | 0.947 | 97 / 98 | 54 / 30 | $9.59 / $9.60 |
| 18 | MISSES BETTER SW | FOR THE REPUBLIC | TOMMY HILFIGER | 0.056 | 0.947 | 65 / 67 | 19 / 12 | $14.46 / $14.20 |
| 19 | MISSES SW | CURVE APPEAL | GREIGE | 0.056 | 0.947 | 64 / 30 | 10 / 3 | $15.83 / $15.81 |
| 20 | BEAUTY | IT HAIR CARE | NICKA K | 0.057 | 0.946 | 29 / 32 | 75 / 72 | $5.65 / $6.18 |
| 21 | MISSES SW | REMI JAMES | RIO AND REIN | 0.059 | 0.944 | 61 / 48 | 134 / 74 | $12.46 / $12.54 |
| 22 | MISSES BETTER SW | TOMMY HILFIGER | Z SUPPLY | 0.060 | 0.943 | 67 / 44 | 12 / 1 | $14.20 / $14.25 |
| 23 | MISSES SW | KINGS ROAD | RAIN & ROSE | 0.065 | 0.939 | 39 / 56 | 90 / 13 | $14.43 / $14.48 |
| 24 | BEAUTY | GEMBELLA | JAPONESQUE | 0.065 | 0.939 | 48 / 49 | 28 / 54 | $4.48 / $4.22 |
| 25 | BEAUTY | CARIBBEAN JOE | DUDE WIPES | 0.066 | 0.938 | 78 / 78 | 21 / 2 | $12.21 / $11.69 |

### Read

- **Most-similar real pairs cluster in BEAUTY, MISSES SW, MISSES BETTER SW** — categories with many small-volume brands at similar price tiers (e.g., $12–17 sportswear bands).
- **Notable competitor pairs**: `AEROPOSTALE ↔ ECKO` (Youngmens, 729 vs 734 LW units, identical price tier), `MARIO BROTHERS ↔ PUMA` (Kids Accessories), `CHLOE ↔ MOSCHINO` (Beauty/Fragrance, both $48 designer fragrances), `TOMMY HILFIGER ↔ Z SUPPLY` (Misses Better SW).
- **Pair (1) — BADGLEY MISCHKA ↔ HARPER 241**: nearly identical (OH ~77 vs 76, both sold 1 unit at exactly $14.99) — likely the same shipping pack or licensor.
- **WOS contributes nothing** to the distance (constant 8 in every cell), so the similarity is really measured on 3 features: total OH, total LW units, and average price.

---

## Tool call summary

| # | Tool | Purpose | Duration |
|---:|---|---|---:|
| (initial) | `mcp__smartstudio__query_articles` | First attempt LIMIT 200 — overflowed inline-result cap (output saved to file) | — |
| 1 | `mcp__smartstudio__query_articles` | Literal top 25 for inline | 54 ms |
| 2 | `Bash` → `duckdb -readonly` | Literal full top 200 → CSV | sub-second |
| 3 | `mcp__smartstudio__query_articles` | Refined top 25 (active brands) for inline | 52 ms |
| 4 | `Bash` → `duckdb -readonly` | Refined full top 200 → CSV | sub-second |

| Field | Value |
|---|---|
| SmartStudio MCP calls (non-overflowing) | 2 |
| DuckDB CLI calls | 2 |
| Total server-reported MCP time | 106 ms (54 + 52) |
| Brand-L1 cells evaluated | with ≥ 5 articles (literal) / + LW>0 + price ≠ null (refined) |
| Features used in distance | total_oh, total_lw_units, avg_price (avg_wos contributes 0 due to constancy) |
| Output files | `brand-similarity-top200.csv` (literal), `brand-similarity-active-top200.csv` (refined) |
| Data source | DuckDB `article_selection` table (46,610 rows) |
| Backing pipeline | V7 DuckDB path (`pl_v7_extracts` + `pl_v7_build`) |
