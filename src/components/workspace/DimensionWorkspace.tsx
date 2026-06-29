import { useState, useEffect, useCallback } from "react";
import { Save, Plus, Trash2, ChevronUp, ChevronDown, X, Loader2, Layers, Table2 } from "lucide-react";
import { api } from "@/api/client";

interface Level {
  column: string;
  display_name: string;
}

interface DimensionWorkspaceProps {
  dimensionId: string;
}

export function DimensionWorkspace({ dimensionId }: DimensionWorkspaceProps) {
  const [dim, setDim] = useState<any>(null);
  const [displayName, setDisplayName] = useState("");
  const [masterTable, setMasterTable] = useState("");
  // Connection driving the master-table introspection. Persisted in
  // the dimensions row's `datasource_ref` column (re-purposed to hold
  // a connection id) so we don't need a schema migration. Empty =
  // use the tenant's default PG connection.
  const [connectionRef, setConnectionRef] = useState("");
  const [pgConnections, setPgConnections] = useState<any[]>([]);
  const [availableColumns, setAvailableColumns] = useState<any[]>([]);
  const [levels, setLevels] = useState<Level[]>([]);
  const [additionalFilterCols, setAdditionalFilterCols] = useState<string[]>([]);
  const [newFilterCol, setNewFilterCol] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setError(null);
      const [all, conns] = await Promise.all([
        api.getDimensions(),
        api.getDataSources(),
      ]);
      setPgConnections(
        conns.filter((c: any) => c.type === "pg" || c.type === "postgres"),
      );
      const found = all.find((d: any) => d.id === dimensionId);
      if (!found) throw new Error("Dimension not found");
      setDim(found);
      setDisplayName(found.display_name || "");
      setMasterTable(found.master_table || "");
      setConnectionRef(found.datasource_ref || "");
      const lvls = typeof found.levels === "string" ? JSON.parse(found.levels) : found.levels || [];
      setLevels(lvls);
      const addCols = typeof found.additional_filter_cols === "string"
        ? JSON.parse(found.additional_filter_cols)
        : found.additional_filter_cols || [];
      setAdditionalFilterCols(addCols);
    } catch (e: any) {
      setError(e.message || "Failed to load dimension");
    }
  }, [dimensionId]);

  useEffect(() => {
    setDim(null);
    setSaveMsg(null);
    load();
  }, [load]);

  // Introspect columns from `information_schema` whenever the
  // (connection, master_table) pair changes. master_table can be
  // `schema.table` (e.g. `global.product_attributes_filter`) or a
  // bare table name (we default to `global`). The connection is the
  // user-picked PG row, falling back to the tenant's default.
  //
  // Side-effect: if the dimension has no levels defined yet and the
  // table has `l?_name` columns, auto-populate `levels` with them
  // sorted ascending. The user can edit/reorder/delete after.
  useEffect(() => {
    if (!masterTable) {
      setAvailableColumns([]);
      return;
    }
    let cancelled = false;
    const parts = String(masterTable).split(".");
    const schema = parts.length === 2 ? parts[0] : "global";
    const table = parts.length === 2 ? parts[1] : parts[0];

    const pickConnection = (conns: any[]): any | undefined => {
      if (connectionRef) {
        return conns.find((c: any) => c.id === connectionRef);
      }
      return conns.find(
        (c: any) => (c.type === "pg" || c.type === "postgres") && c.is_default === 1,
      ) ?? conns.find((c: any) => c.type === "pg" || c.type === "postgres");
    };

    (async () => {
      try {
        const conns = pgConnections.length > 0 ? pgConnections : await api.getDataSources();
        const pg = pickConnection(conns);
        if (!pg) {
          if (!cancelled) setAvailableColumns([]);
          return;
        }
        const cols = await api.getColumns(pg.id, schema, table);
        if (cancelled) return;
        const colInfo = cols.map((c: any) => ({ name: c.column_name, type: c.data_type }));
        setAvailableColumns(colInfo);
        // Auto-populate hierarchy levels with `<x><n>_name` columns
        // (l0_name..l5_name for product, s0_name..s4_name for store,
        // etc.) when no levels are defined yet. Sorted ascending by
        // the digit so the broadest level lands at index 0. The
        // prefix letter is the dimension's "level marker" — group
        // by prefix and pick the one with the most rows so a table
        // with both is unlikely to mix them up.
        setLevels((prev) => {
          if (prev.length > 0) return prev;
          const candidates = colInfo
            .map((c: any) => c.name as string)
            .filter((n) => /^[a-z]\d+_name$/i.test(n));
          if (candidates.length === 0) return prev;
          // Group by prefix letter and pick the most populous group.
          const byPrefix = new Map<string, string[]>();
          for (const n of candidates) {
            const p = n[0].toLowerCase();
            if (!byPrefix.has(p)) byPrefix.set(p, []);
            byPrefix.get(p)!.push(n);
          }
          const winner = Array.from(byPrefix.values()).sort(
            (a, b) => b.length - a.length,
          )[0];
          const sorted = winner.sort((a, b) => {
            const na = parseInt(a.match(/\d+/)?.[0] ?? "0", 10);
            const nb = parseInt(b.match(/\d+/)?.[0] ?? "0", 10);
            return na - nb;
          });
          return sorted.map((col) => ({ column: col, display_name: col }));
        });
      } catch (_e) {
        if (!cancelled) setAvailableColumns([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [masterTable, connectionRef, pgConnections]);

  const handleSave = async () => {
    setSaving(true);
    setSaveMsg(null);
    try {
      await api.updateDimension(dimensionId, {
        display_name: displayName,
        master_table: masterTable,
        // `datasource_ref` is re-purposed to hold the connection id
        // (tenant default PG when empty). Schema column kept as-is
        // to avoid a migration; semantics are now "which connection
        // to introspect against".
        datasource_ref: connectionRef,
        levels,
        additional_filter_cols: additionalFilterCols,
      });
      setSaveMsg("Saved");
      setTimeout(() => setSaveMsg(null), 2000);
    } catch (e: any) {
      setSaveMsg("Error: " + (e.message || "Save failed"));
    } finally {
      setSaving(false);
    }
  };

  const moveLevel = (idx: number, dir: -1 | 1) => {
    const newIdx = idx + dir;
    if (newIdx < 0 || newIdx >= levels.length) return;
    const copy = [...levels];
    [copy[idx], copy[newIdx]] = [copy[newIdx], copy[idx]];
    setLevels(copy);
  };

  const updateLevel = (idx: number, field: keyof Level, value: string) => {
    setLevels((prev) => prev.map((l, i) => (i === idx ? { ...l, [field]: value } : l)));
  };

  const deleteLevel = (idx: number) => {
    setLevels((prev) => prev.filter((_, i) => i !== idx));
  };

  const addLevel = () => {
    setLevels((prev) => [...prev, { column: "", display_name: "" }]);
  };

  const addFilterCol = () => {
    const col = newFilterCol.trim();
    if (col && !additionalFilterCols.includes(col)) {
      setAdditionalFilterCols((prev) => [...prev, col]);
      setNewFilterCol("");
    }
  };

  const removeFilterCol = (col: string) => {
    setAdditionalFilterCols((prev) => prev.filter((c) => c !== col));
  };

  if (error) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-center">
          <p className="text-red-400 text-sm">{error}</p>
          <button onClick={load} className="mt-3 text-xs text-blue-400 hover:text-blue-300">Retry</button>
        </div>
      </div>
    );
  }

  if (!dim) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-sm text-gray-500">Loading...</div>
      </div>
    );
  }

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 shrink-0 border-b border-gray-800">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 min-w-0">
            <Layers size={18} className="text-purple-400 shrink-0" />
            <h1 className="text-lg font-semibold truncate">{displayName || "Dimension"}</h1>
          </div>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            {saveMsg && (
              <span className={`text-xs ${saveMsg.startsWith("Error") ? "text-red-400" : "text-green-400"}`}>
                {saveMsg}
              </span>
            )}
            <button
              onClick={handleSave}
              disabled={saving}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 hover:bg-blue-500 disabled:opacity-50 transition-colors"
            >
              {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
              Save
            </button>
          </div>
        </div>
        <p className="text-xs text-gray-500 font-mono mt-0.5">{dim.id}</p>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-5 py-4 space-y-6">
        {/* Basic fields */}
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-xs font-medium text-gray-400 mb-1">Display Name</label>
            <input
              type="text"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
            />
          </div>
          <div>
            <label className="block text-xs font-medium text-gray-400 mb-1">Connection</label>
            <select
              value={connectionRef}
              onChange={(e) => setConnectionRef(e.target.value)}
              className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
            >
              <option value="">(default — tenant's first PG)</option>
              {pgConnections.map((c: any) => (
                <option key={c.id} value={c.id}>
                  {c.display_name || c.id}{c.is_default ? " ★" : ""}
                </option>
              ))}
            </select>
          </div>
          <div className="col-span-2">
            <label className="block text-xs font-medium text-gray-400 mb-1">
              Master Table{" "}
              <span className="text-gray-600">
                ({availableColumns.length > 0 ? `${availableColumns.length} columns introspected` : "introspected from PG information_schema"})
              </span>
            </label>
            <div className="flex items-center gap-2">
              <Table2 size={14} className="text-gray-500 shrink-0" />
              <input
                type="text"
                value={masterTable}
                onChange={(e) => setMasterTable(e.target.value)}
                placeholder="schema.table  (e.g. global.product_attributes_filter)"
                className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 font-mono focus:outline-none focus:border-blue-500"
              />
            </div>
          </div>
        </div>

        {/* Hierarchy Levels */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-gray-300">Hierarchy Levels</h2>
            <button
              onClick={addLevel}
              className="flex items-center gap-1 text-xs text-blue-400 hover:text-blue-300"
            >
              <Plus size={12} /> Add Level
            </button>
          </div>
          {levels.length === 0 ? (
            <p className="text-xs text-gray-600 italic">No levels defined. Click "Add Level" to begin.</p>
          ) : (
            <div className="border border-gray-800 rounded overflow-hidden">
              <table className="w-full text-sm">
                <thead>
                  <tr className="bg-gray-900/50 text-gray-400 text-xs">
                    <th className="px-3 py-2 text-left w-10">#</th>
                    <th className="px-3 py-2 text-left">Column</th>
                    <th className="px-3 py-2 text-left">Display Name</th>
                    <th className="px-3 py-2 text-right w-28">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {levels.map((level, idx) => (
                    <tr key={idx} className="border-t border-gray-800 hover:bg-gray-900/30">
                      <td className="px-3 py-1.5 text-gray-500 text-xs">{idx + 1}</td>
                      <td className="px-3 py-1.5">
                        {availableColumns.length > 0 ? (
                          <select
                            value={level.column}
                            onChange={(e) => {
                              const col = e.target.value;
                              const dsCol = availableColumns.find((c: any) => c.name === col);
                              updateLevel(idx, "column", col);
                              if (!level.display_name || level.display_name === level.column) {
                                updateLevel(idx, "display_name", dsCol?.display_name || col);
                              }
                            }}
                            className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                          >
                            <option value="">Select column...</option>
                            {availableColumns.map((c: any) => (
                              <option key={c.name} value={c.name}>{c.name}</option>
                            ))}
                          </select>
                        ) : (
                          <input
                            type="text"
                            value={level.column}
                            onChange={(e) => updateLevel(idx, "column", e.target.value)}
                            className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                            placeholder="column_name"
                          />
                        )}
                      </td>
                      <td className="px-3 py-1.5">
                        <input
                          type="text"
                          value={level.display_name}
                          onChange={(e) => updateLevel(idx, "display_name", e.target.value)}
                          className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                          placeholder="Display Name"
                        />
                      </td>
                      <td className="px-3 py-1.5 text-right">
                        <div className="flex items-center justify-end gap-1">
                          <button
                            onClick={() => moveLevel(idx, -1)}
                            disabled={idx === 0}
                            className="p-1 rounded hover:bg-gray-700 disabled:opacity-30 text-gray-400"
                            title="Move up"
                          >
                            <ChevronUp size={13} />
                          </button>
                          <button
                            onClick={() => moveLevel(idx, 1)}
                            disabled={idx === levels.length - 1}
                            className="p-1 rounded hover:bg-gray-700 disabled:opacity-30 text-gray-400"
                            title="Move down"
                          >
                            <ChevronDown size={13} />
                          </button>
                          <button
                            onClick={() => deleteLevel(idx)}
                            className="p-1 rounded hover:bg-red-900/40 text-gray-400 hover:text-red-400"
                            title="Delete"
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>

        {/* Additional Filter Columns */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-2">Additional Filter Columns</h2>
          <div className="flex flex-wrap gap-2 mb-2">
            {additionalFilterCols.map((col) => (
              <span
                key={col}
                className="flex items-center gap-1 px-2 py-1 rounded-full bg-gray-800 border border-gray-700 text-xs text-gray-300"
              >
                {col}
                <button onClick={() => removeFilterCol(col)} className="hover:text-red-400 transition-colors">
                  <X size={11} />
                </button>
              </span>
            ))}
            {additionalFilterCols.length === 0 && (
              <span className="text-xs text-gray-600 italic">None</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            {availableColumns.length > 0 ? (
              <select
                value={newFilterCol}
                onChange={(e) => setNewFilterCol(e.target.value)}
                className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
              >
                <option value="">Select column...</option>
                {availableColumns
                  .filter((c: any) => !additionalFilterCols.includes(c.name))
                  .slice()
                  .sort((a: any, b: any) =>
                    String(a.name).localeCompare(String(b.name)),
                  )
                  .map((c: any) => (
                    <option key={c.name} value={c.name}>{c.display_name || c.name}</option>
                  ))}
              </select>
            ) : (
              <input
                type="text"
                value={newFilterCol}
                onChange={(e) => setNewFilterCol(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && addFilterCol()}
                className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                placeholder="Column name"
              />
            )}
            <button
              onClick={addFilterCol}
              className="flex items-center gap-1 px-2 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-300 hover:bg-gray-700"
            >
              <Plus size={11} /> Add
            </button>
          </div>
        </div>

        {/* Source Tables */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-2">Source Tables</h2>
          <div className="rounded border border-gray-800 bg-gray-900/30 p-3">
            <div className="flex items-center gap-2 text-xs text-gray-400">
              <Table2 size={13} />
              <span>{masterTable || "No master table configured"}</span>
            </div>
            <p className="text-[11px] text-gray-600 mt-1">
              Source tables are derived from the master table and dimension hierarchy.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
