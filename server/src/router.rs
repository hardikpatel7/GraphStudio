use std::sync::Arc;
use axum::{Router, routing::{get, post, put, delete}};
use crate::{handlers, agent};
use crate::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/identity", get(handlers::identity))

        // Modules
        .route("/modules", get(handlers::modules::list).post(handlers::modules::create))
        .route("/modules/{id}", put(handlers::modules::update).delete(handlers::modules::delete))

        // SubModules
        .route("/modules/{mod_id}/submodules", get(handlers::submodules::list).post(handlers::submodules::create))
        .route("/submodules/{id}", put(handlers::submodules::update).delete(handlers::submodules::delete))

        // Components
        .route("/submodules/{sub_id}/components", get(handlers::components::list).post(handlers::components::create))
        .route("/components/{id}", put(handlers::components::update).delete(handlers::components::delete))

        // DataViews
        .route("/dataviews", get(handlers::dataviews::list).post(handlers::dataviews::create))
        .route("/dataviews/{id}", get(handlers::dataviews::get_one).put(handlers::dataviews::update).delete(handlers::dataviews::delete))
        .route("/dataviews/{id}/introspect-source", post(handlers::dataview_source::introspect_source))
        .route("/dataviews/{id}/data", post(handlers::dataview_source::data))

        // ViewPorts (saved filter+sort lens on a DataView)
        .route("/dataviews/{dv_id}/viewports", get(handlers::viewports::list).post(handlers::viewports::create))
        .route("/viewports/{id}", put(handlers::viewports::update).delete(handlers::viewports::delete))

        // PG query helpers (used by pg_extract step's "Test Query" / "Preview" buttons)
        .route("/pipeline/test-pg-query", post(handlers::pipeline_handler::test_pg_query))
        .route("/pipeline/preview-pg-query", post(handlers::pipeline_handler::preview_pg_query))

        // Snapshots (two-step materialization)
        .route("/dataviews/{dv_id}/snapshots", get(handlers::snapshots::list))
        .route("/dataviews/{dv_id}/snapshots/gcs", post(handlers::snapshots::materialize_gcs))
        .route("/dataviews/{dv_id}/snapshots/local", post(handlers::snapshots::materialize_local))
        .route("/dataviews/{dv_id}/snapshots/direct", post(handlers::snapshots::materialize_direct))
        .route("/dataviews/{dv_id}/snapshots/switch", post(handlers::snapshots::switch_active))
        .route("/query-columns", post(handlers::snapshots::query_columns))

        // Dimensions
        .route("/dimensions", get(handlers::dimensions::list).post(handlers::dimensions::create))
        .route("/dimensions/{id}", put(handlers::dimensions::update).delete(handlers::dimensions::delete))

        // Data Sources (the single connection store — kind=pg/duckdb/bq, config holds creds)
        .route("/connections", get(handlers::datasources::list).post(handlers::datasources::create))
        .route("/connections/{id}", get(handlers::datasources::get_one).put(handlers::datasources::update).delete(handlers::datasources::delete))
        .route("/connections/{id}/clone", post(handlers::datasources::clone_connection))
        .route("/connections/{id}/test", post(handlers::datasources::test_connection))
        .route("/connections/{id}/schemas", get(handlers::datasources::list_schemas))
        .route("/connections/{id}/schemas/{schema}/tables", get(handlers::datasources::list_tables))
        .route("/connections/{id}/schemas/{schema}/routines", get(handlers::datasources::list_routines))
        .route("/connections/{id}/schemas/{schema}/matviews", get(handlers::datasources::list_matviews))
        .route("/connections/{id}/schemas/{schema}/routines/{name}/definition", get(handlers::datasources::routine_definition))
        .route("/connections/{id}/schemas/{schema}/tables/{table}/columns", get(handlers::datasources::list_columns))
        .route("/connections/{id}/dictionary", get(handlers::datasources::dictionary))
        .route("/connections/{id}/run", post(handlers::datasources::execute_query))

        // Derived Tables
        .route("/derived-tables", get(handlers::derived_tables::list).post(handlers::derived_tables::create))
        .route("/derived-tables/{id}", get(handlers::derived_tables::get_one).put(handlers::derived_tables::update).delete(handlers::derived_tables::delete))
        .route("/derived-tables/{id}/materialize", post(handlers::derived_tables::materialize))

        // Shared Pipelines
        .route("/pipelines", get(handlers::shared_pipelines::list).post(handlers::shared_pipelines::create))
        .route("/pipelines/{id}", get(handlers::shared_pipelines::get_one).put(handlers::shared_pipelines::update).delete(handlers::shared_pipelines::delete))
        // Import / export — round-trip a pipeline as a single JSON file.
        .route("/pipelines/import", post(handlers::shared_pipelines::import))
        .route("/pipelines/{id}/export", get(handlers::shared_pipelines::export))
        // ─── shared-pipelines runner: uses the `pipeline` crate from rust-shared-utils.
        .route("/pipelines/{id}/tree-stream", get(handlers::pipeline_v2::run_stream))
        // Cancel the in-flight pipeline run (any pipeline_id — only one runs at a time).
        .route("/pipelines/cancel", post(handlers::pipeline_v2::cancel_run))
        // Read-only snapshot of the in-flight pipeline run (or {} if idle).
        // Polled by the global "running" banner so users see active runs
        // without having to be on the pipeline's workspace.
        .route("/pipelines/active", get(handlers::pipeline_v2::active_run))
        // Bundle export/import — one JSON for many objects across kinds.
        // Lets the user ship dataviews + pipelines + sources + connections
        // (etc.) together in a single document.
        .route("/bundle/inventory", get(handlers::bundle::inventory))
        .route("/bundle/export",    post(handlers::bundle::export))
        .route("/bundle/import",    post(handlers::bundle::import))

        // Sources (unified addressing layer; six kinds — see docs/primer.md §3.2).
        // Replaces query_sources, retired in Phase 4.
        .route("/sources", get(handlers::sources::list).post(handlers::sources::create))
        .route("/sources/{id}", get(handlers::sources::get_one).put(handlers::sources::update).delete(handlers::sources::delete))
        .route("/sources/{id}/materialize", post(handlers::sources::materialize))
        .route("/sources/{id}/cdc/start", post(handlers::sources::cdc_start))
        .route("/sources/{id}/cdc/stop", post(handlers::sources::cdc_stop))

        // Article Selection — V4-style materializer. Runs the rayon-assembly
        // pipeline and writes the result into tenant_data.duckdb::article_selection.
        // The dataview row dv_article_selection (auto-created on first run) reads
        // from there via source = duckdb_table.
        .route("/article-selection/materialize", post(handlers::article_selection::materialize))

        // Graph — article-level reads (RCL Explorer, exception list,
        // brands, aggregate lookups, traversal). Long-term capability;
        // currently backed by `state.legacy_graph`, will be re-pointed
        // at `state.graphs[default_id]` once the metadata engine
        // covers the projection methods.
        .route("/graph/articles/match-product", post(handlers::graph_articles::match_product))
        .route("/graph/articles/resolve-rcl", post(handlers::graph_articles::resolve_rcl))
        .route("/graph/articles/aggregate-at", post(handlers::graph_articles::aggregate_at))
        .route("/graph/articles/article-detail", post(handlers::graph_articles::article_detail))
        .route("/graph/articles/brands", post(handlers::graph_articles::brands_list))
        .route("/graph/articles/memory-stats", post(handlers::graph_articles::memory_stats))
        .route("/graph/articles/exceptions/counts", post(handlers::graph_articles::exceptions_counts))
        .route("/graph/articles/exceptions/list", post(handlers::graph_articles::exceptions_list))
        // Generic graph traversal — every clickable cell in the UI
        // is a `traverse(from, edge) → rows` call. Single primitive,
        // many edges (children, parent, ancestors, articles, stores,
        // brand).
        .route("/graph/articles/traverse", post(handlers::graph_articles::traverse))

        // Cross-filter (graph-backed). Routes through the default graph
        // snapshot named by `[graphs] default_id` in environment.toml.
        .route("/cross-filter", post(handlers::cross_filter::handle_cross_filter))
        .route("/uam/refresh", post(handlers::cross_filter::refresh_uam))

        // Graphs — CRUD + validate + build + stats. CRUD lives over the
        // `graphs` SQLite table; validate runs metadata-only checks.
        // Build reads the spec, runs `graph::build_graph` against
        // the tenant DuckDB, and swaps the result into `state.graphs[id]`.
        // Stats reads back the live snapshot.
        .route("/graphs/parse", post(handlers::graphs::parse_handler))
        .route("/graphs/serialize", post(handlers::graphs::serialize_handler))
        .route("/graphs", get(handlers::graphs::list).post(handlers::graphs::create))
        .route("/graphs/{id}", get(handlers::graphs::get_one)
                                  .put(handlers::graphs::update)
                                  .delete(handlers::graphs::delete))
        .route("/graphs/{id}/validate", post(handlers::graphs::validate_handler))
        .route("/graphs/{id}/build", post(handlers::graphs::build_handler))
        .route("/graphs/{id}/stats", get(handlers::graphs::stats))
        .route("/graphs/{id}/memory-stats", post(handlers::graphs::memory_stats_handler))
        .route("/graphs/{id}/traverse", post(handlers::graphs::traverse_handler))
        .route("/graphs/{id}/node", post(handlers::graphs::node_handler))
        .route("/graphs/{id}/cross-filter", post(handlers::graphs::cross_filter_handler))

        // Feedback queue — MCP-originated capability requests. List + create
        // + status lifecycle (pending / partial / addressed) via PATCH.
        .route("/feedback", get(handlers::feedback::list).post(handlers::feedback::create))
        .route("/feedback/{id}", axum::routing::patch(handlers::feedback::update_status))

        // Activity / Traces (tenant_id is implicit; legacy paths kept the segment but
        // routes now ignore it via state.tenant_id)
        .route("/activity", post(handlers::activity::get_activity))
        .route("/activity/errors", get(handlers::activity::get_errors))
        .route("/activity/pipeline-runs", get(handlers::activity::get_pipeline_runs))
        .route("/activity/settings", get(handlers::activity::get_settings))
        .route("/activity/settings/set", post(handlers::activity::set_setting))
        .route("/activity/follow-up", post(handlers::activity::toggle_follow_up))
        .route("/activity/stream", get(handlers::activity::stream))

        // Saved Queries
        .route("/saved-queries", get(handlers::saved_queries::list).post(handlers::saved_queries::create))
        .route("/saved-queries/{id}", put(handlers::saved_queries::update).delete(handlers::saved_queries::delete))

        // Ingestion
        .route("/ingest/execute", post(handlers::ingest::execute))
        .route("/ingest/methods", post(handlers::ingest::methods))

        // TOML Config Editor
        .route("/config/schema", get(handlers::config_toml::schema))
        .route("/config/files", get(handlers::config_toml::list_files))
        .route("/config/read/{filename}", get(handlers::config_toml::read_file))
        .route("/config/write/{filename}", put(handlers::config_toml::write_file))
        .route("/config/merged", get(handlers::config_toml::merged))
        .route("/config/db-connections", get(handlers::config_toml::db_connections))

        // DuckDB Query Console
        .route("/query", post(handlers::duckdb_query::execute))
        .route("/query/tables", get(handlers::duckdb_query::tables))
        .route("/duckdb/relations", get(handlers::duckdb_query::relations))
        .route("/query/tables/{name}", delete(handlers::duckdb_query::drop_table))

        // Parquet browse
        .route("/parquet/browse", post(handlers::parquet_browse::browse))
        .route("/parquet/materialize", post(handlers::parquet_browse::materialize))

        // Filter Configs
        .route("/filter-configs", get(handlers::filter_configs::list).post(handlers::filter_configs::create))
        .route("/filter-configs/dimension/{dim_ref}", get(handlers::filter_configs::by_dimension))
        .route("/filter-configs/{id}", get(handlers::filter_configs::get_one).put(handlers::filter_configs::update).delete(handlers::filter_configs::delete))
        .route("/filter-configs/{id}/resolve-values", post(handlers::filter_configs::resolve_values))
        .route("/filter-configs/{id}/resolve", post(handlers::filter_configs::resolve_filter))

        // Templates
        .route("/templates", get(handlers::templates::list).post(handlers::templates::create))
        .route("/templates/{id}/clone", post(handlers::templates::clone))

        // Language Packs
        .route("/language-packs", get(handlers::language_packs::list))

        // Code Generation
        .route("/generate/dataview/{dv_id}/preview", post(handlers::generate::preview_dataview))
        .route("/generate/dataview/{dv_id}/write", post(handlers::generate::write_dataview))
        .route("/generate/cargo", post(handlers::generate::run_cargo))
        .route("/generate/cargo/stop", post(handlers::generate::stop_cargo))

        // Agent (planner) — workspaces, sessions, prompts (SSE), usage,
        // models, pricing. See `crate::agent::routes`.
        .merge(agent::router())

        .with_state(state)
}
