# Legacy graph cleanup backlog

The 2026-05 graph-unification sweep folded the hand-coded
`article_graph` module under the `graph` namespace and dropped all
v1/v2 versioning identifiers. **`graph::legacy::ArticleGraph` is still
wired** as the implementation behind the article-level read endpoints
(`graph::articles::*`) because the metadata-driven `graph::Graph`
doesn't cover those projection methods yet. This file lists the
migration work needed before `graph::legacy::*` can be physically
deleted from disk, plus the tenant-data migrations the rename
implies.

## What "legacy" means now

The label applies only to **implementations** slated for removal:

| Path / id | Type | Slated for deletion? |
|---|---|---|
| `server/src/graph/legacy/` | hand-coded `ArticleGraph` engine | yes (delete when caller-list below is empty) |
| `AppState.legacy_graph` | runtime slot for the above | yes |
| `pl_build_article_graph` pipeline | builds the above | yes |
| `build_article_graph` assembly | the build step | yes |
| `server/src/handlers/graph_articles.rs` | HTTP endpoints (`/api/graph/articles/*`) | **no** — capability, only the backing impl changes |
| `server/src/services/graph_articles_grpc.rs` | gRPC `ArticleGraphService` | **no** — same |

When `graph::Graph` covers the projection methods, swap
`graph_articles.rs` + `graph_articles_grpc.rs` from
`state.legacy_graph` → `state.graphs[default_id]`. The HTTP/proto
contract stays; only the backend swaps.

## Migration backlog (callers reading `state.legacy_graph`)

Each item needs `graph::Graph` to grow the equivalent capability,
then the caller swaps. The handlers/services themselves stay (they
already have the right name).

| Caller | Current source of truth | What needs porting |
|---|---|---|
| `handlers/graph_articles.rs` | `state.legacy_graph` for all 9 endpoints | match-product, resolve-rcl, aggregate-at, article-detail, brands, exceptions/counts, exceptions/list, traverse. `graph::Graph` has `traverse`/`memory_stats`/`cross_filter` parity; the rest need projection methods. |
| `services/graph_articles_grpc.rs` | `state.legacy_graph` | same surface as handlers |
| `handlers/dataview_source.rs` (`kind == "article_graph"`) | `state.legacy_graph` projection | port `graph::legacy::projection::project_page` to operate on `graph::Graph` |
| `services/cross_filter_grpc.rs` (lines 51, 137) | `state.legacy_graph` | route through `get_default_graph(&state)` — same swap the HTTP cross-filter already did |
| `cross_filter/resolver.rs` | imports `graph::legacy::*` for NodeId resolution | becomes dead once gRPC cross-filter swaps; orphan it then |
| UAM cold-load (`main.rs:504-541`, `uam/store.rs`) | waits on `state.legacy_graph` | switch the wait loop + `cold_load` signature to use `state.graphs[default_id]` |
| `pipeline_assemblies.rs::run_build_article_graph` | builds `state.legacy_graph` | delete the assembly + the `pl_build_article_graph` SQLite seed once nothing reads `state.legacy_graph` |

## Physical paths to delete once the backlog is empty

```
server/src/graph/legacy/                        # entire dir
server/src/graph/parity.rs                      # parity test loses meaning
```

Module-decl lines that must be removed at the same time:

```
server/src/graph/mod.rs                         pub mod legacy;
server/src/graph/mod.rs                         mod parity;
server/src/main.rs (AppState)                   pub legacy_graph: …
server/src/main.rs (constructor)                legacy_graph: Arc::new(…)
server/src/db/mod.rs                            pl_build_article_graph seed
server/src/pipeline_assemblies.rs               "build_article_graph" arm + run_build_article_graph
```

## Tenant-data migrations operators run today

The Rust-side rename ships new pipeline/assembly ids, but tenant
SQLite rows seeded before this commit still reference the old ones.
Apply these one-time UPDATEs against each tenant's
`<home>/smartstudio/<tenant_id>/data/smartstudio.db`:

```sql
-- Rename the pipeline row + its JSON assembly_id
UPDATE pipelines
SET id           = 'pl_build_article_graph',
    display_name = 'Build legacy graph (in-memory)',
    pipeline     = REPLACE(
                     REPLACE(pipeline, 'article_graph_v8', 'build_article_graph'),
                     'assemble_v8_graph', 'assemble_legacy_graph'
                   )
WHERE id = 'pl_v8_build_graph';

-- The 2026-05 sweep dropped the per-source engine="v2" branch in
-- handlers/dataview_source.rs. Any parallel DataView/source pair
-- left over from the v1/v2 cutover is now meaningless. For the
-- bealls tenant the bealls-inventorysmart-uat-replica-2 pair was
-- already removed; run this for any tenant that still has them:
DELETE FROM dataviews WHERE id LIKE '%_v2' AND id LIKE 'dv_v8_%';
DELETE FROM sources   WHERE id LIKE '%_v2' AND id LIKE 'src_v8_%';

```

The source-kind cleanup (`article_graph` → `graph`; `uam_entitlement`
+ `uam_summary` → `duckdb_table` with `target_table = "uam_summary"`)
is handled automatically on next boot — `db/mod.rs::install_schema`
detects the old CHECK constraint, recreates the `sources` table with
the new list, and remaps existing rows during the copy. No operator
action needed.

## UAM materialization caveat

The previous `uam_entitlement` source kind showed an exploded view
(one row per (user, acl, entitled article) with hierarchy + brand +
channel + product_codes joined from the graph). That projection
required walking the graph for every row — at bealls scale (~218
unrestricted users × ~48k articles ≈ 10M rows × 13 columns), full
materialization would be ~3GB per cold-load.

The materialization now writes only `uam_summary` (~221 rows on
bealls). Both old source rows (`src_uam_entitlements` and
`src_uam_summary`) are auto-migrated to point at this same table —
the column shape is the summary's (counts, restricted flag), not
the previous exploded one. **The exploded view is gone** until either
(a) graph projections (article + hierarchy) are also materialized to
DuckDB so a `duckdb_query` can join them with `uam_summary` and the
entitled-article lists, or (b) UAM is rewritten on RCL with a cheaper
projection model.

Operators can also rename `src_v8_articles` / `dv_v8_articles` to drop
the `_v8` suffix if desired — purely cosmetic, no engine impact.

## Repo-level stale paths from prior sweeps

The repo's `data/` folder used to hold tenant runtime artifacts and
seed content; this was untracked + extracted in the earlier
data-folder-cleanup commit (`582cd90`). All seed content lives at
`templates/<app_type>/`; runtime state lives at
`<home>/smartstudio/<tenant_id>/data/`. Local `data/` directory is
safe to `rm -rf` at any time.
