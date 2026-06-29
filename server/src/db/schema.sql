-- Per-tenant SQLite schema. Each running SmartStudio instance owns one tenant
-- (identified by environment.toml), so there's no client_id/app_id keying — every
-- row in this DB belongs to the running tenant by definition.

CREATE TABLE IF NOT EXISTS app_types (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'classic',
    default_environments TEXT NOT NULL DEFAULT '["dev","test","uat","prod"]',
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO app_types (id, display_name, category) VALUES ('inventorysmart', 'InventorySmart', 'classic');

CREATE TABLE IF NOT EXISTS dimensions (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL, master_table TEXT NOT NULL,
    datasource_ref TEXT,
    levels TEXT NOT NULL DEFAULT '[]', additional_filter_cols TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- `data_sources` table removed in Phase 4 of source-unification.
-- The replacement, `connections`, is defined further down (see source-unification block).

CREATE TABLE IF NOT EXISTS modules (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL, route TEXT NOT NULL,
    icon TEXT, permission_key TEXT, sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS dataviews (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL, contract TEXT NOT NULL DEFAULT '{}',
    dimensions TEXT NOT NULL DEFAULT '[]', columns TEXT NOT NULL DEFAULT '[]',
    sort TEXT NOT NULL DEFAULT '{}', backend_workflow TEXT NOT NULL DEFAULT '{}',
    cascading_filters TEXT NOT NULL DEFAULT '[]',
    -- A dataview's "source" defines what query/table backs it. See dataview_source.rs.
    -- Default = pipeline (sink inferred from backend_workflow at read time).
    source TEXT NOT NULL DEFAULT '{"type":"pipeline","config":{}}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS submodules (
    id TEXT PRIMARY KEY,
    module_id TEXT NOT NULL,
    display_name TEXT NOT NULL, tab_label TEXT,
    dataview_refs TEXT NOT NULL DEFAULT '[]', primary_dataview TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (module_id) REFERENCES modules(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS components (
    id TEXT PRIMARY KEY,
    submodule_id TEXT NOT NULL,
    display_name TEXT NOT NULL, tab_label TEXT,
    dataview_refs TEXT NOT NULL DEFAULT '[]', primary_dataview TEXT,
    config TEXT NOT NULL DEFAULT '{}', sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (submodule_id) REFERENCES submodules(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS filter_configs (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL, dimension_ref TEXT NOT NULL,
    filter_columns TEXT NOT NULL DEFAULT '[]', mandatory_columns TEXT NOT NULL DEFAULT '[]',
    cascading_rules TEXT NOT NULL DEFAULT '[]', config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS derived_tables (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    source_query TEXT NOT NULL DEFAULT '',
    source_type TEXT NOT NULL DEFAULT 'pg',
    datasource_id TEXT,
    materialized INTEGER NOT NULL DEFAULT 0,
    output_table_name TEXT,
    output_format TEXT NOT NULL DEFAULT 'pg_table',
    schedule TEXT,
    last_run_at TEXT,
    last_run_status TEXT,
    last_run_message TEXT,
    row_count INTEGER,
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS saved_queries (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    sql_text TEXT NOT NULL DEFAULT '',
    engine TEXT NOT NULL DEFAULT 'duckdb',
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS viewports (
    id TEXT PRIMARY KEY,
    dataview_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    filter_config_ref TEXT,
    filters TEXT NOT NULL DEFAULT '{}',
    sort TEXT NOT NULL DEFAULT '{}',
    page_size INTEGER NOT NULL DEFAULT 100,
    role_filter TEXT NOT NULL DEFAULT '{}',
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (dataview_id) REFERENCES dataviews(id) ON DELETE CASCADE
);

-- `shared_pipelines` and `query_sources` tables removed in Phase 4 of source-unification.
-- Replacements: `pipelines` and `sources` (defined further down).

CREATE TABLE IF NOT EXISTS templates (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL, description TEXT,
    app_snapshot TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ─────────────────────────────────────────────────────────────────────────────
-- Source-unification tables (Phase 1 of strangler migration).
-- See docs/plans/source-unification.md.
--
-- During Phases 1-3 these coexist with their predecessors (data_sources,
-- query_sources, shared_pipelines). The migration logic in db/mod.rs::run_migrations
-- copies rows from old → new on each boot. Old tables are dropped in Phase 4.
-- ─────────────────────────────────────────────────────────────────────────────

-- `connections` — renamed from `data_sources`. Same shape; the new term
-- replaces "data source" (which was overloaded with the addressing-layer
-- concept, now called "Source").
CREATE TABLE IF NOT EXISTS connections (
    id TEXT PRIMARY KEY,
    display_name TEXT,
    type TEXT NOT NULL,                                   -- 'pg' | 'bq' | …
    is_default INTEGER NOT NULL DEFAULT 0,
    config TEXT NOT NULL DEFAULT '{}',                    -- JSON: host, port, user, password, database, …
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- `pipelines` — renamed from `shared_pipelines`.
CREATE TABLE IF NOT EXISTS pipelines (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    pipeline TEXT NOT NULL DEFAULT '[]',                  -- JSON: array of step configs
    -- JSON: pipeline::PipelineTrigger (Phase 2 of misty-hinton).
    -- Default `{"kind":"manual"}` keeps pre-trigger pipelines working unchanged.
    trigger TEXT NOT NULL DEFAULT '{"kind":"manual"}',
    -- pipeline::Placement (Phase 3 of misty-hinton). 'duck_db_only' or
    -- 'duck_db_and_in_memory'. Default keeps existing pipelines unchanged.
    placement TEXT NOT NULL DEFAULT 'duck_db_only',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- `sources` — addressable data layer for DataViews. Kind-discriminated.
-- See docs/primer.md §3.2 for the full description of each kind.
CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN
        ('pg_query', 'bq_query', 'duckdb_query', 'parquet_glob', 'duckdb_table', 'cdc_pg', 'graph', 'ch_query')),
    -- Connection reference (used by pg_query / bq_query / cdc_pg). NULL for the rest.
    connection_ref TEXT,
    -- Kind-specific config JSON. e.g. {"sql": "..."} for pg_query, {"path": "..."}
    -- for parquet_glob, {"upstream_table": "...", "slot": "...", "publication": "..."}
    -- for cdc_pg.
    config TEXT NOT NULL DEFAULT '{}',
    -- DuckDB table name this Source produces or mirrors. Set for `duckdb_table`
    -- (where a Pipeline will populate it) and `cdc_pg` (the live mirror).
    target_table TEXT,
    -- Primary key column(s) for `cdc_pg`. JSON array.
    primary_key TEXT NOT NULL DEFAULT '[]',
    -- Whether CDC streaming should be active for this Source (only meaningful
    -- when kind = 'cdc_pg').
    cdc_enabled INTEGER NOT NULL DEFAULT 0,
    -- Last successful population timestamp (for `duckdb_table` and `cdc_pg`).
    last_populated_at TEXT,
    -- Lifecycle state surfaced in the UI:
    --   'not_yet_populated' | 'populating' | 'populated' | 'failed' | 'streaming'
    status TEXT NOT NULL DEFAULT 'not_yet_populated',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- `pipeline_source_targets` — many-to-many mapping of which Pipelines target
-- which `duckdb_table` Sources as outputs. Lets the UI answer "what produced
-- this table?" and "what tables does this pipeline produce?".
CREATE TABLE IF NOT EXISTS pipeline_source_targets (
    pipeline_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    -- Last run id (link into activity log). NULL until first run.
    last_run_id TEXT,
    last_run_at TEXT,
    last_run_status TEXT,                                 -- 'success' | 'failed' | 'running'
    PRIMARY KEY (pipeline_id, source_id),
    FOREIGN KEY (pipeline_id) REFERENCES pipelines(id) ON DELETE CASCADE,
    FOREIGN KEY (source_id) REFERENCES sources(id) ON DELETE CASCADE
);

-- ─────────────────────────────────────────────────────────────────────────────
-- `graphs` — TOML-defined article-graph specs (Phase 1 of article_graph).
-- Each row is a named graph; `toml_text` is the authoring artifact (humans
-- edit it in deployed envs, UI generates it for fresh graphs). `last_validated_at`
-- and `error_log` are updated by POST /api/graphs/:id/validate and by build-time
-- pre-flight validation.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS graphs (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    toml_text TEXT NOT NULL DEFAULT '',
    last_validated_at TEXT,
    -- JSON array of ValidationIssue. Empty/NULL means last validate run passed.
    error_log TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
