const BASE_URL = "/api";

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE_URL}${path}`, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error || res.statusText);
  }
  return res.json();
}

export interface Identity {
  id: string;            // tenant_id, e.g. "bealls-inventorysmart-dev"
  client: string;
  app_type: string;
  environment: string;
  display_name: string;
}

export const api = {
  // Tenant identity (single source of truth — replaces clients/apps/tenant nesting)
  getIdentity: () => request<Identity>("/identity"),

  // Modules
  getModules: () => request<any[]>("/modules"),
  createModule: (data: any) => request<any>("/modules", { method: "POST", body: JSON.stringify(data) }),
  updateModule: (id: string, data: any) => request<any>(`/modules/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteModule: (id: string) => request<any>(`/modules/${id}`, { method: "DELETE" }),

  // DataViews
  getDataViews: () => request<any[]>("/dataviews"),
  getDataView: (id: string) => request<any>(`/dataviews/${id}`),
  createDataView: (data: any) => request<any>("/dataviews", { method: "POST", body: JSON.stringify(data) }),
  updateDataView: (id: string, data: any) => request<any>(`/dataviews/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteDataView: (id: string) => request<any>(`/dataviews/${id}`, { method: "DELETE" }),
  introspectDataViewSource: (id: string) =>
    request<{ source: any; columns: { name: string; type: string }[]; engine: string }>(
      `/dataviews/${id}/introspect-source`, { method: "POST" }),
  getDataViewData: (id: string, opts?: {
    limit?: number;
    offset?: number;
    sort_col?: string;
    sort_dir?: "ASC" | "DESC";
    /// Cross-filter selections — one entry per active dropdown.
    /// Backend resolves via `cross_filter::resolver::apply_filters` when
    /// the source is `article_graph`; pg/duckdb paths ignore for now.
    filters?: { attribute_name: string; values: string[]; operator?: "in" }[];
    /// Pivot the DataView at a different graph node level. Only honored
    /// when the source is `article_graph`. Accepted: L0..L5, ARTICLE,
    /// PRODUCT_CODE, CHANNEL, STORE_CODE.
    node_kind?: string;
    /// Phase-1 exception rule narrowing. AND-composes with `filters` —
    /// candidates limited to articles firing any of these rules.
    /// Accepted wire names: stockout / overstock / below_min / reserve_gap
    /// / no_eligible_stores.
    rules?: string[];
    /// Tell the server to skip the COUNT(*) companion query and return
    /// total = 0. Pass `true` when the cardinality cannot have changed
    /// (paginating, re-sorting). Pass `false` (default) when filters,
    /// rules, search terms, or node_kind change.
    skip_total?: boolean;
  }) =>
    request<{ rows: any[]; columns: { name: string }[]; total: number; duration_ms: number; sql: string }>(
      `/dataviews/${id}/data`, { method: "POST", body: JSON.stringify(opts || {}) }),

  // SubModules
  getSubModules: (modId: string) => request<any[]>(`/modules/${modId}/submodules`),
  createSubModule: (modId: string, data: any) => request<any>(`/modules/${modId}/submodules`, { method: "POST", body: JSON.stringify(data) }),
  updateSubModule: (id: string, data: any) => request<any>(`/submodules/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteSubModule: (id: string) => request<any>(`/submodules/${id}`, { method: "DELETE" }),

  // Components
  getComponents: (subId: string) => request<any[]>(`/submodules/${subId}/components`),
  createComponent: (subId: string, data: any) => request<any>(`/submodules/${subId}/components`, { method: "POST", body: JSON.stringify(data) }),
  updateComponent: (id: string, data: any) => request<any>(`/components/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteComponent: (id: string) => request<any>(`/components/${id}`, { method: "DELETE" }),

  // Filter Configs
  getFilterConfigs: () => request<any[]>("/filter-configs"),
  getFilterConfigsByDimension: (dimRef: string) => request<any[]>(`/filter-configs/dimension/${dimRef}`),
  createFilterConfig: (data: any) => request<any>("/filter-configs", { method: "POST", body: JSON.stringify(data) }),
  updateFilterConfig: (id: string, data: any) => request<any>(`/filter-configs/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteFilterConfig: (id: string) => request<any>(`/filter-configs/${id}`, { method: "DELETE" }),
  resolveFilterValues: (id: string, context?: Record<string, string[]>) =>
    request<{ columns: Record<string, string[]> }>(`/filter-configs/${id}/resolve-values`, { method: "POST", body: JSON.stringify({ context: context || {} }) }),
  resolveFilter: (id: string, selections: Record<string, string[]>) =>
    request<{ where_clause: string; cte: string; entity_count: number | null; dimension_ref: string }>(`/filter-configs/${id}/resolve`, { method: "POST", body: JSON.stringify({ selections }) }),

  // Dimensions
  getDimensions: () => request<any[]>("/dimensions"),
  createDimension: (data: any) => request<any>("/dimensions", { method: "POST", body: JSON.stringify(data) }),
  updateDimension: (id: string, data: any) => request<any>(`/dimensions/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteDimension: (id: string) => request<any>(`/dimensions/${id}`, { method: "DELETE" }),

  // Data Sources (the single connection store — kind=pg/duckdb/bq, config holds creds)
  getDataSources: () => request<any[]>("/connections"),
  getDataSource: (id: string) => request<any>(`/connections/${id}`),
  createDataSource: (data: any) => request<any>("/connections", { method: "POST", body: JSON.stringify(data) }),
  cloneDataSource: (id: string, data: { id: string; display_name?: string }) =>
    request<any>(`/connections/${id}/clone`, { method: "POST", body: JSON.stringify(data) }),
  updateDataSource: (id: string, data: any) => request<any>(`/connections/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteDataSource: (id: string) => request<any>(`/connections/${id}`, { method: "DELETE" }),
  testDataSource: (id: string) => request<any>(`/connections/${id}/test`, { method: "POST" }),
  getSchemas: (dsId: string) => request<string[]>(`/connections/${dsId}/schemas`),
  getTables: (dsId: string, schema: string) => request<any[]>(`/connections/${dsId}/schemas/${schema}/tables`),
  getColumns: (dsId: string, schema: string, table: string) => request<any[]>(`/connections/${dsId}/schemas/${schema}/tables/${table}/columns`),
  getRoutines: (dsId: string, schema: string) => request<any[]>(`/connections/${dsId}/schemas/${schema}/routines`),
  getRoutineDefinition: (dsId: string, schema: string, name: string) => request<any>(`/connections/${dsId}/schemas/${schema}/routines/${name}/definition`),
  getMatViews: (dsId: string, schema: string) => request<any[]>(`/connections/${dsId}/schemas/${schema}/matviews`),
  getDistinctValues: (dsId: string, data: any) => request<any[]>(`/connections/${dsId}/distinct-values`, { method: "POST", body: JSON.stringify(data) }),
  previewQuery: (dsId: string, data: any) => request<any>(`/connections/${dsId}/preview`, { method: "POST", body: JSON.stringify(data) }),
  executeQuery: (dsId: string, data: { sql: string; engine?: string; limit?: number; offset?: number; sort?: any; filters?: any; duckdb_file?: string }) => {
    const payload = JSON.stringify(data);
    const hex = Array.from(new TextEncoder().encode(payload)).map(b => b.toString(16).padStart(2, "0")).join("");
    return request<any>(`/connections/${dsId}/run`, { method: "POST", body: JSON.stringify({ p: hex }) });
  },
  seedMockData: (dsId: string, data: { dataview_id: string; table_name?: string; row_count?: number }) =>
    request<any>(`/connections/${dsId}/seed-mock`, { method: "POST", body: JSON.stringify(data) }),

  // Derived Tables
  getDerivedTables: () => request<any[]>("/derived-tables"),
  createDerivedTable: (data: any) => request<any>("/derived-tables", { method: "POST", body: JSON.stringify(data) }),
  updateDerivedTable: (id: string, data: any) => request<any>(`/derived-tables/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteDerivedTable: (id: string) => request<any>(`/derived-tables/${id}`, { method: "DELETE" }),
  materializeDerivedTable: (id: string) => request<any>(`/derived-tables/${id}/materialize`, { method: "POST" }),

  // Shared Pipelines
  getSharedPipelines: () => request<any[]>("/pipelines"),
  getSharedPipeline: (id: string) => request<any>(`/pipelines/${id}`),
  createSharedPipeline: (data: any) => request<any>("/pipelines", { method: "POST", body: JSON.stringify(data) }),
  updateSharedPipeline: (id: string, data: any) => request<any>(`/pipelines/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteSharedPipeline: (id: string) => request<any>(`/pipelines/${id}`, { method: "DELETE" }),
  /// Triggers a browser download of the pipeline's JSON. Server returns
  /// `Content-Disposition: attachment` so the file picker fires from a
  /// regular `<a>` click rather than going through the JSON `request()`
  /// helper.
  exportSharedPipelineUrl: (id: string) => `/api/pipelines/${id}/export`,
  /// Snapshot of the in-flight pipeline run, or `{}` when idle.
  /// Polled by the global running-pipeline banner.
  getActivePipelineRun: () => request<{ pipeline_id?: string; ran_for_ms?: number }>("/pipelines/active"),
  /// Cancel the active pipeline run. Returns `cancelling` on success,
  /// 409 when no run is active.
  cancelPipelineRun: () => request<any>("/pipelines/cancel", { method: "POST" }),

  // Bundle export/import — one JSON for many objects across kinds
  // (dataviews, pipelines, connections, sources, dimensions,
  //  filter_configs, saved_queries).
  /// List every object across every supported kind so the picker UI
  /// renders without 7 separate fetches. Returns `{ kind: [{id, display_name}] }`.
  getBundleInventory: () => request<Record<string, { id: string; display_name: string }[]>>("/bundle/inventory"),
  /// URL for the export download. POST to it with body
  /// `{ kinds: { kind: [ids] } }` to get a downloadable JSON.
  exportBundleUrl: () => `/api/bundle/export`,
  /// Import a bundle. `mode = "new"` auto-suffixes clashing ids,
  /// `mode = "replace"` overwrites existing rows by id.
  importBundle: (data: any, mode: "new" | "replace") =>
    request<any>("/bundle/import", {
      method: "POST",
      body: JSON.stringify({ data, mode }),
    }),
  /// Import (or replace) a pipeline from a JSON document.
  ///
  /// `mode = "new"`     — create with a fresh id (use `target_id` to override
  ///                      the auto-suffixed default).
  /// `mode = "replace"` — overwrite the existing row at `target_id` (or
  ///                      `data.id` if `target_id` is omitted).
  importSharedPipeline: (
    data: any,
    mode: "new" | "replace",
    target_id?: string,
  ) =>
    request<any>("/pipelines/import", {
      method: "POST",
      body: JSON.stringify({ data, mode, target_id }),
    }),

  // Sources (unified addressing layer; six kinds — see docs/primer.md §3.2).
  getSources: () => request<any[]>("/sources"),
  getSource: (id: string) => request<any>(`/sources/${id}`),
  createSource: (data: any) => request<any>("/sources", { method: "POST", body: JSON.stringify(data) }),
  updateSource: (id: string, data: any) => request<any>(`/sources/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteSource: (id: string) => request<any>(`/sources/${id}`, { method: "DELETE" }),
  // Materialize / CDC actions (Phase 2b).
  materializeSource: (id: string) => request<any>(`/sources/${id}/materialize`, { method: "POST" }),
  startCdcSource:    (id: string) => request<any>(`/sources/${id}/cdc/start`,    { method: "POST" }),
  stopCdcSource:     (id: string) => request<any>(`/sources/${id}/cdc/stop`,     { method: "POST" }),

  // /api/query-sources removed in Phase 4. Use getSources / materializeSource /
  // startCdcSource / stopCdcSource above.

  // Saved Queries
  getSavedQueries: () => request<any[]>("/saved-queries"),
  saveSavedQuery: (data: any) => request<any>("/saved-queries", { method: "POST", body: JSON.stringify(data) }),
  updateSavedQuery: (id: string, data: any) => request<any>(`/saved-queries/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteSavedQuery: (id: string) => request<any>(`/saved-queries/${id}`, { method: "DELETE" }),

  // Ingestion
  getIngestMethods: (dvId: string) => request<any>("/ingest/methods", { method: "POST", body: JSON.stringify({ dataview_id: dvId }) }),
  executeIngest: (dvId: string, method: string) => request<any>("/ingest/execute", { method: "POST", body: JSON.stringify({ dataview_id: dvId, method }) }),

  // Generated Service (direct call to standalone service)
  generatedServiceCall: (baseUrl: string, endpoint: string, data?: any) =>
    fetch(`${baseUrl}${endpoint}`, { method: data ? "POST" : "GET", headers: { "Content-Type": "application/json" }, ...(data ? { body: JSON.stringify(data) } : {}) }).then(r => r.json()),

  // TOML Config Editor (tenant_id is implicit from server config)
  getConfigSchema: () => request<any>("/config/schema"),
  getConfigFiles: () => request<any>("/config/files"),
  readConfigFile: (filename: string) => request<any>(`/config/read/${filename}`),
  writeConfigFile: (filename: string, content: any) =>
    request<any>(`/config/write/${filename}`, { method: "PUT", body: JSON.stringify({ content }) }),
  getMergedConfig: () => request<any>("/config/merged"),
  getDbConnections: () => request<any>("/config/db-connections"),

  // Parquet browse
  browseParquet: (path: string) => request<any>("/parquet/browse", { method: "POST", body: JSON.stringify({ path }) }),
  materializeDataView: (dataviewId: string) => request<any>("/parquet/materialize", { method: "POST", body: JSON.stringify({ dataview_id: dataviewId }) }),

  // Activity / Traces (tenant implicit)
  getActivity: (opts?: { limit?: number; offset?: number; category?: string; hours_ago?: number; follow_up_only?: boolean }) =>
    request<any>("/activity", { method: "POST", body: JSON.stringify({ limit: opts?.limit || 50, offset: opts?.offset || 0, category: opts?.category, hours_ago: opts?.hours_ago, follow_up_only: opts?.follow_up_only }) }),
  toggleFollowUp: (rowId: number) =>
    request<any>("/activity/follow-up", { method: "POST", body: JSON.stringify({ row_id: rowId }) }),
  getErrors: () => request<any>("/activity/errors"),
  getPipelineRuns: () => request<any>("/activity/pipeline-runs"),
  getEnvSettings: () => request<any>("/activity/settings"),
  setEnvSetting: (key: string, value: string) =>
    request<any>("/activity/settings/set", { method: "POST", body: JSON.stringify({ key, value }) }),

  // ViewPorts
  getViewPorts: (dvId: string) => request<any[]>(`/dataviews/${dvId}/viewports`),
  createViewPort: (dvId: string, data: any) => request<any>(`/dataviews/${dvId}/viewports`, { method: "POST", body: JSON.stringify(data) }),
  updateViewPort: (id: string, data: any) => request<any>(`/viewports/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  deleteViewPort: (id: string) => request<any>(`/viewports/${id}`, { method: "DELETE" }),

  // Snapshots
  getSnapshots: (dvId: string) => request<any>(`/dataviews/${dvId}/snapshots`),
  materializeGcs: (dvId: string) => request<any>(`/dataviews/${dvId}/snapshots/gcs`, { method: "POST" }),
  materializeLocal: (dvId: string) => request<any>(`/dataviews/${dvId}/snapshots/local`, { method: "POST" }),
  materializeDirect: (dvId: string) => request<any>(`/dataviews/${dvId}/snapshots/direct`, { method: "POST" }),
  switchSnapshot: (dvId: string, step: string, snapshotTs: string) =>
    request<any>(`/dataviews/${dvId}/snapshots/switch`, { method: "POST", body: JSON.stringify({ step, snapshot_ts: snapshotTs }) }),
  queryColumns: (data: { type: string; query?: string; sp_name?: string; dataview_id?: string }) =>
    request<{ columns: string[] }>("/query-columns", { method: "POST", body: JSON.stringify(data) }),

  // Templates
  getTemplates: () => request<any[]>("/templates"),
  createTemplate: (data: any) => request<any>("/templates", { method: "POST", body: JSON.stringify(data) }),
  cloneTemplate: (id: string, data: any) => request<any>(`/templates/${id}/clone`, { method: "POST", body: JSON.stringify(data) }),

  // Language Packs
  getLanguagePacks: () => request<any[]>("/language-packs"),

  // Code Generation — per DataView gRPC service
  previewDataViewService: (dvId: string) =>
    request<{ dataview_id: string; files: Record<string, string> }>(`/generate/dataview/${dvId}/preview`, { method: "POST" }),
  writeDataViewService: (dvId: string, outputDir?: string) =>
    request<{ dataview_id: string; output_dir: string; files_written: string[] }>(
      `/generate/dataview/${dvId}/write${outputDir ? `?output_dir=${encodeURIComponent(outputDir)}` : ""}`,
      { method: "POST" },
    ),
  runCargo: (action: "check" | "build" | "run", workingDir: string) =>
    request<{ action: string; success?: boolean; exit_code?: number; stdout?: string; stderr?: string; pid?: number; running?: boolean; working_dir: string; elapsed_ms: number }>(
      "/generate/cargo", { method: "POST", body: JSON.stringify({ action, working_dir: workingDir }) },
    ),
  stopCargo: (pid: number) =>
    request<{ killed: boolean; pid: number }>("/generate/cargo/stop", { method: "POST", body: JSON.stringify({ pid }) }),

  // V8 — article_graph (RCL Explorer + aggregate lookups). Backed by the
  // in-memory ArticleGraph snapshot; pl_build_article_graph must have run at
  // least once or these return 503 FAILED_PRECONDITION.
  articleGraphMatchProduct: (key: { product_code?: string; article?: string }) =>
    request<{ hierarchy?: ArticleGraphHierarchy }>("/graph/articles/match-product", {
      method: "POST",
      body: JSON.stringify(key),
    }),
  articleGraphResolveRcl: (
    key: { product_code?: string; article?: string },
    kinds?: ("dc_policy" | "constraints" | "psm")[],
  ) =>
    request<ArticleGraphResolveRclResponse>("/graph/articles/resolve-rcl", {
      method: "POST",
      body: JSON.stringify({ ...key, kinds: kinds ?? [] }),
    }),
  articleGraphAggregateAt: (kind: ArticleGraphNodeKind, name: string) =>
    request<ArticleGraphAggregateAtResponse>("/graph/articles/aggregate-at", {
      method: "POST",
      body: JSON.stringify({ kind, name }),
    }),

  /// Generic graph traversal. Single primitive — `from` is a (kind,
  /// name) pair, `edge` is the relationship to walk. The same call
  /// powers every clickable cell in the DataView preview, and click
  /// chains (each result row is itself traversable).
  graphTraverse: (
    from: { kind: GraphTraverseKind; name: string },
    edge: GraphTraverseEdge,
    filters?: { attribute_name: string; values: string[]; operator?: "in" }[],
    rules?: string[],
  ) =>
    request<GraphTraverseResponse>("/graph/articles/traverse", {
      method: "POST",
      body: JSON.stringify({
        from,
        edge,
        ...(filters && filters.length > 0 ? { filters } : {}),
        ...(rules && rules.length > 0 ? { rules } : {}),
      }),
    }),

  /// All brands present in the graph, ranked by total OH DESC. Used by
  /// the Detail View's "Brands" pseudo-root in the left tree.
  brandsList: (limit?: number) =>
    request<BrandsListResponse>("/graph/articles/brands", {
      method: "POST",
      body: JSON.stringify(limit ? { limit } : {}),
    }),

  /// Bundled article detail (hierarchy + metrics + RCL trace + sizes +
  /// risk flags). One round-trip for the Detail View's article focus.
  articleDetail: (key: { article?: string; product_code?: string }) =>
    request<any>("/graph/articles/article-detail", {
      method: "POST",
      body: JSON.stringify(key),
    }),

  /// Exception view counts — chip badges. Returns one count per Phase-1
  /// rule, AND-composed with the supplied cross-filter selections.
  exceptionsCounts: (filters?: ExceptionFilter[]) =>
    request<ExceptionCountsResponse>("/graph/articles/exceptions/counts", {
      method: "POST",
      body: JSON.stringify({ filters: filters ?? [] }),
    }),

  /// Exception view list — paginated articles firing any of the
  /// selected rules. Each row carries the same payload as a Live View
  /// row plus a `risk_flags: string[]` tagging which rules fired.
  exceptionsList: (
    rules: string[],
    filters?: ExceptionFilter[],
    opts?: { limit?: number; offset?: number },
  ) =>
    request<ExceptionListResponse>("/graph/articles/exceptions/list", {
      method: "POST",
      body: JSON.stringify({
        rules,
        filters: filters ?? [],
        limit: opts?.limit,
        offset: opts?.offset,
      }),
    }),
};

export interface BrandsListResponse {
  brands: { name: string; article_count: number; oh: number; lw_units: number; lw_revenue: number }[];
  duration_ms: number;
}

export type ExceptionFilter = { attribute_name: string; values: string[]; operator?: "in" };
export interface ExceptionCountsResponse {
  total_articles: number;
  counts: Record<string, number>;
  duration_ms: number;
}
export interface ExceptionListResponse {
  rows: Record<string, any>[];
  total: number;
  duration_ms: number;
}

export type GraphTraverseKind =
  | "L0" | "L1" | "L2" | "L3" | "L4" | "L5"
  | "ARTICLE" | "PRODUCT_CODE" | "CHANNEL" | "STORE_CODE" | "BRAND";

export type GraphTraverseEdge =
  | "children"   // next level down (l0→l1, article→product_codes, channel→stores)
  | "parent"     // immediate parent (1 row)
  | "ancestors"  // all parents up to root
  | "articles"   // subtree articles (l_n) or cross-edge (brand, channel)
  | "stores"     // channel → store_codes
  | "brand";     // article → its brand value (1 row)

export interface GraphTraverseResponse {
  rows: Record<string, any>[];
  total: number;
  duration_ms: number;
}

export type ArticleGraphNodeKind =
  | "L0" | "L1" | "L2" | "L3" | "L4" | "L5"
  | "ARTICLE" | "PRODUCT_CODE" | "CHANNEL" | "STORE_CODE";

export interface ArticleGraphHierarchy {
  product_code: string;
  article: string;
  l0_name: string;
  l1_name: string;
  l2_name: string;
  l3_name: string;
  l4_name: string;
  l5_name: string;
  brand: string;
  channel: string;
}

export interface ArticleGraphResolveRclResponse {
  hierarchy?: ArticleGraphHierarchy;
  dc_policy?: {
    rcl_code: string;
    rule_code: string;
    policy?: {
      default_store_groups: string[];
      default_product_profile: string;
      dc_store_rule: string;
    };
  };
  constraints?: {
    rcl_code: string;
    rule_code: string;
    rows: Array<{
      psa_code: string;
      aps: number;
      wos: number;
      min_stock: number;
      max_stock: number;
    }>;
  };
  psm?: { rcl_code: string; rule_code: string };
  ruleset_version: number;
}

export interface ArticleGraphAggregateAtResponse {
  aggregates?: {
    oh: number;
    oo: number;
    it: number;
    reserve_quantity: number;
    allocated_units: number;
    lw_units: number;
    lw_revenue: number;
    lw_margin: number;
  };
  graph_version: number;
}
