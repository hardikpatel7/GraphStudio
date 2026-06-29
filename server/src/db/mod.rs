use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::sync::Mutex;

/// Thread-safe SQLite database wrapper.
/// Uses a Mutex since rusqlite::Connection is not Send+Sync.
pub struct Database {
    conn: Mutex<Connection>,
    path: String,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap_or(std::path::Path::new(".")))?;
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        // Migrations for existing databases (ALTER TABLE is a no-op if column exists)
        Self::run_migrations(&conn);
        Ok(Database { conn: Mutex::new(conn), path: path.to_string() })
    }

    /// Open a lightweight second connection for background tasks (CDC, etc.).
    /// This is NOT a full Database — just enough for simple execute() calls.
    pub fn clone_for_cdc(&self) -> CdcDb {
        CdcDb { path: self.path.clone() }
    }

    /// Execute a query returning rows as Vec<serde_json::Value>.
    /// JSON text columns are parsed inline.
    pub fn query(&self, sql: &str, params: &[&dyn rusqlite::types::ToSql]) -> Result<Vec<Value>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("lock: {}", e))?;
        let mut stmt = conn.prepare(sql)?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let rows = stmt.query_map(params, |row| {
            let mut obj = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                let val = row_value_to_json(row, i, name);
                obj.insert(name.clone(), val);
            }
            Ok(Value::Object(obj))
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Execute a query returning a single row.
    pub fn query_one(&self, sql: &str, params: &[&dyn rusqlite::types::ToSql]) -> Result<Value> {
        let rows = self.query(sql, params)?;
        rows.into_iter().next().ok_or_else(|| anyhow!("not found"))
    }

    /// Execute an INSERT/UPDATE/DELETE, return rows affected.
    pub fn execute(&self, sql: &str, params: &[&dyn rusqlite::types::ToSql]) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow!("lock: {}", e))?;
        Ok(conn.execute(sql, params)?)
    }

    /// Run safe ALTER TABLE migrations — silently ignores "duplicate column" errors.
    fn run_migrations(conn: &Connection) {
        let alters = [
            "ALTER TABLE query_sources ADD COLUMN table_name TEXT",
            "ALTER TABLE query_sources ADD COLUMN row_count INTEGER",
            "ALTER TABLE query_sources ADD COLUMN materialized_at TEXT",
            "ALTER TABLE query_sources ADD COLUMN cdc_slot TEXT",
            "ALTER TABLE query_sources ADD COLUMN cdc_publication TEXT",
            "ALTER TABLE query_sources ADD COLUMN cdc_lsn TEXT",
            "ALTER TABLE query_sources ADD COLUMN cdc_status TEXT NOT NULL DEFAULT 'idle'",
            "ALTER TABLE data_sources ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE dataviews ADD COLUMN source TEXT NOT NULL DEFAULT '{\"type\":\"pipeline\",\"config\":{}}'",
            // shared_pipelines simplification: drop legacy fields. SQLite ≥ 3.35 supports DROP COLUMN.
            "ALTER TABLE shared_pipelines DROP COLUMN description",
            "ALTER TABLE shared_pipelines DROP COLUMN tags",
            // Phase 2 of misty-hinton: trigger column on pipelines.
            "ALTER TABLE pipelines ADD COLUMN trigger TEXT NOT NULL DEFAULT '{\"kind\":\"manual\"}'",
            // Phase 3 of misty-hinton: placement column on pipelines.
            "ALTER TABLE pipelines ADD COLUMN placement TEXT NOT NULL DEFAULT 'duck_db_only'",
            // Persisted execution mode — the pipeline's saved default
            // for sequence vs parallel. Used as the fallback when
            // /tree-stream is hit without an explicit ?execution=...
            // override and when a parent pipeline calls this one as
            // a `run_pipeline` step.
            "ALTER TABLE pipelines ADD COLUMN execution TEXT NOT NULL DEFAULT 'sequence'",
            // Free-form description. Used to surface intent + caveats
            // (e.g. "production-only — requires PG asv2_* MVs") on the
            // pipeline editor and as a sidebar tooltip.
            "ALTER TABLE pipelines ADD COLUMN description TEXT NOT NULL DEFAULT ''",
        ];
        for sql in &alters {
            // Ignore error (column already exists)
            let _ = conn.execute_batch(sql);
        }

        // Keep `sources.kind` CHECK in sync with ALLOWED_KINDS.
        // SQLite has no ALTER TABLE for CHECK constraints, so the
        // table is recreated when the existing constraint is missing
        // a current kind OR still references a retired one. Idempotent.
        //
        // 2026-05 sweep:
        //   - Renamed `article_graph` → `graph` (the in-memory graph
        //     access mechanism; no longer versioned).
        //   - Dropped `uam_entitlement` and `uam_summary` as kinds —
        //     UAM is a policy layer, not a data-fetch mechanism; rows
        //     get redirected to `duckdb_table` reading from the
        //     materialized `uam_summary` DuckDB table written at every
        //     UAM cold-load.
        if let Ok(needs_migrate) = conn.query_row(
            "SELECT 1 FROM sqlite_master \
             WHERE name='sources' \
               AND (sql LIKE '%article_graph%' \
                    OR sql LIKE '%uam_entitlement%' \
                    OR sql LIKE '%uam_summary%' \
                    OR sql NOT LIKE '%''graph''%' \
                    OR sql NOT LIKE '%''ch_query''%')",
            [],
            |_r| Ok(true),
        ) {
            if needs_migrate {
                let migration = r#"
                    BEGIN;
                    CREATE TABLE sources_new (
                        id TEXT PRIMARY KEY,
                        display_name TEXT NOT NULL,
                        kind TEXT NOT NULL CHECK (kind IN
                            ('pg_query', 'bq_query', 'duckdb_query', 'parquet_glob', 'duckdb_table', 'cdc_pg', 'graph', 'ch_query')),
                        connection_ref TEXT,
                        config TEXT NOT NULL DEFAULT '{}',
                        target_table TEXT,
                        primary_key TEXT NOT NULL DEFAULT '[]',
                        cdc_enabled INTEGER NOT NULL DEFAULT 0,
                        last_populated_at TEXT,
                        status TEXT NOT NULL DEFAULT 'not_yet_populated',
                        created_at TEXT NOT NULL DEFAULT (datetime('now')),
                        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT INTO sources_new
                        (id, display_name, kind, connection_ref, config, target_table,
                         primary_key, cdc_enabled, last_populated_at, status,
                         created_at, updated_at)
                    SELECT id, display_name,
                           CASE
                             WHEN kind = 'article_graph'   THEN 'graph'
                             WHEN kind = 'uam_entitlement' THEN 'duckdb_table'
                             WHEN kind = 'uam_summary'    THEN 'duckdb_table'
                             ELSE kind
                           END,
                           connection_ref, config,
                           CASE
                             WHEN kind IN ('uam_entitlement', 'uam_summary')
                                  THEN 'uam_summary'
                             ELSE target_table
                           END,
                           primary_key, cdc_enabled, last_populated_at, status,
                           created_at, updated_at
                    FROM sources;
                    DROP TABLE sources;
                    ALTER TABLE sources_new RENAME TO sources;
                    COMMIT;
                "#;
                if let Err(e) = conn.execute_batch(migration) {
                    tracing::warn!(error=%e, "failed to migrate sources.kind CHECK");
                }
            }
        }

        // Phase 1 of source-unification: copy rows from old tables into new
        // ones (connections, pipelines, sources). Idempotent — uses
        // INSERT OR IGNORE so re-running is safe. See docs/plans/source-unification.md.
        //
        // DISABLED: this also seeds 5 default pipelines + 2 dimensions on
        // every fresh tenant (pl_v7_extracts / pl_v7_build / pl_build_article_graph
        // / pl_article_selection / pl_asv2_bootstrap). New tenants now start
        // truly empty; existing tenants keep whatever was already seeded.
        // Re-enable if a fresh tenant ever needs the legacy-table migration
        // path (data_sources → connections / shared_pipelines → pipelines /
        // query_sources → sources) — but those legacy tables don't exist on
        // newly bootstrapped DBs anyway, so the migration body is a no-op
        // there.
        // if let Err(e) = run_source_unification_phase1(conn) {
        //     tracing::warn!(error = %e, "Phase 1 source-unification migration encountered errors (non-fatal)");
        // }
    }

    /// Execute multiple statements in a transaction.
    pub fn transaction<F, T>(&self, f: F) -> Result<T>
    where F: FnOnce(&Connection) -> Result<T>
    {
        let mut conn = self.conn.lock().map_err(|e| anyhow!("lock: {}", e))?;
        let tx = conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}

/// Source-unification migrations. Idempotent — runs every boot through Phase 4.
///
/// Phase 1 (still needed for tenants upgraded from pre-unification builds):
///   - data_sources    → connections
///   - shared_pipelines → pipelines
///   - query_sources   → sources rows
///   - dataviews.source inline → sources rows (synthetic `src_dv_<id>`)
///   - dataviews.source field flip to source-id binding (Phase 3)
///
/// Phase 4 (this commit): once every legacy table's data has been migrated
/// (or the table is empty / absent on fresh installs), DROP it. The DROP
/// statements use `IF EXISTS` so fresh installs that never had the legacy
/// tables stay no-op; upgraded installs lose the table on the first boot
/// after Phase 4 deploys.
fn run_source_unification_phase1(conn: &Connection) -> Result<()> {
    // ── Step 1-3: Phase 1 INSERT OR IGNORE migration (kept for upgrade safety) ──

    // 1. connections ← data_sources (only if data_sources still exists).
    let _ = conn.execute_batch(
        "INSERT OR IGNORE INTO connections \
            (id, display_name, type, is_default, config, created_at, updated_at) \
         SELECT id, display_name, type, is_default, config, created_at, updated_at \
         FROM data_sources",
    );

    // 2. pipelines ← shared_pipelines (only if shared_pipelines still exists).
    let _ = conn.execute_batch(
        "INSERT OR IGNORE INTO pipelines \
            (id, display_name, pipeline, created_at, updated_at) \
         SELECT id, display_name, pipeline, created_at, updated_at \
         FROM shared_pipelines",
    );

    // 3. sources ← query_sources (only if query_sources still exists).
    let _ = migrate_query_sources_to_sources(conn);

    // 4. sources ← dataviews.source inline values.
    let _ = migrate_dataview_sources(conn);

    // 5. (Phase 3) flip each dataview.source from inline → source-id binding,
    //    pointing at the synthetic `src_dv_<dataview_id>` row created in step 4.
    let _ = migrate_dataview_source_field(conn);

    // ── Step 6: Phase 4 — drop legacy tables once data is in the new homes. ──
    let _ = conn.execute_batch(
        "DROP TABLE IF EXISTS query_sources;\
         DROP TABLE IF EXISTS data_sources;\
         DROP TABLE IF EXISTS shared_pipelines;",
    );

    // ── Step 7: V4 worked-example seed (Phase 1 of misty-hinton plan).
    //    Idempotent INSERT OR IGNORE — won't clobber operator edits.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO pipelines (id, display_name, pipeline, placement) \
         VALUES (?1, ?2, ?3, 'duck_db_and_in_memory')",
        rusqlite::params!["pl_article_selection", "Article Selection (V6)", PL_ARTICLE_SELECTION_JSON],
    );

    // Phase 3.5 of misty-hinton — one-shot bootstrap that runs the SQL behind
    // each `asv2_*` MV directly against PG (default connection) and lands
    // each result as a DuckDB table. Lets us escape the `mv_asv2_*` MV layer
    // (no GRANT on MVs, no REFRESH cron) — once these tables exist in
    // tenant DuckDB, the assembly can later be flipped to read from DuckDB
    // instead of doing its own PG COPY against the MVs.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO pipelines (id, display_name, pipeline, placement) \
         VALUES (?1, ?2, ?3, 'duck_db_only')",
        rusqlite::params![
            "pl_asv2_bootstrap",
            "asv2_* bootstrap (one-shot)",
            PL_ASV2_BOOTSTRAP_JSON
        ],
    );

    // V7 — flat-extract + DuckDB-join redesign, split into two pipelines:
    //   `pl_v7_extracts`  — pg_extract steps only. Pulls source tables from
    //                       PG into tenant DuckDB. Hybrid: most source tables
    //                       come through raw, but the two huge fact tables
    //                       (article_inventory_dashboard 27M, woc_master 13M)
    //                       are aggregated PG-side via the asv2_txs_metrics +
    //                       asv2_woc bodies.
    //   `pl_v7_build`    — duckdb_query steps + custom_rust assembly. Reads
    //                       raw_* / asv2_* via the `tenant.<table>` ATTACH
    //                       (set up in pipeline_v2 via state.duckdb_path).
    //                       Produces the remaining 6 asv2_* tables locally
    //                       in DuckDB, then runs the assembly.
    //
    // Run extracts whenever PG data has changed. Run build whenever you want
    // a fresh article_selection from current local raw_* / asv2_* state
    // (cheap — local DuckDB joins, no PG round-trip).
    let _ = conn.execute(
        "INSERT OR IGNORE INTO pipelines (id, display_name, pipeline, placement) \
         VALUES (?1, ?2, ?3, 'duck_db_only')",
        rusqlite::params![
            "pl_v7_extracts",
            "V7 — Extracts (raw + PG-aggregated)",
            PL_V7_EXTRACTS_JSON
        ],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO pipelines (id, display_name, pipeline, placement) \
         VALUES (?1, ?2, ?3, 'duck_db_and_in_memory')",
        rusqlite::params![
            "pl_v7_build",
            "V7 — Build asv2_* + Assemble",
            PL_V7_BUILD_JSON
        ],
    );

    // V8 — graph-backed article_selection. Reuses the V7 raw_* / asv2_*
    // tables. Single custom_rust step that builds the in-memory graph
    // and ArcSwaps it onto AppState. No DuckDB write-back — graph IS
    // the materialization. Step config could later select a non-DuckDB
    // source via {"source":"parquet"|"pg"|"bq"} but currently uses the
    // tenant DuckDB unconditionally.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO pipelines (id, display_name, pipeline, placement) \
         VALUES (?1, ?2, ?3, 'duck_db_and_in_memory')",
        rusqlite::params![
            "pl_build_article_graph",
            "Build article graph (in-memory)",
            PL_BUILD_LEGACY_GRAPH_JSON
        ],
    );

    // Default canonical dimensions: every retail tenant has a
    // `product` and a `store` dimension. Seed minimal rows here so
    // the UI's Filter Config workspace + DimensionWorkspace work out
    // of the box. Levels intentionally empty — operators fill them
    // in via the dimension editor (which introspects the master
    // table from PG `information_schema`).
    let _ = conn.execute(
        "INSERT OR IGNORE INTO dimensions (id, display_name, master_table, levels, additional_filter_cols) \
         VALUES (?1, ?2, ?3, '[]', '[]')",
        rusqlite::params![
            "product",
            "Product",
            "global.product_attributes_filter",
        ],
    );
    let _ = conn.execute(
        "INSERT OR IGNORE INTO dimensions (id, display_name, master_table, levels, additional_filter_cols) \
         VALUES (?1, ?2, ?3, '[]', '[]')",
        rusqlite::params![
            "store",
            "Store",
            "global.store_attributes_filter",
        ],
    );

    Ok(())
}

/// Single-step pipeline that invokes the `build_article_graph` Rust
/// assembly. Reads `asv2_*` / `raw_*` tables already populated by
/// `pl_v7_extracts`, builds the legacy graph, ArcSwaps onto AppState.
const PL_BUILD_LEGACY_GRAPH_JSON: &str = r#"[
  {"id":"assemble_article_graph","type":"custom_rust","label":"Build article graph","config":{"assembly_id":"build_article_graph","output_table":"_article_graph_in_memory","source":"duckdb"}}
]"#;

/// Steps for `pl_article_selection`. 8 `pg_extract` steps populate the
/// `asv2_*` MV tables in the pipeline's DuckDB scratch (later phases will
/// drive scoped CDC re-materialization off these), then a single
/// `custom_rust` step invokes the registered `article_selection` assembly
/// to assemble + write the final `article_selection` table.
///
/// 8 PG extracts schema-qualify their FROM clauses to `inventory_smart.asv2_*`
/// so the connection's role-default `search_path` doesn't have to include
/// that schema. They use the default PG connection (no `connection_ref`),
/// which routes to the `is_default=1` row in `connections` (port 5433).
const PL_ARTICLE_SELECTION_JSON: &str = r#"[
  {"id":"extract_asv2_ph_master","type":"pg_extract","label":"asv2_ph_master","config":{"target":"duckdb","table_name":"asv2_ph_master","query":"SELECT ph_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, style_color_description, product_description, sizes, product_codes, product_lifecycle, article_status_tag, brand, channel FROM inventory_smart.asv2_ph_master"}},
  {"id":"extract_asv2_txs_metrics","type":"pg_extract","label":"asv2_txs_metrics","config":{"target":"duckdb","table_name":"asv2_txs_metrics","query":"SELECT ph_code, lw_units, lw_margin, lw_revenue, price, discount, in_stock_perc FROM inventory_smart.asv2_txs_metrics"}},
  {"id":"extract_asv2_inventory","type":"pg_extract","label":"asv2_inventory","config":{"target":"duckdb","table_name":"asv2_inventory","query":"SELECT ph_code, oh, oo, it, reserve_quantity, allocated_units FROM inventory_smart.asv2_inventory"}},
  {"id":"extract_asv2_woc","type":"pg_extract","label":"asv2_woc","config":{"target":"duckdb","table_name":"asv2_woc","query":"SELECT ph_code, woc, avg_max_mod, min_woc, max_woc, woc_mapped_stores_count FROM inventory_smart.asv2_woc"}},
  {"id":"extract_asv2_instock","type":"pg_extract","label":"asv2_instock","config":{"target":"duckdb","table_name":"asv2_instock","query":"SELECT ph_code, in_stock_perc, dc_instock FROM inventory_smart.asv2_instock"}},
  {"id":"extract_asv2_before_alloc","type":"pg_extract","label":"asv2_before_alloc","config":{"target":"duckdb","table_name":"asv2_before_alloc","query":"SELECT ph_code, eaches, packs FROM inventory_smart.asv2_before_alloc"}},
  {"id":"extract_asv2_paf","type":"pg_extract","label":"asv2_paf","config":{"target":"duckdb","table_name":"asv2_paf","query":"SELECT product_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, brand FROM inventory_smart.asv2_paf"}},
  {"id":"extract_asv2_product_dc","type":"pg_extract","label":"asv2_product_dc","config":{"target":"duckdb","table_name":"asv2_product_dc","query":"SELECT product_code, string_agg(dc_code, '|') AS dc_codes FROM inventory_smart.asv2_product_dc GROUP BY product_code"}},
  {"id":"assemble_article_selection","type":"custom_rust","label":"Assemble article_selection","config":{"assembly_id":"article_selection","output_table":"article_selection"}}
]"#;

/// Steps for `pl_asv2_bootstrap`. One pg_extract per MV — each step's `query`
/// is the SELECT body lifted from
/// `server/src/article_selection/migrations/0001_asv2_materialized_views.sql`.
///
/// Each runs against the default PG connection (no `connection_ref`) and
/// lands the result as `asv2_*` in tenant DuckDB. The connecting PG user
/// needs SELECT on the underlying tables (`inventory_smart.ph_master`,
/// `article_inventory_dashboard`, `woc_master`, `sku_dc_*`, `article_instock`,
/// `dc_pack_*`, `global.product_attributes_filter`,
/// `product_mapping_product_dc`, `distribution_centres`,
/// `product_mapping_store_dc`) — which is what `mtp-uat-backend` typically
/// already has. No GRANT on `mv_asv2_*` is needed.
///
/// Indexes are intentionally NOT applied here (DuckDB auto-indexes on PK
/// scans and the shape is already small after aggregation). Add them later
/// if specific lookup paths show up as hot.
const PL_ASV2_BOOTSTRAP_JSON: &str = r#"[
  {"id":"asv2_ph_master","type":"pg_extract","label":"asv2_ph_master","config":{"target":"duckdb","table_name":"asv2_ph_master","query":"SELECT ph.ph_code, ph.article, ph.l0_name, ph.l1_name, ph.l2_name, ph.l3_name, ph.l4_name, ph.l5_name, ph.style_color_description, ph.product_description, ph.sizes, ph.product_codes, ph.product_lifecycle, ph.article_status_tag, ph.brand, ph.channel FROM inventory_smart.ph_master ph WHERE ph.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard)"}},
  {"id":"asv2_txs_metrics","type":"pg_extract","label":"asv2_txs_metrics","config":{"target":"duckdb","table_name":"asv2_txs_metrics","query":"SELECT ph.ph_code, CAST(ROUND(SUM(COALESCE(a.lw_units, 0))) AS INTEGER) AS lw_units, CAST(ROUND(SUM(COALESCE(a.lw_margin, 0))) AS INTEGER) AS lw_margin, CAST(ROUND(SUM(COALESCE(a.lw_revenue, 0))) AS INTEGER) AS lw_revenue, ROUND(COALESCE(SUM(a.lw_revenue) / NULLIF(SUM(a.lw_units), 0), 0)::DECIMAL, 2) AS price, ROUND(COALESCE(SUM(a.msrp * a.discount) / NULLIF(SUM(a.msrp), 0), 0)::DECIMAL, 2) AS discount, ROUND(CASE WHEN COUNT(*) != 0 THEN COUNT(CASE WHEN a.in_stock = 1 THEN 1 END)::FLOAT / COUNT(*) ELSE 0 END::DECIMAL, 4) AS in_stock_perc FROM inventory_smart.ph_master ph JOIN inventory_smart.article_inventory_dashboard a USING (article) GROUP BY ph.ph_code"}},
  {"id":"asv2_inventory","type":"pg_extract","label":"asv2_inventory","config":{"target":"duckdb","table_name":"asv2_inventory","query":"WITH ph_products AS (SELECT ph.ph_code, unnest(ph.product_codes) AS product_code FROM inventory_smart.ph_master ph), product_dc AS (SELECT pp.ph_code, pp.product_code, pmpd.dc_code FROM ph_products pp JOIN global.product_mapping_product_dc pmpd ON pmpd.product_code = pp.product_code AND pmpd.is_active JOIN global.distribution_centres dc ON dc.dc_code = pmpd.dc_code AND dc.is_active AND NOT dc.is_deleted WHERE pmpd.dc_code IN (SELECT dc_code FROM global.product_mapping_store_dc WHERE is_active)), sda AS (SELECT product_code, dc_code, SUM(COALESCE(oh,0)) AS oh, SUM(COALESCE(oo,0)) AS oo, SUM(COALESCE(it,0)) AS it FROM inventory_smart.sku_dc_available_units GROUP BY 1,2), reserv AS (SELECT product_code, dc_code, SUM(COALESCE(quantity,0)) AS quantity FROM inventory_smart.sku_dc_reserved_units GROUP BY 1,2) SELECT pd.ph_code, COALESCE(SUM(s.oh), 0) AS oh, COALESCE(SUM(s.oo), 0) AS oo, COALESCE(SUM(s.it), 0) AS it, COALESCE(SUM(r.quantity), 0) AS reserve_quantity, 0::bigint AS allocated_units FROM product_dc pd LEFT JOIN sda s ON s.product_code = pd.product_code AND s.dc_code = pd.dc_code LEFT JOIN reserv r ON r.product_code = pd.product_code AND r.dc_code = pd.dc_code GROUP BY pd.ph_code"}},
  {"id":"asv2_woc","type":"pg_extract","label":"asv2_woc","config":{"target":"duckdb","table_name":"asv2_woc","query":"SELECT ph.ph_code, ROUND(AVG(wm.woc)::NUMERIC, 2) AS woc, ROUND(AVG(wm.max_mod)::NUMERIC, 2) AS avg_max_mod, ROUND(MIN(wm.woc)::NUMERIC, 2) AS min_woc, ROUND(MAX(wm.woc)::NUMERIC, 2) AS max_woc, COUNT(DISTINCT wm.store_code) AS woc_mapped_stores_count FROM inventory_smart.ph_master ph JOIN inventory_smart.woc_master wm ON wm.l4_name = ph.l4_name WHERE wm.woc IS NOT NULL GROUP BY ph.ph_code"}},
  {"id":"asv2_instock","type":"pg_extract","label":"asv2_instock","config":{"target":"duckdb","table_name":"asv2_instock","query":"SELECT ph.ph_code, ROUND(CASE WHEN SUM(total_count) != 0 THEN SUM(in_stock_count)::FLOAT / SUM(total_count)::FLOAT ELSE 0 END::NUMERIC, 4) AS in_stock_perc, ROUND(CASE WHEN SUM(dc_instock_total_count) != 0 THEN SUM(dc_instock_count)::FLOAT / SUM(dc_instock_total_count)::FLOAT ELSE 0 END::NUMERIC * 100, 2) AS dc_instock FROM inventory_smart.article_instock ai JOIN inventory_smart.ph_master ph USING (article) GROUP BY ph.ph_code"}},
  {"id":"asv2_before_alloc","type":"pg_extract","label":"asv2_before_alloc","config":{"target":"duckdb","table_name":"asv2_before_alloc","query":"SELECT ph.ph_code, COALESCE(SUM(CASE WHEN dpi.pack_type = 'eaches' THEN dpc.units_in_pack * dpi.oh_pack_qty ELSE 0 END), 0) AS eaches, COALESCE(SUM(CASE WHEN dpi.pack_type = 'packs' THEN dpc.units_in_pack * dpi.oh_pack_qty ELSE 0 END), 0) AS packs FROM inventory_smart.ph_master ph JOIN inventory_smart.dc_pack_inventory dpi ON dpi.article = ph.article JOIN inventory_smart.dc_pack_configuration dpc ON dpc.pack_type_id = dpi.pack_type_id AND dpc.article = dpi.article AND dpc.pack_type = dpi.pack_type WHERE dpi.dc_code IN (SELECT dc_code FROM global.product_mapping_store_dc WHERE is_active) GROUP BY ph.ph_code"}},
  {"id":"asv2_paf","type":"pg_extract","label":"asv2_paf","config":{"target":"duckdb","table_name":"asv2_paf","query":"SELECT paf.product_code, paf.article, paf.l0_name, paf.l1_name, paf.l2_name, paf.l3_name, paf.l4_name, paf.l5_name, paf.brand FROM global.product_attributes_filter paf WHERE paf.active = true AND NOT paf.is_deleted AND paf.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard)"}},
  {"id":"asv2_product_dc","type":"pg_extract","label":"asv2_product_dc","config":{"target":"duckdb","table_name":"asv2_product_dc","query":"SELECT DISTINCT pmpd.product_code, pmpd.dc_code FROM global.product_mapping_product_dc pmpd JOIN global.distribution_centres dc ON dc.dc_code = pmpd.dc_code AND dc.is_active AND NOT dc.is_deleted WHERE pmpd.is_active AND pmpd.product_code IN (SELECT unnest(ph.product_codes) FROM inventory_smart.ph_master ph WHERE ph.article IN (SELECT DISTINCT article FROM inventory_smart.article_inventory_dashboard))"}}
]"#;

/// V7 extracts — pulls source tables from PG into tenant DuckDB.
///
/// 16 `pg_extract` steps. Hybrid strategy: most source tables come through
/// raw (small enough that DuckDB-side aggregation later is cheap); the two
/// huge fact tables (article_inventory_dashboard 27M, woc_master 13M) are
/// pre-aggregated PG-side directly into `asv2_txs_metrics` + `asv2_woc`
/// (saves ~99% wire transfer on those paths).
///
/// Runs alone — no derived tables built in this pipeline. Pair with
/// `pl_v7_build` to produce article_selection.
///
/// Run in **parallel** mode for extract concurrency.
const PL_V7_EXTRACTS_JSON: &str = r#"[
  {"id":"raw_ph_master","type":"pg_extract","label":"raw_ph_master","config":{"target":"duckdb","table_name":"raw_ph_master","query":"SELECT ph_code::TEXT AS ph_code, article::TEXT AS article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, style_color_description, product_description, sizes, array_to_string(product_codes, '|') AS product_codes_str, product_lifecycle, article_status_tag, brand, channel FROM inventory_smart.ph_master"}},
  {"id":"raw_woc_by_l4","type":"pg_extract","label":"raw_woc_by_l4 (l4-level aggregates)","config":{"target":"duckdb","table_name":"raw_woc_by_l4","query":"SELECT l4_name::TEXT AS l4_name, ROUND(AVG(woc)::NUMERIC, 2) AS woc, ROUND(AVG(max_mod)::NUMERIC, 2) AS avg_max_mod, ROUND(MIN(woc)::NUMERIC, 2) AS min_woc, ROUND(MAX(woc)::NUMERIC, 2) AS max_woc, COUNT(DISTINCT store_code) AS woc_mapped_stores_count FROM inventory_smart.woc_master WHERE woc IS NOT NULL GROUP BY l4_name"}},
  {"id":"raw_article_instock","type":"pg_extract","label":"raw_article_instock","config":{"target":"duckdb","table_name":"raw_article_instock","query":"SELECT article::TEXT AS article, total_count, in_stock_count, dc_instock_total_count, dc_instock_count FROM inventory_smart.article_instock"}},
  {"id":"raw_dc_pack_inventory","type":"pg_extract","label":"raw_dc_pack_inventory","config":{"target":"duckdb","table_name":"raw_dc_pack_inventory","query":"SELECT article::TEXT AS article, dc_code::TEXT AS dc_code, pack_type_id::TEXT AS pack_type_id, pack_type::TEXT AS pack_type, oh_pack_qty FROM inventory_smart.dc_pack_inventory"}},
  {"id":"raw_dc_pack_configuration","type":"pg_extract","label":"raw_dc_pack_configuration","config":{"target":"duckdb","table_name":"raw_dc_pack_configuration","query":"SELECT pack_type_id::TEXT AS pack_type_id, article::TEXT AS article, pack_type::TEXT AS pack_type, units_in_pack FROM inventory_smart.dc_pack_configuration"}},
  {"id":"raw_sku_dc_available_units","type":"pg_extract","label":"raw_sku_dc_available_units","config":{"target":"duckdb","table_name":"raw_sku_dc_available_units","query":"SELECT product_code::TEXT AS product_code, dc_code::TEXT AS dc_code, size::TEXT AS size, article::TEXT AS article, oh, oo, it FROM inventory_smart.sku_dc_available_units"}},
  {"id":"raw_sku_dc_reserved_units","type":"pg_extract","label":"raw_sku_dc_reserved_units","config":{"target":"duckdb","table_name":"raw_sku_dc_reserved_units","query":"SELECT product_code::TEXT AS product_code, dc_code::TEXT AS dc_code, size::TEXT AS size, article::TEXT AS article, quantity FROM inventory_smart.sku_dc_reserved_units"}},
  {"id":"raw_last_allocated_details","type":"pg_extract","label":"raw_last_allocated_details","config":{"target":"duckdb","table_name":"raw_last_allocated_details","query":"SELECT article::TEXT AS article, updated_at FROM inventory_smart.last_allocated_details"}},
  {"id":"raw_product_profile_master","type":"pg_extract","label":"raw_product_profile_master","config":{"target":"duckdb","table_name":"raw_product_profile_master","query":"SELECT ph_code::TEXT AS ph_code, pp_code::TEXT AS pp_code, name, special_classification FROM inventory_smart.product_profile_master"}},
  {"id":"raw_paf_sizes","type":"pg_extract","label":"raw_paf_sizes","config":{"target":"duckdb","table_name":"raw_paf_sizes","query":"SELECT article::TEXT AS article, size::TEXT AS size, size_name FROM global.product_attributes_filter WHERE active = true AND NOT is_deleted"}},
  {"id":"raw_psa_store_map","type":"pg_extract","label":"raw_psa_store_map (PSA → store)","config":{"target":"duckdb","table_name":"raw_psa_store_map","query":"SELECT psa_code::TEXT AS psa_code, store_code::TEXT AS store_code, l0_name::TEXT AS l0_name, l1_name::TEXT AS l1_name FROM global.product_store_attributes_filter"}},
  {"id":"raw_store_channels","type":"pg_extract","label":"raw_store_channels","config":{"target":"duckdb","table_name":"raw_store_channels","query":"SELECT store_code::TEXT AS store_code, channel::TEXT AS channel, COALESCE(s0_name, '')::TEXT AS s0_name, COALESCE(s1_name, '')::TEXT AS s1_name, COALESCE(s2_name, '')::TEXT AS s2_name, active FROM global.store_attributes_filter"}},
  {"id":"raw_store_master","type":"pg_extract","label":"raw_store_master","config":{"target":"duckdb","table_name":"raw_store_master","query":"SELECT store_code::TEXT AS store_code, active, is_deleted FROM global.store_master"}},
  {"id":"raw_paf_rcl_hash","type":"pg_extract","label":"raw_paf_rcl_hash","config":{"target":"duckdb","table_name":"raw_paf_rcl_hash","query":"SELECT product_code::TEXT AS product_code, rcl_hash->>'16' AS h16, rcl_hash->>'33' AS h33, rcl_hash->>'65538' AS h65538 FROM global.product_attributes_filter WHERE active = true AND NOT is_deleted AND rcl_hash IS NOT NULL"}},
  {"id":"raw_rcl_psm_rule_dim","type":"pg_extract","label":"raw_rcl_psm_rule_dim","config":{"target":"duckdb","table_name":"raw_rcl_psm_rule_dim","query":"SELECT rcl_code, rule_code, rcl_dimension::text AS dim_json FROM global.rcl_product_mapping_product_store_rule WHERE rcl_code IN (SELECT rcl_code FROM global.rcl_master WHERE module_code = 101 AND NOT is_deleted AND validity @> CURRENT_DATE)"}},
  {"id":"raw_rcl_psm_eligibility","type":"pg_extract","label":"raw_rcl_psm_eligibility (12M rows)","config":{"target":"duckdb","table_name":"raw_rcl_psm_eligibility","query":"SELECT rcl_code, rule_code, psa_code::TEXT AS psa_code FROM global.rcl_product_mapping_product_store WHERE rcl_code IN (SELECT rcl_code FROM global.rcl_master WHERE module_code = 101 AND NOT is_deleted AND validity @> CURRENT_DATE) AND (validity IS NULL OR validity @> CURRENT_DATE)"}},
  {"id":"raw_rcl_psm_priorities","type":"pg_extract","label":"raw_rcl_psm_priorities","config":{"target":"duckdb","table_name":"raw_rcl_psm_priorities","query":"SELECT rcl_code, priority FROM global.rcl_master WHERE module_code = 101 AND NOT is_deleted AND validity @> CURRENT_DATE"}},
{"id":"raw_product_dc_mapping","type":"pg_extract","label":"raw_product_dc_mapping","config":{"target":"duckdb","table_name":"raw_product_dc_mapping","query":"SELECT product_code::TEXT AS product_code, dc_code::TEXT AS dc_code FROM global.product_mapping_product_dc WHERE is_active = true"}},
  {"id":"raw_store_dc_mapping","type":"pg_extract","label":"raw_store_dc_mapping","config":{"target":"duckdb","table_name":"raw_store_dc_mapping","query":"SELECT store_code::TEXT AS store_code, dc_code::TEXT AS dc_code FROM global.product_mapping_store_dc WHERE is_active = true"}},
  {"id":"raw_distribution_centres","type":"pg_extract","label":"raw_distribution_centres","config":{"target":"duckdb","table_name":"raw_distribution_centres","query":"SELECT dc_code::TEXT AS dc_code, name FROM global.distribution_centres WHERE is_active = true AND NOT is_deleted"}},
  {"id":"raw_store_groups","type":"pg_extract","label":"raw_store_groups","config":{"target":"duckdb","table_name":"raw_store_groups","query":"SELECT sg_code::TEXT AS sg_code, name, is_deleted FROM global.store_groups WHERE is_deleted = false"}},
  {"id":"raw_store_groups_mapping","type":"pg_extract","label":"raw_store_groups_mapping","config":{"target":"duckdb","table_name":"raw_store_groups_mapping","query":"SELECT sg_code::TEXT AS sg_code, store_code::TEXT AS store_code FROM global.store_groups_mapping"}},
  {"id":"raw_dc_store_policy_user_rule","type":"pg_extract","label":"raw_dc_store_policy_user_rule","config":{"target":"duckdb","table_name":"raw_dc_store_policy_user_rule","query":"SELECT rule_code::TEXT AS rule_code, rule_type::TEXT AS rule_type, values FROM inventory_smart.dc_store_policy_user_rule"}},
  {"id":"raw_aid","type":"pg_extract","label":"raw_aid (per-(article,store) txs facts, partitioned by l1_name)","config":{"target":"duckdb","table_name":"raw_aid","partition_column":"l1_name","partition_values_sql":"SELECT DISTINCT l1_name FROM inventory_smart.ph_master WHERE l1_name IS NOT NULL","query":"SELECT a.article::TEXT AS article, a.store_code::TEXT AS store_code, COALESCE(a.lw_units, 0) AS lw_units, COALESCE(a.lw_margin, 0) AS lw_margin, COALESCE(a.lw_revenue, 0) AS lw_revenue, COALESCE(a.msrp, 0) AS msrp, COALESCE(a.discount, 0) AS discount, COALESCE(a.in_stock, 0) AS in_stock FROM inventory_smart.article_inventory_dashboard a WHERE a.article IN (SELECT article FROM inventory_smart.ph_master WHERE l1_name = {l1_name})"}}
]"#;

/// V7 build — runs against tenant DuckDB directly (no tmp scratch / merge,
/// because all the source tables already live there from `pl_v7_extracts`).
/// References tables by their plain names. The runner detects "no pg_extract
/// steps" → tenant-write mode in `execute_pipeline_run`.
const PL_V7_BUILD_JSON: &str = r#"[
  {"id":"derive_aid_articles","type":"duckdb_query","label":"derive raw_aid_articles","config":{"sql":"CREATE OR REPLACE TABLE raw_aid_articles AS SELECT DISTINCT article FROM raw_aid WHERE article IS NOT NULL"}},
  {"id":"derive_raw_paf","type":"duckdb_query","label":"derive raw_paf (unnest from raw_ph_master)","config":{"sql":"CREATE OR REPLACE TABLE raw_paf AS SELECT unnest(string_split(product_codes_str, '|')) AS product_code, CAST(article AS VARCHAR) AS article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, brand FROM raw_ph_master WHERE article IS NOT NULL AND product_codes_str IS NOT NULL AND product_codes_str <> ''"}},
  {"id":"build_asv2_txs_metrics","type":"duckdb_query","label":"build asv2_txs_metrics (from raw_aid)","config":{"sql":"CREATE OR REPLACE TABLE asv2_txs_metrics AS SELECT CAST(ph.ph_code AS VARCHAR) AS ph_code, CAST(ph.article AS VARCHAR) AS article, CAST(ROUND(SUM(COALESCE(TRY_CAST(a.lw_units AS BIGINT), 0))) AS BIGINT) AS lw_units, CAST(ROUND(SUM(COALESCE(TRY_CAST(a.lw_margin AS BIGINT), 0))) AS BIGINT) AS lw_margin, CAST(ROUND(SUM(COALESCE(TRY_CAST(a.lw_revenue AS BIGINT), 0))) AS BIGINT) AS lw_revenue, ROUND(COALESCE(SUM(TRY_CAST(a.lw_revenue AS DOUBLE)) / NULLIF(SUM(TRY_CAST(a.lw_units AS DOUBLE)), 0), 0), 2) AS price, ROUND(COALESCE(SUM(TRY_CAST(a.msrp AS DOUBLE) * TRY_CAST(a.discount AS DOUBLE)) / NULLIF(SUM(TRY_CAST(a.msrp AS DOUBLE)), 0), 0), 2) AS discount, ROUND(CASE WHEN COUNT(*) != 0 THEN COUNT(CASE WHEN TRY_CAST(a.in_stock AS INTEGER) = 1 THEN 1 END)::DOUBLE / COUNT(*) ELSE 0 END, 4) AS in_stock_perc FROM raw_ph_master ph JOIN raw_aid a ON CAST(ph.article AS VARCHAR) = CAST(a.article AS VARCHAR) GROUP BY ph.ph_code, ph.article"}},
  {"id":"build_asv2_woc","type":"duckdb_query","label":"build asv2_woc (join ph_l4 × raw_woc_by_l4)","config":{"sql":"CREATE OR REPLACE TABLE asv2_woc AS WITH ph_l4 AS (SELECT DISTINCT CAST(ph_code AS VARCHAR) AS ph_code, CAST(l4_name AS VARCHAR) AS l4_name FROM raw_ph_master) SELECT p.ph_code, w.woc, w.avg_max_mod, w.min_woc, w.max_woc, w.woc_mapped_stores_count FROM ph_l4 p JOIN raw_woc_by_l4 w ON CAST(w.l4_name AS VARCHAR) = p.l4_name"}},
  {"id":"build_asv2_ph_master","type":"duckdb_query","label":"build asv2_ph_master","config":{"sql":"CREATE OR REPLACE TABLE asv2_ph_master AS SELECT ph_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, style_color_description, product_description, sizes, product_codes_str AS product_codes, product_lifecycle, article_status_tag, brand, channel FROM raw_ph_master WHERE CAST(article AS VARCHAR) IN (SELECT CAST(article AS VARCHAR) FROM raw_aid_articles)"}},
  {"id":"build_asv2_inventory","type":"duckdb_query","label":"build asv2_inventory","config":{"sql":"CREATE OR REPLACE TABLE asv2_inventory AS WITH ph_products AS (SELECT ph_code, unnest(string_split(product_codes_str, '|')) AS product_code FROM raw_ph_master), product_dc AS (SELECT pp.ph_code, pp.product_code, CAST(pmpd.dc_code AS VARCHAR) AS dc_code FROM ph_products pp JOIN raw_product_dc_mapping pmpd USING (product_code) JOIN raw_distribution_centres dc ON CAST(dc.dc_code AS VARCHAR) = CAST(pmpd.dc_code AS VARCHAR) WHERE CAST(pmpd.dc_code AS VARCHAR) IN (SELECT CAST(dc_code AS VARCHAR) FROM raw_store_dc_mapping)), sda AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, SUM(COALESCE(TRY_CAST(oh AS BIGINT), 0)) AS oh, SUM(COALESCE(TRY_CAST(oo AS BIGINT), 0)) AS oo, SUM(COALESCE(TRY_CAST(it AS BIGINT), 0)) AS it FROM raw_sku_dc_available_units GROUP BY 1,2), reserv AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, SUM(COALESCE(TRY_CAST(quantity AS BIGINT), 0)) AS quantity FROM raw_sku_dc_reserved_units GROUP BY 1,2) SELECT pd.ph_code, COALESCE(SUM(s.oh), 0) AS oh, COALESCE(SUM(s.oo), 0) AS oo, COALESCE(SUM(s.it), 0) AS it, COALESCE(SUM(r.quantity), 0) AS reserve_quantity, 0::BIGINT AS allocated_units FROM product_dc pd LEFT JOIN sda s ON s.product_code = pd.product_code AND s.dc_code = pd.dc_code LEFT JOIN reserv r ON r.product_code = pd.product_code AND r.dc_code = pd.dc_code GROUP BY pd.ph_code"}},
  {"id":"build_asv2_instock","type":"duckdb_query","label":"build asv2_instock","config":{"sql":"CREATE OR REPLACE TABLE asv2_instock AS SELECT ph.ph_code, ROUND(CASE WHEN SUM(TRY_CAST(total_count AS BIGINT)) != 0 THEN SUM(TRY_CAST(in_stock_count AS BIGINT))::FLOAT / SUM(TRY_CAST(total_count AS BIGINT))::FLOAT ELSE 0 END::NUMERIC, 4) AS in_stock_perc, ROUND(CASE WHEN SUM(TRY_CAST(dc_instock_total_count AS BIGINT)) != 0 THEN SUM(TRY_CAST(dc_instock_count AS BIGINT))::FLOAT / SUM(TRY_CAST(dc_instock_total_count AS BIGINT))::FLOAT ELSE 0 END::NUMERIC * 100, 2) AS dc_instock FROM raw_article_instock ai JOIN raw_ph_master ph ON CAST(ai.article AS VARCHAR) = CAST(ph.article AS VARCHAR) GROUP BY ph.ph_code"}},
  {"id":"build_asv2_before_alloc","type":"duckdb_query","label":"build asv2_before_alloc","config":{"sql":"CREATE OR REPLACE TABLE asv2_before_alloc AS SELECT ph.ph_code, COALESCE(SUM(CASE WHEN CAST(dpi.pack_type AS VARCHAR) = 'eaches' THEN TRY_CAST(dpc.units_in_pack AS BIGINT) * TRY_CAST(dpi.oh_pack_qty AS BIGINT) ELSE 0 END), 0) AS eaches, COALESCE(SUM(CASE WHEN CAST(dpi.pack_type AS VARCHAR) = 'packs' THEN TRY_CAST(dpc.units_in_pack AS BIGINT) * TRY_CAST(dpi.oh_pack_qty AS BIGINT) ELSE 0 END), 0) AS packs FROM raw_ph_master ph JOIN raw_dc_pack_inventory dpi ON CAST(dpi.article AS VARCHAR) = CAST(ph.article AS VARCHAR) JOIN raw_dc_pack_configuration dpc ON CAST(dpc.pack_type_id AS VARCHAR) = CAST(dpi.pack_type_id AS VARCHAR) AND CAST(dpc.article AS VARCHAR) = CAST(dpi.article AS VARCHAR) AND CAST(dpc.pack_type AS VARCHAR) = CAST(dpi.pack_type AS VARCHAR) WHERE CAST(dpi.dc_code AS VARCHAR) IN (SELECT CAST(dc_code AS VARCHAR) FROM raw_store_dc_mapping) GROUP BY ph.ph_code"}},
  {"id":"build_asv2_paf","type":"duckdb_query","label":"build asv2_paf","config":{"sql":"CREATE OR REPLACE TABLE asv2_paf AS SELECT product_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, brand FROM raw_paf WHERE CAST(article AS VARCHAR) IN (SELECT CAST(article AS VARCHAR) FROM raw_aid_articles)"}},
  {"id":"build_asv2_product_dc","type":"duckdb_query","label":"build asv2_product_dc","config":{"sql":"CREATE OR REPLACE TABLE asv2_product_dc AS WITH ph_products AS (SELECT DISTINCT unnest(string_split(product_codes_str, '|')) AS product_code FROM raw_ph_master WHERE CAST(article AS VARCHAR) IN (SELECT CAST(article AS VARCHAR) FROM raw_aid_articles)) SELECT CAST(pmpd.product_code AS VARCHAR) AS product_code, string_agg(CAST(pmpd.dc_code AS VARCHAR), '|') AS dc_codes FROM raw_product_dc_mapping pmpd JOIN raw_distribution_centres dc ON CAST(dc.dc_code AS VARCHAR) = CAST(pmpd.dc_code AS VARCHAR) WHERE CAST(pmpd.product_code AS VARCHAR) IN (SELECT product_code FROM ph_products) GROUP BY CAST(pmpd.product_code AS VARCHAR)"}},
  {"id":"build_asv2_inventory_per_size_dc","type":"duckdb_query","label":"build asv2_inventory_per_size_dc (oh_map/rq_map source)","config":{"sql":"CREATE OR REPLACE TABLE asv2_inventory_per_size_dc AS WITH ph_products AS (SELECT CAST(ph_code AS VARCHAR) AS ph_code, unnest(string_split(product_codes_str, '|')) AS product_code FROM raw_ph_master WHERE CAST(article AS VARCHAR) IN (SELECT CAST(article AS VARCHAR) FROM raw_aid_articles)), sda AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, CAST(size AS VARCHAR) AS size, SUM(COALESCE(TRY_CAST(oh AS BIGINT), 0)) AS oh FROM raw_sku_dc_available_units GROUP BY 1,2,3), reserv AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, CAST(size AS VARCHAR) AS size, SUM(COALESCE(TRY_CAST(quantity AS BIGINT), 0)) AS quantity FROM raw_sku_dc_reserved_units GROUP BY 1,2,3) SELECT pp.ph_code, s.size, s.dc_code, SUM(s.oh)::BIGINT AS oh, COALESCE(SUM(r.quantity), 0)::BIGINT AS rq FROM ph_products pp JOIN sda s ON s.product_code = pp.product_code LEFT JOIN reserv r ON r.product_code = pp.product_code AND r.dc_code = s.dc_code AND r.size = s.size WHERE CAST(s.dc_code AS VARCHAR) IN (SELECT CAST(dc_code AS VARCHAR) FROM raw_store_dc_mapping) GROUP BY 1,2,3"}},
  {"id":"build_asv2_store_index","type":"duckdb_query","label":"build asv2_store_index (canonical store ordering)","config":{"sql":"CREATE OR REPLACE TABLE asv2_store_index AS SELECT (row_number() OVER (ORDER BY store_code))::INTEGER - 1 AS idx, store_code FROM (SELECT DISTINCT store_code FROM raw_aid) ORDER BY store_code"}},
  {"id":"build_asv2_aid_per_store","type":"duckdb_query","label":"build asv2_aid_per_store (per-article × position-aligned store arrays)","config":{"sql":"CREATE OR REPLACE TABLE asv2_aid_per_store AS WITH per_store AS (SELECT article, store_code, SUM(COALESCE(TRY_CAST(lw_units AS BIGINT), 0)) AS lw_units, SUM(COALESCE(TRY_CAST(lw_revenue AS BIGINT), 0)) AS lw_revenue, SUM(COALESCE(TRY_CAST(lw_margin AS BIGINT), 0)) AS lw_margin, CAST(MAX(COALESCE(TRY_CAST(in_stock AS INTEGER), 0)) AS TINYINT) AS in_stock FROM raw_aid GROUP BY article, store_code), article_set AS (SELECT DISTINCT article FROM raw_aid), dense AS (SELECT a.article, si.idx, COALESCE(ps.lw_units, 0) AS lw_units, COALESCE(ps.lw_revenue, 0) AS lw_revenue, COALESCE(ps.lw_margin, 0) AS lw_margin, COALESCE(ps.in_stock, CAST(0 AS TINYINT)) AS in_stock FROM article_set a CROSS JOIN asv2_store_index si LEFT JOIN per_store ps ON ps.article = a.article AND ps.store_code = si.store_code) SELECT article, array_agg(lw_units ORDER BY idx) AS lw_units, array_agg(lw_revenue ORDER BY idx) AS lw_revenue, array_agg(lw_margin ORDER BY idx) AS lw_margin, array_agg(in_stock ORDER BY idx) AS in_stock FROM dense GROUP BY article"}},
  {"id":"build_asv2_dc_index","type":"duckdb_query","label":"build asv2_dc_index (canonical DC ordering)","config":{"sql":"CREATE OR REPLACE TABLE asv2_dc_index AS SELECT (row_number() OVER (ORDER BY dc_code))::INTEGER - 1 AS idx, dc_code FROM (SELECT DISTINCT dc_code FROM raw_store_dc_mapping) ORDER BY dc_code"}},
  {"id":"build_asv2_inventory_per_dc","type":"duckdb_query","label":"build asv2_inventory_per_dc (per-article × position-aligned DC arrays)","config":{"sql":"CREATE OR REPLACE TABLE asv2_inventory_per_dc AS WITH ph_products AS (SELECT article, unnest(string_split(product_codes_str, '|')) AS product_code FROM raw_ph_master), sda AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, SUM(COALESCE(TRY_CAST(oh AS BIGINT), 0)) AS oh, SUM(COALESCE(TRY_CAST(oo AS BIGINT), 0)) AS oo, SUM(COALESCE(TRY_CAST(it AS BIGINT), 0)) AS it FROM raw_sku_dc_available_units GROUP BY 1,2), reserv AS (SELECT CAST(product_code AS VARCHAR) AS product_code, CAST(dc_code AS VARCHAR) AS dc_code, SUM(COALESCE(TRY_CAST(quantity AS BIGINT), 0)) AS rq FROM raw_sku_dc_reserved_units GROUP BY 1,2), per_article_dc AS (SELECT pp.article, di.dc_code, SUM(COALESCE(s.oh, 0)) AS oh, SUM(COALESCE(s.oo, 0)) AS oo, SUM(COALESCE(s.it, 0)) AS it, SUM(COALESCE(r.rq, 0)) AS reserve_quantity FROM ph_products pp CROSS JOIN asv2_dc_index di LEFT JOIN sda s ON s.product_code = pp.product_code AND s.dc_code = di.dc_code LEFT JOIN reserv r ON r.product_code = pp.product_code AND r.dc_code = di.dc_code GROUP BY pp.article, di.dc_code), article_set AS (SELECT DISTINCT article FROM raw_ph_master), dense AS (SELECT a.article, di.idx, COALESCE(pad.oh, 0) AS oh, COALESCE(pad.oo, 0) AS oo, COALESCE(pad.it, 0) AS it, COALESCE(pad.reserve_quantity, 0) AS reserve_quantity FROM article_set a CROSS JOIN asv2_dc_index di LEFT JOIN per_article_dc pad ON pad.article = a.article AND pad.dc_code = di.dc_code) SELECT article, array_agg(oh ORDER BY idx) AS oh, array_agg(oo ORDER BY idx) AS oo, array_agg(it ORDER BY idx) AS it, array_agg(reserve_quantity ORDER BY idx) AS reserve_quantity FROM dense GROUP BY article"}},
  {"id":"check_aid_per_store_alignment","type":"duckdb_query","label":"verify asv2_aid_per_store + asv2_inventory_per_dc array lengths","config":{"sql":"CREATE OR REPLACE TABLE _asv2_aid_check AS SELECT (SELECT COUNT(*) FROM asv2_store_index) AS expected_store_len, (SELECT COUNT(*) FROM asv2_dc_index) AS expected_dc_len, (SELECT COUNT(*) FROM asv2_aid_per_store WHERE len(lw_units) != (SELECT COUNT(*) FROM asv2_store_index) OR len(lw_revenue) != len(lw_units) OR len(lw_margin) != len(lw_units) OR len(in_stock) != len(lw_units)) AS aid_misaligned, (SELECT COUNT(*) FROM asv2_inventory_per_dc WHERE len(oh) != (SELECT COUNT(*) FROM asv2_dc_index) OR len(oo) != len(oh) OR len(it) != len(oh) OR len(reserve_quantity) != len(oh)) AS inv_misaligned; SELECT CASE WHEN (SELECT aid_misaligned FROM _asv2_aid_check) > 0 OR (SELECT inv_misaligned FROM _asv2_aid_check) > 0 THEN error('asv2_aid_per_store / asv2_inventory_per_dc: array lengths do not match index tables — pipeline build is corrupt') ELSE 1 END AS ok; DROP TABLE _asv2_aid_check"}},
  {"id":"assemble_article_selection","type":"custom_rust","label":"Assemble article_selection","config":{"assembly_id":"article_selection_v7","output_table":"article_selection"}}
]"#;

/// Read every row in query_sources and insert a corresponding sources row.
fn migrate_query_sources_to_sources(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, display_name, source_type, connection_ref, config, columns, \
                table_name, row_count, materialized_at, \
                cdc_slot, cdc_publication, cdc_lsn, cdc_status, \
                created_at, updated_at \
         FROM query_sources",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,                 // id
            row.get::<_, String>(1)?,                 // display_name
            row.get::<_, Option<String>>(2)?,         // source_type
            row.get::<_, Option<String>>(3)?,         // connection_ref
            row.get::<_, Option<String>>(4)?,         // config (JSON text)
            row.get::<_, Option<String>>(5)?,         // columns (unused in sources)
            row.get::<_, Option<String>>(6)?,         // table_name
            row.get::<_, Option<i64>>(7)?,            // row_count (unused)
            row.get::<_, Option<String>>(8)?,         // materialized_at
            row.get::<_, Option<String>>(9)?,         // cdc_slot
            row.get::<_, Option<String>>(10)?,        // cdc_publication
            row.get::<_, Option<String>>(11)?,        // cdc_lsn
            row.get::<_, String>(12)?,                // cdc_status
            row.get::<_, String>(13)?,                // created_at
            row.get::<_, String>(14)?,                // updated_at
        ))
    })?;

    for row in rows.flatten() {
        let (id, display_name, _source_type, connection_ref, qs_config, _columns,
             table_name, _row_count, materialized_at,
             cdc_slot, cdc_publication, cdc_lsn, cdc_status,
             created_at, updated_at) = row;

        let has_cdc = cdc_slot.is_some() || cdc_status == "running" || cdc_status == "reconnecting";
        let has_table = table_name.as_deref().map(|s| !s.is_empty()).unwrap_or(false);

        let kind = if has_cdc {
            "cdc_pg"
        } else if has_table {
            "duckdb_table"
        } else {
            "pg_query"
        };

        // Build a Source `config` JSON object combining the legacy QuerySource
        // config (sql, schema, table_name) with CDC fields.
        let mut config: serde_json::Value = qs_config
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(slot) = &cdc_slot { config["cdc_slot"] = serde_json::json!(slot); }
        if let Some(pub_) = &cdc_publication { config["cdc_publication"] = serde_json::json!(pub_); }
        if let Some(lsn) = &cdc_lsn { config["cdc_lsn"] = serde_json::json!(lsn); }

        let status = if has_cdc {
            "streaming"
        } else if has_table {
            "populated"
        } else {
            "not_yet_populated"
        };

        let cdc_enabled = if has_cdc { 1i64 } else { 0i64 };

        let _ = conn.execute(
            "INSERT OR IGNORE INTO sources \
                (id, display_name, kind, connection_ref, config, target_table, \
                 primary_key, cdc_enabled, last_populated_at, status, \
                 created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                display_name,
                kind,
                connection_ref,
                config.to_string(),
                table_name,
                "[]",
                cdc_enabled,
                materialized_at,
                status,
                created_at,
                updated_at,
            ],
        );
    }
    Ok(())
}

/// (Phase 3) Rewrite each dataview's inline `source` field to point at the
/// synthetic `src_dv_<dataview_id>` Source row created by
/// `migrate_dataview_sources`. Idempotent — only modifies rows whose `source`
/// isn't already in the `{type:'source', ...}` shape AND for which a matching
/// `src_dv_<id>` row exists.
fn migrate_dataview_source_field(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, source FROM dataviews")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows.flatten() {
        let (dv_id, source_json) = row;
        let source: serde_json::Value = match serde_json::from_str(&source_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = source.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Already in the new shape? Skip.
        if kind == "source" { continue; }

        // Only rewrite kinds we mapped in step 4 (migrate_dataview_sources).
        // Anything else (legacy 'pipeline' or unknown) we leave alone.
        if !matches!(kind, "pg_query" | "bq_query" | "duckdb_query" | "parquet_glob" | "duckdb_table") {
            continue;
        }

        let synthetic_id = format!("src_dv_{}", dv_id);

        // Confirm the synthetic Source row actually exists. If migration
        // step 4 skipped it (e.g., kind was unmapped), don't rewrite.
        let exists: Result<i64> = conn
            .query_row(
                "SELECT 1 FROM sources WHERE id = ?1",
                rusqlite::params![&synthetic_id],
                |r| r.get(0),
            )
            .map_err(|e| anyhow!("{e}"));
        if exists.is_err() { continue; }

        // Rewrite the dataview's source field to the new binding shape.
        let new_source = serde_json::json!({
            "type": "source",
            "config": {
                "source_id": synthetic_id,
                "output": serde_json::Value::Null,
            }
        });
        let _ = conn.execute(
            "UPDATE dataviews SET source = ?1, updated_at = datetime('now') WHERE id = ?2 AND source != ?1",
            rusqlite::params![new_source.to_string(), &dv_id],
        );
    }
    Ok(())
}

/// Snapshot every dataview's inline `source` field into a corresponding row in
/// the new `sources` table. The dataview row itself is left unchanged here
/// (`migrate_dataview_source_field` flips the binding afterwards).
fn migrate_dataview_sources(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id, display_name, source FROM dataviews")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,            // id
            row.get::<_, String>(1)?,            // display_name
            row.get::<_, String>(2)?,            // source (JSON text)
        ))
    })?;

    for row in rows.flatten() {
        let (dv_id, dv_name, source_json) = row;
        let source: serde_json::Value = match serde_json::from_str(&source_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let kind = source.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let cfg = source.get("config").cloned().unwrap_or_else(|| serde_json::json!({}));

        // Skip the legacy 'pipeline' kind — those dataviews used the now-removed
        // backend_workflow path and don't map cleanly to a single Source row.
        // Phase 3 will handle these case-by-case (likely by binding to a
        // duckdb_table Source matching the pipeline's output).
        let mapped_kind = match kind {
            "pg_query" | "bq_query" | "duckdb_query" | "parquet_glob" | "duckdb_table" => kind,
            _ => continue,
        };

        let connection_ref = cfg.get("connection_ref").and_then(|v| v.as_str()).map(String::from);
        let target_table = cfg.get("table_name").and_then(|v| v.as_str()).map(String::from);

        let synthetic_id = format!("src_dv_{}", dv_id);
        let display_name = format!("{} (DV-derived)", dv_name);

        let status = if mapped_kind == "duckdb_table" {
            "populated"  // Optimistic; the table presumably already exists if the DV was working.
        } else {
            "not_yet_populated"  // Live execution kinds don't have a "populated" state.
        };

        let _ = conn.execute(
            "INSERT OR IGNORE INTO sources \
                (id, display_name, kind, connection_ref, config, target_table, \
                 primary_key, cdc_enabled, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                synthetic_id,
                display_name,
                mapped_kind,
                connection_ref,
                cfg.to_string(),
                target_table,
                "[]",
                0i64,
                status,
            ],
        );
    }
    Ok(())
}

/// Known JSON columns that should be parsed from TEXT to JSON.
const JSON_COLS: &[&str] = &[
    "config", "levels", "additional_filter_cols", "contract", "dimensions",
    "columns", "sort", "backend_workflow", "cascading_filters",
    "dataview_refs", "filter_columns", "mandatory_columns",
    "cascading_rules", "app_snapshot", "default_environments",
    "filters", "role_filter", "source",
];

fn row_value_to_json(row: &rusqlite::Row, idx: usize, col_name: &str) -> Value {
    // Try as string first (most common)
    if let Ok(s) = row.get::<_, Option<String>>(idx) {
        match s {
            Some(s) if JSON_COLS.contains(&col_name) => {
                serde_json::from_str(&s).unwrap_or(Value::String(s))
            }
            Some(s) => Value::String(s),
            None => Value::Null,
        }
    } else if let Ok(n) = row.get::<_, Option<i64>>(idx) {
        match n {
            Some(n) => Value::Number(n.into()),
            None => Value::Null,
        }
    } else if let Ok(f) = row.get::<_, Option<f64>>(idx) {
        match f {
            Some(f) => serde_json::json!(f),
            None => Value::Null,
        }
    } else {
        Value::Null
    }
}

/// Lightweight DB handle for background tasks. Opens a new connection per call.
pub struct CdcDb {
    path: String,
}

impl CdcDb {
    pub fn execute(&self, sql: &str, params: &[&dyn rusqlite::types::ToSql]) -> Result<usize> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(conn.execute(sql, params)?)
    }
}
