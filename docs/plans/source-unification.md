# Plan: Source Unification (strangler migration)

> See [`../primer.md`](../primer.md) for the full target conceptual model that this plan implements.

## Context

Today the smartstudio data layer has three overlapping mental models for "where data comes from":

- `data_sources` (= Connections — credentials)
- `query_sources` (= the QuerySource UX: single PG → DuckDB with CDC bolted on)
- `shared_pipelines` (= multi-step batch DAGs)

DataViews carry inline `source` definitions (`{type:'pg_query', sql:...}` etc.) that bypass these three concepts entirely. The result: duplicated specs across DataViews, no lineage layer, CDC capability tied specifically to the QuerySource entity (pipelines can't stream), and term overload (`data_source` and "Source" mean different things).

This plan unifies the data layer around the two clean concepts described in the primer:

- **Source** — addressable layer DataViews bind to. Six kinds. All explicit rows in a new `sources` table.
- **Pipeline** — independent batch multi-step DAGs that populate `duckdb_table` Sources.

CDC moves from a bolted-on QuerySource feature to a first-class `cdc_pg` Source kind, backed by a new `cdc` crate in `rust-shared-utils` (parallel to `pipeline` and `rcl`). The `data_sources` SQLite table is renamed to `connections` to free up the term.

The migration is **strangler-style**: new tables added alongside, data migrated, callers flipped, old tables dropped at the end. Each phase is shippable in isolation.

## Locked-in design decisions

| Area | Decision |
|---|---|
| Phasing | Strangler — add new alongside, migrate, flip, drop |
| Pipelines | Batch multi-step DAGs only; no CDC; manually triggered |
| CDC | Standalone Source kind `cdc_pg`; self-managing runtime; auto-resume on boot; PG-only today |
| Source storage | All explicit rows in new `sources` table; no implicit/auto-discovery |
| Source kinds | `pg_query`, `bq_query`, `duckdb_query`, `parquet_glob`, `duckdb_table`, `cdc_pg` |
| `duckdb_table` lifecycle | Source row created first; Pipeline targets it by id; pipeline run populates |
| Multi-writer | Allow — multiple pipelines may target the same Source; last writer wins; lineage retains history |
| Source-with-CDC migration | Today's `query_sources` rows → `cdc_pg` Source rows |
| Connection rename | `data_sources` → `connections` (Phase 1) |
| DV binding | Always `{type:'source', config:{source_id, output?}}` |
| DV scope in this plan | Only flip binding shape; columns/sort/contract/dimensions/cascading_filters/viewport untouched |
| Source delete + bound DVs | Block uniformly across all kinds |
| Pipeline ↔ table coupling | Loose — tables outlive pipelines |
| Unpopulated DV | Friendly empty state with "Run X" hints |
| CDC code home | New `rust-shared-utils/cdc` crate |
| V4 (article-selection) | Deferred to a separate effort — wraps as a custom kind later |
| DataView + Filter + ViewPort refinement | Deferred to a separate planning session |

## Phased rollout

### Phase 1 — New tables + Connections rename (backend; no UI flip)

Stand up the new schema. Existing UX keeps working against the old tables.

- `server/src/db/schema.sql` adds:
  - `connections` (same shape as `data_sources`, renamed).
  - `sources` (kind-discriminated columns: `id, display_name, kind, connection_ref, config JSON, target_table, primary_key JSON, cdc_enabled, last_populated_at, status, created_at, updated_at`).
  - `pipeline_source_targets` (many-to-many; which Pipelines target which `duckdb_table` Sources).
- `server/src/db/migrate.rs` (new) populates new tables from old:
  - `connections` from `data_sources`.
  - `cdc_pg` Source rows from `query_sources` (preserving CDC config).
  - `pg_query` / `duckdb_table` / etc. Source rows from each existing `dataviews.source` inline value, preserving the new id for Phase 3.
- `rust-shared-utils/cdc` (new crate) lifted from `server/src/cdc/`. Smartstudio depends on it via git path. Provides `Streamer`, `LsnState`, `ChangeApplier` traits/types; smartstudio binds `cdc_pg` Source config to the streamer.
- No UI changes. No handler swaps.

### Phase 2 — Sources tab + handlers (UI flip for Sources only)

New Sources tab now reads from `sources`. DataViews and Pipelines unchanged.

- `server/src/handlers/sources.rs` (new): CRUD, materialize, start/stop CDC, status, delete-with-block.
- `src/components/workspace/SourcesWorkspace.tsx` (new) replaces `QuerySourceWorkspace.tsx`. Form supports all six kinds; for `pg_query`, surfaces a "Materialize + keep live with CDC" toggle that creates a `cdc_pg` row.
- `Sidebar` "Sources" tab points at the new API.
- Connections tab swaps `data_sources` → `connections` API.
- Old QuerySource handler/UI kept in place but unused; flagged for removal in Phase 4.
- DataViews unaffected — still read their inline `source` field.

### Phase 3 — Pipelines target Sources; DataView binding flip

- `pipeline` crate's data-producing steps gain `target_source_id` config. Step's executor validates the target Source exists with `kind='duckdb_table'`.
- Pipeline run records `(pipeline_id, source_id, run_id)` in activity log for lineage; updates Source `last_populated_at` + `status`.
- `dataview_source.rs` read path switches on `source.type === 'source'` → resolve Source → dispatch by Source kind.
- DataView rewrite: for each existing inline `source`, look up the pre-created Source row (from Phase 1 migration) and rewrite to `{type:'source', config:{source_id, output:null}}`.
- Backward compat for one release: still handle old inline shapes if any DataView wasn't migrated.

### Phase 4 — Drop old tables + UI cleanup

- Drop `data_sources`, `query_sources`, `shared_pipelines`.
- Remove `handlers/datasources.rs`, `handlers/query_sources.rs`, `QuerySourceWorkspace.tsx`.
- Remove inline-`source` backward-compat code path.

## Critical files

### New
- `rust-shared-utils/cdc/` — new crate (PG WAL streamer + LSN state, lifted from `server/src/cdc/`).
- `server/src/db/migrate.rs` — phase-by-phase migration.
- `server/src/handlers/sources.rs` — Sources CRUD/materialize/CDC/delete.
- `src/components/workspace/SourcesWorkspace.tsx` — six-kind Source editor.

### Modified
- `server/src/db/schema.sql` — new tables; old dropped at Phase 4.
- `server/Cargo.toml` — `cdc = { git = "...", branch = "develop/dev-v4" }`.
- `server/src/main.rs` — wire Sources routes; CDC autostart loop on boot.
- `server/src/handlers/dataview_source.rs` — Source-id resolution + dispatch.
- `server/src/handlers/dataviews.rs` — accept new binding shape.
- `server/src/handlers/pipeline_v2.rs` — accept `target_source_id` on data-producing steps.
- `server/src/handlers/mod.rs` — register `sources`.
- `src/api/client.ts` — `getSources`, `createSource`, `deleteSource`, `materializeSource`, `start/stopCdcSource`.
- `src/components/Sidebar.tsx` — Sources tab points at new API.
- `src/components/workspace/DataViewWorkspace.tsx` — schema-tab Source picker.

### Removed at Phase 4
- `server/src/handlers/datasources.rs`
- `server/src/handlers/query_sources.rs`
- `src/components/workspace/QuerySourceWorkspace.tsx`
- `data_sources`, `query_sources`, `shared_pipelines` tables.

## Reused utilities

- `pipeline` crate — extended (not re-written) with `target_source_id` step config.
- `rcl` crate — unchanged; resolves Connections via the renamed `connections` table.
- pgwire-replication code in `server/src/cdc/` — lifted to `rust-shared-utils/cdc`, no rewrite.
- DuckDB / parquet read path in `dataview_source.rs` — kept; only the source-resolution prelude changes.
- Default-Connection resolution helper — reused by all kind-specific executors.

## Verification

Each phase ships independently. Smoke tests before proceeding to the next phase.

**Phase 1**
- `cargo build` clean (smartstudio + new `cdc` crate).
- After migration: `connections`, `sources`, `pipeline_source_targets` tables populated; row counts match `data_sources` + `query_sources` + (one per existing inline DV `source`).
- `rust-shared-utils/cdc` unit tests pass (LSN tracking, pgoutput parsing).
- Old UI (Sources tab = QuerySource UX) still functional.

**Phase 2**
- New Sources tab loads migrated rows for all six kinds.
- Create / edit / delete each kind via the new UI.
- `pg_query` + Materialize + CDC creates a `cdc_pg` row; CDC stream starts; restart server → CDC auto-resumes.
- Connections tab uses the renamed `connections` table; existing connections still functional in pipelines.

**Phase 3**
- Create empty `duckdb_table` Source → Source state shows `not_yet_populated`.
- Create Pipeline targeting it → run → Source state flips to `populated`; activity log records lineage.
- Migrated DataView with `source_id` binding renders correctly.
- Delete a Source bound by a DataView → blocked with the listed-DVs error.
- DataView bound to unpopulated Source shows the friendly empty state with run hints.

**Phase 4**
- Build clean after dropping old tables/handlers/UI.
- All DataViews continue to render through the new path.
- No backward-compat fallback code remains.

## Out of scope

- **Article Selection** — separate effort; will wrap as a custom Source kind later. Document at `docs/article-selection.md`.
- **DataView columns / sort / contract / dimensions / cascading_filters / ViewPort refinement** — separate planning session.
- **Pipeline scheduling / cron** — manual run only today.
- **Resume-from-failed-step** — pipeline failure restarts from the beginning.
- **MySQL / BigQuery CDC** — `cdc_pg` only today; new kinds when needed.
- **Multi-tenant Source sharing** — single tenant per binary, unchanged.

## Follow-ups outside this plan

- Author `docs/article-selection.md` covering the article_selection_v4 in-process materializer (separate doc, separate effort).
- DataView + Filter + ViewPort refinement is its own planning session.
