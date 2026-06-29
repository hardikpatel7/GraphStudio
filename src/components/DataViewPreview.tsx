import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";
import {
  ArrowUpDown, ArrowUp, ArrowDown, X, Filter,
  ChevronLeft, ChevronRight, RefreshCw, AlertCircle,
  Loader2, Database,
} from "lucide-react";

/// Column names that get a tiny "rcl" link rendered next to their
/// value. Click → opens the InspectorPanel with the per-row RCL trace
/// for that product. Matched by lowercase name; both `product_code`
/// and `article` work because either resolves the article node in the
/// V8 graph.
const RCL_LOOKUP_COLUMNS = new Set(["product_code", "article"]);

/// Map a column name (lowercase) to the graph-node `kind` it
/// represents. Cells whose column name is in this map render as
/// clickable links — clicking traverses the graph one step down
/// (children) and opens the result in the InspectorPanel.
///
/// Columns NOT in this map render as plain text.
type GraphLinkKind = "L0" | "L1" | "L2" | "L3" | "L4" | "L5" | "ARTICLE" | "PRODUCT_CODE" | "CHANNEL" | "STORE_CODE" | "BRAND";

const GRAPH_LINK_COLUMNS: Record<string, GraphLinkKind> = {
  l0_name: "L0",
  l1_name: "L1",
  l2_name: "L2",
  l3_name: "L3",
  l4_name: "L4",
  l5_name: "L5",
  article: "ARTICLE",
  product_code: "PRODUCT_CODE",
  channel: "CHANNEL",
  store_code: "STORE_CODE",
  brand: "BRAND",
};

/// Default edge for a click on a cell of the given kind. Picks the
/// most useful "down" traversal for each kind:
///   - hierarchy levels go to their immediate children
///   - article goes to product_codes (children)
///   - product_code goes to its parent article
///   - brand/channel cross-edge to articles
///   - store_code goes to parent (channel)
function defaultEdgeFor(kind: GraphLinkKind): "children" | "parent" | "articles" | "stores" {
  switch (kind) {
    case "L0":
    case "L1":
    case "L2":
    case "L3":
    case "L4":
    case "L5":
      return "children";
    case "ARTICLE":
      return "children"; // → product_codes
    case "PRODUCT_CODE":
      return "parent";   // → article
    case "BRAND":
      return "articles"; // → all articles with this brand
    case "CHANNEL":
      return "stores";   // → store_codes under channel
    case "STORE_CODE":
      return "parent";   // → channel
  }
}

/* ===================================================
   DataView Preview

   Always shows data from the configured data source (DuckDB/parquet or PG).
   The UI doesn't know or care how the data source was populated --
   it could be from a pipeline (PG->parquet), mock data seed, or external load.
   =================================================== */

interface Props {
  dataview: any;
  compact?: boolean;
  /** Shared cache for dimension distinct values (persists across DataView switches) */
  distinctCache?: Record<string, string[]>;
  onDistinctFetched?: (col: string, values: string[]) => void;
}

function distinctVals(data: Record<string, any>[], col: string): string[] {
  const s = new Set<string>();
  for (const r of data) { const v = r[col]; if (v != null && v !== "") s.add(String(v)); }
  return [...s].sort();
}

export function DataViewPreview({ dataview, compact, distinctCache: sharedCache, onDistinctFetched }: Props) {
  const allCols: any[] = dataview.columns || [];
  const visCols = allCols.filter((c: any) => c.visible !== false);
  const openInspector = useWorkspaceStore((s) => s.openInspector);

  // -- State --
  const [dataSources, setDataSources] = useState<any[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<Record<string, any>[] | null>(null);
  const [total, setTotal] = useState(0);
  const [autoLoaded, setAutoLoaded] = useState(false);

  // -- Filter configurations bound to this DataView (one per applicable dimension) --
  const [filterConfigs, setFilterConfigs] = useState<any[]>([]);

  // -- Pivot picker (graph sources only). The dataview.source
  // field stores a binding (`{type:"source", config:{source_id}}`), so
  // we need to resolve through the sources table to know the underlying
  // kind. Once we have the source row, we treat `kind === "graph"`
  // as the trigger to render the picker.
  const [resolvedSourceKind, setResolvedSourceKind] = useState<string | null>(null);
  const [resolvedSourceCfg, setResolvedSourceCfg] = useState<any>(null);
  useEffect(() => {
    let cancelled = false;
    const sourceId =
      dataview?.source?.type === "source"
        ? (dataview.source.config?.source_id as string | undefined)
        : null;
    if (!sourceId) {
      setResolvedSourceKind(null);
      setResolvedSourceCfg(null);
      return;
    }
    api.getSource(sourceId)
      .then((row: any) => {
        if (cancelled) return;
        setResolvedSourceKind(row?.kind ?? null);
        setResolvedSourceCfg(row?.config ?? null);
      })
      .catch(() => {
        if (cancelled) return;
        setResolvedSourceKind(null);
        setResolvedSourceCfg(null);
      });
    return () => {
      cancelled = true;
    };
  }, [dataview.id, dataview?.source]);
  const isGraphSource = resolvedSourceKind === "graph";
  const [nodeKind, setNodeKind] = useState<string>("ARTICLE");
  useEffect(() => {
    setNodeKind((resolvedSourceCfg?.node_kind as string) || "ARTICLE");
  }, [dataview.id, resolvedSourceCfg]);

  // -- Filters --
  // `filters` holds the pending dropdown selections; `appliedFilters` is what
  // the data fetch has actually been issued with. The Apply button copies
  // pending → applied (and only then does the table refetch).
  const [filters, setFilters] = useState<Record<string, Record<string, string[]>>>({});
  const [appliedFilters, setAppliedFilters] = useState<Record<string, Record<string, string[]>>>({});
  const [expandedFilter, setExpandedFilter] = useState<string | null>(null);
  const [sortCol, setSortCol] = useState<string>(dataview.sort?.default_column || "");
  const [sortDir, setSortDir] = useState<"ASC" | "DESC">((dataview.sort?.default_direction as any) || "DESC");
  const [searchText, setSearchText] = useState("");
  const [colSearches, setColSearches] = useState<Record<string, string>>({});
  const [pageSize, setPageSize] = useState(10);
  const [page, setPage] = useState(1);

  // -- Distinct values cache --
  // Use shared cache from parent if provided, otherwise local
  const [localDistinctCache, setLocalDistinctCache] = useState<Record<string, string[]>>({});
  const distinctCache = sharedCache || localDistinctCache;
  const setDistinctCache = (updater: (prev: Record<string, string[]>) => Record<string, string[]>) => {
    if (onDistinctFetched) {
      // Let parent manage the cache
      const updated = updater(distinctCache);
      for (const [col, vals] of Object.entries(updated)) {
        if (!distinctCache[col]) onDistinctFetched(col, vals);
      }
    } else {
      setLocalDistinctCache(updater);
    }
  };
  const [loadingDistinct, setLoadingDistinct] = useState<string | null>(null);

  // -- Data sources kept around for distinct-value lookups; no longer drives the read path --
  useEffect(() => {
    api.getDataSources().then(setDataSources).catch(() => {});
  }, [dataview.id]);

  // -- Resolve filter configurations bound via dataview.dimensions --
  // dataview.dimensions accepts both legacy ["product","store"] and the new
  // [{dimension_ref, filter_config_id}] shape. Only the new shape can supply
  // a filter config; legacy entries render no filter strip for that dim.
  useEffect(() => {
    let cancelled = false;
    const dims: any[] = Array.isArray(dataview.dimensions) ? dataview.dimensions : [];
    const wantedIds = dims
      .map((d) => (d && typeof d === "object" ? d.filter_config_id : null))
      .filter((x): x is string => typeof x === "string" && x.length > 0);
    if (wantedIds.length === 0) {
      setFilterConfigs([]);
      return;
    }
    api.getFilterConfigs()
      .then((all: any[]) => {
        if (cancelled) return;
        const byId = new Map(all.map((fc) => [String(fc.id), fc]));
        // Preserve dataview.dimensions order so the filter strip mirrors the binding order.
        const ordered = wantedIds
          .map((id) => byId.get(id))
          .filter((fc): fc is any => !!fc);
        setFilterConfigs(ordered);
      })
      .catch(() => setFilterConfigs([]));
    return () => {
      cancelled = true;
    };
  }, [dataview.id, dataview.dimensions]);

  // Flatten filter-config columns into a single ordered list for the strip.
  // Each entry remembers which dimension/filter-config it came from so the
  // distinct-values dropdown is keyed correctly.
  const filterEntries = useMemo(() => {
    const out: {
      dim: string;
      fcId: string;
      col: string;
      display: string;
      order: number;
      singleSelect: boolean;
    }[] = [];
    for (const fc of filterConfigs) {
      const dim = String(fc.dimension_ref || "");
      const cols: any[] = Array.isArray(fc.filter_columns) ? fc.filter_columns : [];
      for (const c of cols) {
        if (!c || typeof c !== "object" || !c.column) continue;
        out.push({
          dim,
          fcId: String(fc.id),
          col: String(c.column),
          display: String(c.display_name || c.column),
          order: typeof c.display_order === "number" ? c.display_order : 999,
          singleSelect: !!c.single_select,
        });
      }
    }
    out.sort((a, b) => {
      if (a.dim !== b.dim) return a.dim.localeCompare(b.dim);
      return a.order - b.order;
    });
    return out;
  }, [filterConfigs]);

  // The dataview's `source` field is now the source of truth — backend
  // /api/dataviews/{id}/data dispatches by source.type. We don't pick a data_source here.
  const hasSource = !!(dataview.source && typeof dataview.source === "object" && dataview.source.type);
  const duckDs = dataSources.find((d) => d.type === "duckdb" || d.type === "parquet" || d.type === "pg" || d.type === "postgres");
  const pgDs   = dataSources.find((d) => d.type === "pg" || d.type === "postgres");
  const ds = duckDs || pgDs; // used only by the legacy distinct-value lookups below

  // -- Fetch data via the source-driven /data endpoint --
  // The backend dispatches based on dataview.source.type (duckdb_table / parquet_glob /
  // duckdb_query / pg_query / bq_query). Filters/search are not yet wired through
  // server-side (backend /data accepts only limit/offset/sort) — they apply nowhere for
  // the moment. Tracked as a follow-up.
  // Flatten APPLIED dropdown selections into the cross_filter wire shape:
  // [{attribute_name, values, operator: "in"}]. The dimension key is
  // dropped — the resolver looks attributes up by name across the whole
  // graph.
  const filtersPayload = useMemo(() => {
    // Each entry now carries its dimension so the backend can classify
    // store-dim vs product-dim filters (needed to route store-filtered
    // metric reads through the asv2_aid_per_store fast path).
    const out: { attribute_name: string; dimension: string; values: string[]; operator: "in" }[] = [];
    for (const dim of Object.keys(appliedFilters)) {
      const cols = appliedFilters[dim] || {};
      for (const col of Object.keys(cols)) {
        const vals = cols[col];
        if (vals && vals.length > 0) {
          out.push({ attribute_name: col, dimension: dim, values: vals, operator: "in" });
        }
      }
    }
    return out;
  }, [appliedFilters]);

  // True when the pending `filters` differ from the applied set — drives
  // the Apply button's enabled state and dirty indicator.
  const filtersDirty = useMemo(
    () => JSON.stringify(filters) !== JSON.stringify(appliedFilters),
    [filters, appliedFilters],
  );

  // Cardinality fingerprint: deps that can change the result-set size
  // (and therefore COUNT(*)). Page and sort_col / sort_dir are excluded
  // — paginating or re-sorting the same query can't change total. When
  // this string is unchanged from the last fetch, we tell the server to
  // skip the count query (`skip_total: true`) and reuse our prior total.
  const cardinalityKey = useMemo(
    () => JSON.stringify({ filters: filtersPayload, colSearches, nodeKind: isGraphSource ? nodeKind : null }),
    [filtersPayload, colSearches, isGraphSource, nodeKind],
  );
  const lastCardinalityKeyRef = useRef<string | null>(null);

  const fetchData = useCallback(async () => {
    if (!hasSource) { setData(null); setError(null); return; }
    setLoading(true);
    setError(null);
    const skip_total = lastCardinalityKeyRef.current === cardinalityKey;
    try {
      const result = await api.getDataViewData(dataview.id, {
        limit: pageSize,
        offset: (page - 1) * pageSize,
        sort_col: sortCol || undefined,
        sort_dir: sortDir,
        filters: filtersPayload,
        node_kind: isGraphSource ? nodeKind : undefined,
        skip_total,
      });
      setData(result.rows || []);
      // When skip_total fires, server returns 0 — keep the prior total.
      if (!skip_total) setTotal(result.total ?? (result.rows?.length || 0));
      lastCardinalityKeyRef.current = cardinalityKey;
    } catch (err: any) {
      setError(err.message || "Failed to load data");
      setData(null);
    }
    setLoading(false);
  }, [dataview.id, hasSource, pageSize, page, sortCol, sortDir, filtersPayload, nodeKind, isGraphSource, cardinalityKey]);

  // -- Auto-load on first mount when source is configured --
  useEffect(() => {
    if (hasSource && !autoLoaded) {
      setAutoLoaded(true);
      fetchData();
    }
  }, [hasSource, autoLoaded, fetchData]);

  // -- Re-fetch on filter/sort/page change --
  // Note: tracks `appliedFilters`, not `filters`. Pending selections in
  // the dropdowns don't trigger a fetch — the Apply button does.
  const appliedFiltersKey = JSON.stringify(appliedFilters);
  useEffect(() => {
    if (data !== null) fetchData();
  }, [sortCol, sortDir, page, appliedFiltersKey, colSearches, nodeKind]);

  // -- Fetch distinct values for filter dropdowns --
  // For graph-backed DataViews the dropdown is filter-aware: each
  // call to /api/cross-filter narrows by the *other* applied filters but
  // self-excludes (we don't filter `l1_name` distincts by the current
  // `l1_name` selection — that would collapse the dropdown to whatever's
  // already picked). Picking l0_name=WOMEN'S → the l1_name dropdown
  // shrinks to only WOMEN'S L1s.
  //
  // For non-graph sources we fall back to scanning the loaded page —
  // imperfect, but no dimension/distinct endpoint exists for DuckDB / PG
  // sources today.
  //
  // Cache shape: graph results are keyed by `${col}@${fingerprint}` where
  // the fingerprint encodes the relevant filter context. The shared cache
  // (used by parents) keeps the simple `col → values` shape for
  // backward-compat — only the unfiltered domain lands there. Filter-aware
  // results live in a private `graphDistinctCache` that's invalidated when
  // applied filters change.
  const [graphDistinctCache, setGraphDistinctCache] = useState<Record<string, string[]>>({});
  const fetchDistinct = useCallback(async (col: string, dimRef?: string) => {
    if (isGraphSource && dimRef) {
      // Self-exclusion: drop any filter on this column so the dropdown
      // shows the full set of values consistent with the *other* filters.
      const otherFilters = filtersPayload.filter((f) => f.attribute_name !== col);
      const fp = JSON.stringify(otherFilters);
      const cacheKey = `${col}@${fp}`;
      if (graphDistinctCache[cacheKey]) {
        // Still warm the simple cache so non-filter-aware consumers see it.
        if (!distinctCache[col] && otherFilters.length === 0) {
          setDistinctCache((p) => ({ ...p, [col]: graphDistinctCache[cacheKey] }));
        }
        return;
      }
      setLoadingDistinct(col);
      try {
        const r = await fetch("/api/cross-filter", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            attributes: [{ attribute_name: col, dimension: dimRef }],
            filters: otherFilters,
          }),
        });
        if (r.ok) {
          const body = await r.json();
          const vals: string[] = (body?.data?.[col] || []).map(String).sort();
          setGraphDistinctCache((p) => ({ ...p, [cacheKey]: vals }));
          // Mirror the unfiltered domain into the simple cache so any
          // shared-cache consumer (parent components) can reuse it.
          if (otherFilters.length === 0) {
            setDistinctCache((p) => ({ ...p, [col]: vals }));
          }
          return;
        }
        // Fall through to client-side distinct on non-200 response so the
        // dropdown still has *something* rather than showing empty.
      } finally {
        setLoadingDistinct(null);
      }
    }
    if (distinctCache[col]) return;
    setLoadingDistinct(col);
    try {
      if (data) setDistinctCache((p) => ({ ...p, [col]: distinctVals(data, col) }));
    } finally {
      setLoadingDistinct(null);
    }
  }, [isGraphSource, data, distinctCache, filtersPayload, graphDistinctCache]);

  // -- Effective columns (from live result or config) --
  const effectiveCols = useMemo(() => {
    if (data?.length) {
      return Object.keys(data[0]).map((k) => allCols.find((c: any) => c.name === k) || { name: k, type: "VARCHAR", display_name: k, visible: true });
    }
    return visCols;
  }, [data, visCols, allCols]);

  const totalRows = total;
  const totalPages = Math.max(1, Math.ceil(totalRows / pageSize));

  // -- Helpers --
  const setFilterVal = useCallback((dim: string, col: string, vals: string[]) => {
    setFilters((p) => ({ ...p, [dim]: { ...p[dim], [col]: vals } }));
    setPage(1);
  }, []);
  const clearAll = () => {
    setFilters({});
    setAppliedFilters({});
    setColSearches({});
    setSearchText("");
    setPage(1);
  };
  const applyFilters = () => {
    setAppliedFilters(filters);
    setPage(1);
  };
  const activeCount = Object.values(filters).reduce((s, d) => s + Object.values(d).filter((v) => v.length > 0).length, 0) + (searchText ? 1 : 0);
  const handleSort = (c: string) => { if (sortCol === c) setSortDir((d) => d === "ASC" ? "DESC" : "ASC"); else { setSortCol(c); setSortDir("DESC"); } setPage(1); };

  // -- No data source configured --
  if (!ds && dataSources.length === 0 && !loading) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-gray-500">
        <Database size={32} className="text-gray-300 mb-3" />
        <p className="text-sm font-medium">No data available</p>
        <p className="text-xs text-gray-400 mt-1 text-center max-w-xs">
          Run the pipeline in Backend mode to materialize data.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Error indicator (above the table) */}
      {error && (
        <div className="mb-2 px-3 py-1.5 bg-red-50 border border-red-200 rounded-md flex items-center gap-2 shrink-0">
          <AlertCircle size={12} className="text-red-500 shrink-0" />
          <span className="text-xs text-red-600 truncate flex-1">{error}</span>
          <button onClick={() => setError(null)} className="text-[10px] text-red-500 hover:underline shrink-0">Dismiss</button>
        </div>
      )}

      {/* View-as picker — pivots an graph DataView between hierarchy levels. */}
      {isGraphSource && (
        <div className="flex items-center gap-2 bg-white border border-gray-200 rounded-lg px-3 py-2 mb-2 shrink-0">
          <span className="text-[10px] uppercase tracking-wider text-gray-500 font-medium">View as</span>
          <select
            value={nodeKind}
            onChange={(e) => {
              setNodeKind(e.target.value);
              setPage(1);
            }}
            className="text-xs px-2 py-1 rounded-md border border-gray-200 bg-white text-gray-700 focus:outline-none focus:border-blue-400"
            title="Project the same DataView at different graph node levels"
          >
            <option value="L0">L0</option>
            <option value="L1">L1</option>
            <option value="L2">L2</option>
            <option value="L3">L3</option>
            <option value="L4">L4</option>
            <option value="L5">L5</option>
            <option value="ARTICLE">Article</option>
            <option value="PRODUCT_CODE">Product Code</option>
            <option value="CHANNEL">Channel</option>
            <option value="STORE_CODE">Store Code</option>
          </select>
          <span className="text-[10px] text-gray-400 ml-1">
            {totalRows.toLocaleString()} rows
          </span>
        </div>
      )}

      {/* Filters — driven by the bound Filter Configurations (one per applicable dimension). */}
      {filterEntries.length > 0 && (
        <div className="flex items-center gap-2 flex-wrap bg-white border border-gray-200 rounded-lg px-3 py-2 mb-2 shrink-0">
          <Filter size={13} className="text-gray-500 shrink-0" />
          {filterEntries.map((fe, idx) => {
            // Show a thin separator + dim label whenever the dimension changes.
            const prev = idx > 0 ? filterEntries[idx - 1] : null;
            const showDimLabel = !prev || prev.dim !== fe.dim;
            const sel = filters[fe.dim]?.[fe.col] || [];
            const expandedKey = `${fe.dim}::${fe.col}`;
            const expanded = expandedFilter === expandedKey;
            const vals = distinctCache[fe.col] || (data ? distinctVals(data, fe.col) : []);
            return (
              <div key={expandedKey} className="flex items-center gap-2">
                {showDimLabel && (
                  <span className="text-[10px] uppercase tracking-wider text-gray-400 font-medium pl-1">
                    {fe.dim}
                  </span>
                )}
                <div className="relative">
                  <button onClick={() => {
                    if (expanded) setExpandedFilter(null);
                    else { setExpandedFilter(expandedKey); if (!distinctCache[fe.col]) fetchDistinct(fe.col, fe.dim); }
                  }}
                    className={`flex items-center gap-1 text-xs px-2 py-1 rounded-md border transition-all ${
                      sel.length > 0 ? "bg-blue-50 text-blue-700 border-blue-200 font-medium"
                        : "bg-gray-50 text-gray-600 border-gray-200 hover:border-gray-300"
                    }`}>
                    {fe.display}
                    {sel.length > 0 && <span className="bg-blue-200 text-blue-700 text-[9px] px-1 rounded-full">{sel.length}</span>}
                  </button>
                  {expanded && (
                    <FilterDropdown
                      values={vals}
                      selected={sel}
                      loading={loadingDistinct === fe.col}
                      label={fe.display}
                      singleSelect={fe.singleSelect}
                      onToggle={(v) => {
                        if (fe.singleSelect) {
                          // Single-select: clicking a value replaces the selection
                          // (or clears it if the same value is clicked again).
                          const next = sel.length === 1 && sel[0] === v ? [] : [v];
                          setFilterVal(fe.dim, fe.col, next);
                          setExpandedFilter(null);
                        } else {
                          setFilterVal(fe.dim, fe.col, sel.includes(v) ? sel.filter((x: string) => x !== v) : [...sel, v]);
                        }
                      }}
                      onSelectAll={() => setFilterVal(fe.dim, fe.col, [...vals])}
                      onClear={() => setFilterVal(fe.dim, fe.col, [])}
                    />
                  )}
                </div>
              </div>
            );
          })}

          <div className="ml-auto flex items-center gap-2">
            {activeCount > 0 && (
              <button onClick={clearAll} className="flex items-center gap-1 text-[10px] text-red-600 hover:underline">
                <X size={10} /> Clear ({activeCount})
              </button>
            )}
            <button
              onClick={applyFilters}
              disabled={!filtersDirty}
              title={filtersDirty ? "Apply pending filter changes" : "No pending filter changes"}
              className={`flex items-center gap-1 text-[11px] px-2 py-1 rounded font-medium transition-colors ${
                filtersDirty
                  ? "bg-blue-600 text-white hover:bg-blue-700"
                  : "bg-gray-100 text-gray-400 cursor-not-allowed"
              }`}
            >
              Apply filters
              {filtersDirty && <span className="ml-1 inline-block w-1.5 h-1.5 rounded-full bg-amber-300" />}
            </button>
          </div>
        </div>
      )}

      {/* Close dropdown */}
      {expandedFilter && <div className="fixed inset-0 z-40" onClick={() => setExpandedFilter(null)} />}

      {/* -- Table -- */}
      <div className="bg-white rounded-lg border border-gray-200 flex-1 flex flex-col overflow-hidden min-h-0">
        <div className="flex-1 overflow-auto">
          <table className="w-full">
            <thead className="sticky top-0 z-10">
              <tr className="bg-gray-50">
                {effectiveCols.map((col: any) => {
                  const sorted = sortCol === col.name;
                  return (
                    <th key={col.name} onClick={col.sortable !== false ? () => handleSort(col.name) : undefined}
                      className={`text-left px-3 py-2 text-[11px] font-semibold uppercase tracking-wider whitespace-nowrap border-b border-gray-200 select-none ${
                        col.sortable !== false ? "cursor-pointer hover:bg-gray-100" : ""} ${sorted ? "text-blue-600 bg-blue-50/50" : "text-gray-600"}`}>
                      <div className="flex items-center gap-1">
                        {col.display_name || col.name}
                        {col.sortable !== false && (sorted ? (sortDir === "ASC" ? <ArrowUp size={10}/> : <ArrowDown size={10}/>) : <ArrowUpDown size={9} className="text-gray-400"/>)}
                      </div>
                    </th>
                  );
                })}
              </tr>
              {/* Per-column search row — only render if any column is searchable */}
              {effectiveCols.some((c: any) => c.searchable) && (
                <tr className="bg-gray-50/50">
                  {effectiveCols.map((col: any) => (
                    <th key={`search-${col.name}`} className="px-1 py-1 border-b border-gray-200">
                      {col.searchable && (
                        <input
                          value={colSearches[col.name] || ""}
                          onChange={(e) => {
                            setColSearches((prev) => ({ ...prev, [col.name]: e.target.value }));
                            setPage(1);
                          }}
                          placeholder="search..."
                          className="w-full px-1.5 py-0.5 text-[10px] font-normal border border-gray-200 rounded bg-white focus:outline-none focus:ring-1 focus:ring-blue-400 focus:border-blue-400"
                        />
                      )}
                    </th>
                  ))}
                </tr>
              )}
            </thead>
            <tbody>
              {(data || []).map((row, ri) => (
                <tr key={ri} className="border-b border-gray-50 hover:bg-blue-50/30">
                  {effectiveCols.map((col: any) => {
                    const colKey = String(col.name).toLowerCase();
                    const cellValue = row[col.name];
                    const hasValue = cellValue != null && cellValue !== "";
                    const showRcl = RCL_LOOKUP_COLUMNS.has(colKey) && hasValue;
                    const traverseKind = GRAPH_LINK_COLUMNS[colKey];
                    const showTraverse = !!traverseKind && hasValue;
                    return (
                      <td
                        key={col.name}
                        className={`px-3 py-1.5 text-sm whitespace-nowrap ${col.editable ? "bg-amber-50/20" : ""}`}
                      >
                        {col.editable ? (
                          <EditableCell value={cellValue} col={col} onChange={(v) => { row[col.name] = v; }} />
                        ) : (
                          <span className="inline-flex items-center gap-2">
                            {showTraverse ? (
                              <button
                                type="button"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  openInspector({
                                    kind: "graph_traverse",
                                    from: { kind: traverseKind, name: String(cellValue) },
                                    edge: defaultEdgeFor(traverseKind),
                                  });
                                }}
                                className="text-left text-blue-600 hover:text-blue-800 hover:underline cursor-pointer"
                                title={`Traverse ${traverseKind} → ${defaultEdgeFor(traverseKind)}`}
                              >
                                {renderCell(cellValue, col)}
                              </button>
                            ) : (
                              renderCell(cellValue, col)
                            )}
                            {showRcl && (
                              <button
                                type="button"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  const v = String(cellValue);
                                  const key =
                                    colKey === "article"
                                      ? { kind: "rcl_trace" as const, article: v }
                                      : { kind: "rcl_trace" as const, product_code: v };
                                  openInspector(key);
                                }}
                                className="text-[11px] font-medium text-indigo-600 hover:text-indigo-800 hover:underline"
                                title="Look up RCL match for this row"
                              >
                                rcl
                              </button>
                            )}
                          </span>
                        )}
                      </td>
                    );
                  })}
                </tr>
              ))}
              {(!data || data.length === 0) && !loading && (
                <tr><td colSpan={effectiveCols.length} className="text-center py-16">
                  {error ? (
                    <div className="text-gray-400 text-sm">Error -- check notifications</div>
                  ) : !ds ? (
                    <div>
                      <Database size={24} className="text-gray-300 mx-auto mb-2" />
                      <div className="text-sm text-gray-500 font-medium">No data available</div>
                      <div className="text-xs text-gray-400 mt-1">Run the pipeline to materialize data</div>
                    </div>
                  ) : !hasSource ? (
                    <div>
                      <div className="text-sm text-gray-500 font-medium">No source configured</div>
                      <div className="text-xs text-gray-400 mt-1">Pick a source kind on the Schema tab.</div>
                    </div>
                  ) : (
                    <div>
                      <div className="text-sm text-gray-500 font-medium">No data</div>
                      <div className="text-xs text-gray-400 mt-1">The data source returned no rows. Populate it by running the pipeline or seeding mock data.</div>
                    </div>
                  )}
                </td></tr>
              )}
              {loading && !data && (
                <tr><td colSpan={effectiveCols.length} className="text-center py-16">
                  <Loader2 size={20} className="text-blue-400 animate-spin mx-auto mb-2" />
                  <div className="text-sm text-gray-500">Loading data...</div>
                </td></tr>
              )}
            </tbody>
          </table>
        </div>

        <div className={`flex items-center justify-between border-t border-gray-100 bg-gray-50/50 shrink-0 ${compact ? "px-3 py-1.5" : "px-4 py-2"}`}>
          <span className="flex items-center gap-2 text-xs text-gray-500">
            {totalRows > 0 ? `${((page-1)*pageSize+1).toLocaleString()} – ${Math.min(page*pageSize, totalRows).toLocaleString()} of ${totalRows.toLocaleString()} rows` : "0 rows"}
            {loading && <Loader2 size={11} className="text-blue-500 animate-spin" />}
          </span>
          <div className="flex items-center gap-2">
            <button onClick={() => { setDistinctCache(() => ({})); fetchData(); }} disabled={loading || !hasSource}
              title="Reload"
              className="p-1 text-gray-500 hover:text-gray-700 disabled:opacity-30 rounded hover:bg-gray-100">
              <RefreshCw size={13} />
            </button>
            <select value={pageSize} onChange={(e) => { setPageSize(Number(e.target.value)); setPage(1); }}
              className="text-[10px] px-1.5 py-1 border border-gray-200 rounded bg-white text-gray-600">
              {[10, 25, 50, 100, 500].map((n) => <option key={n} value={n}>{n} / page</option>)}
            </select>
            <div className="flex items-center gap-1">
              <button onClick={() => setPage((p) => Math.max(1,p-1))} disabled={page<=1} className="p-1 text-gray-500 hover:text-gray-700 disabled:opacity-30 rounded hover:bg-gray-100"><ChevronLeft size={14}/></button>
              <span className="text-xs text-gray-600 px-1">{page} / {totalPages}</span>
              <button onClick={() => setPage((p) => Math.min(totalPages,p+1))} disabled={page>=totalPages} className="p-1 text-gray-500 hover:text-gray-700 disabled:opacity-30 rounded hover:bg-gray-100"><ChevronRight size={14}/></button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/* -- Filter dropdown -- */
function FilterDropdown({ values, selected, loading, label, singleSelect, onToggle, onSelectAll, onClear }: {
  values: string[]; selected: string[]; loading: boolean; label: string;
  singleSelect?: boolean;
  onToggle: (v: string) => void;
  onSelectAll?: () => void;
  onClear: () => void;
}) {
  const [search, setSearch] = useState("");
  const filtered = search ? values.filter((v) => v.toLowerCase().includes(search.toLowerCase())) : values;
  const allSelected = values.length > 0 && selected.length === values.length;
  return (
    <div className="absolute top-full left-0 mt-1 z-50 w-64 bg-white rounded-lg border border-gray-200 shadow-lg overflow-hidden">
      <div className="p-2 border-b border-gray-100">
        <div className="flex items-center justify-between mb-1">
          <span className="text-[10px] font-semibold text-gray-600 truncate">
            {label}{singleSelect ? " · single" : ""}
          </span>
          <span className="text-[10px] text-gray-500 font-medium tabular-nums shrink-0 ml-2">
            {selected.length}/{values.length}
          </span>
        </div>
        {(!singleSelect && onSelectAll && values.length > 0) || selected.length > 0 ? (
          <div className="flex items-center gap-3 mb-1.5">
            {!singleSelect && onSelectAll && !allSelected && values.length > 0 && (
              <button onClick={onSelectAll} className="text-[10px] text-blue-600 hover:underline">Select all</button>
            )}
            {selected.length > 0 && (
              <button onClick={onClear} className="text-[10px] text-blue-600 hover:underline">
                {singleSelect ? "Clear" : "Clear all"}
              </button>
            )}
          </div>
        ) : null}
        <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Type to filter..." autoFocus
          className="w-full px-2 py-1 text-xs border border-gray-200 rounded focus:outline-none focus:ring-1 focus:ring-blue-400" />
      </div>
      <div className="max-h-52 overflow-auto p-1">
        {loading && <div className="flex items-center justify-center py-4 text-xs text-gray-400"><Loader2 size={12} className="animate-spin mr-1.5" /> Loading...</div>}
        {!loading && filtered.length === 0 && <div className="text-center py-3 text-xs text-gray-400">{search ? "No matches" : "No values"}</div>}
        {filtered.filter((v) => selected.includes(v)).map((v) => (
          <button key={v} onClick={() => onToggle(v)} className="w-full text-left px-2 py-1.5 text-xs rounded bg-blue-50 text-blue-700 font-medium mb-0.5">&#10003; {v}</button>
        ))}
        {filtered.filter((v) => !selected.includes(v)).map((v) => (
          <button key={v} onClick={() => onToggle(v)} className="w-full text-left px-2 py-1.5 text-xs rounded text-gray-700 hover:bg-gray-50">{v}</button>
        ))}
      </div>
    </div>
  );
}

/* -- Editable cell -- */
function EditableCell({ value, col, onChange }: { value: any; col: any; onChange: (v: any) => void }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(String(value ?? ""));
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => { if (editing && ref.current) { ref.current.focus(); ref.current.select(); } }, [editing]);
  const commit = () => { setEditing(false); const t = col.type || ""; if (t === "INTEGER") onChange(parseInt(draft)||0); else if (t.startsWith("NUMERIC")) onChange(parseFloat(draft)||0); else onChange(draft); };
  if (editing) return <input ref={ref} value={draft} onChange={(e) => setDraft(e.target.value)} onBlur={commit} onKeyDown={(e) => { if (e.key==="Enter") commit(); if (e.key==="Escape") { setDraft(String(value??"")); setEditing(false); } }}
    className="w-full px-1.5 py-0.5 text-sm border border-blue-400 rounded bg-white outline-none ring-2 ring-blue-200/50 -my-0.5" />;
  return <button onClick={() => { setDraft(String(value??"")); setEditing(true); }} className="text-left w-full group" title="Click to edit">
    <span className="border-b border-dashed border-amber-300 group-hover:border-amber-500 group-hover:bg-amber-50/50">{renderCell(value, {...col, editable: false})}</span>
  </button>;
}

/* -- Cell renderer -- */
function renderCell(val: any, col: any) {
  if (val == null || val === "") return <span className="text-gray-400">&#8212;</span>;
  if (typeof val === "boolean") return <span className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${val ? "bg-green-50 text-green-600" : "bg-gray-100 text-gray-500"}`}>{val ? "Yes" : "No"}</span>;
  if (Array.isArray(val)) return <div className="flex gap-0.5">{val.slice(0,3).map((v,i)=><span key={i} className="text-[10px] bg-gray-100 text-gray-700 px-1 rounded">{String(v)}</span>)}{val.length>3&&<span className="text-[10px] text-gray-500">+{val.length-3}</span>}</div>;
  const n = col.name?.toLowerCase() || "";
  if (n === "status") { const c: Record<string,string> = { Active:"bg-green-50 text-green-700", Inactive:"bg-red-50 text-red-600", Seasonal:"bg-blue-50 text-blue-600", Clearance:"bg-amber-50 text-amber-600", Draft:"bg-gray-100 text-gray-600", "In Progress":"bg-blue-50 text-blue-600", Submitted:"bg-purple-50 text-purple-600", Finalized:"bg-green-50 text-green-700" }; return <span className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${c[String(val)]||"bg-gray-100 text-gray-700"}`}>{String(val)}</span>; }
  if (typeof val === "number") { const t = col.type||""; if (t?.startsWith("NUMERIC")||n.includes("wos")||n==="woc"||n.includes("percent")||n.includes("in_stock")) return <span className="tabular-nums text-gray-800">{val.toFixed(2)}</span>; if (n.includes("revenue")||n.includes("margin")||n.includes("price")) return <span className="tabular-nums text-gray-800">${val.toLocaleString()}</span>; return <span className="tabular-nums text-gray-800">{val.toLocaleString()}</span>; }
  if (typeof val === "string" && val.includes("@")) return <span className="text-xs text-blue-600">{val}</span>;
  if (typeof val === "string" && /^\d{4}-\d{2}-\d{2}/.test(val)) return <span className="text-xs text-gray-700 font-mono">{val}</span>;
  if (col.editable) return <span className="text-gray-800 border-b border-dashed border-amber-300">{String(val)}</span>;
  return <span className="text-gray-800">{String(val)}</span>;
}
