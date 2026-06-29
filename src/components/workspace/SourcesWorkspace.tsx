import { useState, useEffect, useCallback } from "react";
import {
  Save,
  Loader2,
  Trash2,
  Database,
  FileSearch,
  Globe,
  HardDrive,
  Radio,
  Layers,
  PlayCircle,
  Square,
  Zap,
} from "lucide-react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";

interface SourcesWorkspaceProps {
  sourceId: string;
}

type Kind =
  | "pg_query"
  | "ch_query"
  | "bq_query"
  | "duckdb_query"
  | "parquet_glob"
  | "duckdb_table"
  | "cdc_pg";

const KIND_LABELS: Record<Kind, string> = {
  pg_query: "PG Query",
  ch_query: "ClickHouse Query",
  bq_query: "BQ Query",
  duckdb_query: "DuckDB Query",
  parquet_glob: "Parquet Glob",
  duckdb_table: "DuckDB Table",
  cdc_pg: "CDC (Postgres)",
};

const KIND_DESCRIPTIONS: Record<Kind, string> = {
  pg_query: "Live SQL against Postgres. Executed each DataView read.",
  ch_query: "Live SQL against ClickHouse (HTTP interface). Honors allow_write_access on the connection.",
  bq_query: "Live SQL against BigQuery. Executed each DataView read.",
  duckdb_query: "Live SQL against the tenant DuckDB. Useful for joining tables on read.",
  parquet_glob: "Read parquet files at a path (filesystem or GCS).",
  duckdb_table: "Read an existing DuckDB table. Populated by a Pipeline.",
  cdc_pg: "Streaming PG → DuckDB mirror. Self-managing; auto-resumes on boot.",
};

const KIND_ICONS: Record<Kind, React.ReactNode> = {
  pg_query: <Database size={13} />,
  ch_query: <Database size={13} />,
  bq_query: <Globe size={13} />,
  duckdb_query: <FileSearch size={13} />,
  parquet_glob: <HardDrive size={13} />,
  duckdb_table: <Layers size={13} />,
  cdc_pg: <Radio size={13} />,
};

const KIND_NEEDS_CONNECTION: Record<Kind, boolean> = {
  pg_query: true,
  ch_query: true,
  bq_query: true,
  duckdb_query: false,
  parquet_glob: false,
  duckdb_table: false,
  cdc_pg: true,
};

export function SourcesWorkspace({ sourceId }: SourcesWorkspaceProps) {
  const [src, setSrc] = useState<any>(null);
  const [displayName, setDisplayName] = useState("");
  const [kind, setKind] = useState<Kind>("pg_query");
  const [connectionRef, setConnectionRef] = useState<string>("");
  const [config, setConfig] = useState<Record<string, any>>({});
  const [targetTable, setTargetTable] = useState("");
  const [primaryKey, setPrimaryKey] = useState<string[]>([]);
  const [cdcEnabled, setCdcEnabled] = useState(false);
  const [status, setStatus] = useState("not_yet_populated");
  const [connections, setConnections] = useState<any[]>([]);

  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [materializing, setMaterializing] = useState(false);
  const [cdcStarting, setCdcStarting] = useState(false);
  const [cdcStopping, setCdcStopping] = useState(false);
  const [actionMsg, setActionMsg] = useState<string | null>(null);

  const select = useWorkspaceStore((s) => s.select);

  const load = useCallback(async () => {
    try {
      setError(null);
      const [record, conns] = await Promise.all([
        api.getSource(sourceId),
        api.getDataSources(),
      ]);
      setConnections(conns);
      setSrc(record);
      setDisplayName(record.display_name || "");
      setKind((record.kind as Kind) || "pg_query");
      setConnectionRef(record.connection_ref || "");
      const cfg =
        typeof record.config === "string"
          ? JSON.parse(record.config || "{}")
          : record.config || {};
      setConfig(cfg);
      setTargetTable(record.target_table || "");
      const pk =
        typeof record.primary_key === "string"
          ? JSON.parse(record.primary_key || "[]")
          : record.primary_key || [];
      setPrimaryKey(Array.isArray(pk) ? pk : []);
      setCdcEnabled(Boolean(record.cdc_enabled));
      setStatus(record.status || "not_yet_populated");
    } catch (e: any) {
      setError(e.message || "Failed to load Source");
    }
  }, [sourceId]);

  useEffect(() => {
    setSrc(null);
    setSaveMsg(null);
    load();
  }, [load]);

  const handleSave = async () => {
    setSaving(true);
    setSaveMsg(null);
    try {
      await api.updateSource(sourceId, {
        display_name: displayName,
        connection_ref: connectionRef,
        config,
        target_table: targetTable,
        primary_key: primaryKey,
        cdc_enabled: cdcEnabled,
      });
      setSaveMsg("Saved");
      setTimeout(() => setSaveMsg(null), 1800);
    } catch (e: any) {
      setSaveMsg("Error: " + (e?.message || "Save failed"));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    const label = displayName || sourceId;
    if (!window.confirm(`Delete Source "${label}"? This cannot be undone.`))
      return;
    setDeleting(true);
    try {
      await api.deleteSource(sourceId);
      select(null);
      window.dispatchEvent(new CustomEvent("sources-changed"));
    } catch (e: any) {
      setSaveMsg("Error: " + (e?.message || "Delete failed"));
      setDeleting(false);
    }
  };

  const updateConfig = (key: string, value: any) =>
    setConfig((prev) => ({ ...prev, [key]: value }));

  // Save before action so the server sees the latest values.
  const saveQuiet = async () => {
    await api.updateSource(sourceId, {
      display_name: displayName,
      connection_ref: connectionRef,
      config,
      target_table: targetTable,
      primary_key: primaryKey,
      cdc_enabled: cdcEnabled,
    });
  };

  const handleMaterialize = async () => {
    setMaterializing(true);
    setActionMsg(null);
    try {
      await saveQuiet();
      const res = await api.materializeSource(sourceId);
      setActionMsg(`Materialized ${res.rows ?? 0} rows in ${res.duration_ms ?? 0}ms`);
      await load();
    } catch (e: any) {
      setActionMsg("Error: " + (e?.message || "Materialize failed"));
    } finally {
      setMaterializing(false);
    }
  };

  const handleStartCdc = async () => {
    setCdcStarting(true);
    setActionMsg(null);
    try {
      await saveQuiet();
      const res = await api.startCdcSource(sourceId);
      setActionMsg(`CDC streaming from LSN ${res.start_lsn ?? "?"}`);
      await load();
    } catch (e: any) {
      setActionMsg("Error: " + (e?.message || "Start CDC failed"));
    } finally {
      setCdcStarting(false);
    }
  };

  const handleStopCdc = async () => {
    setCdcStopping(true);
    setActionMsg(null);
    try {
      await api.stopCdcSource(sourceId);
      setActionMsg("CDC stopped");
      await load();
    } catch (e: any) {
      setActionMsg("Error: " + (e?.message || "Stop CDC failed"));
    } finally {
      setCdcStopping(false);
    }
  };

  if (error) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-center">
          <p className="text-red-400 text-sm">{error}</p>
          <button
            onClick={load}
            className="mt-3 text-xs text-blue-400 hover:text-blue-300"
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (!src) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-sm text-gray-500">Loading…</div>
      </div>
    );
  }

  const inputCls =
    "w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500";
  const taCls =
    "w-full px-3 py-2 text-xs font-mono rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500";

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 shrink-0 border-b border-gray-800">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 min-w-0">
            <span className="text-blue-400 shrink-0">{KIND_ICONS[kind]}</span>
            <h1 className="text-lg font-semibold truncate">
              {displayName || "Source"}
            </h1>
            <span className="text-[10px] px-2 py-0.5 rounded font-medium shrink-0 bg-blue-900/50 text-blue-400">
              {KIND_LABELS[kind]}
            </span>
            <span
              className={`text-[10px] px-2 py-0.5 rounded font-medium shrink-0 ${
                status === "populated" || status === "streaming"
                  ? "bg-green-900/60 text-green-400"
                  : status === "failed"
                    ? "bg-red-900/40 text-red-400"
                    : status === "populating"
                      ? "bg-amber-900/50 text-amber-400"
                      : "bg-gray-800 text-gray-500"
              }`}
            >
              {status.replace(/_/g, " ")}
            </span>
          </div>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            {saveMsg && (
              <span
                className={`text-xs ${
                  saveMsg.startsWith("Error") ? "text-red-400" : "text-green-400"
                }`}
              >
                {saveMsg}
              </span>
            )}
            <button
              onClick={handleSave}
              disabled={saving || deleting}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 hover:bg-blue-500 disabled:opacity-50 transition-colors"
            >
              {saving ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Save size={12} />
              )}
              Save
            </button>
            <button
              onClick={handleDelete}
              disabled={saving || deleting}
              title="Delete this Source"
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-red-700 hover:bg-red-600 text-white disabled:opacity-50 transition-colors"
            >
              {deleting ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Trash2 size={12} />
              )}
              Delete
            </button>
          </div>
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-5 py-4 space-y-6">
        {/* Display Name */}
        <div>
          <label className="block text-xs font-medium text-gray-400 mb-1">
            Display Name
          </label>
          <input
            type="text"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            className={inputCls}
          />
        </div>

        {/* Kind (read-only after creation) */}
        <div>
          <label className="block text-xs font-medium text-gray-400 mb-1">
            Kind <span className="text-gray-600">(immutable)</span>
          </label>
          <div className="px-3 py-2 text-sm rounded bg-gray-800/50 border border-gray-700 text-gray-300 flex items-center gap-2">
            {KIND_ICONS[kind]}
            {KIND_LABELS[kind]}
            <span className="text-[10px] text-gray-500 ml-2">
              {KIND_DESCRIPTIONS[kind]}
            </span>
          </div>
        </div>

        {/* Connection (only for kinds that need one) */}
        {KIND_NEEDS_CONNECTION[kind] && (
          <div>
            <label className="block text-xs font-medium text-gray-400 mb-1">
              Connection
            </label>
            <select
              value={connectionRef}
              onChange={(e) => setConnectionRef(e.target.value)}
              className={inputCls}
            >
              <option value="">— select connection —</option>
              {connections.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.display_name || c.id} ({c.type})
                </option>
              ))}
            </select>
          </div>
        )}

        {/* Kind-specific config */}
        {(kind === "pg_query" || kind === "ch_query" || kind === "bq_query" || kind === "duckdb_query") && (
          <div>
            <label className="block text-xs font-medium text-gray-400 mb-1">
              SQL
            </label>
            <textarea
              value={config.sql || ""}
              onChange={(e) => updateConfig("sql", e.target.value)}
              className={taCls}
              rows={10}
              placeholder="SELECT * FROM …"
            />
          </div>
        )}

        {kind === "parquet_glob" && (
          <>
            <div>
              <label className="block text-xs font-medium text-gray-400 mb-1">
                Path
              </label>
              <input
                type="text"
                value={config.path || ""}
                onChange={(e) => updateConfig("path", e.target.value)}
                className={inputCls}
                placeholder="{PARQUET_HOME}/some/glob/*.parquet  or  gs://bucket/path/**/*.parquet"
              />
            </div>
            <div>
              <label className="flex items-center gap-2 text-xs text-gray-400">
                <input
                  type="checkbox"
                  checked={Boolean(config.hive_partitioning)}
                  onChange={(e) =>
                    updateConfig("hive_partitioning", e.target.checked)
                  }
                />
                Hive partitioning
              </label>
            </div>
          </>
        )}

        {kind === "duckdb_table" && (
          <div>
            <label className="block text-xs font-medium text-gray-400 mb-1">
              Target DuckDB Table
            </label>
            <input
              type="text"
              value={targetTable}
              onChange={(e) => setTargetTable(e.target.value)}
              className={inputCls}
              placeholder="my_table"
            />
            <p className="text-[10px] text-gray-500 mt-1">
              Pipelines populate this table. Until the first run, status stays
              <code className="px-1 text-gray-400"> not_yet_populated</code>.
            </p>
          </div>
        )}

        {kind === "cdc_pg" && (
          <>
            <div>
              <label className="block text-xs font-medium text-gray-400 mb-1">
                Upstream PG Table
              </label>
              <input
                type="text"
                value={config.upstream_table || ""}
                onChange={(e) => updateConfig("upstream_table", e.target.value)}
                className={inputCls}
                placeholder="inventory_smart.orders"
              />
            </div>
            <div>
              <label className="block text-xs font-medium text-gray-400 mb-1">
                Target DuckDB Table
              </label>
              <input
                type="text"
                value={targetTable}
                onChange={(e) => setTargetTable(e.target.value)}
                className={inputCls}
                placeholder="orders_live"
              />
            </div>
            <div>
              <label className="block text-xs font-medium text-gray-400 mb-1">
                Primary Key (comma-separated)
              </label>
              <input
                type="text"
                value={primaryKey.join(", ")}
                onChange={(e) =>
                  setPrimaryKey(
                    e.target.value
                      .split(",")
                      .map((s) => s.trim())
                      .filter(Boolean),
                  )
                }
                className={inputCls}
                placeholder="order_id"
              />
            </div>
            <div>
              <label className="flex items-center gap-2 text-xs text-gray-400">
                <input
                  type="checkbox"
                  checked={cdcEnabled}
                  onChange={(e) => setCdcEnabled(e.target.checked)}
                />
                CDC streaming enabled (auto-resume on boot)
              </label>
            </div>

            {/* Materialize + CDC actions */}
            <div className="border-t border-gray-800 pt-4 space-y-3">
              <h2 className="text-sm font-semibold text-gray-300">Actions</h2>
              <div className="flex flex-wrap items-center gap-2">
                <button
                  onClick={handleMaterialize}
                  disabled={materializing || cdcStarting || cdcStopping}
                  className="flex items-center gap-1.5 px-4 py-2 text-xs font-medium rounded bg-gray-800 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 transition-colors"
                  title="Initial COPY from PG into the target DuckDB table. Drops + recreates."
                >
                  {materializing ? <Loader2 size={13} className="animate-spin" /> : <Zap size={13} />}
                  Materialize
                </button>
                {status === "streaming" ? (
                  <button
                    onClick={handleStopCdc}
                    disabled={cdcStarting || cdcStopping || materializing}
                    className="flex items-center gap-1.5 px-4 py-2 text-xs font-medium rounded bg-red-900/40 border border-red-800 text-red-300 hover:bg-red-900/60 disabled:opacity-50 transition-colors"
                    title="Stop CDC streaming"
                  >
                    {cdcStopping ? <Loader2 size={13} className="animate-spin" /> : <Square size={13} />}
                    Stop CDC
                  </button>
                ) : (
                  <button
                    onClick={handleStartCdc}
                    disabled={cdcStarting || cdcStopping || materializing}
                    className="flex items-center gap-1.5 px-4 py-2 text-xs font-medium rounded bg-green-900/40 border border-green-800 text-green-300 hover:bg-green-900/60 disabled:opacity-50 transition-colors"
                    title="Open WAL slot + publication, start streaming changes"
                  >
                    {cdcStarting ? <Loader2 size={13} className="animate-spin" /> : <PlayCircle size={13} />}
                    Start CDC
                  </button>
                )}
                {actionMsg && (
                  <span className={`text-xs ${actionMsg.startsWith("Error") ? "text-red-400" : "text-green-400"}`}>
                    {actionMsg}
                  </span>
                )}
              </div>
              <p className="text-[10px] text-gray-500">
                Materialize first (initial seed), then Start CDC. With <code className="px-1 text-gray-400">cdc_enabled</code>{" "}
                checked, the stream auto-resumes on server restart.
              </p>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
