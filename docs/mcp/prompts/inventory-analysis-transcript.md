# Inventory Analysis Transcript — Ladies Footwear / Brand Health

Conversation excerpt covering the last 10 analytical exchanges against the freshly materialized `article_selection` (46,610 rows, V7 DuckDB path).

---

## 1. Top 10 articles by last week revenue

**Prompt**

> Top 10 articles by last week revenue

**Response**

| # | Article | Brand | L1 / L2 | LW rev | LW units | Margin | Price | OH/NAI | In-stk % | Stores |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108186673-110 | REEBOK | ATH FOOTWEAR / Ladies Mod Ath | **$11,602** | 315 | $5,890 | $36.84 | 12 | 60.9% | 600 |
| 2 | 107997589-100 | DASH | MISSES SW / MS Jackets | $10,012 | 523 | $4,782 | $19.14 | 12 | 12.2% | 600 |
| 3 | 108185756-660 | CHENAULT | MISSES SW / WC Cas Knits | $9,264 | 390 | $4,356 | $23.76 | 6 | 7.4% | 600 |
| 4 | 108011572-110 | SKECHERS | ATH FOOTWEAR / Ladies Better Ath | $9,062 | 208 | $3,827 | $43.57 | 3 | 50.7% | 600 |
| 5 | 108021922-460 | VINCE CAMUTO | MISSES BETTER SW / EC Btr Woven | $8,895 | 369 | $4,498 | $24.11 | 12 | 42.1% | **0** ⚠ |
| 6 | 108020968-690 | CE CE BY CYNTHIA STEFFE | MISSES BETTER SW / EC Btr Woven | $8,615 | 222 | $4,269 | $38.82 | 12 | 5.7% | **0** ⚠ |
| 7 | 108174453-2 | HEY DUDE | LADIES FOOTWEAR / Fashion Athleisure | $8,211 | 242 | $4,086 | $33.94 | 27 | 26.9% | 600 |
| 8 | 108011487-60 | SKECHERS | ATH FOOTWEAR / Mens Better Ath | $7,941 | 182 | $3,119 | $43.63 | 5 | 66.4% | 600 |
| 9 | 108229945-1 | JOIE DE VIVRE | MISSES SW / WC WTW Knits | $7,279 | 763 | $3,946 | $9.53 | 6 | 18.5% | 600 |
| 10 | 108220955-110 | MAX STUDIO | MISSES BETTER SW / WC Btr Woven | $7,128 | 372 | $3,336 | $19.17 | 8 | 8.0% | **0** ⚠ |

**Observations**

- **Footwear dominates the high-AOV slots** — Reebok #1, Skechers #4 and #8, Hey Dude #7.
- **Most movers are critically low on hand** — 7 of 10 have OH ≤ 12 units across 600 stores.
- **`mapped_stores_count = 0` anomaly** on three rows (Vince Camuto, CE CE, Max Studio) — selling well but no eligible stores. Likely a RCL eligibility / store-group mapping bug.
- **Highest-unit mover ≠ highest revenue**: Joie de Vivre `108229945-1` moved 763 units (most) but at $9.53 lands at #9.

---

## 2. How many articles are loaded?

**Prompt**

> How many articles are loaded in the article selection?

**Response**

**46,610 articles** loaded — one row per distinct `ph_code` / `article` (no duplicates).

---

## 3. Top 10 articles by on-hand units

**Prompt**

> Show me the top 10 articles by on-hand units

**Response**

| # | Article | Brand | L1 / L2 | OH | OO | IT | Reserve | NAI | LW units | LW rev | In-stk % | Stores | Max |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108023240-1 | CLARKS | LADIES FOOTWEAR / Casual Sandals | **389** | 0 | 0 | 0 | 389 | 0 | $0 | 0.0% | 600 | 5 |
| 2 | 107266145-460 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 272 | 0 | 0 | 0 | 272 | 0 | $0 | 0.0% | 600 | 5 |
| 3 | 107266145-100 | REEL LEGENDS | MENS OUTDOOR / Fishing Tops | 210 | 0 | 0 | 0 | 210 | 0 | $0 | 0.0% | 600 | 5 |
| 4 | 108031542-242 | CLARKS | LADIES FOOTWEAR / Casual Sandals | 199 | 0 | 0 | 0 | 199 | 0 | $0 | 0.0% | 600 | 5 |
| 5 | 108139310-260 | ARIAT | MENS FOOTWEAR / Boots | 183 | 0 | 0 | 0 | 183 | 22 | $2,832 | 2.6% | 600 | 5 |
| 6 | 108010932-50 | TAILORBYRD | MENS FURN-ACC / Loungewear | 178 | 0 | 0 | 0 | 178 | 0 | $0 | 0.0% | 600 | 5 |
| 7 | 108137093-200 | HOSS | MENS FOOTWEAR / Boots | 175 | 0 | 0 | 0 | 175 | 8 | $793 | 0.8% | 600 | 5 |
| 8 | 108162357-400 | LIFESTRIDE | LADIES FOOTWEAR / Heels-Wedges | 167 | 0 | 0 | 0 | 167 | 44 | $836 | 2.7% | 600 | 5 |
| 9 | 108061407-400 | CUISINART | DOMESTICS / Kitchen Textiles | 142 | 0 | 0 | 0 | 142 | 0 | $0 | 0.0% | 600 | 5 |
| 10 | 108158394-3 | TOMMY HILFIGER | MENS FOOTWEAR / Casual | 141 | 0 | 0 | 0 | 141 | 35 | $1,356 | 14.3% | 600 | 5 |

**Read**

- 6 of 10 sold 0 units last week — sitting inventory.
- All 10 trip the overstock flag (OH ≫ `max_stock = 5`) — system-wide config quirk.
- In-stock perc ~0% almost everywhere despite high OH — inventory sitting at DCs, not stores.
- This is a clearance / markdown candidate list.

---

## 4. Top 10 CLARKS articles by LW revenue

**Prompt**

> Show 10 articles in brand CLARKS sorted by last week revenue.

**Response**

| # | Article | L2 | LW rev | LW units | Margin | Price | OH/NAI | In-stk % | Stores |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108192034-1 | EC Heels-Wedges | **$3,062** | 96 | $1,557 | $31.92 | 44 | 22.3% | 600 |
| 2 | 108162776-1 | EC Heels-Wedges | $3,012 | 94 | $1,486 | $32.06 | 71 | 9.2% | 600 |
| 3 | 108192056-250 | EC Heels-Wedges | $2,685 | 84 | $1,339 | $31.93 | 66 | 6.8% | 600 |
| 4 | 108191987-1 | EC Casual Sandals | $2,100 | 72 | $1,008 | $29.14 | 46 | 23.5% | 600 |
| 5 | 108162866-1 | EC Casual Sandals | $2,052 | 70 | $1,050 | $29.36 | 39 | 15.1% | 600 |
| 6 | 108191982-100 | EC Casual Sandals | $1,800 | 62 | $910 | $29.04 | 48 | 12.1% | 600 |
| 7 | 108191910-400 | EC Casual Sandals | $1,472 | 51 | $743 | $28.85 | 49 | 14.8% | 600 |
| 8 | 107921124-100 | EC Casual Sandals | $1,163 | 40 | $505 | $29.06 | 8 | 14.8% | 600 |
| 9 | 108125486-1 | EC Casual Sandals | $1,023 | 36 | $547 | $28.39 | 3 | 13.9% | 600 |
| 10 | 108162783-1 | EC Heels-Wedges | $1,003 | 30 | $482 | $33.39 | 56 | 7.4% | 600 |

- Clean Heels-Wedges ($31–33) vs Casual Sandals ($28–29) split.
- Margin rates ~45–52% — healthy footwear margins.
- In-stock 7–24% despite 600 mapped stores — distribution problem, not buying.
- #8 and #9 are nearly tapped out (OH = 8 and 3) — chase candidates.

---

## 5. Stockouts in Women's Footwear

**Prompt**

> Find stockouts in Women's footwear.

**Response**

| Metric | Count |
|---|---:|
| Articles in L1 | 1,353 |
| Strict stockouts (`oh = 0`) | **0** |
| Functional stockouts (`oh > 0` but `in_stock_perc = 0`) | **1,032 (76%)** |

**Top 15 functional stockouts that are actually selling**

| Article | Brand | L2 | OH/NAI | LW units | LW rev | Price |
|---|---|---|---:|---:|---:|---:|
| 108031515-1 | HEY DUDE | EC Closed Flat | 39 | 6 | $217 | $36.06 |
| 107810015-100 | REEF | WC Closed Flat | 14 | 1 | $73 | $73.11 |
| 108223875-272 | DOLCE VITA | EC Heels-Wedges | 18 | 2 | $65 | $32.30 |
| 108132756-419 | SKECHERS | WC Casual Sandals | 18 | 1 | $44 | $44.29 |
| 108166583-100 | GOOD CHOICE | EC Heels-Wedges | 11 | 1 | $35 | $34.99 |
| 107931140-100 | NATURALIZER | EC Heels-Wedges | 17 | 1 | $35 | $34.99 |
| 108181593-1 | EASY SPIRIT | EC Casual Sandals | 26 | 1 | $31 | $31.49 |
| 107960449-250 | JBU | EC Casual Sandals | 17 | 1 | $30 | $29.99 |
| 107226450-450 | CLARKS | EC Casual Sandals | 4 | 1 | $30 | $29.99 |
| 107942251-1 | ISLAND SURF | EC Hanging Footwear | 7 | 2 | $30 | $14.99 |
| 108181707-260 | GOOD CHOICE | EC Casual Sandals | 12 | 1 | $28 | $27.99 |
| 108078553-960 | MIA | EC Casual Sandals | 4 | 1 | $27 | $26.59 |
| 107824553-1 | DANSKIN | EC Closed Flat | 54 | 1 | $25 | $24.99 |
| 107916693-100 | ITALIAN SHOEMAKERS | EC Heels-Wedges | 7 | 1 | $24 | $23.65 |
| 108039000-200 | WHITE MOUNTAIN | EC Casual Sandals | 7 | 1 | $24 | $23.99 |

**Read**

No textbook stockouts. The real issue is flow: 76% of the L1 reports 0% store in-stock despite DC-level OH. Hey Dude `108031515-1` is the most actionable: 39 OH, $217 LW rev, 0% in-stock. Net: this L1 has an **allocation/distribution problem**, not a buying one.

---

## 6. Brand health card — CLARKS

**Prompt**

> Give me a health card for brand CLARKS: article count, total OH, stockouts, overstock, top 5 movers by lw_units

**Response**

**Headline**

| Metric | Value |
|---|---:|
| Articles | 107 |
| Total OH | 2,517 |
| Total OO | 0 |
| Total NAI | 2,517 |
| LW units | 818 |
| LW revenue | $25,182 |

**Exceptions**

| Flag | Count |
|---|---:|
| Stockouts (`oh=0`) | 0 |
| Functional stockouts | **81 / 107** ⚠ |
| Overstock (`oh > max_stock`) | 58 / 107 |
| No eligible stores | 20 |
| Reserve gap | 0 |

**Top 5 movers**

| Article | L2 | LW units | LW rev | OH | In-stk % | Max | Flag |
|---|---|---:|---:|---:|---:|---:|---|
| 108192034-1 | EC Heels-Wedges | 96 | $3,062 | 44 | 22.3% | 5 | overstock |
| 108162776-1 | EC Heels-Wedges | 94 | $3,012 | 71 | 9.2% | 5 | overstock |
| 108192056-250 | EC Heels-Wedges | 84 | $2,685 | 66 | 6.8% | 5 | overstock |
| 108191987-1 | EC Casual Sandals | 72 | $2,100 | 46 | 23.5% | 5 | overstock |
| 108162866-1 | EC Casual Sandals | 70 | $2,052 | 39 | 15.1% | 5 | overstock |

**Read**

- Real assortment: 107 articles, $25K LW rev, ~$30 ASP.
- 76% functional stockouts — inventory exists but isn't flowing to floor.
- 20 articles have `mapped_stores_count = 0` — RCL eligibility gap.
- Overstock flags are noisy — top movers ship 70–96 units LW but trip overstock because `max_stock = 5` is universal.
- The constraint is replenishment policy + store allocation, not buying.

---

## 7. Article deep-dive — 108192034-1 (ARLA THONG)

**Prompt**

> Tell me about article 108192034-1 — full picture including per-DC breakdown and which stores it's allocated to. Is it healthy?

**Response**

**Identity**: CLARKS ARLA THONG-Black, Ladies Footwear → EC Heels-Wedges → EC Wedge Sandals → Better Wedge Sandals, Sizes 6–10, Price $31.92, 4% discount, Active.

**Performance LW**: 96 units, $3,062 rev, $1,557 margin (51%), in-stock 22.3%.

**Inventory**: OH 44, OO 0, IT 0, Reserve 0, Allocated 0, NAI 44, ATA-eaches 44, WOS 8 weeks.

**Per-size × DC OH** — all 44 units at DC 214; DC 215 empty.

| Size | Code | DC 214 | DC 215 |
|---|---|---:|---:|
| 6 | 1060 | 10 | 0 |
| 7 | 1070 | 10 | 0 |
| 8 | 1080 | **5** | 0 |
| 9 | 1090 | 10 | 0 |
| 10 | 1100 | 9 | 0 |

**Allocation state**: `au_map` all zeros, `rq_map` all zeros, `last_allocated = null`. **Nothing has been pushed.**

**DCs configured**: 214 (CFC IA Physical WH B, default), 215 (CFC IA Physical WH, default).
**Store coverage**: 600 mapped stores ("Bealls All Stores").
**Policy**: min_stock=1, max_stock=5, min_woc=max_woc=8, min_type=mandatory, profile=ia-recommended.

**Health verdict — mixed; a top performer being starved at the floor.**

✅ #1 mover in Clarks, 51% margin, near full price, 8-week WOS at aggregate, 44 available to allocate.

⚠ 22.3% in-stock means 78% of store-days the article isn't on the floor. Zero allocations queued or in flight. All 44 units at one DC (214); DC 215 empty. Size 8 thin (5 units). The overstock flag from max_stock=5 is misleading for a top mover.

**Recommended actions**
1. Trigger an allocation now — 44 units available, nothing in flight.
2. Rebalance DC 214 → DC 215.
3. Watch size 8 (5 units) — replenishment priority.
4. Reconsider `max_stock = 5` for this article class.

Bottom line: inventory is fine; distribution is broken.

---

## 8. Mens vs Ladies Footwear health comparison

**Prompt**

> Compare inventory health between Mens Footwear and Ladies Footwear: total OH, stockout rate, overstock rate, in-stock %, average APS — which is healthier and why?

**Response**

| Metric | Ladies (3510) | Mens (3520) |
|---|---:|---:|
| Articles | 1,353 | 285 |
| Total OH (= NAI) | 24,563 | 6,301 |
| LW units | 7,239 | 937 |
| LW revenue | $171,261 | $26,941 |
| Strict stockouts | 0 (0%) | 0 (0%) |
| **Functional stockouts** | 1,032 (**76.3%**) | 239 (**83.9%**) |
| Overstock | 1,014 (74.9%) | 232 (81.4%) |
| Avg in-stock % | **1.19%** | 0.83% |
| Avg synthetic APS | **0.0089** | 0.0055 |
| Avg WOS | 8 | 8 |
| **Weekly sell-through** | **29.5%** | 14.9% |

*Note: `aps` column unpopulated; using `lw_units / mapped_stores_count` as synthetic APS.*

**Verdict: Ladies is healthier — across the board.**

| Indicator | Winner | Margin |
|---|---|---|
| Lower functional stockout rate | Ladies | 76.3% vs 83.9% |
| Lower overstock-flag rate | Ladies | 74.9% vs 81.4% |
| Higher avg floor in-stock | Ladies | 1.19% vs 0.83% (≈1.4×) |
| Higher store-level velocity | Ladies | 0.0089 vs 0.0055 (≈1.6×) |
| Faster weekly sell-through | **Ladies** | **29.5% vs 14.9% (≈2×)** |

**Why Ladies is healthier**

1. **Velocity matches depth**: Ladies has 4.7× more articles and 3.9× more units OH, but moved 7.7× more units and 6.4× more revenue — the bigger pile is justified by sales.
2. **Floor distribution works marginally better**: 1.19% vs 0.83% — both terrible, but Mens is ~30% worse.
3. **Mens is more inventory-stagnant**: 6.7-week vs 3.4-week turn — Mens is sitting on roughly twice as much idle stock per dollar of demand.

Both are unhealthy in absolute terms: 0% median in-stock and ~80% functional-stockout rates mean most articles in both L1s never touch the floor in a normal week. The primary problem in both is **store-level allocation/distribution**, not buying.

Net: if prioritizing, fix Mens first — worse health profile and weaker demand to justify the inventory.

---

## 9. Service-level problem categories + below-min sellers + reserve-release

**Prompt**

> Find categories with a service-level problem: in_stock_perc < 80% and lw_units > 0. Within those, show top 5 articles by lw_revenue that are currently below min_stock. For each, tell me if reserve_quantity could be released to help.

**Response**

Two data-shape issues that change the answer:

- **`reserve_quantity = 0` across all 46,610 rows** (max = 0). No reserves to release anywhere.
- **`wos = min_woc = 8` for every row** (both columns are constants). So `wos < min_woc` is never true.
- The literal "below min_stock" interpretation is empty too (`min_stock` is a 0/1 flag).

All three under-stocked filters yield nothing — not because everything's fine, but because **the metrics aren't variant in this dataset**.

The service-level problem itself is real. Top 10 problem L2s:

| L1 / L2 | Affected articles | LW rev at risk | LW units | Avg in-stk % | Reserve available |
|---|---:|---:|---:|---:|---:|
| MISSES SW / WC CAS SS-SL KNITS | 259 | **$296,520** | 25,918 | 6.9% | 0 |
| MISSES SW / WC WTW KNITS | 169 | $159,705 | 14,739 | 7.1% | 0 |
| BEAUTY / HAIR CARE | 284 | $155,558 | 12,492 | 27.4% | 0 |
| BEAUTY / LADIES FRAGRNCE | 175 | $129,676 | 8,140 | 23.7% | 0 |
| PLUS SW / WC MP KNIT TOPS | 149 | $104,665 | 8,458 | 7.4% | 0 |
| BEAUTY / SKIN CARE | 240 | $96,922 | 10,360 | 24.6% | 0 |
| MISSES SW / MS PANTS | 182 | $93,680 | 6,059 | 3.3% | 0 |
| MISSES BETTER SW / EC BETTER BOTTOMS | 111 | $83,204 | 4,518 | 3.6% | 0 |
| MISSES SW / MS SHORTS | 137 | $81,298 | 5,550 | 3.5% | 0 |
| MISSES SW / EC WOVEN TOPS | 193 | $74,878 | 5,156 | 3.2% | 0 |

**Revenue at risk just in the top 10 problem L2s: ~$1.28M last week.**

**Release-reserves angle**: `reserve_quantity` is uniformly 0 in the dataset. Either the feed isn't populated or this client doesn't hold reserves. There is nothing to release in this snapshot.

---

## 10. Monday-morning inventory triage — Ladies Footwear

**Prompt**

> Run a Monday-morning inventory triage on Ladies Footwear: identify the top 5 problems across stockouts, overstock, reserve gaps, no-eligible-stores. For each stockout, articulate WHY — is OO arriving, IT in transit, units stuck at the wrong DC per oh_map, or is mapped_stores_count low? Then look at the surplus side: for each DC sitting on excess oh that isn't allocated, propose which understocked stores from au_map could absorb it. Surface 3 concrete actions I can take this morning.

**Response**

### L1-level dimensions

| Dimension | Count | Notes |
|---|---:|---|
| Strict stockouts (`oh=0`) | **0** | Nothing literally out |
| Functional stockouts | 1,032 | 76% — inventory at DC, not on floor |
| Overstock flag | 1,014 | Mostly noise (`max_stock=5` universally tight) |
| Reserve gap | 0 | `reserve_quantity = 0` across dataset |
| No-eligible-stores | 0 | All LF articles have ≥1 mapped store |

Four "categories" collapse to two real problems: **floor stockouts on sellers** and **dead piles**.

### Top 5 problems (3 selling stockouts + 2 dead piles)

**A. Selling-but-stocked-out**

| # | Article | Brand | L2 | OH | OO | IT | Stores | LW units | LW rev | In-stk % |
|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | 108031515-1 | HEY DUDE | EC Closed Flat | 39 | 0 | 0 | 600 | 6 | $217 | 0% |
| 2 | 107810015-100 | REEF | WC Closed Flat | 14 | 0 | 0 | 600 | 1 | $73 | 0% |
| 3 | 108223875-272 | DOLCE VITA | EC Heels-Wedges | 18 | 0 | 0 | 600 | 2 | $65 | 0% |

**B. Dead piles**

| # | Article | Brand | L2 | OH | LW units | Last allocated |
|---:|---|---|---|---:|---:|---|
| 4 | 108023240-1 | CLARKS | EC Casual Sandals (Breeze Piper) | 389 | 0 | 04/13/2026 |
| 5 | 108031542-242 | CLARKS | EC Casual Sandals | 199 | 0 | — |

### Why each stockout?

| Hypothesis | #1 HEY DUDE | #2 REEF | #3 DOLCE VITA |
|---|---|---|---|
| OO arriving? | **No** — OO=0 | OO=0 | OO=0 |
| IT in transit? | **No** — IT=0 | IT=0 | IT=0 |
| Mapped stores low? | **No** — 600 (full) | 600 | 600 |
| Stuck at wrong DC? | **YES** — all 39 at DC 215; DC 214 empty | same pattern | same pattern |
| Allocation queued? | **No** — `au_map` all zeros, `last_allocated=null` | same | same |

**HEY DUDE 108031515-1 `oh_map`** (Sz 6/7/8/9/10 = 6/9/14/6/4 at DC 215; **Sz 11 missing entirely**): 39 units at DC 215 only; DC 214 empty even though it's a configured default. Size 11 fully gone — no recovery without buying.

Root cause is universal: pipeline empty (no OO/IT), each article single-homed at one DC, allocator hasn't pushed anything. The store-floor 0% isn't because inventory is missing — it's because **the allocator isn't running**.

### Surplus-side proposal

**CLARKS Breeze Piper 108023240-1 — 389 units at DC 215, 0 allocated**

| Size | Code | DC 215 OH | DC 214 OH | Notes |
|---|---|---:|---:|---|
| 6 | 1060 | 2 | 0 | thin |
| 7 | 1070 | 94 | 0 | strong mid-run |
| 8 | 1080 | **250** | 0 | huge size-8 over-buy |
| 9 | 1090 | 36 | 0 | adequate |
| 10 | 1100 | 7 | 0 | thin |

`au_map` is empty → the standard "send to understocked stores from au_map" doesn't apply. Every one of the 600 mapped stores is equally at zero. Pragmatic proposal: one-shot min-stock push from DC 215 — 1 unit each of size 7/8/9 to ~36 stores, or 5-unit-per-store push to ~50 high-traffic stores.

Cross-DC angle: pair this with a DC-214-only seller (e.g., ARLA THONG, 44 OH all at DC 214) for a same-day inter-DC transfer.

### Three concrete actions for this morning

1. **Run the allocator for the top 5 selling stockouts** (Hey Dude, Reef, Dolce Vita, Skechers, Naturalizer). Combined: ~106 units available, zero in-flight, 600 stores apiece. Even a 1-unit/store push to top-30 stores per article moves in-stock from 0 → ~5% by tomorrow.

2. **Rebalance DC 214 ↔ DC 215** for single-homed sellers. Identify ~50–100 LF articles where OH>20 sits entirely at one DC; schedule a same-day inter-DC transfer of half the run. Structural fix: make the parquet/allocator distribute across both defaults at receipt time.

3. **Mark Clarks Breeze Piper (108023240-1) for clearance review** before pushing — 389 units, last allocated a month ago, $0 LW revenue, 250-unit size-8 over-buy. Flag for buy-side disposition, not allocation.

**Underlying systemic issue**: across every LF article I inspected, `au_map` is all zeros and `last_allocated` is null or weeks old. The allocation engine isn't running on cadence — that's the single biggest lever on this L1's health.
