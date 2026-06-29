# Inventory Analysis Prompts — Plain-English Explanations

A walkthrough of the 22 prompts from the SmartStudio inventory-analysis session, in chronological order, with what each one was actually asking for in plain language.

---

## Prompt 1: *"Is the article selection up to date?"*

You were asking whether the master table that all the inventory analysis runs against — `article_selection` — had been recently refreshed with fresh data from the source PostgreSQL database. Essentially: "Is the dataset I'm about to analyze current, or am I about to read stale numbers?" This kicked off the whole sequence of fixes (the broken port-rewrite, the missing PG materialized views, switching to the V7 DuckDB path) that we did before any of the actual data analysis started.

---

## Prompt 2: *"What's the average price by L1?"*

The first real data-analysis prompt. You were asking, for each top-level product category (L1 — things like Ladies Footwear, Mens Outdoor, Beauty), what the typical price of an article is. Specifically the **average price** computed across all articles that have a meaningful price recorded. L1 is the broadest level of the product hierarchy — basically the "department." The answer told you which departments sell at premium ($45 Mens Footwear) vs. budget ($5 Consumables) tiers, before any further drill-downs.

---

## Prompt 3: *"show the breakdown by L2 within footwear"*

After seeing footwear lead the price ranking at the L1 (department) level, you wanted to drill one level deeper. L2 is the next layer of the product hierarchy — basically "sub-departments" inside each L1. So you were asking: within each footwear L1 (Mens Footwear, Athletic Footwear, Ladies Footwear, Childrens Footwear), break the average price down by sub-category (heels-wedges, casual sandals, boots, athletic, kids athletic, etc.) and show me what each looks like. The answer revealed that Mens Boots at $91.50 was the highest-priced niche, while Childrens Footwear capped around $21.

---

## Prompt 4: *"show the top 10 brands by avg price"*

A pivot from category to brand. You wanted to know which brands position themselves at the highest price points across the whole assortment — i.e., which brands carry the most expensive average article. The first result was misleading because most of the top spots were brands with just 1 article each (BIRKENSTOCK at $117 from a single SKU). That tiny-sample issue triggered the next prompt, where you filtered for brands with enough articles to be statistically meaningful.

---

## Prompt 5: *"filter to brands with at least 10 articles"*

A refinement of the previous question. You noticed the first cut was dominated by brands with only 1–5 SKUs (which makes any "average" meaningless), so you re-asked the same question but only counting brands that carry at least 10 articles in the assortment. That floor surfaced real premium players — Versace and Dolce & Gabbana at the top (~$45 avg, both fragrance brands in Beauty), then Clarks, Sol de Janeiro, Drunk Elephant, etc. — actual brand-strategy signals rather than noise from tiny samples.

---

## Prompt 6: *"drill into Versace's L1 mix"*

Versace landed at the top of the refined brand-price ranking, so you wanted to know what Versace actually *sells* in this assortment — which top-level categories (L1s) Versace shows up in, and how it's distributed across them. The answer turned out to be a single L1: all 20 Versace articles sat in **3200-BEAUTY** (range $23–$67), nothing else. That confirmed Versace is here purely as a Beauty/fragrance brand, not apparel.

---

## Prompt 7: *"yes drill into L2/L3"*

Since Versace's L1 turned out to be a single category (Beauty), L1 alone wasn't informative — you wanted to go two and three levels deeper. L2 is sub-department (Ladies Fragrance vs. Mens Fragrance), L3 is the next layer down (Lux Singles, Fragrance Sets, Cosmetics tiers). The drill confirmed Versace is split cleanly: 12 Ladies (8 Lux Singles + 4 Fragrance Sets) and 8 Mens (all Lux Singles), with Fragrance Sets carrying the premium ($52–67) and Mens Lux Singles as the entry tier ($23.62). All fragrance, no other Beauty subcategories.

---

## Prompt 8: *"compare to dolce & gabbana"*

Versace was #1 by average price among brands with ≥10 articles; Dolce & Gabbana was #2 right behind it at almost the same average ($44.65 vs $44.77). You wanted a side-by-side: does D&G occupy the exact same niche as Versace, or do they actually look different underneath? The drill showed they're nearly identical at the top-line ($44 avg, same Beauty/Fragrance L1) but the mix differs — D&G is heavier in Ladies Lux Singles (10 vs 8) and thinner in Fragrance Sets (1 vs 4), plus one stray cosmetics SKU dragging slightly. Same premium-fragrance tier, different shape.

---

## Prompt 9: *"Articles with high APS but low in_stock_perc (under-stocked best-sellers)"*

You shifted from price analysis to operational analysis. The question: find the articles that are clearly **selling fast** but are **rarely on the store floor** — i.e., demand is being suppressed by under-stocking. **APS** = "Average Per-Store sales rate" — how many units the article moves per store carrying it. **in_stock_perc** = the fraction of time/stores the article is actually available on shelves. So: high APS + low in-stock = a best-seller that's stocking out — leaving sales on the table. This is the classic "chase replenishment" hunt list. The catch we discovered: the literal `aps` column was empty in the dataset, so we used `lw_units / mapped_stores_count` as a synthetic APS. The result surfaced 16+ Misses Sportswear WC Casual Knits articles selling 300–700 units a week with only 6–12 units on hand.

---

## Prompt 10: *"Give me a daily inventory health card for brand FILA: count, total OH, total OO, stockouts, overstock, top 5 movers by lw_units"*

You wanted a one-glance "dashboard tile" for a specific brand — the kind of summary an inventory manager would want to see every morning. For FILA, summarize the headline numbers (how many articles, total units on hand, total on order), the exception counts (stockouts: oh=0; overstock: oh > policy max), and the top 5 last-week sellers. **OH** = On Hand (units currently in stock). **OO** = On Order (units coming from suppliers, not yet arrived). **lw_units** = Last Week's unit sales. The result showed FILA is tiny here — just 6 articles, all in Boys Apparel/Active, and 100% tripping the overstock flag because `max_stock = 5` is a too-tight policy ceiling rather than real surplus.

---

## Prompt 11: *"Give me a daily inventory health card for brand NIKE: count, total OH, total OO, stockouts, overstock, top 5 movers by lw_units"*

Same template as the FILA card, but applied to NIKE for comparison. You were using the health-card pattern to size up another brand. The contrast was telling: NIKE has 29 articles (5× FILA's count), 150 total units on hand, but **zero sell-through last week** across the entire NIKE assortment. 17 of 29 articles tripped overstock (again driven by the same `max_stock = 5` policy quirk). Net: NIKE is dead inventory in this snapshot — either the assortment needs winding down or the policy needs revisiting.

---

## Prompt 12: *"Find below-min stock articles"*

A simple-sounding ask: find articles where current OH has dropped below the minimum stock policy — i.e., articles flagged as "below safety stock, needs reorder." But this surfaced a data-shape problem: in this dataset, `min_stock` isn't a *unit threshold* (like "keep at least 50 units"). It's a **0/1 flag** indicating "is a min-stock policy in effect" (1 = yes, 0 = no). So the literal `oh < min_stock` test returns zero matches across all 46,610 articles. The real "below minimum" signal would be `wos < min_woc` — weeks of supply below the weeks-of-cover floor — but those two columns are both constant 8, so even that variant returns nothing. I called out the dataset's flat min/max columns rather than fabricating an answer.

---

## Prompt 13: *"Top 10 articles by last week revenue"*

A pivot from policy-driven exception flags (which weren't yielding insight) back to actual commercial results. Just: which 10 specific articles brought in the most money last week, and what do they look like? **lw_revenue** = sum of revenue generated by an article over the last 7 days. The answer revealed that footwear dominated the high-revenue slots (Reebok #1 at $11.6K, Skechers at #4 and #8, Hey Dude at #7) — pricier items with reasonable in-stock rates compound into revenue leaders. It also surfaced a recurring anomaly: 3 of the 10 top revenue articles had `mapped_stores_count = 0` (no eligible stores per RCL) — selling well anyway, which pointed at a master-data eligibility bug.

---

## Prompt 14: *"How many articles are loaded in the article selection?"*

A simple scope/sanity question — basically "how big is the universe I'm analyzing?" Just count the total rows in the `article_selection` table. The answer (46,610 articles, all unique by ph_code/article) anchored every subsequent percentage and rate calculation. Useful baseline before doing more exotic queries.

---

## Prompt 15: *"Show me the top 10 articles by on-hand units"*

The flip side of "top revenue." Where last-week revenue tells you what's *selling*, on-hand units tells you what's *sitting*. Which 10 articles have the largest piles in the warehouse right now? The result was a clearance/markdown candidate list: 6 of the 10 sold zero units last week despite massive OH (Clarks sandal at #1 with 389 units, $0 LW revenue). Every one tripped the `oh > max_stock = 5` overstock flag. Inventory parked at DCs but not moving — the inverse of the "best-sellers under-stocked" list from prompt 9.

---

## Prompt 16: *"Show 10 articles in brand CLARKS sorted by last week revenue."*

After the broad top-10 across all brands, you wanted to zoom into CLARKS specifically. Clarks had shown up in the dead-stock list (#1 by OH was a Clarks sandal with $0 revenue) but also had appeared earlier as a premium brand by avg price. So which Clarks articles are actually *selling*, ranked by revenue? The answer showed a clean two-category Clarks lineup — Heels-Wedges around $32 and Casual Sandals around $29 — with healthy 45–52% margin rates but in-stock rates of just 7–24% across 600 mapped stores. Selling reasonably well despite poor floor distribution.

---

## Prompt 17: *"Find stockouts in Women's footwear."*

You wanted to scan the Ladies Footwear category (L1 `3510-LADIES FOOTWEAR`) specifically for stockouts — articles that have run out of stock so customers walking into a store can't buy them. The textbook stockout definition is `oh = 0`. The result surprised: **zero** strict stockouts in all 1,353 Ladies Footwear articles. But this turned out to mask a worse problem — **1,032 of 1,353 (76%)** had OH > 0 at the DC but `in_stock_perc = 0` at the store level. So inventory existed somewhere, but it wasn't reaching the sales floor. We called these "functional stockouts" — same impact on the customer, different root cause (distribution problem, not buying problem).

---

## Prompt 18: *"Give me a health card for brand CLARKS: article count, total OH, stockouts, overstock, top 5 movers by lw_units"*

Same health-card template as FILA (prompt 10) and NIKE (prompt 11), now applied to CLARKS so you could compare across brands of different scale. Where FILA had 6 articles and NIKE had 29, CLARKS turned out to be a real assortment — 107 articles, $25K weekly revenue, ~$30 ASP. The exceptions section revealed: 0 strict stockouts (as with everything in this dataset), 81 of 107 articles in functional stockout (76%, matching the L1-wide pattern from prompt 17), 58 of 107 tripping overstock (because of the universal `max_stock = 5` policy), and 20 articles with zero mapped stores — an RCL eligibility gap concentrated in this brand.

---

## Prompt 19: *"Tell me about article 108192034-1 — full picture including per-DC breakdown and which stores it's allocated to. Is it healthy?"*

Zoom from brand-level summaries all the way down to a single SKU. The article ID `108192034-1` was the #1 Clarks revenue earner from prompt 16 — ARLA THONG-Black, a wedge sandal. You wanted everything: identity, prices, sales last week, total OH, which **DCs** (distribution centers / warehouses) hold the stock, per-size breakdown at each DC, which stores have allocations, and a verdict on whether the article is healthy. This was the first prompt to surface the per-DC and per-store JSON maps (`oh_map`, `au_map`). The verdict: a top performer being starved — 44 units sit all at one DC (DC 214), the other DC (215) is empty, zero allocations queued, 600 stores mapped, in-stock only 22%. Inventory fine; distribution broken.

---

## Prompt 20: *"Compare inventory health between Mens Footwear and Ladies Footwear: total OH, stockout rate, overstock rate, in-stock %, average APS — which is healthier and why?"*

A category-vs-category face-off. You wanted to put Mens Footwear (L1 3520) and Ladies Footwear (L1 3510) next to each other on the same scorecard and pick a winner. The five comparison dimensions cover both inventory levels (total OH) and inventory *quality* (stockout rate, overstock rate, floor in-stock %, store-level velocity APS). The verdict: Ladies wins on every dimension — lower functional stockout rate (76% vs 84%), higher in-stock at the floor (1.19% vs 0.83%), nearly double the weekly sell-through (29.5% vs 14.9%). Both are unhealthy in absolute terms, but Mens is roughly twice as inventory-stagnant per dollar of demand. The "why" pointed at the same systemic issue we kept hitting — store-level distribution, not buying.

---

## Prompt 21: *"Find categories with a service-level problem: in_stock_perc < 80% and lw_units > 0. Within those, show top 5 articles by lw_revenue that are currently below min_stock. For each, tell me if reserve_quantity could be released to help."*

A three-part operational drill, focused on rescuing revenue at risk:

1. **Part 1**: identify the worst-performing categories — sub-departments (L2s) where lots of articles are selling but rarely in stock (in-stock < 80% with last-week sales).
2. **Part 2**: within those problem categories, pick the highest-revenue articles that are also running below their minimum stock policy.
3. **Part 3**: for each, check if there's any inventory held in **reserve** (committed-but-not-yet-shipped, kept aside for some allocation rule) that could be released back to the available pool.

The expected workflow: find what's hurting → see what's most worth fixing → check if there's a quick lever (reserves) to fix it.

The dataset stopped this cold: `reserve_quantity = 0` everywhere (no reserves anywhere to release), `wos = min_woc = 8` everywhere (the "below min" filter never fires), and `min_stock` is just a 0/1 flag. So all three under-stocked filters returned empty — not because there's no problem (there's $1.28M of revenue at risk across the top 10 problem L2s), but because the columns that would identify "below min" and "releasable reserves" aren't variant in this dataset.

---

## Prompt 22: *"Run a Monday-morning inventory triage on Ladies Footwear..."*

The big synthesis ask. You wanted me to act like an inventory analyst doing first-thing-Monday triage on the Ladies Footwear category and produce a complete situational report:

1. **Identify the top 5 problems** across the four standard exception types — stockouts (out of stock), overstock (too much stock), reserve gaps (reserves > available), no-eligible-stores (no stores can carry it).
2. **For each stockout, diagnose WHY** — is fresh stock On Order (`oo`), is it In Transit (`it`), is it stuck at the wrong DC (per `oh_map`), or is the article only mapped to a small set of stores (`mapped_stores_count` low)?
3. **For the surplus side**, look at DCs holding excess inventory that hasn't been pushed out, and use `au_map` (allocated-units map per DC+store) to figure out which understocked stores could absorb it.
4. **Three concrete actions** I can execute this morning to improve things.

The result distilled the entire session's findings into one workable plan: 0 strict stockouts in this L1, 1,032 functional stockouts (76%), no reserve gaps (no reserves at all), no eligibility gaps. Every selling stockout traced to the same root cause: zero OO, zero IT, single-DC stranding, and the allocator hasn't pushed anything (`au_map` empty, `last_allocated` null). Three Monday actions: (a) trigger allocation for top 5 selling stockouts, (b) rebalance DC 214 ↔ 215 for single-homed sellers, (c) flag Clarks Breeze Piper for clearance review.
