import { useState, useEffect, useCallback } from "react";
import { Save, Plus, Trash2, ChevronUp, ChevronDown, X, Loader2, Filter, Link2, Shield, GripVertical } from "lucide-react";
import { api } from "@/api/client";

interface FilterColumn {
  column: string;
  display_name: string;
  display_order: number;
  display_type: string;
  single_select?: boolean;
  values_source?: any;
}

/// Suggest filter columns for the given dimension. Sources, in order:
///   1. The dimension's `levels[].column` + `additional_filter_cols`
///      from the SmartStudio `dimensions` table (authoritative if the
///      tenant has dimensions configured).
///   2. Defaults for the canonical dimensions ("product" / "store"):
///      hierarchy levels + brand + channel for product;
///      channel + store_code-side fields for store.
/// Returns an empty list when neither source provides anything.
function suggestedColumnsForDimension(
  dimensionRef: string,
  dimensions: any[],
): string[] {
  const dim = dimensions.find((d) => d.id === dimensionRef);
  const out = new Set<string>();
  if (dim) {
    const levels = Array.isArray(dim.levels) ? dim.levels : [];
    for (const l of levels) {
      if (l?.column) out.add(String(l.column));
    }
    const extras = Array.isArray(dim.additional_filter_cols) ? dim.additional_filter_cols : [];
    for (const c of extras) {
      if (typeof c === "string" && c) out.add(c);
    }
  }
  // Defaults sourced from product_attributes_filter / store_attributes_filter
  // shape — useful when the tenant hasn't seeded a `dimensions` row but
  // still wants to configure filters for the canonical dimensions.
  if (out.size === 0) {
    if (dimensionRef === "product") {
      ["l0_name", "l1_name", "l2_name", "l3_name", "l4_name", "l5_name", "brand", "channel"]
        .forEach((c) => out.add(c));
    } else if (dimensionRef === "store") {
      ["channel", "store_code", "s0_name", "climate"].forEach((c) => out.add(c));
    }
  }
  return Array.from(out);
}

interface CascadingRule {
  trigger: string;
  affects: string[];
  type?: string;
}

interface FilterConfigWorkspaceProps {
  filterConfigId: string;
}

export function FilterConfigWorkspace({ filterConfigId }: FilterConfigWorkspaceProps) {
  const [fc, setFc] = useState<any>(null);
  const [displayName, setDisplayName] = useState("");
  const [dimensionRef, setDimensionRef] = useState("");
  const [filterColumns, setFilterColumns] = useState<FilterColumn[]>([]);
  const [mandatoryColumns, setMandatoryColumns] = useState<string[]>([]);
  const [cascadingRules, setCascadingRules] = useState<CascadingRule[]>([]);
  const [newFilterCol, setNewFilterCol] = useState("");
  const [newMandatoryCol, setNewMandatoryCol] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);
  const [dimensions, setDimensions] = useState<any[]>([]);
  const [editingCol, setEditingCol] = useState<string | null>(null);
  // PG information_schema columns of the dimension's master table.
  // Populated when the dimension changes; powers the "add column"
  // dropdown so users pick from real columns instead of typing.
  const [pgColumns, setPgColumns] = useState<string[]>([]);
  const [pgColumnsLoading, setPgColumnsLoading] = useState(false);
  const [pgColumnsError, setPgColumnsError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setError(null);
      const [configs, dims] = await Promise.all([
        api.getFilterConfigs(),
        api.getDimensions(),
      ]);
      setDimensions(dims);
      const found = configs.find((c: any) => c.id === filterConfigId);
      if (!found) throw new Error("Filter config not found");
      setFc(found);
      setDisplayName(found.display_name || "");
      setDimensionRef(found.dimension_ref || "");

      // filter_columns can be objects or strings
      const rawFc = typeof found.filter_columns === "string" ? JSON.parse(found.filter_columns) : found.filter_columns || [];
      const normalized: FilterColumn[] = rawFc.map((item: any, idx: number) => {
        if (typeof item === "string") {
          return { column: item, display_name: item, display_order: idx, display_type: "dropdown", single_select: false };
        }
        return { ...item, display_order: item.display_order ?? idx };
      });
      normalized.sort((a: FilterColumn, b: FilterColumn) => a.display_order - b.display_order);
      setFilterColumns(normalized);

      const mc = typeof found.mandatory_columns === "string" ? JSON.parse(found.mandatory_columns) : found.mandatory_columns || [];
      setMandatoryColumns(mc);

      const cr = typeof found.cascading_rules === "string" ? JSON.parse(found.cascading_rules) : found.cascading_rules || [];
      // Normalize: affects might be string or array
      const normalizedRules: CascadingRule[] = cr.map((r: any) => ({
        trigger: r.trigger || "",
        affects: Array.isArray(r.affects) ? r.affects : r.affects ? [r.affects] : [],
        type: r.type || "forward",
      }));
      setCascadingRules(normalizedRules);
    } catch (e: any) {
      setError(e.message || "Failed to load filter config");
    }
  }, [filterConfigId]);

  useEffect(() => {
    setFc(null);
    setSaveMsg(null);
    setEditingCol(null);
    load();
  }, [load]);

  // Picker source = the dimension's curated columns (`levels` +
  // `additional_filter_cols`). The dimension is the explicit
  // declaration of which columns participate in this dimension —
  // the Filter Configuration shouldn't expose anything else. To add
  // a new column, edit the dimension first.
  //
  // Order: hierarchy levels first (in their declared order — already
  // ascending by level), then additional filter columns
  // alphabetically. This mirrors the cascading dropdown order in the
  // panel itself: l0 → l1 → ... → additional cols.
  useEffect(() => {
    if (!dimensionRef) {
      setPgColumns([]);
      setPgColumnsError(null);
      return;
    }
    setPgColumnsLoading(false);
    setPgColumnsError(null);
    const dim = dimensions.find((d) => d.id === dimensionRef);
    if (!dim) {
      setPgColumns([]);
      setPgColumnsError(
        `Dimension '${dimensionRef}' not found — define it in the Dimensions tab first.`,
      );
      return;
    }
    const levels: any[] = Array.isArray(dim.levels) ? dim.levels : [];
    const additional: any[] = Array.isArray(dim.additional_filter_cols)
      ? dim.additional_filter_cols
      : [];
    const ordered: string[] = [];
    for (const l of levels) {
      const col = typeof l === "string" ? l : l?.column;
      if (col && !ordered.includes(col)) ordered.push(String(col));
    }
    const sortedExtras = additional
      .filter((c) => typeof c === "string" && c)
      .map(String)
      .sort((a, b) => a.localeCompare(b));
    for (const c of sortedExtras) {
      if (!ordered.includes(c)) ordered.push(c);
    }
    setPgColumns(ordered);
    if (ordered.length === 0) {
      setPgColumnsError(
        `Dimension '${dimensionRef}' has no levels or additional_filter_cols defined yet.`,
      );
    }
  }, [dimensionRef, dimensions]);

  const handleSave = async () => {
    setSaving(true);
    setSaveMsg(null);
    try {
      await api.updateFilterConfig(filterConfigId, {
        display_name: displayName,
        dimension_ref: dimensionRef,
        filter_columns: filterColumns,
        mandatory_columns: mandatoryColumns,
        cascading_rules: cascadingRules,
      });
      setSaveMsg("Saved");
      setTimeout(() => setSaveMsg(null), 2000);
    } catch (e: any) {
      setSaveMsg("Error: " + (e.message || "Save failed"));
    } finally {
      setSaving(false);
    }
  };

  const addFilterCol = () => {
    const col = newFilterCol.trim();
    if (col && !filterColumns.some((f) => f.column === col)) {
      setFilterColumns((prev) => [
        ...prev,
        {
          column: col,
          display_name: col,
          display_order: prev.length,
          display_type: "dropdown",
          single_select: false,
        },
      ]);
      setNewFilterCol("");
    }
  };

  const removeFilterCol = (column: string) => {
    setFilterColumns((prev) => prev.filter((c) => c.column !== column));
  };

  const moveFilterCol = (idx: number, dir: -1 | 1) => {
    const newIdx = idx + dir;
    if (newIdx < 0 || newIdx >= filterColumns.length) return;
    const copy = [...filterColumns];
    [copy[idx], copy[newIdx]] = [copy[newIdx], copy[idx]];
    // Update display_order
    copy.forEach((c, i) => (c.display_order = i));
    setFilterColumns(copy);
  };

  const updateFilterCol = (column: string, field: keyof FilterColumn, value: any) => {
    setFilterColumns((prev) =>
      prev.map((c) => (c.column === column ? { ...c, [field]: value } : c))
    );
  };

  const addMandatoryCol = () => {
    const col = newMandatoryCol.trim();
    if (col && !mandatoryColumns.includes(col)) {
      setMandatoryColumns((prev) => [...prev, col]);
      setNewMandatoryCol("");
    }
  };

  const removeMandatoryCol = (col: string) => {
    setMandatoryColumns((prev) => prev.filter((c) => c !== col));
  };

  const addCascadingRule = () => {
    setCascadingRules((prev) => [...prev, { trigger: "", affects: [], type: "forward" }]);
  };

  const updateCascadingRule = (idx: number, field: string, value: any) => {
    setCascadingRules((prev) =>
      prev.map((r, i) => (i === idx ? { ...r, [field]: value } : r))
    );
  };

  const deleteCascadingRule = (idx: number) => {
    setCascadingRules((prev) => prev.filter((_, i) => i !== idx));
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

  if (!fc) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-sm text-gray-500">Loading...</div>
      </div>
    );
  }

  const availableColumns = filterColumns.map((f) => f.column);

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 shrink-0 border-b border-gray-800">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 min-w-0">
            <Filter size={18} className="text-amber-400 shrink-0" />
            <h1 className="text-lg font-semibold truncate">{displayName || "Filter Config"}</h1>
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
        <p className="text-xs text-gray-500 font-mono mt-0.5">{fc.id}</p>
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
            <label className="block text-xs font-medium text-gray-400 mb-1">Dimension Ref</label>
            <select
              value={dimensionRef}
              onChange={(e) => setDimensionRef(e.target.value)}
              className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
            >
              <option value="">Select dimension...</option>
              {/* Canonical dimensions are always offered, even when no
                  rows exist in the `dimensions` table. The user said:
                  "for product dimension, it is from product_attributes_filter.
                  for store, it is from store_attributes_filter." */}
              {(() => {
                const knownIds = new Set(dimensions.map((d: any) => d.id));
                const fallbacks: { id: string; label: string }[] = [];
                if (!knownIds.has("product")) {
                  fallbacks.push({ id: "product", label: "product (product_attributes_filter)" });
                }
                if (!knownIds.has("store")) {
                  fallbacks.push({ id: "store", label: "store (store_attributes_filter)" });
                }
                return (
                  <>
                    {fallbacks.map((f) => (
                      <option key={f.id} value={f.id}>{f.label}</option>
                    ))}
                    {dimensions.map((d: any) => (
                      <option key={d.id} value={d.id}>{d.display_name || d.id} ({d.id})</option>
                    ))}
                  </>
                );
              })()}
            </select>
          </div>
        </div>

        {/* Filter Columns - ordered list with details */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-sm font-semibold text-gray-300">Filter Columns</h2>
            <span className="text-[10px] text-gray-500">{filterColumns.length} column{filterColumns.length !== 1 ? "s" : ""}</span>
          </div>
          {filterColumns.length === 0 ? (
            <p className="text-xs text-gray-600 italic mb-2">No filter columns defined.</p>
          ) : (
            <div className="space-y-1 mb-2">
              {filterColumns.map((fc, idx) => (
                <div key={fc.column} className="group">
                  <div className="flex items-center gap-2">
                    <GripVertical size={12} className="text-gray-600 shrink-0" />
                    <span className="text-[10px] text-gray-600 w-4 text-right shrink-0">{idx + 1}</span>
                    <button
                      onClick={() => setEditingCol(editingCol === fc.column ? null : fc.column)}
                      className="flex-1 flex items-center gap-3 px-2.5 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-300 hover:border-gray-600 transition-colors text-left"
                    >
                      <span className="font-mono text-gray-200">{fc.column}</span>
                      <span className="text-gray-500">·</span>
                      <span className="text-gray-400">{fc.display_name}</span>
                      <span className="text-gray-500">·</span>
                      <span className="text-gray-500 text-[10px]">{fc.display_type} · {fc.single_select ? "single" : "multiple"}</span>
                    </button>
                    <div className="flex items-center gap-0.5">
                      <button
                        onClick={() => moveFilterCol(idx, -1)}
                        disabled={idx === 0}
                        title="Move up"
                        className="p-0.5 rounded hover:bg-gray-700 disabled:opacity-20 text-gray-400"
                      >
                        <ChevronUp size={12} />
                      </button>
                      <button
                        onClick={() => moveFilterCol(idx, 1)}
                        disabled={idx === filterColumns.length - 1}
                        title="Move down"
                        className="p-0.5 rounded hover:bg-gray-700 disabled:opacity-20 text-gray-400"
                      >
                        <ChevronDown size={12} />
                      </button>
                      <button
                        onClick={() => removeFilterCol(fc.column)}
                        title="Remove"
                        className="p-0.5 rounded hover:bg-red-900/40 text-gray-400 hover:text-red-400"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  </div>
                  {/* Expanded detail editor */}
                  {editingCol === fc.column && (
                    <div className="ml-8 mt-1 mb-2 p-3 rounded bg-gray-900/80 border border-gray-800 space-y-2">
                      <div className="grid grid-cols-2 gap-3">
                        <div>
                          <label className="block text-[10px] text-gray-500 mb-0.5">Display Name</label>
                          <input
                            type="text"
                            value={fc.display_name}
                            onChange={(e) => updateFilterCol(fc.column, "display_name", e.target.value)}
                            className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                          />
                        </div>
                        <div>
                          <label className="block text-[10px] text-gray-500 mb-0.5">Display Type</label>
                          <select
                            value={fc.display_type}
                            onChange={(e) => updateFilterCol(fc.column, "display_type", e.target.value)}
                            className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                          >
                            <option value="dropdown">Dropdown</option>
                            <option value="search">Search</option>
                            <option value="range">Range</option>
                            <option value="toggle">Toggle</option>
                          </select>
                        </div>
                      </div>
                      <div>
                        <label className="block text-[10px] text-gray-500 mb-0.5">Selection</label>
                        <select
                          value={fc.single_select ? "single" : "multiple"}
                          onChange={(e) =>
                            updateFilterCol(fc.column, "single_select", e.target.value === "single")
                          }
                          className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                        >
                          <option value="multiple">Multiple — pick many; show Select All / Clear All</option>
                          <option value="single">Single — pick exactly one</option>
                        </select>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
          {/* Suggested columns based on the chosen dimension. Click to add. */}
          {(() => {
            const suggestions = suggestedColumnsForDimension(dimensionRef, dimensions);
            if (suggestions.length === 0) return null;
            const already = new Set(filterColumns.map((c) => c.column));
            return (
              <div className="mb-2">
                <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
                  Suggested for{" "}
                  <span className="text-gray-300 font-mono">{dimensionRef || "(no dimension)"}</span>
                </div>
                <div className="flex flex-wrap gap-1">
                  {suggestions.map((col) => {
                    const isAdded = already.has(col);
                    return (
                      <button
                        key={col}
                        onClick={() => {
                          if (isAdded) return;
                          setFilterColumns((prev) => [
                            ...prev,
                            {
                              column: col,
                              display_name: col,
                              display_order: prev.length,
                              display_type: "dropdown",
                              single_select: false,
                            },
                          ]);
                        }}
                        disabled={isAdded}
                        title={isAdded ? "Already added" : `Add ${col}`}
                        className={`text-[11px] font-mono px-2 py-0.5 rounded border transition-colors ${
                          isAdded
                            ? "bg-blue-900/40 text-blue-400 border-blue-800 cursor-default"
                            : "bg-gray-800 text-gray-300 border-gray-700 hover:border-blue-500 hover:text-blue-400"
                        }`}
                      >
                        {col}
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })()}
          {/* Picker fed by PG information_schema for the dimension's
              master table. Real columns, not free-text. Falls back to
              a text input when introspection isn't available (e.g. PG
              connection not configured). */}
          <div className="flex items-center gap-2">
            {pgColumnsError ? (
              <input
                type="text"
                value={newFilterCol}
                onChange={(e) => setNewFilterCol(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && addFilterCol()}
                className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                placeholder="Column name (introspect failed — type manually)"
                title={pgColumnsError}
              />
            ) : (
              <select
                value={newFilterCol}
                onChange={(e) => setNewFilterCol(e.target.value)}
                disabled={pgColumnsLoading || pgColumns.length === 0}
                className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 min-w-[280px]"
              >
                <option value="">
                  {pgColumnsLoading
                    ? "Loading columns…"
                    : pgColumns.length === 0
                      ? "Pick a dimension first"
                      : `Select column (${pgColumns.length} available)…`}
                </option>
                {pgColumns
                  .filter((c) => !filterColumns.some((f) => f.column === c))
                  .map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
              </select>
            )}
            <button
              onClick={addFilterCol}
              disabled={!newFilterCol.trim()}
              className="flex items-center gap-1 px-2 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-300 hover:bg-gray-700 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <Plus size={11} /> Add
            </button>
          </div>
        </div>

        {/* Mandatory Columns */}
        <div>
          <div className="flex items-center gap-2 mb-2">
            <Shield size={13} className="text-orange-400" />
            <h2 className="text-sm font-semibold text-gray-300">Mandatory Columns</h2>
          </div>
          <p className="text-[10px] text-gray-500 mb-2">
            These filter columns must have a value selected before data can be loaded.
          </p>
          <div className="flex flex-wrap gap-2 mb-2">
            {mandatoryColumns.map((col) => (
              <span key={col} className="flex items-center gap-1 px-2 py-1 rounded-full bg-orange-900/30 border border-orange-800/50 text-xs text-orange-300">
                {col}
                <button onClick={() => removeMandatoryCol(col)} className="hover:text-red-400 transition-colors">
                  <X size={11} />
                </button>
              </span>
            ))}
            {mandatoryColumns.length === 0 && (
              <span className="text-xs text-gray-600 italic">None</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <select
              value={newMandatoryCol}
              onChange={(e) => setNewMandatoryCol(e.target.value)}
              className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
            >
              <option value="">Select column...</option>
              {availableColumns
                .filter((c) => !mandatoryColumns.includes(c))
                .map((c) => (
                  <option key={c} value={c}>{c}</option>
                ))}
            </select>
            <button onClick={addMandatoryCol} className="flex items-center gap-1 px-2 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-300 hover:bg-gray-700">
              <Plus size={11} /> Add
            </button>
          </div>
        </div>

        {/* Cascading Rules */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <div className="flex items-center gap-2">
              <Link2 size={13} className="text-cyan-400" />
              <h2 className="text-sm font-semibold text-gray-300">Cascading Rules</h2>
            </div>
            <button onClick={addCascadingRule} className="flex items-center gap-1 text-xs text-blue-400 hover:text-blue-300">
              <Plus size={12} /> Add Rule
            </button>
          </div>
          <p className="text-[10px] text-gray-500 mb-2">
            When the trigger column value changes, the affected columns are re-queried with the new filter context.
          </p>
          {cascadingRules.length === 0 ? (
            <p className="text-xs text-gray-600 italic">No cascading rules. Filters operate independently.</p>
          ) : (
            <div className="space-y-2">
              {cascadingRules.map((rule, idx) => (
                <div key={`rule-${idx}`} className="p-3 rounded bg-gray-900/50 border border-gray-800 space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="text-[10px] font-medium text-gray-500 uppercase tracking-wide">Rule {idx + 1}</span>
                    <button onClick={() => deleteCascadingRule(idx)} className="p-1 rounded hover:bg-red-900/40 text-gray-500 hover:text-red-400">
                      <Trash2 size={12} />
                    </button>
                  </div>
                  <div className="flex items-start gap-3">
                    <div className="flex-1">
                      <label className="block text-[10px] text-gray-500 mb-0.5">Trigger</label>
                      <select
                        value={rule.trigger}
                        onChange={(e) => updateCascadingRule(idx, "trigger", e.target.value)}
                        className="w-full px-2 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                      >
                        <option value="">Select column...</option>
                        {availableColumns.map((c) => (
                          <option key={c} value={c}>{c}</option>
                        ))}
                      </select>
                    </div>
                    <div className="flex items-center pt-4 text-gray-500 text-[10px] shrink-0">→</div>
                    <div className="flex-[2]">
                      <label className="block text-[10px] text-gray-500 mb-0.5">Affects</label>
                      <div className="flex flex-wrap gap-1">
                        {rule.affects.map((a) => (
                          <span
                            key={`${idx}-${a}`}
                            className="flex items-center gap-1 px-1.5 py-0.5 rounded bg-cyan-900/30 border border-cyan-800/40 text-[10px] text-cyan-300"
                          >
                            {a}
                            <button
                              onClick={() =>
                                updateCascadingRule(idx, "affects", rule.affects.filter((x) => x !== a))
                              }
                              className="hover:text-red-400"
                            >
                              <X size={9} />
                            </button>
                          </span>
                        ))}
                        <select
                          value=""
                          onChange={(e) => {
                            if (e.target.value) {
                              updateCascadingRule(idx, "affects", [...rule.affects, e.target.value]);
                            }
                          }}
                          className="px-1.5 py-0.5 text-[10px] rounded bg-gray-800 border border-gray-700 text-gray-400 focus:outline-none focus:border-blue-500"
                        >
                          <option value="">+ add</option>
                          {availableColumns
                            .filter((c) => c !== rule.trigger && !rule.affects.includes(c))
                            .map((c) => (
                              <option key={c} value={c}>{c}</option>
                            ))}
                        </select>
                      </div>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Named Filters (placeholder) */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-2">Named Filters</h2>
          <div className="rounded border border-gray-800 bg-gray-900/30 p-4">
            <p className="text-xs text-gray-500">
              Saved filter value sets for testing and reuse. Each named filter stores pre-selected values
              for the filter columns above, allowing quick switching between common filter combinations.
            </p>
            <div className="mt-3 space-y-2">
              <div className="flex items-center gap-2 px-2 py-1.5 rounded bg-gray-800/50 border border-gray-700/50">
                <span className="text-xs text-gray-400 italic">No saved filter sets yet</span>
              </div>
            </div>
            <p className="text-[11px] text-gray-600 mt-2">
              This feature is coming soon. Filter sets will be configurable per filter config.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
