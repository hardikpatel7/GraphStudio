# article_selection (V7) — Source-of-Truth Reference

This doc describes the V7 build path for `article_selection` (Bealls inventory
smart app). It is intended as a maintenance reference: every column is traced
back to its PG source so future changes can be reasoned about without rerunning
the legacy stored procedure to "see what it does".

The legacy stored procedure is `inventory_smart.article_selection_list_v2`
(the file at `sql/article_selection_list_v2.sql` is the canonical copy).
V7 ⊆ PG semantically: V7 may carry extra rows (typically future-active
articles), but for any (article, column) pair where PG has a non-null value,
V7 must produce a value that is byte-for-byte equivalent.

V7 lives in the smartstudio server: `server/src/article_selection/extractor.rs`.

---

## 1. Pipeline shape

Two pipelines, run in order:

1. `pl_v7_extracts` — `pg_extract` steps that pull source tables from PG into
   tenant DuckDB as `raw_*` and `asv2_*` tables. Defined in
   `server/src/db/mod.rs::PL_V7_EXTRACTS_JSON`.

2. `pl_v7_build` — DuckDB query steps that derive intermediate tables, then a
   single `custom_rust` step (`assemble_article_selection`) that runs
   `extract_and_assemble_from_duckdb` (extractor.rs:173). Output table:
   `article_selection`.

The `custom_rust` step is the heart of the build. Everything below describes
what happens inside that step.

---

## 2. Phase breakdown of the assembly step

The phases in `extract_and_assemble_from_duckdb`:

- Phase 1 — read DuckDB inputs (`raw_*`, `asv2_*`) into Rust HashMaps.
- Phase 2 — RCL resolution: per-product DC-policy and constraint resolution
  via the `rcl` crate from `rust-shared-utils`. Returns
  `dc_policy_by_pc: HashMap<product_code, &DcPolicy>` and
  `constraint_by_pc: HashMap<product_code, &[ConstraintRow]>`.
- Phase 3 — `precompute_pc_store_contribs`: for each product, walk its
  RCL constraint rows, expand each row's `psa_code` into stores, apply the
  three eligibility filters, and aggregate per (product, store) into
  `StoreContrib` (sum of aps, avg of wos/min/max, MIN(max) and MAX(min)
  for the validators).
- Phase 4 — rayon-parallel `assemble_row` per PH: combine the precomputed
  contributions for the PH's product_codes with the PH-level inputs
  (txs, inv, woc, instock, before_alloc, …) into one `ArticleSelectionRow`.

---

## 3. Column-by-column source of truth

Every column emitted on `ArticleSelectionRow` (server/src/article_selection/types.rs:41).

### 3.1 Identifying / catalog columns (PG passthrough)

| Column | Source of truth | How V7 computes it |
|---|---|---|
| `ph_code` | `inventory_smart.ph_master.ph_code` | Read in `raw_ph_master`, parsed to i64 in assemble_row. |
| `article` | `ph_master.article` | Direct copy. |
| `l0_name`..`l5_name` | `ph_master.l0_name`..`l5_name` | Direct copy. |
| `style_color_description`, `product_description` | `ph_master.*` | Direct copy. |
| `sizes` | `ph_master.sizes` (PG `text[]` array literal) | `pg_array_to_json` converts PG `{a,b,c}` → JSON `["a","b","c"]`. |
| `upc` | `ph_master.product_codes` (TEXT, `array_to_string(_, '|')` in extract) | Re-split on `|` / `,`, emitted as JSON array. |
| `product_life_cycle`, `article_status_tag`, `brand` | `ph_master.*` | Direct copy. Empty `product_life_cycle` rendered as null. |
| `channel` | `ph_master.channel` (csv text) | Split on `,`, JSON array. |

### 3.2 Inventory rollups (per ph_code)

Computed by the DuckDB step `build_asv2_inventory` in `pl_v7_build` (db/mod.rs:314):

- For each product_code in the PH's `product_codes`, find its active DCs via
  `raw_product_dc_mapping` JOIN `raw_distribution_centres` (active=true,
  is_deleted=false), and require the DC also exists in `raw_store_dc_mapping`.
- Sum `oh`, `oo`, `it` across (product_code, dc_code) from
  `raw_sku_dc_available_units`.
- Sum `quantity` (= reserve_quantity) from `raw_sku_dc_reserved_units`.
- `allocated_units` is set to `0` (PG calls `sku_dc_allocated_units` which is
  derived inside an SP we don't extract; legacy output is overwhelmingly 0).
- `net_available_inventory = oh − reserve_quantity − allocated_units`,
  computed in assemble_row.

| Column | Source | Notes |
|---|---|---|
| `oh`, `oo`, `it`, `reserve_quantity` | sku_dc_available_units / sku_dc_reserved_units, summed in `asv2_inventory` | per-PH bigint |
| `allocated_units` | always 0 | placeholder for `sku_dc_allocated_units` |
| `net_available_inventory` | derived | |

### 3.3 Inventory maps (per (size, dc))

Computed by `build_asv2_inventory_per_size_dc` (db/mod.rs:319) and
`build_inventory_maps` in extractor.rs:1465.

| Column | Source | Notes |
|---|---|---|
| `oh_map` | `sum(oh)` per (size, dc) for the PH's product_codes | JSON `{size: {dc: qty}}` |
| `rq_map` | `sum(quantity)` per (size, dc) from `sku_dc_reserved_units` | same shape |
| `au_map` | placeholder zero per (size, dc) | mirrors `allocated_units` |

### 3.4 Last-allocation timestamp

| Column | Source | Notes |
|---|---|---|
| `last_allocated` | `inventory_smart.last_allocated_details.updated_at` per article | Read directly via `raw_last_allocated_details`. |

### 3.5 Pack / packaging

| Column | Source | Notes |
|---|---|---|
| `pack_type_id` | always `None` | Legacy passes through `dc_pack_inventory.pack_type_id` after CASE matching the chosen pack_type; V7 doesn't currently project this column. Gap acknowledged. |

### 3.6 Transaction metrics (last week)

These are precomputed into `inventory_smart.asv2_txs_metrics` by the legacy
matview-refresh path (which the V7 extracts pipeline reads directly).

| Column | Source | Notes |
|---|---|---|
| `lw_units`, `lw_margin`, `lw_revenue` | `asv2_txs_metrics.*` | i64 |
| `price`, `discount` | `asv2_txs_metrics.*` | f64 |
| `in_stock_perc` | `asv2_instock` MV (article-level) joined to ph_master | f64 |

V7 reads these via `pl_v7_extracts` (steps `asv2_txs_metrics`, `asv2_instock`,
db/mod.rs:281).

### 3.7 RCL constraint aggregates

These are the columns most affected by the RCL resolution chain. Source of
truth: `inventory_smart.generate_rcl_constraint_data(_, 170, current_date)`
called in the legacy SP (line 583 of article_selection_list_v2.sql), then
the `constraint_data` CTE on lines 271–294 averages those rows per
(ph, store) and rolls up per ph.

V7 replicates this without calling the SP:

1. `rcl::resolve_constraints(&ruleset, products)` returns the per-product
   constraint rows. Internally: for each product, walk `rcl_master` priority
   ASC; for each rcl_code, find the matching `rule_code` via the
   `rcl_constraint_master_rule.rcl_dimension`; pick the first match; emit
   the `rcl_constraint_master` rows for that (rcl_code, rule_code).
2. `precompute_pc_store_contribs` (extractor.rs:1066) expands each
   constraint row to its stores via `psa_code → stores`, applies the four
   eligibility filters (see §4), and aggregates per (product, store) into
   a `StoreContrib`.
3. `compute_constraints` (extractor.rs:1175) per PH merges contribs across
   the PH's product_codes, applies the channel filter, and rolls up.

| Column | Source | Inner-aggregation | Outer-aggregation |
|---|---|---|---|
| `aps` | `rcl_constraint_master.aps` | `SUM` over PSA's stores | `AVG` per ph |
| `wos` | `rcl_constraint_master.wos` | `AVG` | `AVG` |
| `min_stock` | `rcl_constraint_master.min_stock` | `AVG` | `AVG` |
| `max_stock` | `rcl_constraint_master.max_stock` | `AVG` | `AVG` |
| `min_stock_validator` | `rcl_constraint_master.max_stock` | `MIN` | `MIN` |
| `max_stock_validator` | `rcl_constraint_master.min_stock` | `MAX` | `MAX` |
| `mapped_stores` | derived from per-(ph,store) groupings | array of stores that survived all filters | sorted ascending by string |
| `mapped_stores_count` | `array_length(mapped_stores, 1)` | | |

### 3.8 WOC (weeks of cover)

Source: `inventory_smart.asv2_woc` MV — per-l4 woc averaged per ph_code.

| Column | Source | Notes |
|---|---|---|
| `wos` | NB: misnamed in V7 — currently maps `asv2_woc.woc` (legacy `cd.wos`). Intentional, preserves the legacy column shape. |
| `avg_max_mod`, `min_woc`, `max_woc` | `asv2_woc.*` | rounded to int |

### 3.9 DC config

Source: `article_dc_config` CTE in legacy SP (article_selection_list_v2.sql:323):

```sql
SELECT ph_code,
  ARRAY_AGG(DISTINCT JSONB_BUILD_OBJECT(
    'value', dc.dc_code, 'label', dc.name, 'is_default', true)) AS dcs
FROM product_store_dc_mapping
JOIN global.distribution_centres dc USING (dc_code)
GROUP BY 1
```

V7 in `compute_dc_config` (extractor.rs:1575): for each product, intersect
`product_dc[pc]` with `store_dc_set` (DCs that have any store mapped) and
emit one entry per DC found via `dist_centres`. Sorted by label.

| Column | Source |
|---|---|
| `dcs` | `product_mapping_product_dc` ∩ `product_mapping_store_dc` ∩ `distribution_centres` |

### 3.10 Store groups

Source: legacy `default_sg_code` per product (DC-policy module 10003 →
`rcl_dc_store_policy.default_store_groups`), expanded via `store_groups`
and `store_groups_mapping`.

V7 in `compute_store_groups` (extractor.rs:1493): from
`dc_policy_by_pc[pc].default_store_groups`, lookup names in
`store_groups_map` and stores in `sg_mapping`. Emit
`[{value: sg_code, label: name, is_default: true}, …]`.

| Column | Source |
|---|---|
| `store_groups` | `rcl_dc_store_policy.default_store_groups` (resolved per product) |

### 3.11 Beginning available to allocate

Computed by `build_asv2_before_alloc` (db/mod.rs:316). Per article, sum
(`units_in_pack × oh_pack_qty`) over `dc_pack_inventory` × `dc_pack_configuration`,
filtered to active store-DC mappings, split by `pack_type` ∈ {eaches, packs}.

| Column | Source |
|---|---|
| `beginning_available_to_allocate_eaches` | `dc_pack_inventory` × `dc_pack_configuration`, pack_type='eaches' |
| `beginning_available_to_allocate_packs` | same, pack_type='packs' |

### 3.12 Allocation rules

Source: per-product DC policy from `rcl_dc_store_policy.dc_store_rule`
joined with `inventory_smart.dc_store_policy_user_rule` (rule_type='dc_store',
'auto_allocation_rule', 'auto_allocation_schedular', 'demand_type', 'min_type').

V7 in `compute_alloc_rules` (extractor.rs:1606+) reads
`raw_dc_store_policy_user_rule` and renders a JSONB object. `min_type` is
stripped out into its own column (legacy v2 behaviour).

| Column | Source |
|---|---|
| `allocation_rules` | `rcl_dc_store_policy.dc_store_rule` → user-rule lookups (excluding `min_type`) |
| `min_type` | the `min_type` slice, surfaced as a separate column |

### 3.13 Product profiles

Source: `inventory_smart.product_profile_master` filtered to
`special_classification = 'ia-recommended'`, returning
`{value: pp_code, name, label: special_classification}`.

V7 reads via `raw_product_profile_master` and `read_product_profiles_duckdb`
(extractor.rs).

| Column | Source |
|---|---|
| `product_profiles` | `product_profile_master` where `special_classification='ia-recommended'` |

### 3.14 Size names

Source: `global.product_attributes_filter` per article — for each `size` in
`ph.sizes`, find the `size_name`. Emit `{size: size_name}` JSON object.

| Column | Source |
|---|---|
| `size_names` | `product_attributes_filter.size_name` keyed by (article, size) |

---

## 4. The four eligibility filters

The single most subtle area is which (product, store) pairs survive into
`mapped_stores` / constraint aggregation. PG legacy applies four filters; V7
applies all four in `precompute_pc_store_contribs`:

1. **PSA L1 match.** A constraint row's `psa_code` (e.g. `30_3510_A`)
   expands to a fixed set of stores via
   `global.product_store_attributes_filter`. We only keep rows where the
   PSA's `l1_name` matches the product's `l1_name` — without this, RCL
   rules that span multiple l1s (e.g. an l0-only rule) would inflate
   constraint aggregation by ~45×.

2. **product → DC × store → DC.** A store is product-eligible iff there
   exists a DC where the product is mapped (via
   `product_mapping_product_dc.is_active=true`) AND the store is mapped
   (via `product_mapping_store_dc.is_active=true`) to that same DC. Built
   in `build_product_store_eligibility` (extractor.rs:797).

3. **PSM eligibility (RCL module 101).** Mirrors
   `global.generate_rcl_psm_data(_, 101, current_date)`: for each product,
   walk rcl_codes [65538, 33, 16] in priority order, find the first whose
   `rcl_hash[rcl_code]` MD5 matches a
   `rcl_product_mapping_product_store_rule.rcl_dimension` MD5, then read
   the eligible `psa_code`s from `rcl_product_mapping_product_store` for
   that (rcl_code, rule_code). Each `psa_code` of the form `30_<store>`
   contributes one store. Implemented in `read_psm_eligible_stores_duckdb`
   (extractor.rs:834).

4. **default_store_groups (RCL module 10003).** Mirrors the legacy
   `_rcl_input_query` join from `store_group esg JOIN sgm ON sgm.sg_code =
   esg.default_sg_code`. For each product, take its resolved
   `default_store_groups` (DC-policy module 10003 → rcl_codes [183, 2]),
   expand via `store_groups_mapping` into a per-product allowed-store set,
   and intersect with the surviving stores. Implemented in
   `build_default_sg_stores_by_pc` (extractor.rs:1134).

5. **Channel filter** (applied in `compute_constraints`, not
   `precompute_pc_store_contribs`): the store's `channel` from
   `store_attributes_filter` must match the PH's `channel`. Without this,
   `mapped_stores` over-inflates by ~17% (BFL stores leak into BLS PHs).

The validation criterion is: for any (article, store) pair on a real PG
output row, V7 must include the same (article, store) pair after applying
filters 1–5. The 15-article fixture in `/tmp/v7-compare/pg.json` matches
byte-for-byte today.

---

## 5. Performance: what's been done and why

These are independent wins; each one is justifiable on its own.

### 5.1 Pre-aggregated MVs are read directly, not recomputed

`asv2_txs_metrics`, `asv2_woc`, `asv2_instock`, `asv2_inventory_per_size_dc`
already exist in PG as materialized views, refreshed on a schedule by the
data team. V7 reads those MVs directly via `pl_v7_extracts` instead of
re-running the underlying GROUP BYs over `article_inventory_dashboard`
(>100M rows for txs alone). Replicating those aggregations would dwarf
everything else V7 does.

### 5.2 RCL is resolved in-process from a small in-memory snapshot

`rcl::RuleStore` loads five tables (`rcl_master`, `rcl_dc_store_policy*`,
`rcl_constraint_master*`) into a compact `RuleSet` once at boot and
refreshes via PG LISTEN/NOTIFY. Per-product resolution is a HashMap lookup
per rcl_code in the priority chain plus a hash-equality check on
`rcl_dimension`. The legacy SP recreates per-call temp tables and runs
`generate_rcl_*` SPs that rebuild the same intermediate state every
invocation — V7 amortizes that cost across all article rows in a single
build.

### 5.3 `precompute_pc_store_contribs` memoizes the per-product expansion

Without this step, computing the constraint aggregates per PH would re-run
`for each constraint_row: expand psa_code → stores; filter; aggregate` for
every PH, which is `O(48k PHs × 6 product_codes × 800 constraint_rows ×
700 PSA stores)` ≈ 160 G operations. Memoizing per product flips that to
`O(173k products × 800 × 700)` once and `O(48k × 6 × ~200)` for the
per-PH merge — ~5–10× fewer ops in practice.

### 5.4 Rayon parallelism on per-PH assembly

`assemble_row` is pure CPU and per-PH-independent. The build runs it via
`active_ph.par_iter()` over 48k PHs, which scales near-linearly to all
cores. The legacy SP serializes everything in one Postgres backend.

### 5.5 PSM resolver in Rust beats the failed DuckDB approach

An earlier attempt expanded the (product × eligible-psa) cross-join in a
DuckDB SQL step. The output was 705M rows. We bailed and moved the
resolution into Rust: read `raw_rcl_psm_eligibility` (12M rows) into a
`HashMap<(rcl_code, rule_code), HashSet<store_code>>` (~130 MB), and
resolve per-product as a constant-time lookup. Read time went from 9+ min
to 17 s, and we never have to materialize the cross-join.

### 5.6 Eligibility maps are HashSets, looked up per-store

The four filters are HashSet `contains` calls inside the inner loop
(`for store in stores`). Each is `O(1)`. The alternative — a SQL JOIN on
each filter — would force the runtime to spill 100M+ row intermediates
to disk.

### 5.7 Reads done in parallel (extract layer)

`extract_and_assemble` issues `copy_table` calls for the small dim tables
(`product_mapping_*`, `distribution_centres`, `store_groups*`,
`store_attributes_filter`) in parallel via `tokio::try_join!`. This pulls
~7 small tables in the time it takes to pull the slowest one (typically
the store-groups mapping at ~6 M rows).

### 5.8 In-memory store rehydrated on boot

After a build, `article_selection` is also held in a process-local
in-memory map keyed by `ph_code`. CDC-style updates (`extract_and_assemble_scoped`)
patch this map without re-reading the entire DuckDB table. Reads from
the gRPC service hit memory, not the file.

### 5.9 The build pipeline writes to DuckDB once at the end

`assemble_article_selection` collects 48k rows in memory and bulk-inserts
into DuckDB exactly once. No row-by-row commits. DuckDB column-store
ingest is the cheap part anyway, but doing it as a single transaction
avoids per-row checkpoint overhead.

### 5.10 The `psm_eligible` map carries entries only for products with a match

Products without a hash match across rcl_codes [65538, 33, 16] don't get
an entry. The downstream filter checks "if map non-empty AND product
absent → skip product entirely", which matches PG's behaviour (no PSM
input rows = product is excluded). This avoids carrying empty hashsets
for ~700 products.

### 5.11 ConstraintRow stays as `&[ConstraintRow]` borrows, not clones

`rcl::resolve_constraints` returns `HashMap<String, &[ConstraintRow]>`
where the slice borrows directly from the in-memory `RuleSet`. There's
zero copying of constraint rows during resolution — only the enclosing
`HashMap` is freshly allocated.

---

## 6. What could make this faster still

These aren't done yet; they are real options for future work.

### 6.1 Skip `read_paf_sizes_duckdb` for PHs that already cache

`raw_paf_sizes` returns ~1.4 M (article, size, size_name) rows. We read
them all even though only the 48 k PHs that survive `raw_aid_articles`
actually need their sizes. A `WHERE article IN (SELECT article FROM
raw_aid_articles)` predicate at extract time would shrink that to the
actual working set.

### 6.2 Reuse `eligible_stores_by_pc` as a HashSet of `&str`

Today the eligibility map stores `HashSet<String>`. The product_dc /
store_dc tables are stable for the duration of the build, so we could
keep `HashSet<&'static str>` over interned strings (e.g. via a `Bumpalo`
arena) and avoid cloning store_code into every set membership check.

### 6.3 Replace `psa_to_stores: (String, Vec<String>)` with a flat slice

The current shape `HashMap<String, (String, Vec<String>)>` allocates one
`Vec<String>` per PSA. There are ~12 K PSAs, each with up to ~800 stores.
A single packed `Vec<u32>` plus a per-PSA offset slice would shrink the
memory footprint and let `precompute_pc_store_contribs` iterate
contiguously.

### 6.4 Replace `psm_eligibility` HashMap key with a packed `(u32, u32)`

`HashMap<(rcl_code, rule_code), HashSet<store_code>>` keys today are
`(String, String)` for parser convenience. They are always integers in
PG. A `(u32, u32)` key would fit in 8 bytes vs ~48 bytes/string and
eliminate `String` hashing on every lookup.

### 6.5 Move the channel filter into `precompute_pc_store_contribs`

Currently the channel filter runs in `compute_constraints` after merge.
Pushing it into `precompute_pc_store_contribs` means stores that
mismatch the PH's channel never even enter the per-product `by_store`
HashMap, saving the merge cost for the ~17% of stores that get filtered
out anyway.

### 6.6 RCL `RuleSet` could deduplicate by `psa_code`

Multiple rcl_codes can share the same `psa_code`. The `RuleSet`
currently stores per-(rcl_code, rule_code) `Vec<ConstraintRow>` with
duplicated psa entries. A flat per-rule `psa_id` index plus a single
`Vec<ConstraintRow>` keyed by psa_id would let us hash-join faster in
the resolver.

### 6.7 Pipe inventory maps through `RecordBatch` instead of JSON strings

`oh_map`, `rq_map`, `au_map` are written as JSON-string columns in
DuckDB. The downstream consumer (gRPC) parses the JSON back. A typed
`MAP<VARCHAR, MAP<VARCHAR, BIGINT>>` would skip both writes (we serialize
in Rust) and reads (the gRPC server reparses).

### 6.8 Snapshotted `pl_v7_extracts` for "small dim" tables

The dim tables (`distribution_centres`, `store_groups*`,
`product_attributes_filter` columns we use) change at most once per day.
We currently re-extract them on every build. A second pipeline that
runs only the small dims on a daily cron, plus a fast pipeline that
extracts only the large fact tables on-demand, would shrink the median
build time by ~20 s.

### 6.9 CDC-only refresh during the day

Today every refresh re-runs the full V7 build. The scoped path
(`extract_and_assemble_scoped`) already exists; it just isn't wired into
a "refresh changed PHs only" trigger. Listening on PG NOTIFYs from
`ph_master`, `article_inventory_dashboard`, etc., and rebuilding only the
changed PHs would let us cut the median refresh latency from ~5 min
(full build) to seconds.

### 6.10 Avoid the assembly-time `pg_array_to_json` / `delim_to_json_array` round-trip

`sizes` and `upc` come out of PG as text array literals or `|`-separated
text. Parsing them at assembly time costs allocations per row × 48 k
rows. A `pl_v7_extracts` step that emits them as JSON arrays directly
(via `array_to_json` in PG) would shave milliseconds × 48 k rows off the
hot path and let `assemble_row` take a `&str` view instead of allocating
a fresh `String` per column.

---

## Appendix A — Legacy stored procedure source

This is `inventory_smart.article_selection_list_v2`, the source of truth
for V7's output. Verbatim copy of `sql/article_selection_list_v2.sql` —
maintained alongside this doc so it stays readable when the SQL file
moves or rotates.

The SP's per-call control flow:

1. Build `_query_pa` (product filter) and `_channel_filter` from input.
2. Build a paged temp table `ph_data_<uuid>` from `ph_master ⋈ aid`.
3. Build `prod_data_<uuid>` (unnested product_codes) and feed it to
   `generate_rcl_dc_store_policy(_, 10003, current_date)` → temp
   `_ph_configuration_mapping_<uuid>` (default_store_groups,
   default_product_profile, dc_store_rule per ph_code).
4. Build `_rcl_input_table = rcl_psm_input_data_<uuid>` from the
   `_rcl_input_query_format` CTE — joins ph + sgm + PSAF restricted to
   the article's default_sg.
5. Run `generate_rcl_psm_data(_, 101, current_date)` →
   `rcl_psm_resolved_data_<uuid>`, then keep only rank-1 rows per
   product_code via `RANK() OVER (… ORDER BY rm.priority ASC,
   array_length(rm.level, 1) DESC)`.
6. Build `_constraints_input_table = rcl_constraint_input_data_<uuid>`
   by joining the PSM-resolved table with `product_attributes_filter`
   and `product_store_attributes_filter` on (l0, l1, store_code).
7. Run `generate_rcl_constraint_data(_, 170, current_date)` →
   `constraints_resolved_data_<uuid>`.
8. Run the `_query_combine_format` CTE chain to assemble final rows.

```sql
-- DROP FUNCTION inventory_smart.article_selection_list_v2(refcursor, jsonb, jsonb, _int4, _bpchar, jsonb, text, int4);

CREATE OR REPLACE FUNCTION inventory_smart.article_selection_list_v2(input refcursor, jsonb, jsonb, integer[], character[], jsonb, uuid text, default_sg_code integer)
 RETURNS refcursor
 LANGUAGE plpgsql
 SECURITY DEFINER
AS $function$
 	declare
 		_query_pa text := '';
 		_channel text := inventory_smart.get_channel_from_input($3);
 		_channel_filter text := inventory_smart.get_channel_str_from_input($3);
 		_store_active_filter text := '{"active": []}'::jsonb || $3;
 		_query_sa text := global.form_attribute_table_filters_v2('store_attributes', 'store_code', _store_active_filter::jsonb);
       _query_sa_psm text := '';
       _ph_query text := '';
 		_product_filters jsonb := $2 ;
 		_query_combine_format text := '';
 		_query_combine text := '';
 		_query_combine_count_format text := '';
 		_query_combine_count text := '';

        _count int := 1;
 		_batch_count int := 0;
 		_ph_sort text ;
 		_ph_search text;
        _overall_search text;
        _limit int;
        _offset int;
        _limit_clause text := '';
		_temp_query text := '';
		vl_unique_identifier text := $7;
		_rcl_input_query_format text:= '';
		_rcl_input_query text := '';
		_rcl_input_table text := '';
		_rcl_psm_resolved_table text := '';
		start_time timestamp;
        end_time timestamp;
        ph_data_id text := 'ph_data_' || uuid  || '';
       _constraints_input_table text = '';
       _resolved_articles varchar[];

	ph_configuration_mapping text := '_ph_configuration_mapping_' || uuid || '';

 	begin
		_rcl_input_table := 'public.rcl_psm_input_data_' || vl_unique_identifier;
		_constraints_input_table := 'public.rcl_constraint_input_data_' || vl_unique_identifier;
		_rcl_psm_resolved_table := 'public.rcl_psm_resolved_data_' || vl_unique_identifier;
		SELECT * FROM inventory_smart.form_search_sort_clause($6, 'ph_master', 'inventory_smart') INTO _ph_sort, _ph_search, _overall_search, _limit, _offset;
		raise notice '_ph_sort : %', _ph_sort;
		raise notice ' _ph_search : %', _ph_search;
		raise notice ' _overall_search: %', _overall_search;
		raise notice '_limit : %', _limit;
		raise notice ' _offset: %', _offset;
        select * from inventory_smart.form_product_store_attribute_filter_query('dummy', $2, $3, 'global', 'product_store_attributes_filter_store_code') into _query_sa_psm, _ph_query;

 		_query_pa := inventory_smart.form_main_table_filters(
 		  'ph_master',
 		  _product_filters
 		);

        _query_pa := _query_pa || _ph_search || _ph_sort;
       _query_pa := replace(_query_pa, '%', '%%');
       _query_pa := _query_pa || ' %1$s';
 		raise notice ' _query_pa %', _query_pa;
       raise notice ' _query_sa_psm % ', _query_sa_psm;
        if (_channel_filter = '') IS FALSE then
        	_channel_filter := ' WHERE channel in  '||_channel_filter||' ' ;
        end if;

       _query_combine_count_format := 'SELECT count(*) FROM ( SELECT * FROM inventory_smart.ph_master ' || _query_pa || ' ) sq;';


      _rcl_input_query_format := '
			with store_group as (
				select pcm.* , sg.name
                    from (
                        select
                            ph.ph_code,
                            unnest(product_code_size_map) as product,
							coalesce(pcms.default_store_group, '||default_sg_code||') as default_sg_code
                        from
                            %1$s ph
                        left join (
                			select ph_code, unnest(default_store_groups) default_store_group from ' || ph_configuration_mapping || '
						) pcms
						using(ph_code)
                    ) pcm
                    join global.store_groups sg on pcm.default_sg_code = sg.sg_code
                    where is_deleted = false
			),
			sgm as MATERIALIZED (
				select asgm.sg_code , psaf.store_code, psaf.psa_code from global.store_groups_mapping asgm
				join (
					select store_code, psa_code '||_query_sa_psm||'
				)psaf on asgm.store_code=psaf.store_code
			)
			select product->>''product_code'' as product_code, sgm.store_code, sgm.psa_code
                    from store_group esg
                    join sgm
                        on sgm.sg_code = esg.default_sg_code
                    group by 1, 2, 3';

 		-- ============================================================
 		-- OPTIMIZED _query_combine_format (v2)
 		-- Changes from original:
 		-- 1. ph CTE reads ph_data once, ph_unnested unnests once
 		-- 2. Derived grain views (psm_ph_article_dc, psm_ph_article_store, psm_ph_store)
 		-- 3. Pre-aggregated inventory sources (sda_agg, reserv_agg, alloc_agg, ladt_agg)
 		-- 4. Uses last_allocation_date_table instead of last_allocated_details
 		-- 5. No SELECT DISTINCT * FROM final_result wrapper
 		-- Format params: %1$s=ph_data_id, %2$s=rcl_psm_resolved_table, %3$s=resolved_articles,
 		--   %4$s=vl_unique_identifier, %5$s=ph_configuration_mapping, %6$s=default_sg_code, %7$s=limit
 		-- ============================================================
 		_query_combine_format := $Q$
			WITH ph AS (
				SELECT
					ph_code, channel, article, product_codes,
					l0_name, l1_name, l2_name, l3_name, l4_name, l5_name,
					style_color_description, product_description,
					sizes, product_lifecycle, article_status_tag, brand,
					"offset",
					product_code_size_map
				FROM %1$s
			),
			ph_unnested AS (
				SELECT
					ph.ph_code, ph.channel, ph.article, ph.product_codes,
					p->>'product_code' AS product_code,
					p->>'size' AS size
				FROM ph
				CROSS JOIN LATERAL UNNEST(product_code_size_map) AS p
			),
			product_dc_map AS (
				SELECT phu.ph_code, phu.channel, phu.article, phu.product_code, phu.size, pmpd.dc_code
				FROM ph_unnested phu
				JOIN global.product_mapping_product_dc pmpd
					ON pmpd.product_code = phu.product_code AND pmpd.is_active
				JOIN global.distribution_centres gdc
					ON gdc.dc_code = pmpd.dc_code AND gdc.is_active AND NOT gdc.is_deleted
			),
			product_store_dc_mapping AS (
				SELECT DISTINCT
					pmps.product_code, pmps.store_code, pdc.ph_code, pdc.article, pmsd.dc_code, pdc.size
				FROM ph
				JOIN %2$s pmps ON pmps.product_code = ANY(ph.product_codes)
				JOIN product_dc_map pdc ON pdc.ph_code = ph.ph_code
				JOIN global.product_mapping_store_dc pmsd
					ON pmsd.store_code = pmps.store_code AND pmsd.dc_code = pdc.dc_code AND pmsd.is_active
			),
			psm_ph_article_dc AS (
				SELECT DISTINCT ph_code, article, dc_code FROM product_store_dc_mapping
			),
			psm_ph_article_store AS (
				SELECT DISTINCT ph_code, article, store_code FROM product_store_dc_mapping
			),
			psm_ph_store AS (
				SELECT DISTINCT ph_code, store_code, product_code FROM product_store_dc_mapping
			),
			product_dc_map_after_store_eligible AS (
				SELECT DISTINCT ph_code, product_code, size, article, dc_code FROM product_store_dc_mapping
			),
			aid AS (
				SELECT psdm.ph_code, art.*
				FROM inventory_smart.article_inventory_dashboard art
				JOIN psm_ph_article_store psdm ON psdm.article = art.article AND psdm.store_code = art.store_code
			),
			txs_metrics AS (
				SELECT
					ph_code,
					CAST(ROUND(SUM(COALESCE(lw_units, 0))) AS INTEGER) AS lw_units,
					CAST(ROUND(SUM(COALESCE(lw_margin, 0))) AS INTEGER) AS lw_margin,
					CAST(ROUND(SUM(COALESCE(lw_revenue, 0))) AS INTEGER) AS lw_revenue,
					ROUND(COALESCE(SUM(lw_revenue) / NULLIF(SUM(lw_units), 0), 0)::DECIMAL, 2) AS price,
					ROUND(CAST(COALESCE(SUM(msrp * discount) / NULLIF(SUM(msrp), 0), 0) AS NUMERIC), 2) AS discount,
					ROUND(CAST(CASE WHEN COUNT(*) != 0 THEN COUNT(CASE WHEN in_stock = 1 THEN 1 ELSE NULL END) / CAST(COUNT(*) AS FLOAT) ELSE 0 END AS NUMERIC), 2) AS in_stock_perc
				FROM aid
				GROUP BY 1
			),
			before_allocated AS (
				SELECT
					ph.ph_code,
					SUM(dpi.eaches) AS eaches,
					SUM(dpi.packs) AS packs
				FROM psm_ph_article_dc ph
				JOIN (
					SELECT
						dpi.dc_code,
						dpi.article,
						CASE WHEN dpi.pack_type = 'eaches' THEN COALESCE(dpc.units_in_pack, 1) * COALESCE(dpi.oh_pack_qty, 0) ELSE 0 END AS eaches,
						CASE WHEN dpi.pack_type = 'packs' THEN COALESCE(dpc.units_in_pack, 1) * COALESCE(dpi.oh_pack_qty, 0) ELSE 0 END AS packs
					FROM inventory_smart.dc_pack_inventory dpi
					JOIN inventory_smart.dc_pack_configuration dpc
						ON dpi.pack_type_id = dpc.pack_type_id AND dpi.article = dpc.article AND dpi.pack_type = dpc.pack_type
				) dpi ON dpi.article = ph.article AND dpi.dc_code = ph.dc_code
				GROUP BY ph.ph_code
			),
			sda_agg AS (
				SELECT product_code, dc_code, size, article,
					SUM(COALESCE(oh, 0)) AS oh,
					SUM(COALESCE(oo, 0)) AS oo,
					SUM(COALESCE(it, 0)) AS it
				FROM inventory_smart.sku_dc_available_units
				GROUP BY 1, 2, 3, 4
			),
			reserv_agg AS (
				SELECT product_code, dc_code, size, article,
					SUM(COALESCE(quantity, 0)) AS quantity
				FROM inventory_smart.sku_dc_reserved_units
				GROUP BY 1, 2, 3, 4
			),
			alloc_agg AS (
				SELECT article, dc_code, size,
					MAX(updated_at) AS allocated_time,
					COALESCE(SUM(quantity), 0) AS quantity
				FROM inventory_smart.sku_dc_allocated_units('', '%3$s')
				GROUP BY 1, 2, 3
			),
			ladt_agg AS (
				SELECT article, updated_at AS allocated_time
				FROM inventory_smart.last_allocated_details
			),
			inventory_details_product_dc_level AS (
				SELECT
					ph.product_code,
					ph.ph_code,
					ph.dc_code,
					ph.size,
					sda.oh AS oh,
					sda.oo AS oo,
					sda.it AS it,
					COALESCE(sku_reserv.quantity, 0) AS total_reserve,
					COALESCE(sdal.quantity, 0) AS allocated_units,
					(sda.oh - COALESCE(sku_reserv.quantity, 0) - COALESCE(sdal.quantity, 0)) AS net_available_inventory,
					COALESCE(sdal.allocated_time, ladt.allocated_time, null) AS allocated_time
				FROM product_dc_map_after_store_eligible ph
				JOIN sda_agg sda USING (product_code, dc_code, size, article)
				LEFT JOIN reserv_agg sku_reserv USING (product_code, dc_code, size, article)
				LEFT JOIN alloc_agg sdal ON sdal.article = sda.article AND sdal.dc_code = sda.dc_code AND sdal.size = sda.size
				LEFT JOIN ladt_agg ladt ON ladt.article = sda.article
			),
			agg_inventory_details AS (
				SELECT product_code, ph_code, size,
					JSONB_OBJECT_AGG(dc_code, oh) AS oh_map,
					JSONB_OBJECT_AGG(dc_code, allocated_units) AS au_map,
					JSONB_OBJECT_AGG(dc_code, total_reserve) AS rq_map,
					SUM(oh) AS oh,
					SUM(oo) AS oo,
					SUM(it) AS it,
					SUM(total_reserve) AS reserve_quantity,
					SUM(net_available_inventory) AS net_available_inventory,
					SUM(allocated_units) AS allocated_units,
					MAX(allocated_time) AS allocated_time
				FROM inventory_details_product_dc_level
				GROUP BY 1, 2, 3
			),
			final_inventory AS (
				SELECT ph_code,
					JSONB_OBJECT_AGG(size, oh_map) AS oh_map,
					JSONB_OBJECT_AGG(size, au_map) AS au_map,
					JSONB_OBJECT_AGG(size, rq_map) AS rq_map,
					SUM(oh) AS oh,
					SUM(oo) AS oo,
					SUM(it) AS it,
					SUM(reserve_quantity) AS reserve_quantity,
					SUM(net_available_inventory) AS net_available_inventory,
					SUM(allocated_units) AS allocated_units,
					MAX(allocated_time) AS allocated_time
				FROM agg_inventory_details
				GROUP BY 1
			),
			constraint_data AS (
				SELECT ph_code,
					ROUND(AVG(aps)::NUMERIC, 2) AS aps,
					ROUND(AVG(wos)::NUMERIC, 2) AS wos,
					ROUND(AVG(min_stock)::NUMERIC, 2) AS min_stock,
					ROUND(AVG(max_stock)::NUMERIC, 2) AS max_stock,
					ROUND(MIN(min_validator)::NUMERIC, 2) AS min_stock_validator,
					ROUND(MAX(max_validator)::NUMERIC, 2) AS max_stock_validator,
					ARRAY_AGG(store_code) AS mapped_stores,
					ARRAY_LENGTH(ARRAY_AGG(store_code), 1) AS mapped_stores_count
				FROM (
					SELECT psm.ph_code,
						cm.store_code,
						SUM(aps) AS aps,
						AVG(wos) AS wos,
						AVG(min_stock) AS min_stock,
						AVG(max_stock) AS max_stock,
						MIN(max_stock) AS min_validator,
						MAX(min_stock) AS max_validator
					FROM constraints_resolved_data_%4$s cm
					JOIN psm_ph_store psm USING (product_code, store_code)
					GROUP BY 1, 2
				) foo
				GROUP BY 1
			),
			woc_data AS (
				SELECT ph_code,
					ROUND(AVG(woc)::NUMERIC, 2) AS woc,
					ROUND(AVG(max_mod)::NUMERIC, 2) AS avg_max_mod,
					ROUND(MIN(woc)::NUMERIC, 2) AS min_woc,
					ROUND(MAX(woc)::NUMERIC, 2) AS max_woc,
					COUNT(DISTINCT store_code) AS woc_mapped_stores_count
				FROM (
					SELECT psm.ph_code,
						psm.store_code,
						wm.woc,
						wm.max_mod
					FROM ph
					JOIN psm_ph_store psm ON psm.ph_code = ph.ph_code
					JOIN inventory_smart.woc_master wm
						ON wm.l4_name = ph.l4_name AND wm.store_code = psm.store_code
					WHERE wm.woc IS NOT NULL
				) woc_detail
				GROUP BY ph_code
			),
			product_profiles_ia AS (
				SELECT ph.ph_code,
					JSONB_BUILD_OBJECT('value', pp_code, 'name', name, 'label', special_classification) AS iapp
				FROM ph
				JOIN inventory_smart.product_profile_master ppm ON ph.ph_code = ppm.ph_code
				WHERE special_classification = 'ia-recommended'
			),
			article_dc_config AS (
				SELECT ph_code,
					ARRAY_AGG(DISTINCT
						JSONB_BUILD_OBJECT(
							'value', dc.dc_code, 'label', dc.name,
							'is_default', true
						)
					) AS dcs
				FROM product_store_dc_mapping
				JOIN global.distribution_centres dc USING (dc_code)
				GROUP BY 1
			),
			article_udpp_config AS (
				SELECT ph.ph_code,
					JSONB_BUILD_OBJECT('value', ppm.pp_code, 'name', ppm.name, 'label', ppm.special_classification) AS udpp
				FROM ph
				JOIN %5$s pcm USING (ph_code)
				JOIN inventory_smart.product_profile_master ppm
					ON pcm.default_product_profile = ppm.pp_code
				JOIN inventory_smart.product_profile_user_mapping_size ppums
					ON pcm.default_product_profile = ppums.pp_code AND ppums.size = ANY(ph.sizes)
			),
			article_sg_config AS (
				SELECT
					pcm.ph_code,
					ARRAY_AGG(
						JSONB_BUILD_OBJECT(
							'value', sg_code, 'label', name, 'is_default', true
						)
					) AS store_groups
				FROM (
					SELECT
						ph.ph_code,
						COALESCE(pcms.default_store_group, %6$s) AS default_sg_code
					FROM ph
					LEFT JOIN (
						SELECT ph_code, UNNEST(default_store_groups) AS default_store_group FROM %5$s
					) pcms USING (ph_code)
				) pcm
				JOIN global.store_groups sg ON pcm.default_sg_code = sg.sg_code
				WHERE sg.is_deleted = false
				GROUP BY 1
			),
			inventory_stock_stats AS (
				SELECT
					ph.ph_code,
					ROUND(CAST(CASE WHEN SUM(total_count) != 0 THEN CAST(SUM(in_stock_count) AS FLOAT)/CAST(SUM(total_count) AS FLOAT) ELSE 0 END AS NUMERIC), 4) AS in_stock_perc,
					ROUND(CAST(CASE WHEN SUM(dc_instock_total_count) != 0 THEN CAST(SUM(dc_instock_count) AS FLOAT)/CAST(SUM(dc_instock_total_count) AS FLOAT) ELSE 0 END AS NUMERIC) * 100, 2) AS dc_instock
				FROM inventory_smart.article_instock
				JOIN ph USING (article)
				GROUP BY 1
			),
			allocation_rule AS (
				SELECT dc_store_policy.ph_code, dspur.rule_code, dspur.values AS alloc_rules
				FROM %5$s dc_store_policy
				JOIN inventory_smart.dc_store_policy_user_rule dspur
					ON dc_store_policy.dc_store_rule = dspur.rule_code
			)
			SELECT %7$s AS limit,
				ph."offset",
				ph.l0_name,
				ph.l1_name,
				ph.l2_name,
				ph.l3_name,
				ph.l4_name,
				ph.l5_name,
				ph.style_color_description,
				ph.article,
				ph.ph_code,
				ph.product_description,
				ph.sizes,
				ph.product_codes upc,
				ph.product_lifecycle AS product_life_cycle,
				ph.article_status_tag,
				ph.brand,
				STRING_TO_ARRAY(ph.channel, ',') AS channel,
				COALESCE(reserve_quantity, 0) AS reserve_quantity,
				COALESCE(oh, 0) AS oh,
				COALESCE(oo, 0) AS oo,
				COALESCE(it, 0) AS it,
				null AS pack_type_id,
				oh_map,
				rq_map,
				COALESCE(allocated_units, 0) AS allocated_units,
				((COALESCE(oh, 0) - COALESCE(reserve_quantity, 0)) - COALESCE(allocated_units, 0)) AS net_available_inventory,
				au_map,
				CASE WHEN udpp IS NULL THEN ARRAY[iapp || '{"is_default": true}']
					WHEN iapp = udpp THEN ARRAY[iapp || '{"is_default": true}']
					ELSE ARRAY[udpp || '{"is_default": true}', iapp || '{"is_default": false}']
				END AS product_profiles,
				TO_CHAR(allocated_time::TIMESTAMP, 'MM/DD/YYYY') AS last_allocated,
				dcs,
				store_groups,
				cd.mapped_stores_count,
				cd.mapped_stores,
				cd.aps,
				CAST(ROUND(cd.min_stock) AS INTEGER) AS min_stock,
				CAST(ROUND(cd.max_stock) AS INTEGER) AS max_stock,
				CAST(ROUND(cd.min_stock_validator) AS INTEGER) AS min_stock_validator,
				CAST(ROUND(cd.max_stock_validator) AS INTEGER) AS max_stock_validator,
				CAST(ROUND(wd.woc) AS INTEGER) AS wos,
				CAST(ROUND(wd.avg_max_mod) AS INTEGER) AS avg_max_mod,
				CAST(ROUND(wd.min_woc) AS INTEGER) AS min_woc,
				CAST(ROUND(wd.max_woc) AS INTEGER) AS max_woc,
				tm.lw_units,
				tm.lw_margin,
				tm.lw_revenue,
				tm.price,
				tm.discount,
				iss.in_stock_perc,
				b_alloc.eaches AS beginning_available_to_allocate_eaches,
				b_alloc.packs AS beginning_available_to_allocate_packs,
				alloc_rule.alloc_rules AS allocation_rules,
				COALESCE(alloc_rule.alloc_rules->>'min_type', (SELECT values->>'min_type' FROM inventory_smart.dc_store_policy_user_rule WHERE rule_code = 1 AND rule_type = 'dc-store-rule')) AS min_type
			FROM ph
			JOIN article_sg_config asgc ON ph.ph_code = asgc.ph_code
			LEFT JOIN final_inventory inv_info ON inv_info.ph_code = ph.ph_code
			JOIN constraint_data cd ON ph.ph_code = cd.ph_code
			LEFT JOIN woc_data wd ON ph.ph_code = wd.ph_code
			LEFT JOIN product_profiles_ia ppi ON ph.ph_code = ppi.ph_code
			JOIN article_dc_config adc ON ph.ph_code = adc.ph_code
			LEFT JOIN article_udpp_config ppu ON ph.ph_code = ppu.ph_code
			LEFT JOIN txs_metrics tm ON ph.ph_code = tm.ph_code
			LEFT JOIN inventory_stock_stats iss ON ph.ph_code = iss.ph_code
			LEFT JOIN before_allocated b_alloc ON b_alloc.ph_code = ph.ph_code
			LEFT JOIN allocation_rule alloc_rule ON ph.ph_code = alloc_rule.ph_code
			WHERE (COALESCE(oh, 0) - COALESCE(reserve_quantity, 0) - COALESCE(allocated_units, 0)) > 0
		$Q$;

 		WHILE _count > 0 AND _batch_count = 0 LOOP
 			IF TRIM(_ph_sort) = '' THEN
 				_limit_clause := 'ORDER BY article  LIMIT ' || _limit || ' OFFSET ' || _offset;
 			ELSE
 				_limit_clause := 'LIMIT ' || _limit || ' OFFSET ' || _offset;
 			END IF;

			_temp_query := format('drop table if exists %1$s cascade;', ph_data_id);
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing first temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			_temp_query := format(
				'create temp table %2$s as (
								with ph_data_without_offset as (
								select
									*
								from
									inventory_smart.ph_master
									join (select distinct article from inventory_smart.article_inventory_dashboard ) aid using (article)
								' || _query_pa || '
								)
								SELECT *, %3$s + ROW_NUMBER () OVER (' || replace(_ph_sort, '%', '%%') || ') as offset
									FROM ph_data_without_offset
								)',
				_limit_clause,
				ph_data_id,
				_offset
			);
			raise notice ' ph _Data query %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing second temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for prod_data_'||vl_unique_identifier,'drop table if exists prod_data_'||vl_unique_identifier||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute format('drop table if exists prod_data_%1$s cascade;', vl_unique_identifier);
			_temp_query := format('create unlogged table prod_data_%1$s as (select unnest(product_codes) as product_code from  %2$s);',vl_unique_identifier, ph_data_id);
			raise notice ' prod data query %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing third temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||ph_configuration_mapping,'drop table if exists '||ph_configuration_mapping||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute format('drop table if exists %1$s cascade', ph_configuration_mapping);
			_temp_query := format (
				'create temp table %1$s as (
								select array_agg(resolved_data.product_code) as product_codes,
										max(resolved_data.default_store_groups) as default_store_groups,
										max(resolved_data.default_product_profile) as default_product_profile,
										max(resolved_data.default_store_groups) as default_store_groups_selected,
										max(dc_store_rule) as dc_store_rule,
										psaf.article, psaf.ph_code
                                        from
								(
									select * from inventory_smart.generate_rcl_dc_store_policy(''prod_data_%2$s'', 10003, current_date)
                                ) resolved_data
								join
								(
									select unnest(product_codes) as product_code,
										article, ph_code
									from inventory_smart.ph_master
								) psaf
								using (product_code)
								group by psaf.article, psaf.ph_code
							);',
				ph_configuration_mapping, vl_unique_identifier
			);

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing fourth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			raise notice 'ph  configuration data query : %', _temp_query;
			execute _temp_query;

			_rcl_input_query := format(_rcl_input_query_format, ph_data_id);
			raise notice '_rcl_input_query: %', _rcl_input_query;
			start_time := clock_timestamp();

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||_rcl_input_table,'drop table if exists '||_rcl_input_table||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute 'drop table if exists ' || _rcl_input_table ||' cascade; ';
			_rcl_input_query:= 'create unlogged table ' || _rcl_input_table || ' as ( ' || _rcl_input_query || ' );';

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing create table query for _rcl_input_query', _rcl_input_query, jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			raise notice '_rcl_input_query: %', _rcl_input_query;
			execute _rcl_input_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||_rcl_psm_resolved_table, 'drop table if exists '|| _rcl_psm_resolved_table || ';', jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute 'drop table if exists '|| _rcl_psm_resolved_table || ';';
			_temp_query := format('create unlogged table %2$s as (
				select * from global.generate_rcl_psm_data(''%1$s'', 101, current_date));', _rcl_input_table, _rcl_psm_resolved_table);

			raise notice 'RCL resolution query : %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing fifth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			_temp_query := format('CREATE UNLOGGED TABLE %1$s_filtered AS (
				SELECT product_code, store_code, rcl_code, is_active
				FROM (
					SELECT psm.*, RANK() OVER (
						PARTITION BY psm.product_code
						ORDER BY rm.priority ASC, array_length(rm.level, 1) DESC
					) as rnk
					FROM %1$s psm
					JOIN global.rcl_master rm ON psm.rcl_code = rm.rcl_code
				) ranked
				WHERE rnk = 1
			); DROP TABLE IF EXISTS %1$s; ALTER TABLE %1$s_filtered RENAME TO %2$s;',
			_rcl_psm_resolved_table, split_part(_rcl_psm_resolved_table, '.', 2));
			raise notice 'PSM specificity filter query: %', _temp_query;
			execute _temp_query;

			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to resolve PSM::::  %', end_time - start_time;
			start_time := clock_timestamp();

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before dropping _constraints_input_table','drop table if exists '||_constraints_input_table||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute format('drop table if exists %1$s cascade;', _constraints_input_table);
			_temp_query := format('create unlogged table %1$s as (select psm.product_code, psm.store_code, psaf.psa_code, paf.article from %2$s psm
				join
					global.product_attributes_filter paf using (product_code)
				join
					global.product_store_attributes_filter psaf
					on paf.l0_name=psaf.l0_name and paf.l1_name = psaf.l1_name and psm.store_code=psaf.store_code
				);',
				_constraints_input_table, _rcl_psm_resolved_table);
			raise notice 'temp query for constraints  : %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing sixth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before dropping table constraints_resolved_data_'||vl_unique_identifier ,'drop table if exists constraints_resolved_data_'||vl_unique_identifier||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute format('drop table if exists constraints_resolved_data_%1$s', vl_unique_identifier);
			_rcl_input_query := format('create temp table constraints_resolved_data_%2$s as (
				select * from inventory_smart.generate_rcl_constraint_data(''%1$s'', 170, current_date)
			)', _constraints_input_table, vl_unique_identifier);
			raise notice ' constraints resolution query: % ', _rcl_input_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'constraints resolution query' ,_rcl_input_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _rcl_input_query;
			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to resolve Constraints::::  %', end_time - start_time;
			execute format('select array_agg(distinct article) from constraints_resolved_data_%1$s', vl_unique_identifier) into _resolved_articles;
			_resolved_articles := coalesce(_resolved_articles, '{}');
			raise notice 'resolved articles: %', _resolved_articles;

			start_time := clock_timestamp();

			_query_combine := format(
				_query_combine_format,
				ph_data_id,                 -- %1$s
				_rcl_psm_resolved_table,    -- %2$s
				_resolved_articles,         -- %3$s
				vl_unique_identifier,       -- %4$s (used in constraints_resolved_data_%4$s)
				ph_configuration_mapping,   -- %5$s
				default_sg_code,            -- %6$s
				_limit                      -- %7$s
			);
			raise notice 'Query ------- : %', _query_combine;

			OPEN $1 SCROLL FOR EXECUTE _query_combine;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', '_query_combine' ,_query_combine,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to query combine::::  %', end_time - start_time;

			MOVE FORWARD ALL FROM $1;
			GET DIAGNOSTICS _batch_count := ROW_COUNT;
			MOVE BACKWARD ALL FROM $1;

			IF _batch_count = 0 THEN
				_query_combine_count = format(_query_combine_count_format, _limit_clause);
				perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', '_query_combine_count' ,_query_combine_count,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
				EXECUTE _query_combine_count INTO _count;
			END IF;
			_offset := _offset + _limit;
			_limit := _limit + _limit;
			IF _batch_count = 0 AND _count > 0 THEN CLOSE $1; END IF;

		END LOOP;
		raise notice ' dropping the rcl input tables';
 	RETURN $1;
 	end
 $function$
;
```
