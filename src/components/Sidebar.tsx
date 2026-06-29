import { useEffect, useState, useMemo } from "react";
import { Search, Plus, Star, Trash2, Upload, Loader2 } from "lucide-react";
import { api } from "@/api/client";
import { useDataViewsStore } from "@/stores/dataviews";
import { useWorkspaceStore } from "@/stores/workspace";
import { useActivePipelineRun, formatRunDuration } from "@/hooks/useActivePipelineRun";

function SidebarItem({
  label,
  subtitle,
  active,
  onClick,
  onDelete,
  badge,
}: {
  label: string;
  subtitle?: string;
  active: boolean;
  onClick: () => void;
  /** When provided, a trash icon shows on hover; clicking it calls
   *  this handler instead of selecting the row. The handler is
   *  responsible for confirming with the user and refreshing the list. */
  onDelete?: () => void;
  badge?: React.ReactNode;
}) {
  return (
    <div
      onClick={onClick}
      className={`group flex items-center gap-1.5 px-3 py-1.5 text-sm cursor-pointer ${
        active
          ? "text-blue-400 bg-gray-800/60 border-l-2 border-blue-500"
          : "text-gray-400 hover:text-gray-200 hover:bg-gray-800/50"
      }`}
    >
      <div className="flex-1 min-w-0">
        <div className="truncate">{label}</div>
        {subtitle && (
          <div className="truncate text-[10px] text-gray-500 font-mono">{subtitle}</div>
        )}
      </div>
      {badge}
      {onDelete && (
        <button
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          className="opacity-0 group-hover:opacity-100 transition-opacity text-gray-500 hover:text-red-400"
          title="Delete"
          aria-label="Delete"
        >
          <Trash2 size={12} />
        </button>
      )}
    </div>
  );
}

export default function Sidebar() {
  const { dataviews, fetchDataViews } = useDataViewsStore();
  const activeTab = useWorkspaceStore((s) => s.activeTab);
  const selected = useWorkspaceStore((s) => s.selected);
  const select = useWorkspaceStore((s) => s.select);
  const sidebarSearch = useWorkspaceStore((s) => s.sidebarSearch);
  const setSidebarSearch = useWorkspaceStore((s) => s.setSidebarSearch);

  const [sharedPipelines, setSharedPipelines] = useState<any[]>([]);
  const [dimensions, setDimensions] = useState<any[]>([]);
  const [filterConfigs, setFilterConfigs] = useState<any[]>([]);
  const [connections, setConnections] = useState<any[]>([]);
  const [sources, setSources] = useState<any[]>([]);
  // Graphs are the TOML-defined v2 article-graph specs from the
  // `graphs` SQLite table. List endpoint omits `toml_text` for
  // payload size; the GraphDesigner workspace fetches the body on
  // selection.
  const [graphs, setGraphs] = useState<any[]>([]);
  const [creatingConn, setCreatingConn] = useState(false);
  const [newConnName, setNewConnName] = useState("");
  const [newConnType, setNewConnType] = useState<"pg" | "clickhouse">("pg");
  const [creatingDv, setCreatingDv] = useState(false);
  const [newDvName, setNewDvName] = useState("");
  const [creatingSp, setCreatingSp] = useState(false);
  const [newSpName, setNewSpName] = useState("");
  const [creatingSrc, setCreatingSrc] = useState(false);
  const [newSrcName, setNewSrcName] = useState("");
  const [newSrcKind, setNewSrcKind] = useState("pg_query");
  const [creatingFc, setCreatingFc] = useState(false);
  const [newFcName, setNewFcName] = useState("");
  const [newFcDim, setNewFcDim] = useState("product");
  const [creatingDim, setCreatingDim] = useState(false);
  const [newDimName, setNewDimName] = useState("");
  const [creatingGraph, setCreatingGraph] = useState(false);
  const [newGraphName, setNewGraphName] = useState("");
  const [newDimMasterTable, setNewDimMasterTable] = useState("");

  const reloadConnections = () => {
    api.getDataSources().then(setConnections).catch(() => {});
  };

  useEffect(() => {
    fetchDataViews();
    api.getSharedPipelines().then(setSharedPipelines).catch(() => {});
    api.getDimensions().then(setDimensions).catch(() => {});
    api.getFilterConfigs().then(setFilterConfigs).catch(() => {});
    api.getDataSources().then(setConnections).catch(() => {});
    api.getSources().then(setSources).catch(() => {});
    // Hit /api/graphs directly — not in api/client yet, so use fetch.
    fetch("/api/graphs")
      .then((r) => r.json())
      .then(setGraphs)
      .catch(() => {});
  }, [fetchDataViews, activeTab]);

  const activeRun = useActivePipelineRun();

  // Listen for cross-component mutations (delete from a workspace) and
  // refetch the affected list. Workspaces dispatch one of these on success.
  useEffect(() => {
    const onConnections = () => reloadConnections();
    const onDataViews = () => fetchDataViews();
    const onSources = () => api.getSources().then(setSources).catch(() => {});
    const onGraphs = () => {
      fetch("/api/graphs")
        .then((r) => r.json())
        .then(setGraphs)
        .catch(() => {});
    };
    window.addEventListener("connections-changed", onConnections);
    window.addEventListener("dataviews-changed", onDataViews);
    window.addEventListener("sources-changed", onSources);
    window.addEventListener("graphs-changed", onGraphs);
    return () => {
      window.removeEventListener("connections-changed", onConnections);
      window.removeEventListener("dataviews-changed", onDataViews);
      window.removeEventListener("sources-changed", onSources);
      window.removeEventListener("graphs-changed", onGraphs);
    };
  }, [fetchDataViews]);

  const coreServices = [
    { id: "rcl-resolution", display_name: "RCL Resolution" },
    { id: "cross-filter", display_name: "Cross-Filter" },
  ];

  const query = sidebarSearch.toLowerCase();

  // Get items for the active tab
  const items = useMemo(() => {
    let raw: any[] = [];
    switch (activeTab) {
      case "dataview":
        raw = [...dataviews].sort((a, b) =>
          (a.display_name || "").localeCompare(b.display_name || "")
        );
        break;
      case "shared_pipeline":
        raw = sharedPipelines;
        break;
      case "core_service":
        raw = coreServices;
        break;
      case "dimension":
        raw = dimensions;
        break;
      case "filter_config":
        raw = filterConfigs;
        break;
      case "source":
        raw = sources;
        break;
      case "connection":
        raw = connections;
        break;
      case "graph":
        raw = graphs;
        break;
    }
    if (query) {
      return raw.filter((item) =>
        item.display_name?.toLowerCase().includes(query)
      );
    }
    return raw;
  }, [activeTab, dataviews, sharedPipelines, dimensions, filterConfigs, sources, connections, graphs, query]);

  const isActive = (id: string) =>
    selected?.type === activeTab && selected?.id === id;

  const handleCreateConnection = async () => {
    const name = newConnName.trim();
    if (!name) return;
    // Opaque, collision-resistant id. Display name is purely cosmetic; refs go by id.
    const id = `ds_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    const starterConfig =
      newConnType === "clickhouse"
        ? {
            host: "",
            port: 8123,
            username: "default",
            password: "",
            ssl: false,
            query_timeout_seconds: 30,
            allow_write_access: false,
          }
        : { host: "", port: 5432, database: "", user: "", password: "", schema: "public" };
    try {
      await api.createDataSource({
        id,
        type: newConnType,
        display_name: name,
        config: starterConfig,
      });
      setNewConnName("");
      setNewConnType("pg");
      setCreatingConn(false);
      reloadConnections();
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateGraph = async () => {
    const name = newGraphName.trim();
    if (!name) return;
    const id = `grph_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    // Starter spec — minimal valid graph: one hierarchy ("product"
    // with `article` as the only level), one spine source that
    // attaches there. Passes `POST /api/graphs/:id/validate` clean
    // so the new graph opens without immediate errors; users fill
    // in real columns / metrics / hierarchies from there.
    const starterToml = `id           = "${id}"
display_name = "${name.replace(/"/g, '\\"')}"

# Spine source — replace the table + column values below with your
# tenant's DuckDB shape. The engine reads via the generic
# SourceReader, so this can be a base table or a view.
[[sources]]
alias       = "ph_master"
table       = "asv2_ph_master"
attaches_at = "article"

# Primary hierarchy. The first [hierarchy.<name>] block is the
# primary by convention (Decision 31); add more for auxiliary
# dimensions (store, brand, …).
[hierarchy.product]
source = "ph_master"

[hierarchy.product.article]
column = "article"
`;
    try {
      const r = await fetch("/api/graphs", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id, display_name: name, toml_text: starterToml }),
      });
      if (!r.ok) throw new Error(await r.text());
      // Refresh the sidebar list + open the new graph.
      const fresh = await fetch("/api/graphs").then((x) => x.json()).catch(() => []);
      setGraphs(fresh);
      setNewGraphName("");
      setCreatingGraph(false);
      select({ type: "graph", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateDataView = async () => {
    const name = newDvName.trim();
    if (!name) return;
    const id = `dv_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    try {
      await api.createDataView({ id, display_name: name });
      setNewDvName("");
      setCreatingDv(false);
      await fetchDataViews();
      select({ type: "dataview", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateSource = async () => {
    const name = newSrcName.trim();
    if (!name) return;
    const id = `src_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    // Defaults per kind so the workspace has a coherent shape from the start.
    const defaults: Record<string, any> = {
      pg_query: { config: { sql: "" } },
      bq_query: { config: { sql: "" } },
      duckdb_query: { config: { sql: "" } },
      parquet_glob: { config: { path: "", hive_partitioning: true } },
      duckdb_table: { config: {}, target_table: "" },
      cdc_pg: { config: { upstream_table: "" }, target_table: "", primary_key: [], cdc_enabled: true },
    };
    const payload = {
      id,
      display_name: name,
      kind: newSrcKind,
      ...(defaults[newSrcKind] || { config: {} }),
    };
    try {
      await api.createSource(payload);
      setNewSrcName("");
      setCreatingSrc(false);
      const fresh = await api.getSources();
      setSources(fresh);
      select({ type: "source", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateSharedPipeline = async () => {
    const name = newSpName.trim();
    if (!name) return;
    const id = `sp_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    try {
      await api.createSharedPipeline({ id, display_name: name, pipeline: [] });
      setNewSpName("");
      setCreatingSp(false);
      const fresh = await api.getSharedPipelines();
      setSharedPipelines(fresh);
      select({ type: "shared_pipeline", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateFilterConfig = async () => {
    const name = newFcName.trim();
    if (!name) return;
    const dim = newFcDim.trim();
    if (!dim) return;
    const id = `fc_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    try {
      await api.createFilterConfig({
        id,
        display_name: name,
        dimension_ref: dim,
        filter_columns: [],
        mandatory_columns: [],
        cascading_rules: [],
      });
      setNewFcName("");
      setCreatingFc(false);
      const fresh = await api.getFilterConfigs();
      setFilterConfigs(fresh);
      select({ type: "filter_config", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  const handleCreateDimension = async () => {
    const name = newDimName.trim();
    if (!name) return;
    const master = newDimMasterTable.trim();
    if (!master) return;
    const id = `dim_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
    try {
      await api.createDimension({
        id,
        display_name: name,
        master_table: master,
        levels: [],
        additional_filter_cols: [],
      });
      setNewDimName("");
      setNewDimMasterTable("");
      setCreatingDim(false);
      const fresh = await api.getDimensions();
      setDimensions(fresh);
      select({ type: "dimension", id });
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    }
  };

  return (
    <div className="h-full bg-gray-900 flex flex-col overflow-hidden">
      {/* Search + count */}
      <div className="px-3 py-2.5">
        <div className="relative">
          <Search
            size={14}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-500"
          />
          <input
            type="text"
            placeholder="Search..."
            value={sidebarSearch}
            onChange={(e) => setSidebarSearch(e.target.value)}
            className="w-full bg-gray-800 text-gray-300 text-sm rounded border border-gray-700 pl-8 pr-3 py-1.5 placeholder-gray-500 focus:outline-none focus:border-gray-600"
          />
        </div>
        <div className="flex items-center justify-between mt-2 px-0.5">
          <span className="text-[10px] text-gray-500">
            {items.length} item{items.length !== 1 ? "s" : ""}
            {query && " matching"}
          </span>
          {activeTab === "dataview" && (
            <button
              onClick={() => setCreatingDv((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="New DataView"
            >
              <Plus size={14} />
            </button>
          )}
          {activeTab === "connection" && (
            <button
              onClick={() => setCreatingConn((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="Add connection"
            >
              <Plus size={14} />
            </button>
          )}
          {activeTab === "shared_pipeline" && (
            <>
              <button
                onClick={() => setCreatingSp((v) => !v)}
                className="flex items-center gap-1 px-2 py-1 text-xs text-gray-300 hover:text-gray-100 border border-gray-700 rounded hover:border-gray-600 bg-gray-800 transition-colors"
                title="New Pipeline"
              >
                <Plus size={12} />
                <span>New pipeline</span>
              </button>
              {/* Import a previously-exported pipeline JSON. Server picks
                  a fresh id by suffixing the source id with `_imported_<unix>`
                  to avoid clashes; the user can rename via the editor afterwards. */}
              <label
                className="flex items-center gap-1 px-2 py-1 text-xs text-gray-300 hover:text-gray-100 border border-gray-700 rounded hover:border-gray-600 bg-gray-800 transition-colors cursor-pointer"
                title="Import pipeline from JSON"
              >
                <Upload size={12} />
                <span>Import</span>
                <input
                  type="file"
                  accept="application/json,.json"
                  className="hidden"
                  onChange={async (e) => {
                    const file = e.target.files?.[0];
                    e.target.value = "";
                    if (!file) return;
                    try {
                      const text = await file.text();
                      const data = JSON.parse(text);
                      await api.importSharedPipeline(data, "new");
                      const fresh = await api.getSharedPipelines();
                      setSharedPipelines(fresh);
                    } catch (err: any) {
                      alert("Import failed: " + (err?.message || "invalid JSON"));
                    }
                  }}
                />
              </label>
            </>
          )}
          {activeTab === "source" && (
            <button
              onClick={() => setCreatingSrc((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="New Source"
            >
              <Plus size={14} />
            </button>
          )}
          {activeTab === "filter_config" && (
            <button
              onClick={() => setCreatingFc((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="New Filter Configuration"
            >
              <Plus size={14} />
            </button>
          )}
          {activeTab === "dimension" && (
            <button
              onClick={() => setCreatingDim((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="New Dimension"
            >
              <Plus size={14} />
            </button>
          )}
          {activeTab === "graph" && (
            <button
              onClick={() => setCreatingGraph((v) => !v)}
              className="text-gray-500 hover:text-gray-300 transition-opacity"
              title="New Graph"
            >
              <Plus size={14} />
            </button>
          )}
        </div>

        {activeTab === "dimension" && creatingDim && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name (e.g., Vendor)"
              value={newDimName}
              onChange={(e) => setNewDimName(e.target.value)}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <input
              type="text"
              placeholder="Master table (e.g., global.vendor_master)"
              value={newDimMasterTable}
              onChange={(e) => setNewDimMasterTable(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateDimension(); }}
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 font-mono focus:outline-none focus:border-blue-500"
            />
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateDimension}
                disabled={!newDimName.trim() || !newDimMasterTable.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingDim(false); setNewDimName(""); setNewDimMasterTable(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "source" && creatingSrc && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name"
              value={newSrcName}
              onChange={(e) => setNewSrcName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateSource(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <select
              value={newSrcKind}
              onChange={(e) => setNewSrcKind(e.target.value)}
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            >
              <option value="pg_query">PG Query (live)</option>
              <option value="bq_query">BQ Query (live)</option>
              <option value="duckdb_query">DuckDB Query (live)</option>
              <option value="parquet_glob">Parquet Glob (static)</option>
              <option value="duckdb_table">DuckDB Table (pipeline-populated)</option>
              <option value="cdc_pg">CDC PG (streaming)</option>
            </select>
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateSource}
                disabled={!newSrcName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingSrc(false); setNewSrcName(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "filter_config" && creatingFc && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name"
              value={newFcName}
              onChange={(e) => setNewFcName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateFilterConfig(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <select
              value={newFcDim}
              onChange={(e) => setNewFcDim(e.target.value)}
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            >
              <option value="product">product (product_attributes_filter)</option>
              <option value="store">store (store_attributes_filter)</option>
              {dimensions.map((d: any) => (
                <option key={d.id} value={d.id}>{d.display_name || d.id} ({d.id})</option>
              ))}
            </select>
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateFilterConfig}
                disabled={!newFcName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingFc(false); setNewFcName(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "shared_pipeline" && creatingSp && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name"
              value={newSpName}
              onChange={(e) => setNewSpName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateSharedPipeline(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateSharedPipeline}
                disabled={!newSpName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingSp(false); setNewSpName(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "dataview" && creatingDv && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name"
              value={newDvName}
              onChange={(e) => setNewDvName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateDataView(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateDataView}
                disabled={!newDvName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingDv(false); setNewDvName(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "connection" && creatingConn && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name"
              value={newConnName}
              onChange={(e) => setNewConnName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateConnection(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <select
              value={newConnType}
              onChange={(e) => setNewConnType(e.target.value as "pg" | "clickhouse")}
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            >
              <option value="pg">Postgres</option>
              <option value="clickhouse">ClickHouse</option>
            </select>
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateConnection}
                disabled={!newConnName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => {
                  setCreatingConn(false);
                  setNewConnName("");
                  setNewConnType("pg");
                }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {activeTab === "graph" && creatingGraph && (
          <div className="mt-2 space-y-1.5 p-2 rounded bg-gray-800/60 border border-gray-700">
            <input
              type="text"
              placeholder="Display name (e.g., Acme Inventory Graph)"
              value={newGraphName}
              onChange={(e) => setNewGraphName(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") handleCreateGraph(); }}
              autoFocus
              className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
            />
            <div className="flex items-center gap-1">
              <button
                onClick={handleCreateGraph}
                disabled={!newGraphName.trim()}
                className="flex-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
              >
                Create
              </button>
              <button
                onClick={() => { setCreatingGraph(false); setNewGraphName(""); }}
                className="text-[11px] px-2 py-1 rounded bg-gray-700 text-gray-300 hover:bg-gray-600"
              >
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Items list */}
      <div className="flex-1 overflow-y-auto">
        {items.map((item) => (
          <SidebarItem
            key={item.id}
            label={item.display_name}
            // Show the id underneath every row. Bundle/per-kind import
            // with mode=new auto-suffixes ids on collision, leaving
            // multiple rows with the same display_name — without the id
            // pinned visible the duplicates are indistinguishable.
            subtitle={item.id}
            active={isActive(item.id)}
            onClick={() => select({ type: activeTab, id: item.id })}
            onDelete={
              activeTab === "shared_pipeline"
                ? async () => {
                    if (!confirm(`Delete pipeline "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteSharedPipeline(item.id);
                      const fresh = await api.getSharedPipelines();
                      setSharedPipelines(fresh);
                      // If the deleted pipeline was open, deselect.
                      if (
                        selected?.type === "shared_pipeline" &&
                        selected.id === item.id
                      ) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : activeTab === "dimension"
                ? async () => {
                    if (!confirm(`Delete dimension "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteDimension(item.id);
                      const fresh = await api.getDimensions();
                      setDimensions(fresh);
                      if (selected?.type === "dimension" && selected.id === item.id) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : activeTab === "filter_config"
                ? async () => {
                    if (!confirm(`Delete filter config "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteFilterConfig(item.id);
                      const fresh = await api.getFilterConfigs();
                      setFilterConfigs(fresh);
                      if (selected?.type === "filter_config" && selected.id === item.id) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : activeTab === "dataview"
                ? async () => {
                    if (!confirm(`Delete DataView "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteDataView(item.id);
                      await fetchDataViews();
                      if (selected?.type === "dataview" && selected.id === item.id) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : activeTab === "source"
                ? async () => {
                    if (!confirm(`Delete source "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteSource(item.id);
                      const fresh = await api.getSources();
                      setSources(fresh);
                      if (selected?.type === "source" && selected.id === item.id) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : activeTab === "connection"
                ? async () => {
                    if (!confirm(`Delete connection "${item.display_name}" (${item.id})?`)) return;
                    try {
                      await api.deleteDataSource(item.id);
                      const fresh = await api.getDataSources();
                      setConnections(fresh);
                      if (selected?.type === "connection" && selected.id === item.id) {
                        select(null);
                      }
                    } catch (err: any) {
                      alert("Delete failed: " + (err?.message || "unknown"));
                    }
                  }
                : undefined
            }
            badge={
              activeTab === "shared_pipeline" && activeRun && activeRun.pipeline_id === item.id ? (
                <span className="flex items-center gap-1 shrink-0" title={`Running — ${formatRunDuration(activeRun.ran_for_ms)} elapsed`}>
                  <Loader2 size={11} className="text-blue-400 animate-spin" strokeWidth={2.5} />
                  <span className="text-[10px] text-blue-300 font-mono">{formatRunDuration(activeRun.ran_for_ms)}</span>
                </span>
              ) : activeTab === "connection" && item.is_default ? (
                <Star size={11} className="fill-yellow-300 text-yellow-300 shrink-0" aria-label="default" />
              ) : null
            }
          />
        ))}
        {items.length === 0 && (
          <div className="px-3 py-6 text-center text-xs text-gray-600">
            {query ? "No matches" : "No items"}
          </div>
        )}
      </div>
    </div>
  );
}
