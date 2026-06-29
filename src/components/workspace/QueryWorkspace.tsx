import { useState, useEffect, useCallback } from "react";
import { Play, Save, Clock, Database, Table2, Loader2, Trash2 } from "lucide-react";
import { api } from "@/api/client";

interface QueryResult {
  columns: string[];
  rows: any[];
  row_count: number;
  total: number;
  duration_ms: number;
}

interface DuckDBTable {
  table_name: string;
  estimated_size: number;
  column_count: number;
}

export function QueryWorkspace() {
  const [sql, setSql] = useState("SHOW TABLES");
  const [result, setResult] = useState<QueryResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedQueries, setSavedQueries] = useState<any[]>([]);
  const [history, setHistory] = useState<{ sql: string; ts: number }[]>([]);
  const [sideTab, setSideTab] = useState<"tables" | "saved" | "history">("tables");
  const [tables, setTables] = useState<DuckDBTable[]>([]);
  const [tablesLoading, setTablesLoading] = useState(false);
  // Connection target. "duckdb" = tenant_data.duckdb (the original mode);
  // any other value is a PG connection id from the connections table.
  const [connection, setConnection] = useState<string>("duckdb");
  const [connections, setConnections] = useState<any[]>([]);

  const runQuery = useCallback(async () => {
    if (!sql.trim()) return;
    setLoading(true);
    setError(null);
    setResult(null);
    const t0 = Date.now();
    try {
      let data: any;
      if (connection === "duckdb") {
        const res = await fetch("/api/query", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ sql, limit: 500 }),
        });
        data = await res.json();
      } else {
        // /api/connections/:id/run uses the connection's stored config
        // directly (no TOML/Secret Manager override) and returns
        // {columns:[{name}], rows:[{...}], total} for both PG and DuckDB.
        const res = await fetch(`/api/connections/${encodeURIComponent(connection)}/run`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ sql }),
        });
        data = await res.json();
      }
      if (data.error) {
        setError(data.error);
      } else {
        // Normalize: /api/query returns columns as string[], /run returns {name}[].
        const cols: string[] = (data.columns || []).map((c: any) =>
          typeof c === "string" ? c : c?.name ?? ""
        );
        const rows = data.rows || [];
        setResult({
          columns: cols,
          rows,
          row_count: data.row_count ?? rows.length,
          total: data.total ?? rows.length,
          duration_ms: data.duration_ms ?? Date.now() - t0,
        });
      }
    } catch (e: any) {
      setError(e.message || "Query failed");
    }
    setLoading(false);
    setHistory((prev) => [{ sql, ts: Date.now() }, ...prev.slice(0, 49)]);
  }, [sql, connection]);

  const loadTables = useCallback(async () => {
    setTablesLoading(true);
    try {
      const res = await fetch("/api/query/tables");
      const data = await res.json();
      setTables(data.tables || []);
    } catch {}
    setTablesLoading(false);
  }, []);

  const dropTable = useCallback(async (name: string) => {
    if (!window.confirm(`Drop table "${name}"? This permanently deletes the underlying data.`)) return;
    try {
      const res = await fetch(`/api/query/tables/${encodeURIComponent(name)}`, { method: "DELETE" });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) {
        window.alert(data?.error || `Drop failed (${res.status})`);
        return;
      }
      await loadTables();
    } catch (e: any) {
      window.alert("Drop failed: " + (e?.message || "unknown error"));
    }
  }, [loadTables]);

  const loadSavedQueries = useCallback(async () => {
    try {
      const qs = await api.getSavedQueries();
      setSavedQueries(qs || []);
    } catch {}
  }, []);

  const deleteSavedQuery = useCallback(async (id: string, label: string) => {
    if (!window.confirm(`Delete saved query "${label}"? This cannot be undone.`)) return;
    try {
      await api.deleteSavedQuery(id);
      await loadSavedQueries();
    } catch (e: any) {
      window.alert("Delete failed: " + (e.message || "unknown error"));
    }
  }, [loadSavedQueries]);

  useEffect(() => {
    loadTables();
    loadSavedQueries();
    api.getDataSources().then((list: any[]) => {
      setConnections(
        (list || []).filter((c: any) =>
          c.type === "pg" || c.type === "postgres" || c.type === "clickhouse"
        )
      );
    }).catch(() => {});
  }, [loadTables, loadSavedQueries]);

  const saveQuery = async () => {
    const name = prompt("Query name:");
    if (!name) return;
    const id = name.toLowerCase().replace(/[^a-z0-9]+/g, "-");
    try {
      await api.saveSavedQuery({ id, display_name: name, sql_text: sql });
      loadSavedQueries();
    } catch {}
  };

  return (
    <div className="flex h-full">
      {/* Left: Tables / Saved / History */}
      <div className="w-56 border-r border-gray-800 flex flex-col shrink-0">
        <div className="flex items-center gap-0.5 px-2 py-1.5 border-b border-gray-800">
          <button
            onClick={() => setSideTab("tables")}
            className={`px-2 py-0.5 text-[10px] font-medium rounded transition-colors ${
              sideTab === "tables" ? "bg-gray-800 text-white" : "text-gray-500 hover:text-gray-300"
            }`}
          >
            Tables
          </button>
          <button
            onClick={() => setSideTab("saved")}
            className={`px-2 py-0.5 text-[10px] font-medium rounded transition-colors ${
              sideTab === "saved" ? "bg-gray-800 text-white" : "text-gray-500 hover:text-gray-300"
            }`}
          >
            Saved
          </button>
          <button
            onClick={() => setSideTab("history")}
            className={`px-2 py-0.5 text-[10px] font-medium rounded transition-colors ${
              sideTab === "history" ? "bg-gray-800 text-white" : "text-gray-500 hover:text-gray-300"
            }`}
          >
            History
          </button>
        </div>
        <div className="flex-1 overflow-y-auto">
          {sideTab === "tables" && (
            <>
              {tablesLoading && (
                <div className="px-3 py-4 text-center">
                  <Loader2 size={14} className="animate-spin text-gray-600 mx-auto" />
                </div>
              )}
              {tables.map((t) => (
                <div
                  key={t.table_name}
                  className="flex items-stretch border-b border-gray-800/50 hover:bg-gray-800 group"
                >
                  <button
                    onClick={() => setSql(`SELECT * FROM "${t.table_name}" LIMIT 100`)}
                    className="flex-1 text-left px-3 py-2 text-xs text-gray-400 group-hover:text-white min-w-0"
                  >
                    <div className="flex items-center gap-1.5">
                      <Table2 size={11} className="text-gray-600 shrink-0" />
                      <span className="font-mono truncate">{t.table_name}</span>
                    </div>
                    <div className="flex items-center gap-2 mt-0.5 text-[9px] text-gray-600">
                      <span>{t.column_count} cols</span>
                    </div>
                  </button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      dropTable(t.table_name);
                    }}
                    title="Drop table"
                    className="shrink-0 px-2 text-gray-600 hover:text-red-400 hover:bg-red-900/30"
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              ))}
              {!tablesLoading && tables.length === 0 && (
                <div className="px-3 py-4 text-[10px] text-gray-600 text-center">
                  No tables yet.<br />Materialize a source first.
                </div>
              )}
              <button
                onClick={loadTables}
                className="w-full px-3 py-1.5 text-[10px] text-gray-600 hover:text-gray-400 border-t border-gray-800"
              >
                Refresh
              </button>
            </>
          )}
          {sideTab === "saved" && (
            <>
              {savedQueries.map((q) => (
                <div
                  key={q.id}
                  className="flex items-center border-b border-gray-800/50 hover:bg-gray-800"
                >
                  <button
                    onClick={() => setSql(q.sql_text || q.query)}
                    className="flex-1 text-left px-3 py-2 text-xs text-gray-400 hover:text-white truncate min-w-0"
                  >
                    <Database size={10} className="inline mr-1.5 text-gray-600" />
                    {q.display_name}
                  </button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      deleteSavedQuery(q.id, q.display_name);
                    }}
                    title="Delete saved query"
                    className="shrink-0 p-1.5 mr-1 rounded text-gray-500 hover:text-red-400 hover:bg-red-900/30"
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              ))}
              {savedQueries.length === 0 && (
                <div className="px-3 py-4 text-[10px] text-gray-600 text-center">No saved queries</div>
              )}
            </>
          )}
          {sideTab === "history" && (
            <>
              {history.map((h, i) => (
                <button
                  key={i}
                  onClick={() => setSql(h.sql)}
                  className="w-full text-left px-3 py-2 text-xs text-gray-400 hover:bg-gray-800 hover:text-white border-b border-gray-800/50"
                >
                  <Clock size={10} className="inline mr-1.5 text-gray-600" />
                  <span className="font-mono text-[10px] truncate block">{h.sql.slice(0, 80)}</span>
                  <span className="text-[9px] text-gray-600 mt-0.5 block">
                    {new Date(h.ts).toLocaleTimeString()}
                  </span>
                </button>
              ))}
              {history.length === 0 && (
                <div className="px-3 py-4 text-[10px] text-gray-600 text-center">No history yet</div>
              )}
            </>
          )}
        </div>
      </div>

      {/* Right: Editor + Results */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* SQL Editor */}
        <div className="border-b border-gray-800 flex flex-col shrink-0">
          <textarea
            value={sql}
            onChange={(e) => setSql(e.target.value)}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                e.preventDefault();
                runQuery();
              }
            }}
            className="w-full bg-gray-950 text-gray-200 text-xs font-mono p-3 resize-none focus:outline-none"
            rows={6}
            placeholder={connection === "duckdb"
              ? "Enter DuckDB SQL... (Cmd+Enter to run)"
              : "Enter PG SQL... (Cmd+Enter to run)"}
            spellCheck={false}
          />
          <div className="flex items-center justify-between px-3 py-1.5 bg-gray-900/50">
            <div className="flex items-center gap-1.5">
              <button
                onClick={runQuery}
                disabled={loading || !sql.trim()}
                className="flex items-center gap-1 px-2.5 py-1 text-[10px] font-medium rounded bg-green-600 hover:bg-green-500 text-white disabled:opacity-50 transition-colors"
              >
                {loading ? <Loader2 size={11} className="animate-spin" /> : <Play size={11} />}
                {loading ? "Running..." : "Run"}
              </button>
              <span className="text-[9px] text-gray-600 ml-1">Cmd+Enter</span>
              <select
                value={connection}
                onChange={(e) => setConnection(e.target.value)}
                title="Run against tenant DuckDB or a registered connection (PG / ClickHouse)"
                className="ml-2 px-1.5 py-0.5 text-[10px] text-gray-200 bg-gray-900 border border-gray-700 rounded hover:border-gray-500 focus:outline-none focus:border-blue-500"
              >
                <option value="duckdb">DuckDB (tenant)</option>
                {connections.length > 0 && <option disabled>───── External connections ─────</option>}
                {connections.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.display_name || c.id}
                    {" ["}{c.type}{"]"}
                    {c.is_default ? "  ★" : ""}
                  </option>
                ))}
              </select>
              <button
                onClick={saveQuery}
                className="flex items-center gap-1 px-2 py-1 text-[10px] font-medium rounded text-gray-400 hover:text-white hover:bg-gray-800 transition-colors"
              >
                <Save size={11} />
                Save
              </button>
            </div>
            {result && (
              <span className="text-[10px] text-gray-400">
                {result.row_count.toLocaleString()} of {result.total.toLocaleString()} rows &middot; {result.duration_ms}ms
              </span>
            )}
          </div>
        </div>

        {/* Results */}
        <div className="flex-1 overflow-auto">
          {error && (
            <div className="m-3 p-3 rounded bg-red-900/20 border border-red-800 text-red-300 text-xs font-mono whitespace-pre-wrap">
              {error}
            </div>
          )}
          {result && result.rows.length > 0 && (
            <div className="overflow-auto">
              <table className="w-full text-xs">
                <thead className="bg-gray-900 sticky top-0 z-10">
                  <tr>
                    <th className="px-2 py-1.5 text-left text-[9px] font-medium text-gray-600 border-b border-gray-800 w-10">#</th>
                    {result.columns.map((col, i) => (
                      <th key={i} className="px-3 py-1.5 text-left text-[10px] font-medium text-gray-400 border-b border-gray-800 whitespace-nowrap">
                        {col}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {result.rows.map((row, ri) => (
                    <tr key={ri} className="hover:bg-gray-900/50 border-b border-gray-800/30">
                      <td className="px-2 py-1 text-gray-600 text-[9px]">{ri + 1}</td>
                      {result.columns.map((col, ci) => {
                        const val = row[col];
                        // jsonb / json columns arrive as nested objects/arrays;
                        // String(obj) renders "[object Object]" — stringify so
                        // the cell shows the actual JSON.
                        const cell = val === null || val === undefined
                          ? null
                          : typeof val === "object"
                          ? JSON.stringify(val)
                          : String(val);
                        return (
                          <td
                            key={ci}
                            title={cell ?? undefined}
                            className="px-3 py-1 text-gray-300 whitespace-nowrap font-mono text-[10px] max-w-xs truncate"
                          >
                            {cell === null ? <span className="text-gray-600 italic">null</span> : cell}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          {result && result.rows.length === 0 && !error && (
            <div className="flex items-center justify-center h-32 text-gray-600 text-xs">
              Query returned 0 rows
            </div>
          )}
          {!result && !error && !loading && (
            <div className="flex flex-col items-center justify-center h-full text-gray-600">
              <Database size={24} className="mb-2 text-gray-700" />
              <p className="text-xs">DuckDB Query Console</p>
              <p className="text-[10px] text-gray-700 mt-1">Cmd+Enter to execute &middot; Browse tables on the left</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
