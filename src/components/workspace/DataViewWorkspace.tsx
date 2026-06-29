import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { Save, Columns3, Filter, Code2, Loader2, Eye, EyeOff, ArrowUpDown, Search, RefreshCw, Trash2, Plus, AlertTriangle, Link2, Pencil, GripVertical, Network, ChevronRight, ChevronLeft, ChevronDown, ShieldAlert, Layers } from "lucide-react";
import { api } from "@/api/client";
import { DataViewPreview } from "@/components/DataViewPreview";
import { useWorkspaceStore } from "@/stores/workspace";

/// Wire shape the backend now requires for `dataviews.source`. Inline
/// kind shapes (`{type:"duckdb_table",config:{table_name}}`, etc.) are
/// no longer accepted — see `server/src/handlers/dataview_source.rs:46`
/// (`resolve_source_binding`). Legacy DataViews with the inline shape
/// surface as a "Migrate" affordance in the SchemaTab.
type SourceBinding = {
  type: "source";
  config: { source_id: string; output?: string };
};
function isBoundSource(s: any): s is SourceBinding {
  return s && typeof s === "object" && s.type === "source" && typeof s?.config?.source_id === "string";
}

const TABS = ["Schema", "Live View", "Tree View", "Detail View", "Exception View", "Filter Configurations", "Generate"] as const;
type Tab = (typeof TABS)[number];

// Tabs that only render meaningful content for graph-backed sources.
// When the bound source's kind isn't `article_graph`, these tabs are hidden
// from the tab strip rather than rendering an inline "not available" stub.
const GRAPH_ONLY_TABS: ReadonlySet<Tab> = new Set([
  "Tree View",
  "Detail View",
  "Exception View",
]);

const TAB_ICONS: Record<Tab, React.ReactNode> = {
  Schema: <Columns3 size={13} />,
  "Live View": <Eye size={13} />,
  "Tree View": <Network size={13} />,
  "Detail View": <Layers size={13} />,
  "Exception View": <ShieldAlert size={13} />,
  "Filter Configurations": <Filter size={13} />,
  Generate: <Code2 size={13} />,
};

interface DataViewWorkspaceProps {
  dataviewId: string;
}

export function DataViewWorkspace({ dataviewId }: DataViewWorkspaceProps) {
  const [dv, setDv] = useState<any>(null);
  const [activeTab, setActiveTab] = useState<Tab>("Schema");
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [renaming, setRenaming] = useState(false);
  // null until the bound source has been fetched (or the dv has no source).
  // Used to decide whether graph-only tabs (Tree/Detail/Exception View) are
  // shown — they're hidden for non-graph sources rather than
  // rendering an inline "not available" message.
  const [sourceKind, setSourceKind] = useState<string | null>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const select = useWorkspaceStore((s) => s.select);

  const loadDataView = useCallback(async () => {
    try {
      setError(null);
      const data = await api.getDataView(dataviewId);
      setDv(data);
    } catch (e: any) {
      setError(e.message || "Failed to load DataView");
    }
  }, [dataviewId]);

  useEffect(() => {
    setDv(null);
    setSourceKind(null);
    loadDataView();
  }, [loadDataView]);

  // Resolve the bound source's kind. Graph-only tabs hide unless this is
  // "graph". Subtabs (Tree/Detail/Exception) used to render their
  // own inline "not available" stub each — lifting the check up means the
  // tabs themselves disappear, which is what the user expects.
  useEffect(() => {
    let cancelled = false;
    const sourceId =
      dv?.source?.type === "source" ? (dv.source.config?.source_id as string | undefined) : null;
    if (!sourceId) {
      setSourceKind(null);
      return;
    }
    api.getSource(sourceId)
      .then((row: any) => { if (!cancelled) setSourceKind(row?.kind ?? null); })
      .catch(() => { if (!cancelled) setSourceKind(null); });
    return () => { cancelled = true; };
  }, [dv?.id, dv?.source]);

  const isGraphSource = sourceKind === "graph";
  const visibleTabs = TABS.filter((t) => isGraphSource || !GRAPH_ONLY_TABS.has(t));

  // If the active tab gets hidden (e.g. user switches between dataviews
  // backed by different source kinds), fall back to Schema.
  useEffect(() => {
    if (!visibleTabs.includes(activeTab)) {
      setActiveTab("Schema");
    }
  }, [visibleTabs, activeTab]);

  const handleDelete = async () => {
    const label = dv?.display_name || dataviewId;
    if (!window.confirm(`Delete DataView "${label}"? This cannot be undone.`)) return;
    setDeleting(true);
    try {
      await api.deleteDataView(dataviewId);
      select(null);
      window.dispatchEvent(new CustomEvent("dataviews-changed"));
    } catch (e: any) {
      setDeleting(false);
      setError(e.message || "Delete failed");
    }
  };

  const beginRename = () => {
    if (renaming) return;
    setNameDraft(dv?.display_name || "");
    setEditingName(true);
  };

  const cancelRename = () => {
    setEditingName(false);
    setNameDraft("");
  };

  const commitRename = async () => {
    const trimmed = nameDraft.trim();
    // Empty trimmed → invalid: keep the input visible, don't save.
    if (!trimmed) return;
    // No-op rename: just close the input.
    if (trimmed === (dv?.display_name || "")) {
      setEditingName(false);
      return;
    }
    setRenaming(true);
    try {
      await api.updateDataView(dataviewId, { display_name: trimmed });
      setEditingName(false);
      setNameDraft("");
      await loadDataView();
      // Sidebar listens for this and refetches the DataView list.
      window.dispatchEvent(new CustomEvent("dataviews-changed"));
    } catch (e: any) {
      alert("Rename failed: " + (e?.message || "Unknown error"));
    } finally {
      setRenaming(false);
    }
  };

  // Focus + select-all when entering edit mode so users can overwrite cleanly.
  useEffect(() => {
    if (editingName && nameInputRef.current) {
      nameInputRef.current.focus();
      nameInputRef.current.select();
    }
  }, [editingName]);

  if (error) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-center">
          <p className="text-red-400 text-sm">{error}</p>
          <button onClick={loadDataView} className="mt-3 text-xs text-blue-400 hover:text-blue-300">
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (!dv) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-sm text-gray-500">Loading...</div>
      </div>
    );
  }

  // Header badge: prefer the bound source's kind (the new shape), else
  // mark legacy inline shapes for the user. Detail lookup happens in
  // the Schema tab.
  const sourceLabel = isBoundSource(dv.source)
    ? "Source"
    : dv.source?.type
      ? `Legacy: ${dv.source.type}`
      : "No source";

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col">
      {/* Header */}
      <div className="px-5 pt-4 pb-0 shrink-0">
        <div className="flex items-center gap-3 mb-3 min-w-0">
          {editingName ? (
            <input
              ref={nameInputRef}
              type="text"
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitRename();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  cancelRename();
                }
              }}
              onBlur={() => {
                // Save on blur, but if the trimmed value is empty just cancel
                // so the user isn't trapped in an invalid state.
                if (!nameDraft.trim()) {
                  cancelRename();
                } else {
                  commitRename();
                }
              }}
              disabled={renaming}
              className="flex-1 min-w-0 text-lg font-semibold text-gray-100 bg-gray-950 border border-gray-700 rounded px-2 py-0.5 focus:outline-none focus:border-blue-500 disabled:opacity-60"
            />
          ) : (
            <div className="flex items-center gap-1.5 min-w-0">
              <h1
                className="text-lg font-semibold text-gray-100 truncate cursor-text"
                onDoubleClick={beginRename}
                title="Double-click to rename"
              >
                {dv.display_name}
              </h1>
              <button
                onClick={beginRename}
                title="Rename DataView"
                className="text-gray-500 hover:text-gray-200 transition-colors shrink-0"
              >
                <Pencil size={13} />
              </button>
            </div>
          )}
          <span className="text-[10px] px-2 py-0.5 rounded bg-blue-900/50 text-blue-400 font-medium shrink-0">
            {sourceLabel}
          </span>
          <button
            onClick={handleDelete}
            disabled={deleting}
            title="Delete this DataView"
            className="ml-auto flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-red-700 hover:bg-red-600 text-white disabled:opacity-50 transition-colors shrink-0"
          >
            {deleting ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
            Delete
          </button>
        </div>

        {/* Tabs */}
        <div className="flex gap-4 border-b border-gray-800">
          {visibleTabs.map((tab) => (
            <button
              key={tab}
              onClick={() => setActiveTab(tab)}
              className={`pb-2 text-sm flex items-center gap-1.5 transition-colors ${
                activeTab === tab
                  ? "text-blue-400 border-b-2 border-blue-500"
                  : "text-gray-400 hover:text-gray-200"
              }`}
            >
              {TAB_ICONS[tab]}
              {tab}
            </button>
          ))}
        </div>
      </div>

      {/* Tab Content */}
      <div className="flex-1 overflow-auto px-5 py-4">
        {activeTab === "Schema" && <SchemaTab dataviewId={dataviewId} dv={dv} onReload={loadDataView} />}
        {activeTab === "Live View" && <PreviewTab dv={dv} />}
        {activeTab === "Tree View" && <TreeViewTab dv={dv} />}
        {activeTab === "Detail View" && <DetailViewTab dv={dv} />}
        {activeTab === "Exception View" && <ExceptionViewTab dv={dv} />}
        {activeTab === "Filter Configurations" && <FiltersTab dv={dv} onReload={loadDataView} />}
        {activeTab === "Generate" && <GenerateTab dv={dv} />}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Schema Tab — source card + introspected columns                   */
/* ------------------------------------------------------------------ */

interface SchemaTabProps {
  dataviewId: string;
  dv: any;
  onReload: () => void;
}

const SOURCE_KINDS = [
  { value: "duckdb_table",  label: "DuckDB Table",  desc: "An existing table in tenant_data.duckdb" },
  { value: "parquet_glob",  label: "Parquet Glob",  desc: "Files on disk under PARQUET_HOME" },
  { value: "duckdb_query",  label: "DuckDB Query",  desc: "SQL evaluated against tenant_data.duckdb" },
  { value: "pg_query",      label: "Postgres Query", desc: "SQL evaluated against a PG data_source" },
  { value: "ch_query",      label: "ClickHouse Query", desc: "SQL evaluated against a ClickHouse connection (HTTP interface; honors allow_write_access on the connection)" },
  { value: "bq_query",      label: "BigQuery Query", desc: "SQL evaluated against BQ (not yet implemented)" },
  { value: "graph", label: "Graph",  desc: "Rows from the in-memory graph snapshot (built by pl_build_article_graph). UAM, when needed, is read from the materialized `uam_summary` DuckDB table via duckdb_table." },
];

const KIND_LABEL: Record<string, string> = SOURCE_KINDS.reduce((acc, k) => {
  acc[k.value] = k.label;
  return acc;
}, {} as Record<string, string>);

function SchemaTab({ dataviewId, dv, onReload }: SchemaTabProps) {
  const stored = (dv.source && typeof dv.source === "object") ? dv.source : null;
  const bound = isBoundSource(stored) ? stored.config.source_id : null;
  // Inline-shape DataViews still floating around from before the
  // `source` binding model. We surface a "Migrate" affordance.
  const legacyInline = stored && !isBoundSource(stored) && typeof stored.type === "string"
    ? { type: stored.type as string, config: stored.config ?? {} }
    : null;

  const [sources, setSources] = useState<any[]>([]);
  const [pickedSourceId, setPickedSourceId] = useState<string>(bound ?? "");
  const [introErr, setIntroErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<null | "save" | "introspect" | "create" | "save-columns">(null);
  const [showCreate, setShowCreate] = useState(false);

  // Editable per-column configuration (visible / sortable / searchable /
  // editable / display_name). Initialized from `dv.columns`; introspect
  // merges in any newly-discovered columns from the source while
  // preserving flags for columns that still exist.
  type ColumnCfg = {
    name: string;
    type: string;
    display_name?: string;
    visible: boolean;
    sortable: boolean;
    searchable: boolean;
    editable: boolean;
  };
  const [editedColumns, setEditedColumns] = useState<ColumnCfg[]>([]);
  const [dragIdx, setDragIdx] = useState<number | null>(null);
  const [columnsDirty, setColumnsDirty] = useState(false);

  // Sync edited columns from dv on load / reload — but skip when the
  // user has pending unsaved edits (e.g. just merged in introspection
  // results), otherwise the reload after a source-binding save would
  // blow them away.
  const dirtyRef = useRef(false);
  useEffect(() => {
    dirtyRef.current = columnsDirty;
  }, [columnsDirty]);
  useEffect(() => {
    if (dirtyRef.current) return;
    const raw: any[] = Array.isArray(dv.columns) ? dv.columns : [];
    setEditedColumns(
      raw.map((c) => ({
        name: String(c.name ?? ""),
        type: String(c.type ?? "VARCHAR"),
        display_name: c.display_name ?? c.name,
        visible: c.visible !== false,
        sortable: !!c.sortable,
        searchable: !!c.searchable,
        editable: !!c.editable,
      })),
    );
  }, [dv.id, dv.columns]);

  // Load sources on mount + after any create/edit so the picker stays
  // live. Show all kinds — DataView source resolver projects each kind
  // into the right inline shape downstream.
  const reloadSources = useCallback(async () => {
    try {
      const all = await api.getSources();
      setSources(all || []);
    } catch (_) {
      setSources([]);
    }
  }, []);
  useEffect(() => {
    reloadSources();
  }, [reloadSources]);

  // Re-sync the picker when the DataView reloads with a new binding.
  useEffect(() => {
    setPickedSourceId(bound ?? "");
  }, [dataviewId, bound]);

  const pickedSource = sources.find((s) => s.id === pickedSourceId);

  const handleSave = async () => {
    if (!pickedSourceId) {
      alert("Pick a source (or create a new one) before saving.");
      return;
    }
    setBusy("save");
    try {
      const binding: SourceBinding = {
        type: "source",
        config: { source_id: pickedSourceId },
      };
      await api.updateDataView(dataviewId, { source: binding });
      onReload();
    } catch (e: any) {
      alert("Save failed: " + (e?.message || "Unknown error"));
    } finally {
      setBusy(null);
    }
  };

  const handleIntrospect = async () => {
    if (!pickedSourceId) {
      alert("Bind to a source first.");
      return;
    }
    setBusy("introspect");
    setIntroErr(null);
    try {
      const binding: SourceBinding = {
        type: "source",
        config: { source_id: pickedSourceId },
      };
      await api.updateDataView(dataviewId, { source: binding });
      const r = await api.introspectDataViewSource(dataviewId);
      const cols = r.columns || [];
      // Merge into the editor: keep flags for columns that still exist,
      // append newly-discovered columns with sensible defaults, drop
      // columns that the source no longer returns.
      setEditedColumns((prev) => {
        const byName = new Map(prev.map((c) => [c.name, c]));
        const next: ColumnCfg[] = cols.map((c) => {
          const existing = byName.get(c.name);
          if (existing) return { ...existing, type: c.type };
          return {
            name: c.name,
            type: c.type,
            display_name: c.name,
            visible: true,
            sortable: true,
            searchable: false,
            editable: false,
          };
        });
        setColumnsDirty(true);
        return next;
      });
      onReload();
    } catch (e: any) {
      setIntroErr(e?.message || "Introspection failed");
    } finally {
      setBusy(null);
    }
  };

  const toggleColFlag = (idx: number, key: keyof ColumnCfg) => {
    setEditedColumns((prev) => {
      const copy = prev.slice();
      const c = { ...copy[idx], [key]: !(copy[idx] as any)[key] };
      copy[idx] = c;
      return copy;
    });
    setColumnsDirty(true);
  };
  const setColDisplayName = (idx: number, value: string) => {
    setEditedColumns((prev) => {
      const copy = prev.slice();
      copy[idx] = { ...copy[idx], display_name: value };
      return copy;
    });
    setColumnsDirty(true);
  };
  const setAllVisible = (visible: boolean) => {
    setEditedColumns((prev) => prev.map((c) => ({ ...c, visible })));
    setColumnsDirty(true);
  };
  const removeColumn = (idx: number) => {
    setEditedColumns((prev) => prev.filter((_, i) => i !== idx));
    setColumnsDirty(true);
  };
  const onColDragStart = (idx: number) => setDragIdx(idx);
  const onColDragOver = (e: React.DragEvent, idx: number) => {
    e.preventDefault();
    if (dragIdx === null || dragIdx === idx) return;
    setEditedColumns((prev) => {
      const copy = prev.slice();
      const [moved] = copy.splice(dragIdx, 1);
      copy.splice(idx, 0, moved);
      return copy;
    });
    setDragIdx(idx);
    setColumnsDirty(true);
  };
  const onColDragEnd = () => setDragIdx(null);

  const handleSaveColumns = async () => {
    setBusy("save-columns");
    try {
      await api.updateDataView(dataviewId, { columns: editedColumns });
      setColumnsDirty(false);
      onReload();
    } catch (e: any) {
      alert("Save columns failed: " + (e?.message || "Unknown error"));
    } finally {
      setBusy(null);
    }
  };

  // After NewSourceForm creates a source row, persist the binding to
  // this DataView and close the inline form. Without the explicit
  // updateDataView call, "Create + bind" only updated local state and
  // the user had to remember to click "Save binding" afterwards — the
  // source was created but the DataView still pointed at the old
  // (or no) source on the next reload.
  const onSourceCreated = async (newSource: any) => {
    setShowCreate(false);
    setPickedSourceId(newSource.id);
    reloadSources();
    try {
      await api.updateDataView(dataviewId, {
        source: { type: "source", config: { source_id: newSource.id } },
      });
      onReload();
    } catch (e: any) {
      alert("Created source but binding failed: " + (e?.message || "Unknown error"));
    }
  };

  return (
    <div className="space-y-4">
      {/* Legacy inline-shape banner */}
      {legacyInline && (
        <div className="rounded border border-amber-800 bg-amber-950/30 p-3 text-xs text-amber-300 flex items-start gap-2">
          <AlertTriangle size={14} className="text-amber-400 shrink-0 mt-0.5" />
          <div className="space-y-1">
            <div className="font-medium">Legacy inline source: {KIND_LABEL[legacyInline.type] || legacyInline.type}</div>
            <div className="text-amber-400/80">
              Bind this DataView to a Source row instead. Pick an existing
              one below or click <span className="font-medium">Create source</span> to
              promote the inline config to a new Source.
            </div>
          </div>
        </div>
      )}

      {/* Source binding card */}
      <div className="rounded border border-gray-700 bg-gray-900 p-3 space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Link2 size={13} className="text-gray-400" />
            <span className="text-xs font-medium text-gray-200">Bound source</span>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setShowCreate((v) => !v)}
              disabled={!!busy}
              className="flex items-center gap-1 px-2.5 py-1 text-xs text-gray-300 hover:text-white border border-gray-700 rounded hover:border-gray-600 bg-gray-800 transition-colors disabled:opacity-50"
            >
              <Plus size={11} />
              {showCreate ? "Cancel" : "Create source"}
            </button>
            <button
              onClick={handleSave}
              disabled={!!busy || !pickedSourceId || pickedSourceId === bound}
              className="flex items-center gap-1 px-2.5 py-1 text-xs text-red-300 hover:text-red-200 border border-red-800 rounded hover:border-red-700 bg-red-950/50 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {busy === "save" ? <Loader2 size={11} className="animate-spin" /> : <Save size={11} />}
              Save binding
            </button>
            <button
              onClick={handleIntrospect}
              disabled={!!busy || !pickedSourceId}
              className="flex items-center gap-1 px-2.5 py-1 text-xs text-gray-300 hover:text-white border border-gray-700 rounded hover:border-gray-600 bg-gray-800 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {busy === "introspect" ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
              Introspect schema
            </button>
          </div>
        </div>

        <div>
          <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Source</span>
          <select
            value={pickedSourceId}
            onChange={(e) => setPickedSourceId(e.target.value)}
            className="w-full px-2 py-1 text-[12px] rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
          >
            <option value="">— pick a source —</option>
            {sources.map((s: any) => (
              <option key={s.id} value={s.id}>
                {(s.display_name || s.id)} · {KIND_LABEL[s.kind] || s.kind} · {s.id}
              </option>
            ))}
          </select>
          {sources.length === 0 && (
            <div className="text-[10px] text-gray-600 mt-1">
              No sources defined yet. Click <span className="text-gray-400">Create source</span> to add one.
            </div>
          )}
        </div>

        {/* Bound source details (read-only) */}
        {pickedSource && <BoundSourceDetails source={pickedSource} />}
      </div>

      {/* Inline create form */}
      {showCreate && (
        <NewSourceForm
          presetFromLegacy={legacyInline}
          existingSourceIds={new Set(sources.map((s) => s.id))}
          onCreated={onSourceCreated}
          onCancel={() => setShowCreate(false)}
          busy={busy === "create"}
          setBusy={(b) => setBusy(b ? "create" : null)}
        />
      )}

      {/* Columns editor (visible / sortable / searchable / editable + reorder) */}
      <div className="rounded border border-gray-800">
        <div className="px-3 py-2 border-b border-gray-800 flex items-center gap-3">
          <span className="text-xs font-medium text-gray-300">
            Columns
            {editedColumns.length > 0 && (
              <span className="text-gray-500 ml-1">
                — {editedColumns.filter((c) => c.visible).length}/{editedColumns.length} visible
              </span>
            )}
          </span>
          {editedColumns.length > 0 && (
            <>
              <button
                onClick={() => setAllVisible(true)}
                className="text-[10px] text-blue-400 hover:underline"
                title="Make every column visible"
              >
                Show all
              </button>
              <button
                onClick={() => setAllVisible(false)}
                className="text-[10px] text-gray-400 hover:underline"
                title="Hide every column"
              >
                Hide all
              </button>
            </>
          )}
          <button
            onClick={handleSaveColumns}
            disabled={!columnsDirty || busy === "save-columns"}
            title={columnsDirty ? "Save column flags + order" : "No pending changes"}
            className={`ml-auto flex items-center gap-1 px-2.5 py-1 text-xs rounded transition-colors ${
              columnsDirty
                ? "text-red-300 hover:text-red-200 border border-red-800 hover:border-red-700 bg-red-950/50"
                : "text-gray-500 border border-gray-800 bg-gray-900/40 cursor-not-allowed"
            }`}
          >
            {busy === "save-columns" ? <Loader2 size={11} className="animate-spin" /> : <Save size={11} />}
            Save columns
          </button>
        </div>
        {introErr && (
          <pre className="text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words bg-red-950/30 px-3 py-2">{introErr}</pre>
        )}
        {editedColumns.length === 0 && !introErr && (
          <div className="px-3 py-6 text-center text-xs text-gray-600">
            Click <span className="font-medium text-gray-300">Introspect schema</span> to load columns from the source.
          </div>
        )}
        {editedColumns.length > 0 && (
          <div className="divide-y divide-gray-800/50">
            {editedColumns.map((col, idx) => (
              <div
                key={`${col.name}-${idx}`}
                draggable
                onDragStart={() => onColDragStart(idx)}
                onDragOver={(e) => onColDragOver(e, idx)}
                onDragEnd={onColDragEnd}
                className={`flex items-center gap-2 px-3 py-1.5 text-xs cursor-grab active:cursor-grabbing transition-colors ${
                  dragIdx === idx ? "bg-blue-950/30" : col.visible ? "hover:bg-gray-900/40" : "bg-gray-950/30 opacity-70"
                }`}
              >
                <GripVertical size={12} className="text-gray-600 shrink-0" />
                <button
                  onClick={() => toggleColFlag(idx, "visible")}
                  title={col.visible ? "Hide in tables / preview" : "Show in tables / preview"}
                  className={`shrink-0 ${col.visible ? "text-blue-400" : "text-gray-600"}`}
                >
                  {col.visible ? <Eye size={13} /> : <EyeOff size={13} />}
                </button>
                <div className="flex-1 min-w-0 grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_120px] gap-2 items-baseline">
                  <span className={`font-mono truncate ${col.visible ? "text-gray-200" : "text-gray-500"}`}>{col.name}</span>
                  <input
                    value={col.display_name ?? ""}
                    onChange={(e) => setColDisplayName(idx, e.target.value)}
                    placeholder="display name"
                    className="px-1.5 py-0.5 text-xs rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
                  />
                  <span className="text-[10px] text-gray-500 font-mono truncate">{col.type}</span>
                </div>
                <div className="flex items-center gap-0.5 shrink-0">
                  <button
                    onClick={() => toggleColFlag(idx, "sortable")}
                    title="Sortable in the table header"
                    className={`w-6 h-6 flex items-center justify-center rounded ${col.sortable ? "bg-blue-950/60 text-blue-300" : "text-gray-600 hover:text-gray-300"}`}
                  >
                    <ArrowUpDown size={11} />
                  </button>
                  <button
                    onClick={() => toggleColFlag(idx, "searchable")}
                    title="Show as a per-column search input"
                    className={`w-6 h-6 flex items-center justify-center rounded ${col.searchable ? "bg-green-950/60 text-green-300" : "text-gray-600 hover:text-gray-300"}`}
                  >
                    <Search size={11} />
                  </button>
                  <button
                    onClick={() => toggleColFlag(idx, "editable")}
                    title="Allow inline cell edits"
                    className={`w-6 h-6 flex items-center justify-center rounded ${col.editable ? "bg-amber-950/60 text-amber-300" : "text-gray-600 hover:text-gray-300"}`}
                  >
                    <Pencil size={11} />
                  </button>
                  <button
                    onClick={() => removeColumn(idx)}
                    title="Remove from this DataView"
                    className="w-6 h-6 flex items-center justify-center rounded text-gray-600 hover:text-red-400 ml-0.5"
                  >
                    <Trash2 size={11} />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}


/* ------------------------------------------------------------------ */
/*  BoundSourceDetails — read-only view of a picked source             */
/* ------------------------------------------------------------------ */

function BoundSourceDetails({ source }: { source: any }) {
  const cfg = source.config || {};
  const item = (label: string, value: any) =>
    value !== undefined && value !== null && value !== "" ? (
      <div className="flex items-baseline gap-2">
        <span className="text-[10px] text-gray-500 uppercase tracking-wider w-32 shrink-0">
          {label}
        </span>
        <span className="text-xs font-mono text-gray-300 break-all">{String(value)}</span>
      </div>
    ) : null;
  return (
    <div className="rounded bg-gray-950/40 border border-gray-800 p-3 text-xs space-y-1">
      {item("id", source.id)}
      {item("kind", KIND_LABEL[source.kind] || source.kind)}
      {item("display_name", source.display_name)}
      {item("target_table", source.target_table)}
      {item("connection_ref", source.connection_ref)}
      {source.kind === "duckdb_table" && item("table_name", cfg.table_name)}
      {source.kind === "parquet_glob" && item("path", cfg.path)}
      {(source.kind === "duckdb_query" || source.kind === "pg_query" || source.kind === "bq_query") &&
        cfg.sql && (
          <div className="pt-1">
            <span className="text-[10px] text-gray-500 uppercase tracking-wider block mb-1">SQL</span>
            <pre className="text-[11px] font-mono text-gray-300 whitespace-pre-wrap break-words bg-gray-900/50 px-2 py-1 rounded">
              {cfg.sql}
            </pre>
          </div>
        )}
      {source.kind === "graph" && (
        <>
          {item("node_kind", cfg.node_kind || "ARTICLE")}
          <div className="pt-1 text-[10px] text-gray-600">
            Reads from the in-memory legacy graph (built by{" "}
            <span className="text-gray-400">pl_build_article_graph</span>).
          </div>
        </>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  NewSourceForm — inline create flow                                 */
/* ------------------------------------------------------------------ */

interface NewSourceFormProps {
  presetFromLegacy: { type: string; config: any } | null;
  existingSourceIds: Set<string>;
  onCreated: (s: any) => void;
  onCancel: () => void;
  busy: boolean;
  setBusy: (b: boolean) => void;
}

function NewSourceForm({
  presetFromLegacy,
  existingSourceIds,
  onCreated,
  onCancel,
  busy,
  setBusy,
}: NewSourceFormProps) {
  // Default the new source to the legacy DataView's inline shape if we
  // have one — that's the explicit "promote inline → source" flow.
  const [kind, setKind] = useState<string>(presetFromLegacy?.type || "duckdb_table");
  const [cfg, setCfg] = useState<Record<string, any>>(presetFromLegacy?.config || {});
  const [displayName, setDisplayName] = useState("");
  const [id, setId] = useState("");
  const [pgConnections, setPgConnections] = useState<any[]>([]);

  useEffect(() => {
    api
      .getDataSources()
      .then((all) =>
        setPgConnections(all.filter((d: any) => d.type === "pg" || d.type === "postgres")),
      )
      .catch(() => {});
  }, []);

  // Auto-suggest an id slug from the display_name. Keep editable.
  const suggestId = (name: string) =>
    "src_" + name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
  const onDisplayNameChange = (v: string) => {
    setDisplayName(v);
    if (!id || id === suggestId(displayName)) {
      setId(suggestId(v));
    }
  };

  const setKindAndReset = (k: string) => {
    setKind(k);
    setCfg({});
  };
  const setCfgPatch = (patch: Record<string, any>) =>
    setCfg((prev) => ({ ...prev, ...patch }));

  const submit = async () => {
    if (!id.trim() || !displayName.trim()) {
      alert("id and display_name are required");
      return;
    }
    if (existingSourceIds.has(id)) {
      alert(`Source id '${id}' already exists`);
      return;
    }
    setBusy(true);
    try {
      const body: Record<string, any> = {
        id: id.trim(),
        display_name: displayName.trim(),
        kind,
        config: cfg,
      };
      // duckdb_table sources use target_table to reflect what they
      // point at downstream.
      if (kind === "duckdb_table" && cfg.table_name) {
        body.target_table = cfg.table_name;
      }
      const created = await api.createSource(body);
      onCreated(created);
    } catch (e: any) {
      alert("Create failed: " + (e?.message || "Unknown error"));
    } finally {
      setBusy(false);
    }
  };

  const txt =
    "w-full px-2 py-1 text-[12px] rounded bg-gray-950 border border-gray-800 text-gray-200 font-mono focus:outline-none focus:border-blue-500";
  const ta = `${txt} resize-y min-h-[80px]`;

  return (
    <div className="rounded border border-blue-900/60 bg-blue-950/10 p-3 space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-blue-300">Create new source</span>
        <div className="flex items-center gap-2">
          <button
            onClick={onCancel}
            disabled={busy}
            className="px-2.5 py-1 text-xs text-gray-400 hover:text-gray-200 transition-colors disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={busy || !id || !displayName}
            className="flex items-center gap-1 px-2.5 py-1 text-xs text-white bg-blue-600 hover:bg-blue-500 rounded transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {busy ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
            Create + bind
          </button>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div>
          <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Display name</span>
          <input
            type="text"
            value={displayName}
            onChange={(e) => onDisplayNameChange(e.target.value)}
            className={txt}
            placeholder="article_selection (DuckDB)"
          />
        </div>
        <div>
          <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Source id</span>
          <input
            type="text"
            value={id}
            onChange={(e) => setId(e.target.value)}
            className={txt}
            placeholder="src_my_source"
          />
        </div>
        <div>
          <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Kind</span>
          <select value={kind} onChange={(e) => setKindAndReset(e.target.value)} className={txt}>
            {SOURCE_KINDS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
          <div className="text-[10px] text-gray-600 mt-0.5">
            {SOURCE_KINDS.find((k) => k.value === kind)?.desc}
          </div>
        </div>
        <div />

        {kind === "duckdb_table" && (
          <div className="col-span-2">
            <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Table name</span>
            <input
              type="text"
              value={cfg.table_name ?? ""}
              onChange={(e) => setCfgPatch({ table_name: e.target.value })}
              className={txt}
              placeholder="article_selection"
            />
          </div>
        )}

        {kind === "parquet_glob" && (
          <>
            <div className="col-span-2">
              <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Path</span>
              <input
                type="text"
                value={cfg.path ?? ""}
                onChange={(e) => setCfgPatch({ path: e.target.value })}
                className={txt}
                placeholder="dataset/"
              />
              <div className="text-[10px] text-gray-600 mt-0.5">
                Relative to PARQUET_HOME, or absolute / gs:// path.
              </div>
            </div>
            <div>
              <label className="flex items-center gap-1.5 text-[11px] text-gray-300">
                <input
                  type="checkbox"
                  checked={cfg.hive_partitioning !== false}
                  onChange={(e) => setCfgPatch({ hive_partitioning: e.target.checked })}
                />
                hive_partitioning
              </label>
            </div>
          </>
        )}

        {kind === "duckdb_query" && (
          <div className="col-span-2">
            <span className="block text-[10px] text-gray-500 font-medium mb-0.5">SQL</span>
            <textarea
              value={cfg.sql ?? ""}
              onChange={(e) => setCfgPatch({ sql: e.target.value })}
              className={ta}
              placeholder="SELECT * FROM my_table"
            />
          </div>
        )}

        {kind === "pg_query" && (
          <>
            <div>
              <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Connection</span>
              <select
                value={cfg.connection_ref ?? ""}
                onChange={(e) =>
                  setCfgPatch({ connection_ref: e.target.value || undefined })
                }
                className={txt}
              >
                <option value="">(default — first marked or first PG)</option>
                {pgConnections.map((c: any) => (
                  <option key={c.id} value={c.id}>
                    {c.display_name || c.id}
                    {c.is_default ? " ★" : ""}
                  </option>
                ))}
              </select>
            </div>
            <div />
            <div className="col-span-2">
              <span className="block text-[10px] text-gray-500 font-medium mb-0.5">SQL</span>
              <textarea
                value={cfg.sql ?? ""}
                onChange={(e) => setCfgPatch({ sql: e.target.value })}
                className={ta}
                placeholder="SELECT id, name FROM stores"
              />
            </div>
          </>
        )}

        {kind === "bq_query" && (
          <div className="col-span-2 text-[11px] text-amber-400">
            BigQuery introspection / read isn't wired up yet. Save will persist; introspection will return 501.
          </div>
        )}

        {kind === "graph" && (
          <div className="col-span-2 space-y-2">
            <div>
              <span className="block text-[10px] text-gray-500 font-medium mb-0.5">Node kind</span>
              <select
                value={cfg.node_kind ?? "ARTICLE"}
                onChange={(e) => setCfgPatch({ node_kind: e.target.value })}
                className={txt}
              >
                <option value="ARTICLE">ARTICLE — one row per article (with hierarchy + rolled-up metrics)</option>
                <option value="L0">L0 — one row per L0, aggregated</option>
                <option value="L1">L1</option>
                <option value="L2">L2</option>
                <option value="L3">L3</option>
                <option value="L4">L4</option>
                <option value="L5">L5</option>
                <option value="PRODUCT_CODE">PRODUCT_CODE — one row per product</option>
                <option value="STORE_CODE">STORE_CODE — one row per active store</option>
              </select>
            </div>
            <div className="text-[10px] text-gray-600">
              Reads directly from the in-memory legacy graph snapshot. Run{" "}
              <span className="text-gray-400">pl_build_article_graph</span> first.
            </div>
          </div>
        )}
      </div>
    </div>
  );
}


/* ------------------------------------------------------------------ */
/*  Live View Tab — interactive preview of materialized data           */
/* ------------------------------------------------------------------ */

function PreviewTab({ dv }: { dv: any }) {
  return (
    <div className="h-full -mx-5 -my-4">
      <div className="h-full bg-gray-950 p-4">
        <DataViewPreview dataview={dv} />
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Tree View Tab — drill-down hierarchy with rolled-up metrics        */
/*                                                                     */
/*  Renders one tree per dimension (product / store). Roots are the    */
/*  top-level nodes for that dimension (L0 for product, CHANNEL for    */
/*  store). Children are fetched lazily via /api/graph/articles/traverse on     */
/*  expand. Every node carries pre-aggregated metrics from the V8      */
/*  graph rollup pass — no SQL, no recomputation per row.              */
/* ------------------------------------------------------------------ */

type TreeKind = "L0" | "L1" | "L2" | "L3" | "L4" | "L5" | "ARTICLE" | "PRODUCT_CODE" | "CHANNEL" | "STORE_CODE" | "BRAND";

interface TreeNode {
  // Stable key for the expansion-state set: "<kind>::<name>".
  key: string;
  kind: TreeKind;
  name: string;
  display: string;
  // Metrics — populated from project_single rows. Hierarchy + Article
  // nodes carry rolled-up metrics; product_code / store_code nodes
  // typically don't.
  oh?: number;
  oo?: number;
  it?: number;
  lw_units?: number;
  lw_revenue?: number;
  lw_margin?: number;
  child_count?: number;
  // Rendered after first expand. Undefined = not yet fetched.
  // Empty array = leaf node (already verified, no further drill-down).
  children?: TreeNode[];
  loadingChildren?: boolean;
  errorChildren?: string;
}

const NEXT_KIND_FOR: Partial<Record<TreeKind, TreeKind>> = {
  L0: "L1",
  L1: "L2",
  L2: "L3",
  L3: "L4",
  L4: "L5",
  L5: "ARTICLE",
  ARTICLE: "PRODUCT_CODE",
  CHANNEL: "STORE_CODE",
  BRAND: "ARTICLE",
};

function makeNodeFromRow(row: any, kind: TreeKind): TreeNode {
  // The projection returns different "primary name" columns per kind.
  // Pull whichever one matches the kind so the tree renders correctly
  // even though the row payloads differ.
  const primaryName: string = (() => {
    switch (kind) {
      case "ARTICLE": return String(row.article ?? "");
      case "PRODUCT_CODE": return String(row.product_code ?? "");
      case "CHANNEL": return String(row.channel ?? "");
      case "STORE_CODE": return String(row.store_code ?? "");
      default: return String(row.name ?? "");
    }
  })();
  return {
    key: `${kind}::${primaryName}`,
    kind,
    name: primaryName,
    display: primaryName,
    oh: typeof row.oh === "number" ? row.oh : undefined,
    oo: typeof row.oo === "number" ? row.oo : undefined,
    it: typeof row.it === "number" ? row.it : undefined,
    lw_units: typeof row.lw_units === "number" ? row.lw_units : undefined,
    lw_revenue: typeof row.lw_revenue === "number" ? row.lw_revenue : undefined,
    lw_margin: typeof row.lw_margin === "number" ? row.lw_margin : undefined,
    child_count: typeof row.child_count === "number" ? row.child_count : undefined,
  };
}

function TreeViewTab({ dv }: { dv: any }) {
  const [resolvedSourceKind, setResolvedSourceKind] = useState<string | null>(null);

  // Resolve the bound source's kind so we know whether the article_graph
  // path is available. The tree only makes sense for graph-backed views.
  useEffect(() => {
    let cancelled = false;
    const sourceId =
      dv?.source?.type === "source" ? (dv.source.config?.source_id as string | undefined) : null;
    if (!sourceId) { setResolvedSourceKind(null); return; }
    api.getSource(sourceId)
      .then((row: any) => { if (!cancelled) setResolvedSourceKind(row?.kind ?? null); })
      .catch(() => { if (!cancelled) setResolvedSourceKind(null); });
    return () => { cancelled = true; };
  }, [dv.id, dv?.source]);

  const isGraphSource = resolvedSourceKind === "graph";
  const [dimension, setDimension] = useState<"product" | "store">("product");
  const [roots, setRoots] = useState<TreeNode[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Expansion state lives outside `roots` so we can mutate it without
  // recreating the tree on every toggle.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  // ── Filter panel (multi-select only, regardless of filter-config setting) ──
  // Selections are pending until Apply; appliedFilters drive the
  // refresh and the filters payload sent to /data + /traverse.
  const [filterConfigs, setFilterConfigs] = useState<any[]>([]);
  const [pendingFilters, setPendingFilters] = useState<Record<string, string[]>>({});
  const [appliedFilters, setAppliedFilters] = useState<Record<string, string[]>>({});
  const [openFilterCol, setOpenFilterCol] = useState<string | null>(null);
  const [distinctCache, setDistinctCache] = useState<Record<string, string[]>>({});

  // Load filter configs bound to this dataview (same shape as Live View).
  useEffect(() => {
    let cancelled = false;
    const dims: any[] = Array.isArray(dv.dimensions) ? dv.dimensions : [];
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
        setFilterConfigs(wantedIds.map((id) => byId.get(id)).filter((fc): fc is any => !!fc));
      })
      .catch(() => setFilterConfigs([]));
    return () => { cancelled = true; };
  }, [dv.id, dv.dimensions]);

  // Flatten filter columns. Forced multi-select per the user's spec —
  // we ignore the config's `single_select` field on this surface.
  const filterEntries = useMemo(() => {
    const out: { col: string; display: string; order: number }[] = [];
    for (const fc of filterConfigs) {
      const cols: any[] = Array.isArray(fc.filter_columns) ? fc.filter_columns : [];
      for (const c of cols) {
        if (!c || typeof c !== "object" || !c.column) continue;
        out.push({
          col: String(c.column),
          display: String(c.display_name || c.column),
          order: typeof c.display_order === "number" ? c.display_order : 999,
        });
      }
    }
    out.sort((a, b) => a.order - b.order);
    return out;
  }, [filterConfigs]);

  const filtersDirty = useMemo(
    () => JSON.stringify(pendingFilters) !== JSON.stringify(appliedFilters),
    [pendingFilters, appliedFilters],
  );
  const filtersPayload = useMemo(() => {
    return Object.entries(appliedFilters)
      .filter(([_, vals]) => vals && vals.length > 0)
      .map(([col, values]) => ({ attribute_name: col, values, operator: "in" as const }));
  }, [appliedFilters]);
  const setFilterVal = (col: string, values: string[]) => {
    setPendingFilters((prev) => {
      const next = { ...prev };
      if (values.length === 0) delete next[col];
      else next[col] = values;
      return next;
    });
  };
  const applyFilters = () => setAppliedFilters(pendingFilters);
  const clearFilters = () => {
    setPendingFilters({});
    setAppliedFilters({});
  };
  // Distinct values for a column come from a one-shot graph query: pull
  // the column from a wide ARTICLE projection. Cheap once cached.
  const fetchDistinct = useCallback(
    async (col: string) => {
      if (distinctCache[col]) return;
      try {
        const res = await api.getDataViewData(dv.id, { limit: 5000, node_kind: "ARTICLE" });
        const vals = new Set<string>();
        for (const r of res.rows || []) {
          const v = (r as any)[col];
          if (v != null && v !== "") vals.add(String(v));
        }
        setDistinctCache((p) => ({ ...p, [col]: Array.from(vals).sort() }));
      } catch (_) {
        setDistinctCache((p) => ({ ...p, [col]: [] }));
      }
    },
    [dv.id, distinctCache],
  );

  // Load root nodes when dimension changes. Roots come via the dataview
  // /data path with node_kind override — that's the cheapest path for a
  // full level-0 list (no traverse needed).
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setRoots([]);
    setExpanded(new Set());
    const rootKind: TreeKind = dimension === "product" ? "L0" : "CHANNEL";
    api.getDataViewData(dv.id, { limit: 500, node_kind: rootKind, filters: filtersPayload })
      .then((res) => {
        if (cancelled) return;
        const out: TreeNode[] = (res.rows || [])
          .map((r: any) => makeNodeFromRow(r, rootKind))
          .sort((a, b) => a.display.localeCompare(b.display));
        setRoots(out);
      })
      .catch((e) => { if (!cancelled) setError(e?.message ?? "load failed"); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [dv.id, dimension, isGraphSource, filtersPayload]);

  // Walk the tree to find a node by key, mutate it, and return a fresh
  // copy of `roots` for setState. Pure (returns new arrays / objects on
  // the mutation path; siblings reused).
  const updateNode = useCallback(
    (key: string, mutate: (n: TreeNode) => TreeNode): void => {
      const walk = (nodes: TreeNode[]): TreeNode[] =>
        nodes.map((n) => {
          if (n.key === key) return mutate(n);
          if (n.children && n.children.length > 0) {
            const next = walk(n.children);
            if (next !== n.children) return { ...n, children: next };
          }
          return n;
        });
      setRoots((prev) => walk(prev));
    },
    [],
  );

  const ensureChildrenLoaded = useCallback(
    async (node: TreeNode) => {
      if (node.children !== undefined || node.loadingChildren) return;
      const childKind = NEXT_KIND_FOR[node.kind];
      if (!childKind) {
        // Leaf — mark with empty children so the chevron stops trying.
        updateNode(node.key, (n) => ({ ...n, children: [] }));
        return;
      }
      updateNode(node.key, (n) => ({ ...n, loadingChildren: true, errorChildren: undefined }));
      try {
        const edge = node.kind === "CHANNEL" ? "stores" : "children";
        const resp = await api.graphTraverse(
          { kind: node.kind as any, name: node.name },
          edge as any,
          filtersPayload,
        );
        const out: TreeNode[] = (resp.rows || [])
          .map((r: any) => makeNodeFromRow(r, childKind))
          .sort((a, b) => {
            // For metrics-bearing kinds, sort by lw_revenue DESC so the
            // most active children float to the top. Ties / missing
            // values fall back to name.
            const ar = a.lw_revenue ?? -Infinity;
            const br = b.lw_revenue ?? -Infinity;
            if (ar !== br) return br - ar;
            return a.display.localeCompare(b.display);
          });
        updateNode(node.key, (n) => ({ ...n, children: out, loadingChildren: false }));
      } catch (e: any) {
        updateNode(node.key, (n) => ({
          ...n,
          loadingChildren: false,
          errorChildren: e?.message ?? "load failed",
          children: [],
        }));
      }
    },
    [updateNode, filtersPayload],
  );

  const toggleExpand = useCallback(
    (node: TreeNode) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(node.key)) {
          next.delete(node.key);
        } else {
          next.add(node.key);
          if (node.children === undefined) {
            // Fire-and-forget; updateNode will trigger a re-render.
            void ensureChildrenLoaded(node);
          }
        }
        return next;
      });
    },
    [ensureChildrenLoaded],
  );

  if (!isGraphSource && resolvedSourceKind !== null) {
    return (
      <div className="px-3 py-6 text-sm text-gray-500">
        Tree View is only available for DataViews bound to an{" "}
        <code className="text-gray-300">article_graph</code> source. The current source kind is{" "}
        <code className="text-gray-300">{resolvedSourceKind}</code>.
      </div>
    );
  }

  const fmt = (n?: number, money = false) => {
    if (typeof n !== "number") return "—";
    if (money) return `$${n.toLocaleString()}`;
    return n.toLocaleString();
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-3">
        <span className="text-[10px] uppercase tracking-wider text-gray-500 font-medium">
          Dimension
        </span>
        <div className="flex rounded border border-gray-800 overflow-hidden text-xs">
          <button
            onClick={() => setDimension("product")}
            className={`px-3 py-1 ${
              dimension === "product"
                ? "bg-blue-600 text-white"
                : "bg-gray-900 text-gray-300 hover:bg-gray-800"
            }`}
          >
            Product
          </button>
          <button
            onClick={() => setDimension("store")}
            className={`px-3 py-1 border-l border-gray-800 ${
              dimension === "store"
                ? "bg-blue-600 text-white"
                : "bg-gray-900 text-gray-300 hover:bg-gray-800"
            }`}
          >
            Store
          </button>
        </div>
        <span className="text-[10px] text-gray-500 ml-auto">
          {roots.length > 0 && `${roots.length} root nodes`}
        </span>
      </div>

      {/* Filter panel — every dropdown is multi-select regardless of the
          filter config's `single_select`. Selections stay pending until
          Apply, mirroring the Live View. */}
      {filterEntries.length > 0 && (
        <div className="rounded border border-gray-800 bg-gray-900/40 px-3 py-2 flex items-center gap-2 flex-wrap">
          <Filter size={12} className="text-gray-500 shrink-0" />
          {filterEntries.map((fe) => {
            const sel = pendingFilters[fe.col] || [];
            const isOpen = openFilterCol === fe.col;
            const vals = distinctCache[fe.col] || [];
            return (
              <div key={fe.col} className="relative">
                <button
                  onClick={() => {
                    if (isOpen) setOpenFilterCol(null);
                    else { setOpenFilterCol(fe.col); fetchDistinct(fe.col); }
                  }}
                  className={`flex items-center gap-1 text-xs px-2 py-1 rounded border ${
                    sel.length > 0
                      ? "bg-blue-950/60 border-blue-800 text-blue-200"
                      : "bg-gray-900 border-gray-800 text-gray-300 hover:border-gray-700"
                  }`}
                >
                  {fe.display}
                  {sel.length > 0 && (
                    <span className="bg-blue-700 text-white text-[9px] px-1 rounded-full">
                      {sel.length}
                    </span>
                  )}
                </button>
                {isOpen && (
                  <div className="absolute top-full left-0 mt-1 z-50 w-64 bg-gray-950 rounded border border-gray-800 shadow-lg overflow-hidden">
                    <div className="p-2 border-b border-gray-800 flex items-center justify-between">
                      <span className="text-[10px] uppercase tracking-wider text-gray-400 font-semibold truncate">
                        {fe.display}
                      </span>
                      <span className="text-[10px] text-gray-500 tabular-nums">
                        {sel.length}/{vals.length}
                      </span>
                    </div>
                    <div className="px-2 py-1 flex items-center gap-3 border-b border-gray-800">
                      <button
                        onClick={() => setFilterVal(fe.col, [...vals])}
                        disabled={vals.length === 0 || sel.length === vals.length}
                        className="text-[10px] text-blue-400 hover:underline disabled:opacity-40 disabled:no-underline"
                      >
                        Select all
                      </button>
                      <button
                        onClick={() => setFilterVal(fe.col, [])}
                        disabled={sel.length === 0}
                        className="text-[10px] text-blue-400 hover:underline disabled:opacity-40 disabled:no-underline"
                      >
                        Clear all
                      </button>
                    </div>
                    <div className="max-h-56 overflow-auto p-1">
                      {vals.length === 0 && (
                        <div className="text-[10px] text-gray-500 py-2 px-2">
                          {distinctCache[fe.col] === undefined ? "Loading…" : "No values"}
                        </div>
                      )}
                      {vals.map((v) => (
                        <label
                          key={v}
                          className="flex items-center gap-2 px-2 py-1 text-xs text-gray-200 hover:bg-gray-900 rounded cursor-pointer"
                        >
                          <input
                            type="checkbox"
                            checked={sel.includes(v)}
                            onChange={() => {
                              const next = sel.includes(v)
                                ? sel.filter((x) => x !== v)
                                : [...sel, v];
                              setFilterVal(fe.col, next);
                            }}
                            className="rounded border-gray-600 bg-gray-800 text-blue-500"
                          />
                          <span className="truncate">{v}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
          <div className="ml-auto flex items-center gap-2">
            {Object.keys(pendingFilters).length > 0 && (
              <button
                onClick={clearFilters}
                className="text-[10px] text-red-400 hover:underline"
              >
                Clear
              </button>
            )}
            <button
              onClick={applyFilters}
              disabled={!filtersDirty}
              className={`flex items-center gap-1 text-[11px] px-2 py-1 rounded font-medium ${
                filtersDirty
                  ? "bg-blue-600 text-white hover:bg-blue-500"
                  : "bg-gray-800 text-gray-500 cursor-not-allowed"
              }`}
            >
              Apply filters
              {filtersDirty && (
                <span className="ml-1 inline-block w-1.5 h-1.5 rounded-full bg-amber-300" />
              )}
            </button>
          </div>
        </div>
      )}
      {/* Click-outside catcher for the dropdown */}
      {openFilterCol && (
        <div className="fixed inset-0 z-40" onClick={() => setOpenFilterCol(null)} />
      )}

      {error && (
        <div className="rounded border border-red-900/60 bg-red-950/30 p-2 text-xs text-red-300">
          {error}
        </div>
      )}

      <div className="rounded border border-gray-800 overflow-hidden">
        <div className="bg-gray-900/60 grid grid-cols-[minmax(0,1fr)_80px_110px_110px_120px] gap-2 px-3 py-2 text-[10px] uppercase tracking-wider text-gray-400 font-semibold">
          <span>Name</span>
          <span className="text-right">Children</span>
          <span className="text-right">OH</span>
          <span className="text-right">LW Units</span>
          <span className="text-right">LW Revenue</span>
        </div>
        {loading && (
          <div className="px-3 py-6 text-xs text-gray-500 flex items-center gap-2">
            <Loader2 size={12} className="animate-spin" /> Loading roots…
          </div>
        )}
        {!loading && roots.length === 0 && !error && (
          <div className="px-3 py-6 text-xs text-gray-500">No nodes at the root level.</div>
        )}
        {!loading && roots.length > 0 && (
          <div className="divide-y divide-gray-900/60">
            {roots.map((n) => (
              <TreeRow
                key={n.key}
                node={n}
                depth={0}
                expanded={expanded}
                onToggle={toggleExpand}
                fmt={fmt}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function TreeRow({
  node,
  depth,
  expanded,
  onToggle,
  fmt,
}: {
  node: TreeNode;
  depth: number;
  expanded: Set<string>;
  onToggle: (n: TreeNode) => void;
  fmt: (n?: number, money?: boolean) => string;
}) {
  const isOpen = expanded.has(node.key);
  const hasMore = NEXT_KIND_FOR[node.kind] !== undefined;
  // PRODUCT_CODE / STORE_CODE rows have no metrics in the projection;
  // render dashes rather than zeros so the eye doesn't read "no stock"
  // when really the value just doesn't exist for this kind.
  return (
    <>
      <div
        className="grid grid-cols-[minmax(0,1fr)_80px_110px_110px_120px] gap-2 px-3 py-1.5 text-xs hover:bg-gray-900/40 transition-colors"
        style={{ paddingLeft: `${12 + depth * 18}px` }}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          {hasMore ? (
            <button
              onClick={() => onToggle(node)}
              className="shrink-0 text-gray-500 hover:text-gray-200"
              title={isOpen ? "Collapse" : "Expand"}
            >
              {node.loadingChildren ? (
                <Loader2 size={12} className="animate-spin" />
              ) : isOpen ? (
                <ChevronDown size={12} />
              ) : (
                <ChevronRight size={12} />
              )}
            </button>
          ) : (
            <span className="w-3 inline-block" />
          )}
          <span className="text-[9px] uppercase text-gray-500 font-mono shrink-0">
            {node.kind.toLowerCase().replace("_", "-")}
          </span>
          <span className="text-gray-200 truncate">{node.display || <em>(empty)</em>}</span>
        </div>
        <span className="text-right text-gray-400 tabular-nums">
          {typeof node.child_count === "number" ? node.child_count.toLocaleString() : "—"}
        </span>
        <span className="text-right text-gray-200 tabular-nums">{fmt(node.oh)}</span>
        <span className="text-right text-gray-200 tabular-nums">{fmt(node.lw_units)}</span>
        <span className="text-right text-gray-200 tabular-nums">{fmt(node.lw_revenue, true)}</span>
      </div>
      {node.errorChildren && (
        <div
          className="px-3 py-1 text-[11px] text-red-400"
          style={{ paddingLeft: `${30 + depth * 18}px` }}
        >
          {node.errorChildren}
        </div>
      )}
      {isOpen && node.children && node.children.length > 0 && (
        <div className="bg-gray-950/40">
          {node.children.map((c) => (
            <TreeRow
              key={c.key}
              node={c}
              depth={depth + 1}
              expanded={expanded}
              onToggle={onToggle}
              fmt={fmt}
            />
          ))}
        </div>
      )}
    </>
  );
}

/* ------------------------------------------------------------------ */
/*  Detail View Tab — two-column drilldown                             */
/*                                                                     */
/*  Left:  hierarchy tree (compact, lazy-expand on chevron)            */
/*  Right: detail pane for the currently-focused node                  */
/*                                                                     */
/*  Click a tree row's chevron to expand; click the row's name to load */
/*  it into the right pane. Right pane shows headline metrics, at-risk */
/*  chip counts (scoped to that node), top child branches, top         */
/*  articles in the subtree, and the brand spread.                     */
/*                                                                     */
/*  Phase 1 covers hierarchy nodes (L0..L5). Article + brand detail    */
/*  panes will reuse the same right-pane component and slot in their   */
/*  own section list as follow-ups.                                    */
/* ------------------------------------------------------------------ */

const KIND_TO_LEVEL_ATTRIBUTE: Partial<Record<TreeKind, string>> = {
  L0: "l0_name",
  L1: "l1_name",
  L2: "l2_name",
  L3: "l3_name",
  L4: "l4_name",
  L5: "l5_name",
  // For ARTICLE / BRAND focus, the cross-filter accepts the same
  // attribute names so detail-pane fetches scope cleanly.
  ARTICLE: "article",
  BRAND: "brand",
};

interface NodeDetail {
  // Pulled from /aggregate-at + a one-row count probe.
  metrics: {
    oh?: number;
    oo?: number;
    it?: number;
    lw_units?: number;
    lw_revenue?: number;
    lw_margin?: number;
  };
  articleCount: number;
  // Children at the next level. Used for the "L+1 children" section so
  // the right pane shows distribution without re-issuing a traversal.
  children: TreeNode[];
  // Top-N articles by OH DESC, scoped to this node. Used both as the
  // article list AND as the input to the brand spread groupby.
  topArticles: any[];
  // {rule_name → count} from /exceptions/counts.
  exceptionCounts: Record<string, number>;
}

function DetailViewTab({ dv }: { dv: any }) {
  const [resolvedSourceKind, setResolvedSourceKind] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    const sourceId =
      dv?.source?.type === "source" ? (dv.source.config?.source_id as string | undefined) : null;
    if (!sourceId) { setResolvedSourceKind(null); return; }
    api.getSource(sourceId)
      .then((row: any) => { if (!cancelled) setResolvedSourceKind(row?.kind ?? null); })
      .catch(() => { if (!cancelled) setResolvedSourceKind(null); });
    return () => { cancelled = true; };
  }, [dv.id, dv?.source]);
  const isGraphSource = resolvedSourceKind === "graph";
  // Two top-level modes for the left tree. Hierarchy = L0..ARTICLE..PRODUCT_CODE.
  // Brand = brand list, expandable to its top 200 articles. Cross-mode
  // navigation (brand link in a hierarchy page, article in a brand page)
  // auto-switches mode based on the path's first step.
  const [mode, setMode] = useState<"hierarchy" | "brand">("hierarchy");

  // ── Left: tree state (mirrors TreeViewTab; product dimension only
  //    for now — store comes when it's actually useful) ──
  const [roots, setRoots] = useState<TreeNode[]>([]);
  const [brandRoots, setBrandRoots] = useState<TreeNode[]>([]);
  const [rootsLoading, setRootsLoading] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  // Collapse the left sidebar to give the detail pane more room.
  // Persisted to localStorage so it survives reloads.
  const [leftCollapsed, setLeftCollapsed] = useState<boolean>(() => {
    try { return localStorage.getItem("detail-view-left-collapsed") === "1"; }
    catch { return false; }
  });
  useEffect(() => {
    try { localStorage.setItem("detail-view-left-collapsed", leftCollapsed ? "1" : "0"); }
    catch {}
  }, [leftCollapsed]);

  // Selected node — drives the right pane. null = nothing selected yet.
  const [focused, setFocused] = useState<TreeNode | null>(null);
  // Path from the tree root down to `focused`. Used by DetailPane to
  // build paths for click handlers (e.g. children of focused = path + [child]).
  // Empty array when focused is at the root level.
  const [focusedPath, setFocusedPath] = useState<TreeNode[]>([]);

  // Load roots once the source is confirmed graph-backed. We fetch L0
  // hierarchy roots AND brands in parallel — both render at the same
  // top level in the tree (hierarchy section + Brands section).
  useEffect(() => {
    if (!isGraphSource) return;
    let cancelled = false;
    setRootsLoading(true);
    Promise.all([
      api.getDataViewData(dv.id, { limit: 500, node_kind: "L0" }),
      // Brands list — one row per distinct brand, with rolled-up
      // article_count + oh + lw_revenue. Sorted by OH DESC server-side.
      // No limit — load every brand. 2k+ rows fit fine in the tree
      // and we need them all so navigation never falls off the list.
      api.brandsList().catch(() => ({ brands: [] as any[] })),
    ])
      .then(([levelRes, brandsRes]) => {
        if (cancelled) return;
        const hier: TreeNode[] = (levelRes.rows || [])
          .map((r: any) => makeNodeFromRow(r, "L0"))
          .sort((a, b) => a.display.localeCompare(b.display));
        const brandRoots: TreeNode[] = (brandsRes.brands || []).map((b: any) => ({
          key: `BRAND::${b.name}`,
          kind: "BRAND" as TreeKind,
          name: String(b.name),
          display: String(b.name),
          oh: typeof b.oh === "number" ? b.oh : 0,
          lw_units: typeof b.lw_units === "number" ? b.lw_units : 0,
          lw_revenue: typeof b.lw_revenue === "number" ? b.lw_revenue : 0,
          child_count: typeof b.article_count === "number" ? b.article_count : 0,
        }));
        setRoots(hier);
        setBrandRoots(brandRoots);
        if (hier.length > 0 && !focused) setFocused(hier[0]);
      })
      .catch(() => {})
      .finally(() => { if (!cancelled) setRootsLoading(false); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isGraphSource, dv.id]);

  const updateNode = useCallback(
    (key: string, mutate: (n: TreeNode) => TreeNode) => {
      const walk = (nodes: TreeNode[]): TreeNode[] =>
        nodes.map((n) => {
          if (n.key === key) return mutate(n);
          if (n.children && n.children.length > 0) {
            const next = walk(n.children);
            if (next !== n.children) return { ...n, children: next };
          }
          return n;
        });
      setRoots((prev) => walk(prev));
      setBrandRoots((prev) => walk(prev));
    },
    [],
  );

  // Direct fetch — returns the children list AND updates tree state.
  // navigateToPath uses the return value to walk further without
  // depending on the async state-update timing.
  const fetchChildrenDirect = useCallback(
    async (node: TreeNode): Promise<TreeNode[]> => {
      if (node.children !== undefined) return node.children;
      const childKind = NEXT_KIND_FOR[node.kind];
      if (!childKind) {
        updateNode(node.key, (n) => ({ ...n, children: [] }));
        return [];
      }
      updateNode(node.key, (n) => ({ ...n, loadingChildren: true, errorChildren: undefined }));
      try {
        const edge =
          node.kind === "CHANNEL" ? "stores" :
          node.kind === "BRAND"   ? "articles" :
          "children";
        const resp = await api.graphTraverse({ kind: node.kind as any, name: node.name }, edge as any);
        const all: TreeNode[] = (resp.rows || [])
          .map((r: any) => makeNodeFromRow(r, childKind))
          .sort((a, b) => {
            const ar = a.oh ?? -Infinity;
            const br = b.oh ?? -Infinity;
            if (ar !== br) return br - ar;
            return a.display.localeCompare(b.display);
          });
        const out = node.kind === "BRAND" ? all.slice(0, 200) : all;
        updateNode(node.key, (n) => ({ ...n, children: out, loadingChildren: false }));
        return out;
      } catch (e: any) {
        updateNode(node.key, (n) => ({
          ...n,
          loadingChildren: false,
          errorChildren: e?.message ?? "load failed",
          children: [],
        }));
        return [];
      }
    },
    [updateNode],
  );

  const ensureChildren = useCallback(
    async (node: TreeNode) => {
      if (node.children !== undefined || node.loadingChildren) return;
      await fetchChildrenDirect(node);
    },
    [fetchChildrenDirect],
  );

  const toggleExpand = useCallback(
    (node: TreeNode) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(node.key)) next.delete(node.key);
        else {
          next.add(node.key);
          if (node.children === undefined) void ensureChildren(node);
        }
        return next;
      });
    },
    [ensureChildren],
  );

  // Walk a path of (kind, name) steps and progressively expand+fetch
  // each level so the target node is positioned + highlighted in the
  // left tree. The first step picks the layer:
  //   L0  → roots (hierarchy section)
  //   BRAND → brandRoots (Brands section)
  // Subsequent steps walk down the loaded children of the previous
  // step. Missing levels (e.g. a row with no l5_name) are skipped.
  const navigateToPath = useCallback(
    async (path: { kind: TreeKind; name: string }[]) => {
      if (path.length === 0) return;
      const cleaned = path.filter((p) => p && typeof p.name === "string" && p.name.length > 0);
      if (cleaned.length === 0) return;

      const findIn = (nodes: TreeNode[], k: TreeKind, n: string) =>
        nodes.find((x) => x.kind === k && x.name === n);

      // Pick starting layer + auto-switch mode based on the first
      // step's kind so the tree on the left actually shows the path
      // we're about to walk.
      let layer: TreeNode[];
      if (cleaned[0].kind === "BRAND") {
        layer = brandRoots;
        setMode("brand");
      } else {
        layer = roots;
        setMode("hierarchy");
      }

      const walked: TreeNode[] = [];
      let curr: TreeNode | undefined;
      for (let i = 0; i < cleaned.length; i++) {
        const s = cleaned[i];
        const found = findIn(layer, s.kind, s.name);
        if (!found) break;
        walked.push(found);
        curr = found;
        // Expand ancestors. The final node doesn't need expansion (it's
        // the focus target), but expanding it too is harmless.
        if (i < cleaned.length - 1) {
          setExpanded((prev) => new Set(prev).add(found.key));
          // Drill into its children for the next iteration.
          const kids = await fetchChildrenDirect(found);
          layer = kids;
        }
      }
      if (curr) {
        setFocused(curr);
        // Path stored for the right-pane navigation handlers (children
        // get full path = focusedPath + [child]).
        setFocusedPath(walked);
      }
    },
    [roots, brandRoots, fetchChildrenDirect],
  );

  const fmt = (n?: number, money = false) => {
    if (typeof n !== "number") return "—";
    if (money) return `$${n.toLocaleString()}`;
    return n.toLocaleString();
  };

  if (!isGraphSource && resolvedSourceKind !== null) {
    return (
      <div className="px-3 py-6 text-sm text-gray-500">
        Detail View is only available for DataViews bound to an{" "}
        <code className="text-gray-300">article_graph</code> source.
      </div>
    );
  }

  return (
    <div
      className="grid gap-4 h-full"
      style={{
        gridTemplateColumns: leftCollapsed
          ? "32px minmax(0, 1fr)"
          : "minmax(260px, 1fr) minmax(0, 2fr)",
      }}
    >
      {/* ── Left: tree ── */}
      {leftCollapsed ? (
        <div className="rounded border border-gray-800 overflow-hidden flex flex-col items-center bg-gray-900/40">
          <button
            onClick={() => setLeftCollapsed(false)}
            title="Expand hierarchy sidebar"
            className="w-full h-full flex items-start justify-center pt-2 text-gray-400 hover:bg-gray-900 hover:text-gray-100"
          >
            <ChevronRight size={14} />
          </button>
        </div>
      ) : (
        <div className="rounded border border-gray-800 overflow-hidden flex flex-col min-h-0">
          <div className="bg-gray-900/60 px-2 py-2 border-b border-gray-800 flex items-center gap-2">
            <div className="flex rounded border border-gray-800 overflow-hidden text-[11px]">
              <button
                onClick={() => setMode("hierarchy")}
                className={`px-2 py-0.5 ${
                  mode === "hierarchy"
                    ? "bg-blue-600 text-white"
                    : "bg-gray-900 text-gray-300 hover:bg-gray-800"
                }`}
              >
                Hierarchy
              </button>
              <button
                onClick={() => setMode("brand")}
                className={`px-2 py-0.5 border-l border-gray-800 ${
                  mode === "brand"
                    ? "bg-blue-600 text-white"
                    : "bg-gray-900 text-gray-300 hover:bg-gray-800"
                }`}
              >
                Brand
              </button>
            </div>
            <button
              onClick={() => setLeftCollapsed(true)}
              title="Collapse sidebar"
              className="ml-auto p-0.5 rounded text-gray-500 hover:text-gray-100 hover:bg-gray-800"
            >
              <ChevronLeft size={13} />
            </button>
          </div>
          <div className="flex-1 overflow-auto">
            {rootsLoading && (
              <div className="px-3 py-6 text-xs text-gray-500 flex items-center gap-2">
                <Loader2 size={12} className="animate-spin" /> Loading…
              </div>
            )}
            {!rootsLoading && roots.length === 0 && (
              <div className="px-3 py-6 text-xs text-gray-500">No hierarchy roots found.</div>
            )}
            {!rootsLoading && (
              <div className="divide-y divide-gray-900/60">
                {(mode === "hierarchy" ? roots : brandRoots).map((n) => (
                  <DetailTreeRow
                    key={n.key}
                    node={n}
                    depth={0}
                    ancestors={[]}
                    expanded={expanded}
                    focusedKey={focused?.key ?? null}
                    onToggle={toggleExpand}
                    onFocus={(node, ancestors) => {
                      // Tree-row click: jump to that node and store the
                      // path the recursive renderer threaded down. This
                      // gives the right pane an accurate focusedPath so
                      // children clicks compose correctly.
                      setFocused(node);
                      setFocusedPath([...ancestors, node]);
                    }}
                  />
                ))}
                {(mode === "hierarchy" ? roots : brandRoots).length === 0 && (
                  <div className="px-3 py-6 text-xs text-gray-500">
                    {mode === "hierarchy" ? "No hierarchy roots." : "No brands found."}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {/* ── Right: detail pane ── */}
      <div className="rounded border border-gray-800 overflow-auto">
        {focused ? (
          <DetailPane
            key={focused.key}
            dvId={dv.id}
            node={focused}
            focusedPath={focusedPath}
            fmt={fmt}
            onNavigate={(path) => {
              void navigateToPath(path);
            }}
          />
        ) : (
          <div className="px-3 py-12 text-center text-sm text-gray-500">
            Select a node on the left to inspect its inventory profile.
          </div>
        )}
      </div>
    </div>
  );
}

function DetailTreeRow({
  node,
  depth,
  ancestors,
  expanded,
  focusedKey,
  onToggle,
  onFocus,
}: {
  node: TreeNode;
  depth: number;
  // Path from root down to (but excluding) `node`. Threaded so onFocus
  // receives the full ancestor chain — the parent component uses that
  // to set focusedPath, which the right pane needs to compose
  // children-click navigation paths correctly.
  ancestors: TreeNode[];
  expanded: Set<string>;
  focusedKey: string | null;
  onToggle: (n: TreeNode) => void;
  onFocus: (n: TreeNode, ancestors: TreeNode[]) => void;
}) {
  const isOpen = expanded.has(node.key);
  const hasMore = NEXT_KIND_FOR[node.kind] !== undefined;
  const isFocused = focusedKey === node.key;
  return (
    <>
      <div
        className={`flex items-center gap-1.5 px-2 py-1.5 text-xs cursor-pointer transition-colors ${
          isFocused ? "bg-blue-950/50" : "hover:bg-gray-900/40"
        }`}
        style={{ paddingLeft: `${8 + depth * 14}px` }}
      >
        {hasMore ? (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onToggle(node);
            }}
            className="shrink-0 text-gray-500 hover:text-gray-200"
            title={isOpen ? "Collapse" : "Expand"}
          >
            {node.loadingChildren ? (
              <Loader2 size={11} className="animate-spin" />
            ) : isOpen ? (
              <ChevronDown size={12} />
            ) : (
              <ChevronRight size={12} />
            )}
          </button>
        ) : (
          <span className="w-3 inline-block" />
        )}
        <button
          onClick={() => onFocus(node, ancestors)}
          className={`text-left truncate flex-1 ${
            isFocused ? "text-blue-200 font-medium" : "text-gray-200 hover:text-gray-100"
          }`}
          title={node.display}
        >
          <span className="text-[9px] uppercase text-gray-500 font-mono mr-1.5">
            {node.kind.toLowerCase().replace("_", "-")}
          </span>
          {node.display || <em>(empty)</em>}
        </button>
      </div>
      {isOpen && node.children && node.children.length > 0 && (
        <div className="bg-gray-950/40">
          {node.children.map((c) => (
            <DetailTreeRow
              key={c.key}
              node={c}
              depth={depth + 1}
              ancestors={[...ancestors, node]}
              expanded={expanded}
              focusedKey={focusedKey}
              onToggle={onToggle}
              onFocus={onFocus}
            />
          ))}
        </div>
      )}
    </>
  );
}

/// Right-pane content for a single node. Loads in parallel:
///   - aggregate-at (headline)
///   - data?node_kind=ARTICLE+filter+limit=200 (top articles + brand spread)
///   - exceptions/counts (at-risk chips)
///   - traverse children (next-level distribution)
function DetailPane({
  dvId,
  node,
  focusedPath,
  fmt,
  onNavigate,
}: {
  dvId: string;
  node: TreeNode;
  // Path from a tree root down to and including `node`. The pane uses
  // it to compose navigation paths — e.g. clicking a child under the
  // current focus should navigate via focusedPath + [child].
  focusedPath: TreeNode[];
  fmt: (n?: number, money?: boolean) => string;
  onNavigate?: (path: { kind: TreeKind; name: string }[]) => void;
}) {
  const [detail, setDetail] = useState<NodeDetail | null>(null);
  // Article focus carries an extra payload (RCL trace + sizes + flags)
  // that hierarchy/brand panes don't need. Stored separately so the
  // shared sections (headline, at-risk strip, top articles) reuse the
  // common `detail` shape without bloat.
  const [articleDetail, setArticleDetail] = useState<any | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const isHierarchy = ["L0","L1","L2","L3","L4","L5"].includes(node.kind);
  const isArticle = node.kind === "ARTICLE";
  const isProductCode = node.kind === "PRODUCT_CODE";
  const isBrand = node.kind === "BRAND";
  // Product codes share the article-detail layout (RCL + sizes + flags
  // are identical for every SKU under the same article). The header
  // just changes to make the focused entity clear.
  const isArticleLike = isArticle || isProductCode;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setDetail(null);
    setArticleDetail(null);

    const levelAttr = KIND_TO_LEVEL_ATTRIBUTE[node.kind];
    const filtersForNode = levelAttr
      ? [{ attribute_name: levelAttr, values: [node.name], operator: "in" as const }]
      : [];

    // Article OR product_code focus: the bundled `/article-detail`
    // endpoint accepts either key — we pass `product_code` when the
    // focused node is a SKU so the response's `article` field reflects
    // the parent. The render path is shared (sizes + RCL + flags are
    // article-level; product_code differs only by which size you came
    // through, which we don't visually emphasize yet).
    if (isArticleLike) {
      const key = isProductCode ? { product_code: node.name } : { article: node.name };
      Promise.all([
        api.articleDetail(key),
        api.exceptionsCounts(filtersForNode),
      ])
        .then(([ad, counts]) => {
          if (cancelled) return;
          setArticleDetail(ad);
          setDetail({
            metrics: (ad?.row || {}) as any,
            articleCount: 1,
            children: [],
            topArticles: [],
            exceptionCounts: counts?.counts ?? {},
          });
        })
        .catch((e) => { if (!cancelled) setError(e?.message ?? "load failed"); })
        .finally(() => { if (!cancelled) setLoading(false); });
      return () => { cancelled = true; };
    }

    // Hierarchy / brand focus: existing four-fetch composition. The
    // `traverse` call is a no-op for BRAND because brand→articles is
    // huge — we render top articles instead, which already covers it.
    const fetchChildren = isBrand
      ? Promise.resolve({ rows: [] })
      : api.graphTraverse({ kind: node.kind as any, name: node.name }, "children");

    Promise.all([
      fetch(`/api/graph/articles/aggregate-at`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ kind: node.kind, name: node.name }),
      }).then((r) => r.json()),
      api.getDataViewData(dvId, {
        limit: 200,
        node_kind: "ARTICLE",
        sort_col: "oh",
        sort_dir: "DESC",
        filters: filtersForNode,
      }),
      api.exceptionsCounts(filtersForNode),
      fetchChildren,
    ])
      .then(([agg, data, counts, children]: any[]) => {
        if (cancelled) return;
        const childRows: TreeNode[] = (children.rows || [])
          .map((r: any) => {
            let kind: TreeKind = "L0";
            if (r.level) kind = r.level as TreeKind;
            else if (r.article) kind = "ARTICLE";
            else if (r.product_code) kind = "PRODUCT_CODE";
            else if (r.store_code) kind = "STORE_CODE";
            return makeNodeFromRow(r, kind);
          })
          .sort((a: TreeNode, b: TreeNode) => {
            const ar = a.lw_revenue ?? -Infinity;
            const br = b.lw_revenue ?? -Infinity;
            if (ar !== br) return br - ar;
            return a.display.localeCompare(b.display);
          });
        setDetail({
          metrics: agg?.aggregates ?? {},
          articleCount: data?.total ?? 0,
          children: childRows,
          topArticles: data?.rows ?? [],
          exceptionCounts: counts?.counts ?? {},
        });
      })
      .catch((e) => { if (!cancelled) setError(e?.message ?? "load failed"); })
      .finally(() => { if (!cancelled) setLoading(false); });

    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dvId, node.key]);

  // Brand spread — top 5 brands across the topArticles sample. Used
  // in the hierarchy detail to show "what brands dominate this L1?".
  // Skipped when the focus is itself a brand (would be degenerate).
  const brandSpread = useMemo(() => {
    if (!detail || isBrand) return [] as { brand: string; count: number; ohSum: number }[];
    const acc = new Map<string, { count: number; ohSum: number }>();
    for (const r of detail.topArticles) {
      const b = (r as any).brand || "(unknown)";
      const cur = acc.get(b) || { count: 0, ohSum: 0 };
      cur.count += 1;
      cur.ohSum += typeof (r as any).oh === "number" ? (r as any).oh : 0;
      acc.set(b, cur);
    }
    return Array.from(acc.entries())
      .map(([brand, v]) => ({ brand, count: v.count, ohSum: v.ohSum }))
      .sort((a, b) => b.ohSum - a.ohSum)
      .slice(0, 5);
  }, [detail, isBrand]);

  // Hierarchy spread — for a brand focus, group its articles by L1
  // and rank by OH. Answers "where does this brand live in the
  // catalog?". Computed client-side from the topArticles sample.
  const hierarchySpread = useMemo(() => {
    if (!detail || !isBrand) return [] as { l1: string; count: number; ohSum: number }[];
    const acc = new Map<string, { count: number; ohSum: number }>();
    for (const r of detail.topArticles) {
      const k = (r as any).l1_name || "(unknown)";
      const cur = acc.get(k) || { count: 0, ohSum: 0 };
      cur.count += 1;
      cur.ohSum += typeof (r as any).oh === "number" ? (r as any).oh : 0;
      acc.set(k, cur);
    }
    return Array.from(acc.entries())
      .map(([l1, v]) => ({ l1, count: v.count, ohSum: v.ohSum }))
      .sort((a, b) => b.ohSum - a.ohSum)
      .slice(0, 5);
  }, [detail, isBrand]);

  if (loading) {
    return (
      <div className="px-4 py-12 flex items-center gap-2 text-sm text-gray-500">
        <Loader2 size={13} className="animate-spin" /> Loading detail for{" "}
        <code className="text-gray-300">{node.display}</code>…
      </div>
    );
  }
  if (error) {
    return (
      <div className="px-4 py-6 rounded border border-red-900/60 bg-red-950/30 m-4 text-xs text-red-300">
        {error}
      </div>
    );
  }
  if (!detail) return null;

  const m = detail.metrics;
  const flagged = Object.values(detail.exceptionCounts).reduce((a, b) => a + b, 0);

  return (
    <div className="p-4 space-y-4">
      {/* Headline */}
      <div>
        <div className="flex items-baseline gap-2 mb-1">
          <span className="text-[10px] uppercase tracking-wider text-gray-500 font-mono">
            {node.kind.toLowerCase().replace("_", "-")}
          </span>
          <h3 className="text-base font-semibold text-gray-100 truncate">{node.display}</h3>
        </div>
      </div>

      {/* Metric cards. For ARTICLE focus the "Articles" card is
          replaced with a Brand label since article_count is always 1
          and the brand identity is more useful at a glance. */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
        <MetricCard label="On hand" value={fmt(m.oh)} />
        <MetricCard label="LW revenue" value={fmt(m.lw_revenue, true)} />
        <MetricCard label="LW units" value={fmt(m.lw_units)} />
        {isArticleLike ? (
          <MetricCard label="Brand" value={(m as any).brand || "—"} />
        ) : (
          <MetricCard label="Articles" value={fmt(detail.articleCount)} />
        )}
      </div>

      {/* At-risk chips */}
      <div className="rounded border border-gray-800 bg-gray-900/40 p-3">
        <div className="flex items-baseline gap-2 mb-2">
          <ShieldAlert size={12} className="text-amber-400" />
          <span className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold">
            At-risk
          </span>
          <span className="text-xs text-gray-300">
            <span className="text-gray-100 font-medium">{flagged.toLocaleString()}</span>
            <span className="text-gray-500"> of {detail.articleCount.toLocaleString()}</span>
          </span>
        </div>
        <div className="flex flex-wrap gap-1.5">
          {RULE_DEFS.map((rd) => {
            const tone = TONE_CLASS[rd.tone] || TONE_CLASS.red;
            const c = detail.exceptionCounts[rd.key] ?? 0;
            return (
              <RuleHoverCard key={rd.key} rd={rd}>
                <span className={`px-2 py-0.5 text-[11px] rounded border ${tone.chip}`}>
                  {rd.label}{" "}
                  <span className="font-medium ml-0.5">{c.toLocaleString()}</span>
                </span>
              </RuleHoverCard>
            );
          })}
        </div>
      </div>

      {/* Article / product_code sections: hierarchy breadcrumb, sizes,
          RCL trace. For PRODUCT_CODE focus, the breadcrumb gets an
          extra rung for the parent article. */}
      {isArticleLike && articleDetail && (
        <>
          {/* Hierarchy breadcrumb — clickable so the user can pop up
              to any ancestor level. */}
          <div>
            <div className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold mb-1.5">
              Hierarchy
            </div>
            <div className="flex flex-wrap items-center gap-1 text-xs">
              {(["l0_name","l1_name","l2_name","l3_name","l4_name","l5_name"] as const).map((k, i) => {
                const v = (m as any)[k];
                if (!v) return null;
                const kind = (`L${i}` as TreeKind);
                // Build the path to this ancestor — caller's
                // navigateToPath walks down level by level, expanding
                // and fetching as it goes.
                const ancestorPath: { kind: TreeKind; name: string }[] = [];
                for (let j = 0; j <= i; j++) {
                  const aname = (m as any)[`l${j}_name`];
                  if (typeof aname === "string" && aname) {
                    ancestorPath.push({ kind: `L${j}` as TreeKind, name: String(aname) });
                  }
                }
                return (
                  <span key={k} className="flex items-center gap-1">
                    {i > 0 && <ChevronRight size={11} className="text-gray-600" />}
                    <button
                      onClick={() => onNavigate?.(ancestorPath)}
                      className="text-blue-300 hover:text-blue-200 hover:underline"
                      title={`Drill up to ${kind}: ${v}`}
                    >
                      {String(v)}
                    </button>
                  </span>
                );
              })}
              {/* Parent article rung — only when focused entity is a
                  product_code. Click drills back up to the article. */}
              {isProductCode && articleDetail.article && (
                <span className="flex items-center gap-1">
                  <ChevronRight size={11} className="text-gray-600" />
                  <button
                    onClick={() => {
                      // Build the article's full path so the tree
                      // walker positions cleanly.
                      const articlePath: { kind: TreeKind; name: string }[] = [];
                      for (let j = 0; j <= 5; j++) {
                        const v = (m as any)[`l${j}_name`];
                        if (typeof v === "string" && v) {
                          articlePath.push({ kind: `L${j}` as TreeKind, name: String(v) });
                        }
                      }
                      articlePath.push({ kind: "ARTICLE", name: String(articleDetail.article) });
                      onNavigate?.(articlePath);
                    }}
                    className="text-blue-300 hover:text-blue-200 hover:underline font-mono"
                    title={`Drill up to article ${articleDetail.article}`}
                  >
                    {articleDetail.article}
                  </button>
                </span>
              )}
            </div>
          </div>
          {/* Sizes — same pill style as Exception View. Click navigates
              to the product_code if/when product-code detail ships. */}
          {Array.isArray(articleDetail.sizes) && articleDetail.sizes.length > 0 && (
            <div>
              <div className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold mb-1.5">
                Per-size on hand
              </div>
              <div className="flex flex-wrap gap-1">
                {articleDetail.sizes.map((s: { size: string; oh: number }) => (
                  <span
                    key={s.size}
                    className="px-1.5 py-0.5 text-[11px] rounded border border-gray-700 bg-gray-900 text-gray-300 tabular-nums"
                    title={`size ${s.size}: OH ${s.oh}`}
                  >
                    <span className="text-gray-500">{s.size}</span>
                    <span className="text-gray-200 ml-1 font-medium">{s.oh.toLocaleString()}</span>
                  </span>
                ))}
              </div>
            </div>
          )}
          {/* RCL trace — DC policy + Constraints + PSM. Show the
              resolved values; codes go in tooltips for developers. */}
          <ArticleRclSection rcl={articleDetail.rcl} />
        </>
      )}

      {/* Brand-only: hierarchy spread (where in the catalog this brand
          lives). Top L1s by OH, computed from the topArticles sample. */}
      {isBrand && hierarchySpread.length > 0 && (
        <div>
          <div className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold mb-1.5">
            Hierarchy spread (top 5 L1s by OH)
            <span className="ml-1 text-gray-500 normal-case font-normal">
              over top {detail.topArticles.length} articles
            </span>
          </div>
          <ChildrenBars
            children={hierarchySpread.map((h) => ({
              key: `L1::${h.l1}`,
              kind: "L1" as TreeKind,
              name: h.l1,
              display: h.l1,
              child_count: h.count,
              oh: h.ohSum,
            }))}
            fmt={fmt}
            onClick={(target) =>
              // Hierarchy spread under a brand: target is L1; the path
              // is single-step since L0 is implicit (we only have one
              // L0 in the dataset; the walker auto-picks the only root).
              onNavigate?.([{ kind: target.kind, name: target.name }])
            }
          />
        </div>
      )}

      {/* Children — hierarchy only. Brand+article have their own
          scoped sections above; their "children" would be misleading
          (article→product_code is just sizes; brand→articles is huge). */}
      {isHierarchy && detail.children.length > 0 && (
        <div>
          <div className="flex items-baseline gap-2 mb-1.5">
            <span className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold">
              {NEXT_KIND_FOR[node.kind]?.toLowerCase().replace("_", "-")} children · top by LW Revenue
            </span>
            <span className="text-[10px] text-gray-500">{detail.children.length} total</span>
          </div>
          <ChildrenBars
            children={detail.children.slice(0, 8)}
            fmt={fmt}
            onClick={(child) =>
              // Path = current focus path + the clicked child. Walker
              // expands the whole chain on the left tree.
              onNavigate?.([
                ...focusedPath.map((p) => ({ kind: p.kind, name: p.name })),
                { kind: child.kind, name: child.name },
              ])
            }
          />
        </div>
      )}

      {/* Top articles */}
      {detail.topArticles.length > 0 && (
        <div>
          <div className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold mb-1.5">
            Top articles by OH
          </div>
          <div className="rounded border border-gray-800 overflow-hidden">
            <table className="w-full text-xs">
              <thead className="bg-gray-900/40 text-gray-500 text-[10px] uppercase tracking-wider">
                <tr>
                  <th className="text-left px-3 py-1.5">Article</th>
                  <th className="text-left px-3 py-1.5">Brand</th>
                  <th className="text-left px-3 py-1.5">L2</th>
                  <th className="text-right px-3 py-1.5">OH</th>
                  <th className="text-right px-3 py-1.5">LW Rev</th>
                </tr>
              </thead>
              <tbody>
                {detail.topArticles.slice(0, 10).map((r: any, i: number) => {
                  // Build the article's full hierarchy path so the
                  // tree walker can expand L0..L5 and land on the
                  // article. Skip empty levels (some rows have no L5).
                  const articlePath: { kind: TreeKind; name: string }[] = [];
                  for (let j = 0; j <= 5; j++) {
                    const v = r[`l${j}_name`];
                    if (typeof v === "string" && v) {
                      articlePath.push({ kind: `L${j}` as TreeKind, name: String(v) });
                    }
                  }
                  articlePath.push({ kind: "ARTICLE", name: String(r.article) });
                  return (
                  <tr key={i} className="border-t border-gray-900/60 hover:bg-gray-900/40">
                    <td className="px-3 py-1">
                      <button
                        onClick={() => onNavigate?.(articlePath)}
                        className="font-mono text-blue-300 hover:text-blue-200 hover:underline"
                        title={`Open detail for ${r.article}`}
                      >
                        {r.article}
                      </button>
                    </td>
                    <td className="px-3 py-1">
                      {r.brand ? (
                        <button
                          onClick={() => onNavigate?.([{ kind: "BRAND", name: String(r.brand) }])}
                          className="text-blue-300 hover:text-blue-200 hover:underline"
                          title={`Open detail for brand ${r.brand}`}
                        >
                          {r.brand}
                        </button>
                      ) : (
                        <span className="text-gray-500">—</span>
                      )}
                    </td>
                    <td className="px-3 py-1 text-gray-400 truncate max-w-[200px]">{r.l2_name || "—"}</td>
                    <td className="px-3 py-1 text-right tabular-nums text-gray-200">
                      {(r.oh ?? 0).toLocaleString()}
                    </td>
                    <td className="px-3 py-1 text-right tabular-nums text-gray-300">
                      ${(r.lw_revenue ?? 0).toLocaleString()}
                    </td>
                  </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Brand spread — hierarchy / article focus. Pills are clickable
          links: each navigates to that brand's detail page (where it
          appears as a focused BRAND in the Brands section of the tree). */}
      {!isBrand && brandSpread.length > 0 && (
        <div>
          <div className="flex items-baseline gap-2 mb-1.5">
            <span className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold">
              Brand spread (top 5 by OH)
            </span>
            <span className="text-[10px] text-gray-500">over top {detail.topArticles.length} articles</span>
          </div>
          <ChildrenBars
            children={brandSpread.map((b) => ({
              key: `BRAND::${b.brand}`,
              kind: "BRAND" as TreeKind,
              name: b.brand,
              display: b.brand,
              child_count: b.count,
              oh: b.ohSum,
            }))}
            fmt={fmt}
            onClick={(target) =>
              // Brand pills: single-step path. The walker auto-switches
              // mode to brand and lands on the brand row.
              onNavigate?.([{ kind: target.kind, name: target.name }])
            }
          />
        </div>
      )}
    </div>
  );
}

/// RCL trace block for an article focus. Reads the same shape the
/// `/graph/articles/resolve-rcl` endpoint returns. End users see the
/// resolved values (default_store_groups, dc_store_rule, min/max);
/// codes (rcl_code / rule_code) are kept in tooltips for developers.
function ArticleRclSection({ rcl }: { rcl: any }) {
  if (!rcl || (!rcl.dc_policy && !rcl.constraints && !rcl.psm)) return null;
  const dc = rcl.dc_policy;
  const c = rcl.constraints;
  const psm = rcl.psm;
  return (
    <div>
      <div className="text-[11px] uppercase tracking-wider text-gray-400 font-semibold mb-1.5">
        RCL trace
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 text-xs">
        {dc && (
          <div className="rounded border border-gray-800 bg-gray-900/40 p-2">
            <div className="text-[10px] uppercase text-gray-500 mb-1" title={`rcl_code=${dc.rcl_code} rule_code=${dc.rule_code}`}>
              DC policy
            </div>
            <div className="space-y-0.5 text-gray-200">
              <div>
                <span className="text-gray-500">groups: </span>
                {(dc.policy?.default_store_groups ?? []).join(", ") || "—"}
              </div>
              <div>
                <span className="text-gray-500">dc_rule: </span>
                {dc.policy?.dc_store_rule || "—"}
              </div>
            </div>
          </div>
        )}
        {c && (
          <div className="rounded border border-gray-800 bg-gray-900/40 p-2">
            <div className="text-[10px] uppercase text-gray-500 mb-1" title={`rcl_code=${c.rcl_code} rule_code=${c.rule_code}`}>
              Constraints
            </div>
            <div className="space-y-0.5 text-gray-200">
              {(c.rows ?? []).slice(0, 1).map((row: any, i: number) => (
                <div key={i} className="grid grid-cols-2 gap-x-2">
                  <span><span className="text-gray-500">min:</span> {row.min_stock}</span>
                  <span><span className="text-gray-500">max:</span> {row.max_stock}</span>
                  <span><span className="text-gray-500">wos:</span> {row.wos}</span>
                  <span><span className="text-gray-500">aps:</span> {row.aps}</span>
                </div>
              ))}
              {(c.rows ?? []).length > 1 && (
                <div className="text-[10px] text-gray-500">+{(c.rows ?? []).length - 1} more rows</div>
              )}
              {(c.rows ?? []).length === 0 && <div className="text-gray-500">no constraint rows</div>}
            </div>
          </div>
        )}
        {psm && (
          <div className="rounded border border-gray-800 bg-gray-900/40 p-2">
            <div className="text-[10px] uppercase text-gray-500 mb-1" title={`rcl_code=${psm.rcl_code} rule_code=${psm.rule_code}`}>
              PSM
            </div>
            <div className="text-gray-200">
              <span className="text-gray-500">matched </span>
              <span className="font-mono text-[11px]">{psm.rcl_code}</span>
              <span className="text-gray-500"> / </span>
              <span className="font-mono text-[11px]">{psm.rule_code}</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded border border-gray-800 bg-gray-900/40 px-3 py-2">
      <div className="text-[10px] uppercase tracking-wider text-gray-500">{label}</div>
      <div className="text-lg font-semibold tabular-nums text-gray-100 mt-0.5">{value}</div>
    </div>
  );
}

function ChildrenBars({
  children,
  fmt,
  onClick,
}: {
  children: TreeNode[];
  fmt: (n?: number, money?: boolean) => string;
  // When provided, each row renders as a clickable link that drills the
  // tree on the left and loads the child's detail on the right. Omit
  // (e.g. for the brand-spread section) when the row has no spine path.
  onClick?: (node: TreeNode) => void;
}) {
  // The bar visualizes whichever of LW Revenue / OH each row carries —
  // hierarchy children + brand spread come with lw_revenue; the
  // hierarchy-spread synthetic rows only carry oh. Pick per-row so the
  // chart stays meaningful no matter who's calling.
  const barValue = (c: TreeNode): number =>
    typeof c.lw_revenue === "number" ? c.lw_revenue : (c.oh ?? 0);
  const max = Math.max(...children.map(barValue), 1);
  // Hide columns that are universally empty so the header row doesn't
  // promise data we don't have. e.g. brand-spread has no child_count;
  // hierarchy-spread has no lw_revenue.
  const hasChildCount = children.some((c) => typeof c.child_count === "number");
  const hasLwRev = children.some((c) => typeof c.lw_revenue === "number");
  const gridCols = [
    "minmax(0,1fr)",         // name
    hasChildCount ? "60px" : null,
    "80px",                  // OH
    hasLwRev ? "100px" : null,
    "minmax(0,1fr)",         // bar
  ].filter(Boolean).join(" ");
  return (
    <div className="space-y-0.5">
      {/* Header row — labels every column so users don't have to guess. */}
      <div
        className="grid gap-2 px-2 py-1 text-[9px] uppercase tracking-wider text-gray-500 font-semibold"
        style={{ gridTemplateColumns: gridCols }}
      >
        <span>Name</span>
        {hasChildCount && <span className="text-right">Children</span>}
        <span className="text-right">OH</span>
        {hasLwRev && <span className="text-right">LW Rev</span>}
        <span>{hasLwRev ? "LW Rev" : "OH"} share</span>
      </div>
      {children.map((c) => {
        const pct = (barValue(c) / max) * 100;
        const Wrapper: any = onClick ? "button" : "div";
        return (
          <Wrapper
            key={c.key}
            onClick={onClick ? () => onClick(c) : undefined}
            className={`w-full grid gap-2 items-baseline px-2 py-1 rounded text-left ${
              onClick ? "hover:bg-blue-950/30 cursor-pointer" : "hover:bg-gray-900/40"
            }`}
            style={{ gridTemplateColumns: gridCols }}
            title={onClick ? `Drill into ${c.display}` : c.display}
          >
            <span className={`text-xs truncate ${onClick ? "text-blue-300" : "text-gray-200"}`}>
              {c.display}
            </span>
            {hasChildCount && (
              <span className="text-[11px] tabular-nums text-gray-400 text-right">
                {typeof c.child_count === "number" ? c.child_count.toLocaleString() : "—"}
              </span>
            )}
            <span className="text-[11px] tabular-nums text-gray-300 text-right">
              {fmt(c.oh)}
            </span>
            {hasLwRev && (
              <span className="text-[11px] tabular-nums text-gray-300 text-right">
                {fmt(c.lw_revenue, true)}
              </span>
            )}
            <div className="h-1.5 rounded-full bg-gray-800 overflow-hidden self-center">
              <div
                className="h-full bg-blue-700/70"
                style={{ width: `${pct}%` }}
              />
            </div>
          </Wrapper>
        );
      })}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Exception View Tab (Phase 1) — surface articles violating rules    */
/*                                                                     */
/*  Five rules: stockout, overstock, below_min, reserve_gap,           */
/*  no_eligible_stores. Chips show counts (filter-aware), clicking a   */
/*  chip toggles its rule into the active selection. Result table      */
/*  shows articles firing any selected rule, tagged with risk_flags.   */
/* ------------------------------------------------------------------ */

interface RuleDef {
  key: string;
  label: string;
  tone: string;
  formula: string;
  meaning: string;
}
const RULE_DEFS: RuleDef[] = [
  {
    key: "stockout",
    label: "Stockout",
    tone: "red",
    formula: "OH = 0  AND  lw_units > 0",
    meaning:
      "Article sold last week but on-hand stock is zero now. Likely lost sales — replenish or reallocate.",
  },
  {
    key: "overstock",
    label: "Overstock",
    tone: "amber",
    formula: "OH > max_stock × 1.5",
    meaning:
      "On-hand exceeds 1.5× the constraint maximum. Capital tied up, markdown risk — consider transfer or markdown.",
  },
  {
    key: "below_min",
    label: "Below min",
    tone: "yellow",
    formula: "0 < OH < min_stock",
    meaning:
      "Some inventory remains but it's below the minimum target. At risk of going stockout this week — replenish.",
  },
  {
    key: "reserve_gap",
    label: "Reserve gap",
    tone: "purple",
    formula: "allocated_units > OH + OO + IT",
    meaning:
      "Promised more units than the supply chain physically has (on-hand + on-order + in-transit). Over-committed — reallocate or release.",
  },
  {
    key: "no_eligible_stores",
    label: "No eligible stores",
    tone: "rose",
    formula: "PSM resolver: no match for product_code",
    meaning:
      "No retail location is configured to sell this product. Either the PSM mapping is missing or the rule is mis-keyed — config issue.",
  },
];

/// Hover card for a rule chip — renders the formula in mono and the
/// plain-language meaning below. CSS-only (no JS state, no portal):
/// the wrapper has `group`, the popover is `hidden group-hover:block`.
/// Native `title` is kept too as a fallback for screen readers /
/// keyboard-focus / clipped popovers.
function RuleHoverCard({
  rd,
  children,
  align = "start",
}: {
  rd: RuleDef;
  children: React.ReactNode;
  align?: "start" | "end";
}) {
  return (
    // No native `title` — the rich CSS popover below is the canonical
    // tooltip surface. Browser titles fight with the hover card on
    // some platforms (delayed pop, double-tooltip) so the rich card
    // stands alone.
    <span className="relative inline-flex group">
      {children}
      <span
        role="tooltip"
        className={`pointer-events-none absolute top-full mt-1 hidden group-hover:block z-50 w-72 p-3 rounded border border-gray-700 bg-gray-950/95 shadow-xl backdrop-blur-sm ${
          align === "end" ? "right-0" : "left-0"
        }`}
      >
        <div className="text-[10px] uppercase tracking-wider text-gray-500 font-semibold mb-1.5">
          {rd.label}
        </div>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          Criteria
        </div>
        <code className="block text-[11px] font-mono text-amber-300 bg-black/30 rounded px-1.5 py-1 mb-2">
          {rd.formula}
        </code>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          What it means
        </div>
        <div className="text-xs text-gray-200 leading-relaxed">{rd.meaning}</div>
      </span>
    </span>
  );
}

const TONE_CLASS: Record<string, { card: string; chip: string; chipActive: string }> = {
  red:    { card: "border-red-900/60 bg-red-950/40",       chip: "border-red-900/60 hover:bg-red-950/30 text-red-300",       chipActive: "border-red-500 bg-red-900/60 text-red-100" },
  amber:  { card: "border-amber-900/60 bg-amber-950/40",   chip: "border-amber-900/60 hover:bg-amber-950/30 text-amber-300", chipActive: "border-amber-500 bg-amber-900/60 text-amber-100" },
  yellow: { card: "border-yellow-900/60 bg-yellow-950/40", chip: "border-yellow-900/60 hover:bg-yellow-950/30 text-yellow-300", chipActive: "border-yellow-500 bg-yellow-900/60 text-yellow-100" },
  purple: { card: "border-purple-900/60 bg-purple-950/40", chip: "border-purple-900/60 hover:bg-purple-950/30 text-purple-300", chipActive: "border-purple-500 bg-purple-900/60 text-purple-100" },
  rose:   { card: "border-rose-900/60 bg-rose-950/40",     chip: "border-rose-900/60 hover:bg-rose-950/30 text-rose-300",     chipActive: "border-rose-500 bg-rose-900/60 text-rose-100" },
};

function ExceptionViewTab({ dv }: { dv: any }) {
  const [resolvedSourceKind, setResolvedSourceKind] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    const sourceId =
      dv?.source?.type === "source" ? (dv.source.config?.source_id as string | undefined) : null;
    if (!sourceId) { setResolvedSourceKind(null); return; }
    api.getSource(sourceId)
      .then((row: any) => { if (!cancelled) setResolvedSourceKind(row?.kind ?? null); })
      .catch(() => { if (!cancelled) setResolvedSourceKind(null); });
    return () => { cancelled = true; };
  }, [dv.id, dv?.source]);
  const isGraphSource = resolvedSourceKind === "graph";

  // ── Filter panel (mirrors Tree View's: multi-select, Apply gate) ──
  const [filterConfigs, setFilterConfigs] = useState<any[]>([]);
  const [pendingFilters, setPendingFilters] = useState<Record<string, string[]>>({});
  const [appliedFilters, setAppliedFilters] = useState<Record<string, string[]>>({});
  const [openFilterCol, setOpenFilterCol] = useState<string | null>(null);
  const [distinctCache, setDistinctCache] = useState<Record<string, string[]>>({});
  useEffect(() => {
    let cancelled = false;
    const dims: any[] = Array.isArray(dv.dimensions) ? dv.dimensions : [];
    const ids = dims
      .map((d) => (d && typeof d === "object" ? d.filter_config_id : null))
      .filter((x): x is string => typeof x === "string" && x.length > 0);
    if (ids.length === 0) { setFilterConfigs([]); return; }
    api.getFilterConfigs()
      .then((all: any[]) => {
        if (cancelled) return;
        const byId = new Map(all.map((fc) => [String(fc.id), fc]));
        setFilterConfigs(ids.map((id) => byId.get(id)).filter((fc): fc is any => !!fc));
      })
      .catch(() => setFilterConfigs([]));
    return () => { cancelled = true; };
  }, [dv.id, dv.dimensions]);
  const filterEntries = useMemo(() => {
    const out: { col: string; display: string; order: number }[] = [];
    for (const fc of filterConfigs) {
      const cols: any[] = Array.isArray(fc.filter_columns) ? fc.filter_columns : [];
      for (const c of cols) {
        if (!c || typeof c !== "object" || !c.column) continue;
        out.push({
          col: String(c.column),
          display: String(c.display_name || c.column),
          order: typeof c.display_order === "number" ? c.display_order : 999,
        });
      }
    }
    out.sort((a, b) => a.order - b.order);
    return out;
  }, [filterConfigs]);
  const filtersDirty = useMemo(
    () => JSON.stringify(pendingFilters) !== JSON.stringify(appliedFilters),
    [pendingFilters, appliedFilters],
  );
  const filtersPayload = useMemo(
    () =>
      Object.entries(appliedFilters)
        .filter(([_, v]) => v && v.length > 0)
        .map(([col, values]) => ({ attribute_name: col, values, operator: "in" as const })),
    [appliedFilters],
  );
  const setFilterVal = (col: string, values: string[]) => {
    setPendingFilters((prev) => {
      const next = { ...prev };
      if (values.length === 0) delete next[col];
      else next[col] = values;
      return next;
    });
  };
  const fetchDistinct = useCallback(
    async (col: string) => {
      if (distinctCache[col]) return;
      try {
        const res = await api.getDataViewData(dv.id, { limit: 5000, node_kind: "ARTICLE" });
        const vals = new Set<string>();
        for (const r of res.rows || []) {
          const v = (r as any)[col];
          if (v != null && v !== "") vals.add(String(v));
        }
        setDistinctCache((p) => ({ ...p, [col]: Array.from(vals).sort() }));
      } catch (_) {
        setDistinctCache((p) => ({ ...p, [col]: [] }));
      }
    },
    [dv.id, distinctCache],
  );

  // ── Chip counts + rule selection ──
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [totalArticles, setTotalArticles] = useState<number>(0);
  const [countsLoading, setCountsLoading] = useState(false);
  const [selectedRules, setSelectedRules] = useState<string[]>([]);
  const [rows, setRows] = useState<any[]>([]);
  const [rowsTotal, setRowsTotal] = useState(0);
  const [rowsLoading, setRowsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const PAGE_SIZE = 100;

  // Layout toggle: flat table or hierarchy tree. Tree mode reuses the
  // same drilldown machinery as the dedicated Tree View tab, but with
  // the active rule selection threaded through `rules` so the alive
  // hierarchy is computed against exception articles only.
  const [layout, setLayout] = useState<"table" | "tree">("table");
  const [treeDimension, setTreeDimension] = useState<"product" | "store">("product");
  const [treeRoots, setTreeRoots] = useState<TreeNode[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [treeExpanded, setTreeExpanded] = useState<Set<string>>(new Set());

  // Refresh counts whenever filters change. Independent of selectedRules
  // because the chip counts are always shown (even when nothing selected).
  useEffect(() => {
    if (!isGraphSource) return;
    let cancelled = false;
    setCountsLoading(true);
    api.exceptionsCounts(filtersPayload)
      .then((res) => {
        if (cancelled) return;
        setCounts(res.counts || {});
        setTotalArticles(res.total_articles || 0);
      })
      .catch((e) => { if (!cancelled) setError(e?.message ?? "counts failed"); })
      .finally(() => { if (!cancelled) setCountsLoading(false); });
    return () => { cancelled = true; };
  }, [isGraphSource, filtersPayload]);

  // Refresh result rows when selection or filters change.
  useEffect(() => {
    if (!isGraphSource) return;
    if (selectedRules.length === 0) {
      setRows([]);
      setRowsTotal(0);
      return;
    }
    let cancelled = false;
    setRowsLoading(true);
    api.exceptionsList(selectedRules, filtersPayload, {
      limit: PAGE_SIZE,
      offset: (page - 1) * PAGE_SIZE,
    })
      .then((res) => {
        if (cancelled) return;
        setRows(res.rows || []);
        setRowsTotal(res.total || 0);
      })
      .catch((e) => { if (!cancelled) setError(e?.message ?? "list failed"); })
      .finally(() => { if (!cancelled) setRowsLoading(false); });
    return () => { cancelled = true; };
  }, [isGraphSource, filtersPayload, selectedRules, page]);

  const toggleRule = (key: string) => {
    setPage(1);
    setSelectedRules((prev) =>
      prev.includes(key) ? prev.filter((r) => r !== key) : [...prev, key],
    );
  };

  // ── Tree mode: load roots when filters/rules/dimension change ──
  useEffect(() => {
    if (!isGraphSource || layout !== "tree") return;
    let cancelled = false;
    setTreeLoading(true);
    setTreeError(null);
    setTreeRoots([]);
    setTreeExpanded(new Set());
    const rootKind: TreeKind = treeDimension === "product" ? "L0" : "CHANNEL";
    api.getDataViewData(dv.id, {
      limit: 500,
      node_kind: rootKind,
      filters: filtersPayload,
      rules: selectedRules,
    })
      .then((res) => {
        if (cancelled) return;
        const out: TreeNode[] = (res.rows || [])
          .map((r: any) => makeNodeFromRow(r, rootKind))
          .sort((a, b) => a.display.localeCompare(b.display));
        setTreeRoots(out);
      })
      .catch((e) => { if (!cancelled) setTreeError(e?.message ?? "load failed"); })
      .finally(() => { if (!cancelled) setTreeLoading(false); });
    return () => { cancelled = true; };
  }, [isGraphSource, layout, treeDimension, filtersPayload, selectedRules, dv.id]);

  const updateTreeNode = useCallback(
    (key: string, mutate: (n: TreeNode) => TreeNode) => {
      const walk = (nodes: TreeNode[]): TreeNode[] =>
        nodes.map((n) => {
          if (n.key === key) return mutate(n);
          if (n.children && n.children.length > 0) {
            const next = walk(n.children);
            if (next !== n.children) return { ...n, children: next };
          }
          return n;
        });
      setTreeRoots((prev) => walk(prev));
    },
    [],
  );

  const ensureTreeChildren = useCallback(
    async (node: TreeNode) => {
      if (node.children !== undefined || node.loadingChildren) return;
      const childKind = NEXT_KIND_FOR[node.kind];
      if (!childKind) {
        updateTreeNode(node.key, (n) => ({ ...n, children: [] }));
        return;
      }
      updateTreeNode(node.key, (n) => ({ ...n, loadingChildren: true, errorChildren: undefined }));
      try {
        const edge = node.kind === "CHANNEL" ? "stores" : "children";
        const resp = await api.graphTraverse(
          { kind: node.kind as any, name: node.name },
          edge as any,
          filtersPayload,
          selectedRules,
        );
        const out: TreeNode[] = (resp.rows || [])
          .map((r: any) => makeNodeFromRow(r, childKind))
          .sort((a, b) => {
            const ar = a.lw_revenue ?? -Infinity;
            const br = b.lw_revenue ?? -Infinity;
            if (ar !== br) return br - ar;
            return a.display.localeCompare(b.display);
          });
        updateTreeNode(node.key, (n) => ({ ...n, children: out, loadingChildren: false }));
      } catch (e: any) {
        updateTreeNode(node.key, (n) => ({
          ...n,
          loadingChildren: false,
          errorChildren: e?.message ?? "load failed",
          children: [],
        }));
      }
    },
    [updateTreeNode, filtersPayload, selectedRules],
  );

  const toggleTreeExpand = useCallback(
    (node: TreeNode) => {
      setTreeExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(node.key)) {
          next.delete(node.key);
        } else {
          next.add(node.key);
          if (node.children === undefined) void ensureTreeChildren(node);
        }
        return next;
      });
    },
    [ensureTreeChildren],
  );

  const fmtTree = (n?: number, money = false) => {
    if (typeof n !== "number") return "—";
    if (money) return `$${n.toLocaleString()}`;
    return n.toLocaleString();
  };

  if (!isGraphSource && resolvedSourceKind !== null) {
    return (
      <div className="px-3 py-6 text-sm text-gray-500">
        Exception View is only available for DataViews bound to an{" "}
        <code className="text-gray-300">article_graph</code> source.
      </div>
    );
  }

  const totalPages = Math.max(1, Math.ceil(rowsTotal / PAGE_SIZE));
  const flagged = Object.values(counts).reduce((a, b) => Math.max(a, b), 0);

  return (
    <div className="space-y-3">
      {/* Headline + layout toggle */}
      <div className="flex items-center gap-3 text-xs">
        <ShieldAlert size={14} className="text-amber-400" />
        <span className="text-gray-300">
          {countsLoading ? (
            <span className="inline-flex items-center gap-1 text-gray-500">
              <Loader2 size={11} className="animate-spin" /> Counting…
            </span>
          ) : (
            <>
              <span className="text-gray-100 font-medium">{flagged.toLocaleString()}</span>
              <span className="text-gray-500"> at-risk</span>
              <span className="text-gray-500"> · {totalArticles.toLocaleString()} articles in scope</span>
            </>
          )}
        </span>
        <div className="ml-auto flex rounded border border-gray-800 overflow-hidden text-[11px]">
          <button
            onClick={() => setLayout("table")}
            className={`px-2.5 py-1 ${
              layout === "table"
                ? "bg-blue-600 text-white"
                : "bg-gray-900 text-gray-300 hover:bg-gray-800"
            }`}
          >
            Table
          </button>
          <button
            onClick={() => setLayout("tree")}
            className={`px-2.5 py-1 border-l border-gray-800 ${
              layout === "tree"
                ? "bg-blue-600 text-white"
                : "bg-gray-900 text-gray-300 hover:bg-gray-800"
            }`}
          >
            Tree
          </button>
        </div>
      </div>

      {/* Filter strip */}
      {filterEntries.length > 0 && (
        <div className="rounded border border-gray-800 bg-gray-900/40 px-3 py-2 flex items-center gap-2 flex-wrap">
          <Filter size={12} className="text-gray-500 shrink-0" />
          {filterEntries.map((fe) => {
            const sel = pendingFilters[fe.col] || [];
            const isOpen = openFilterCol === fe.col;
            const vals = distinctCache[fe.col] || [];
            return (
              <div key={fe.col} className="relative">
                <button
                  onClick={() => {
                    if (isOpen) setOpenFilterCol(null);
                    else { setOpenFilterCol(fe.col); fetchDistinct(fe.col); }
                  }}
                  className={`flex items-center gap-1 text-xs px-2 py-1 rounded border ${
                    sel.length > 0
                      ? "bg-blue-950/60 border-blue-800 text-blue-200"
                      : "bg-gray-900 border-gray-800 text-gray-300 hover:border-gray-700"
                  }`}
                >
                  {fe.display}
                  {sel.length > 0 && (
                    <span className="bg-blue-700 text-white text-[9px] px-1 rounded-full">
                      {sel.length}
                    </span>
                  )}
                </button>
                {isOpen && (
                  <div className="absolute top-full left-0 mt-1 z-50 w-64 bg-gray-950 rounded border border-gray-800 shadow-lg overflow-hidden">
                    <div className="p-2 border-b border-gray-800 flex items-center justify-between">
                      <span className="text-[10px] uppercase tracking-wider text-gray-400 font-semibold truncate">
                        {fe.display}
                      </span>
                      <span className="text-[10px] text-gray-500 tabular-nums">
                        {sel.length}/{vals.length}
                      </span>
                    </div>
                    <div className="px-2 py-1 flex items-center gap-3 border-b border-gray-800">
                      <button
                        onClick={() => setFilterVal(fe.col, [...vals])}
                        disabled={vals.length === 0 || sel.length === vals.length}
                        className="text-[10px] text-blue-400 hover:underline disabled:opacity-40"
                      >
                        Select all
                      </button>
                      <button
                        onClick={() => setFilterVal(fe.col, [])}
                        disabled={sel.length === 0}
                        className="text-[10px] text-blue-400 hover:underline disabled:opacity-40"
                      >
                        Clear all
                      </button>
                    </div>
                    <div className="max-h-56 overflow-auto p-1">
                      {vals.length === 0 && (
                        <div className="text-[10px] text-gray-500 py-2 px-2">
                          {distinctCache[fe.col] === undefined ? "Loading…" : "No values"}
                        </div>
                      )}
                      {vals.map((v) => (
                        <label
                          key={v}
                          className="flex items-center gap-2 px-2 py-1 text-xs text-gray-200 hover:bg-gray-900 rounded cursor-pointer"
                        >
                          <input
                            type="checkbox"
                            checked={sel.includes(v)}
                            onChange={() => {
                              const next = sel.includes(v)
                                ? sel.filter((x) => x !== v)
                                : [...sel, v];
                              setFilterVal(fe.col, next);
                            }}
                            className="rounded border-gray-600 bg-gray-800 text-blue-500"
                          />
                          <span className="truncate">{v}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
          <div className="ml-auto flex items-center gap-2">
            {Object.keys(pendingFilters).length > 0 && (
              <button
                onClick={() => { setPendingFilters({}); setAppliedFilters({}); setPage(1); }}
                className="text-[10px] text-red-400 hover:underline"
              >
                Clear
              </button>
            )}
            <button
              onClick={() => { setAppliedFilters(pendingFilters); setPage(1); }}
              disabled={!filtersDirty}
              className={`flex items-center gap-1 text-[11px] px-2 py-1 rounded font-medium ${
                filtersDirty
                  ? "bg-blue-600 text-white hover:bg-blue-500"
                  : "bg-gray-800 text-gray-500 cursor-not-allowed"
              }`}
            >
              Apply filters
              {filtersDirty && (
                <span className="ml-1 inline-block w-1.5 h-1.5 rounded-full bg-amber-300" />
              )}
            </button>
          </div>
        </div>
      )}
      {openFilterCol && (
        <div className="fixed inset-0 z-40" onClick={() => setOpenFilterCol(null)} />
      )}

      {/* Rule chips */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
        {RULE_DEFS.map((rd) => {
          const tone = TONE_CLASS[rd.tone] || TONE_CLASS.red;
          const active = selectedRules.includes(rd.key);
          const count = counts[rd.key] ?? 0;
          return (
            // RuleHoverCard wraps so the popover anchors off the card.
            // The card itself is still the click target (toggles the
            // rule into the selection).
            <RuleHoverCard key={rd.key} rd={rd}>
              <button
                onClick={() => toggleRule(rd.key)}
                className={`block w-full text-left rounded border px-3 py-2 transition-colors ${
                  active ? tone.chipActive : tone.chip
                }`}
              >
                <div className="flex items-center gap-1.5 text-[11px] uppercase tracking-wider">
                  <ShieldAlert size={11} />
                  <span className="truncate">{rd.label}</span>
                </div>
                <div className="text-xl font-semibold tabular-nums mt-1">
                  {countsLoading ? "—" : count.toLocaleString()}
                </div>
              </button>
            </RuleHoverCard>
          );
        })}
      </div>

      {error && (
        <div className="rounded border border-red-900/60 bg-red-950/30 p-2 text-xs text-red-300">
          {error}
        </div>
      )}

      {/* Result — Table view */}
      {layout === "table" && (
      <div className="rounded border border-gray-800 overflow-hidden">
        <div className="bg-gray-900/60 px-3 py-2 flex items-center gap-3 border-b border-gray-800">
          <span className="text-xs text-gray-300">
            {selectedRules.length === 0
              ? "Select one or more chips above to list articles"
              : rowsLoading
                ? "Loading…"
                : `${rowsTotal.toLocaleString()} articles (page ${page} of ${totalPages})`}
          </span>
          {selectedRules.length > 0 && (
            <button
              onClick={() => setSelectedRules([])}
              className="text-[10px] text-blue-400 hover:underline ml-auto"
            >
              Clear selection
            </button>
          )}
        </div>
        {selectedRules.length > 0 && (
          <>
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead className="bg-gray-900/40 text-gray-400 text-[10px] uppercase tracking-wider">
                  <tr>
                    <th className="text-left px-3 py-2">Article</th>
                    <th className="text-left px-3 py-2">Brand</th>
                    <th className="text-left px-3 py-2">L1</th>
                    <th className="text-right px-3 py-2">OH</th>
                    <th className="text-right px-3 py-2">Min</th>
                    <th className="text-right px-3 py-2">Max</th>
                    <th className="text-right px-3 py-2">LW Units</th>
                    <th className="text-left px-3 py-2">Sizes</th>
                    <th className="text-left px-3 py-2">Flags</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r, i) => (
                    <tr key={i} className="border-t border-gray-900/60 hover:bg-gray-900/40">
                      <td className="px-3 py-1.5 font-mono text-gray-200">{r.article}</td>
                      <td className="px-3 py-1.5 text-gray-300">{r.brand || "—"}</td>
                      <td className="px-3 py-1.5 text-gray-300 truncate max-w-[200px]">{r.l1_name || "—"}</td>
                      <td className="px-3 py-1.5 text-right tabular-nums text-gray-200">{(r.oh ?? 0).toLocaleString()}</td>
                      <td className="px-3 py-1.5 text-right tabular-nums text-gray-400">{r.min_stock ?? "—"}</td>
                      <td className="px-3 py-1.5 text-right tabular-nums text-gray-400">{r.max_stock ?? "—"}</td>
                      <td className="px-3 py-1.5 text-right tabular-nums text-gray-200">{(r.lw_units ?? 0).toLocaleString()}</td>
                      <td className="px-3 py-1.5">
                        <div className="flex flex-wrap gap-1 max-w-[280px]">
                          {Array.isArray(r.sizes) && r.sizes.length > 0 ? (
                            r.sizes.map((s: { size: string; oh: number }) => (
                              <span
                                key={s.size}
                                className="px-1.5 py-0.5 text-[10px] rounded border border-gray-700 bg-gray-900 text-gray-300 tabular-nums"
                                title={`size ${s.size}: OH ${s.oh}`}
                              >
                                <span className="text-gray-500">{s.size}</span>
                                <span className="text-gray-200 ml-1 font-medium">{s.oh.toLocaleString()}</span>
                              </span>
                            ))
                          ) : (
                            <span className="text-[10px] text-gray-600">—</span>
                          )}
                        </div>
                      </td>
                      <td className="px-3 py-1.5">
                        <div className="flex flex-wrap gap-1">
                          {(r.risk_flags || []).map((f: string) => {
                            const def = RULE_DEFS.find((d) => d.key === f);
                            const tone = TONE_CLASS[def?.tone || "red"];
                            const pill = (
                              <span className={`px-1.5 py-0.5 text-[10px] rounded border ${tone.chip}`}>
                                {def?.label || f}
                              </span>
                            );
                            // Wrap in the hover card when we have a
                            // RuleDef; fall back to the bare pill for
                            // unknown flag strings (forward-compat
                            // with new rules the backend may add).
                            return def ? (
                              <RuleHoverCard key={f} rd={def}>
                                {pill}
                              </RuleHoverCard>
                            ) : (
                              <span key={f}>{pill}</span>
                            );
                          })}
                        </div>
                      </td>
                    </tr>
                  ))}
                  {!rowsLoading && rows.length === 0 && (
                    <tr>
                      <td colSpan={9} className="px-3 py-6 text-center text-xs text-gray-500">
                        No articles match the selected rules + filters.
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
            {totalPages > 1 && (
              <div className="flex items-center justify-end gap-2 px-3 py-2 border-t border-gray-800 bg-gray-900/40">
                <button
                  onClick={() => setPage((p) => Math.max(1, p - 1))}
                  disabled={page <= 1}
                  className="px-2 py-1 text-[11px] rounded bg-gray-800 text-gray-300 disabled:opacity-40 hover:bg-gray-700"
                >
                  Prev
                </button>
                <span className="text-[11px] text-gray-400 tabular-nums">
                  {page} / {totalPages}
                </span>
                <button
                  onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
                  disabled={page >= totalPages}
                  className="px-2 py-1 text-[11px] rounded bg-gray-800 text-gray-300 disabled:opacity-40 hover:bg-gray-700"
                >
                  Next
                </button>
              </div>
            )}
          </>
        )}
      </div>
      )}

      {/* Result — Tree view */}
      {layout === "tree" && (
        <div className="space-y-2">
          <div className="flex items-center gap-3">
            <span className="text-[10px] uppercase tracking-wider text-gray-500 font-medium">
              Dimension
            </span>
            <div className="flex rounded border border-gray-800 overflow-hidden text-xs">
              <button
                onClick={() => setTreeDimension("product")}
                className={`px-3 py-1 ${
                  treeDimension === "product"
                    ? "bg-blue-600 text-white"
                    : "bg-gray-900 text-gray-300 hover:bg-gray-800"
                }`}
              >
                Product
              </button>
              <button
                onClick={() => setTreeDimension("store")}
                className={`px-3 py-1 border-l border-gray-800 ${
                  treeDimension === "store"
                    ? "bg-blue-600 text-white"
                    : "bg-gray-900 text-gray-300 hover:bg-gray-800"
                }`}
              >
                Store
              </button>
            </div>
            <span className="text-[10px] text-gray-500 ml-auto">
              {selectedRules.length === 0
                ? "Select one or more chips above to scope the tree"
                : `${treeRoots.length} root${treeRoots.length === 1 ? "" : "s"} contain at-risk articles`}
            </span>
          </div>
          {treeError && (
            <div className="rounded border border-red-900/60 bg-red-950/30 p-2 text-xs text-red-300">
              {treeError}
            </div>
          )}
          <div className="rounded border border-gray-800 overflow-hidden">
            <div className="bg-gray-900/60 grid grid-cols-[minmax(0,1fr)_80px_110px_110px_120px] gap-2 px-3 py-2 text-[10px] uppercase tracking-wider text-gray-400 font-semibold">
              <span>Name</span>
              <span className="text-right">Children</span>
              <span className="text-right">OH</span>
              <span className="text-right">LW Units</span>
              <span className="text-right">LW Revenue</span>
            </div>
            {selectedRules.length === 0 ? (
              <div className="px-3 py-6 text-xs text-gray-500">
                Select one or more chips above to drill down by hierarchy.
              </div>
            ) : treeLoading ? (
              <div className="px-3 py-6 text-xs text-gray-500 flex items-center gap-2">
                <Loader2 size={12} className="animate-spin" /> Loading roots…
              </div>
            ) : treeRoots.length === 0 ? (
              <div className="px-3 py-6 text-xs text-gray-500">
                No hierarchy nodes contain at-risk articles for this scope.
              </div>
            ) : (
              <div className="divide-y divide-gray-900/60">
                {treeRoots.map((n) => (
                  <TreeRow
                    key={n.key}
                    node={n}
                    depth={0}
                    expanded={treeExpanded}
                    onToggle={toggleTreeExpand}
                    fmt={fmtTree}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Filters Tab — bind a Filter Configuration per applicable dimension */
/* ------------------------------------------------------------------ */

interface DimensionBinding {
  dimension_ref: string;
  filter_config_id: string | null;
}

function FiltersTab({ dv, onReload }: { dv: any; onReload: () => void }) {
  const [allDimensions, setAllDimensions] = useState<any[]>([]);
  const [allFilterConfigs, setAllFilterConfigs] = useState<any[]>([]);
  const [bindings, setBindings] = useState<DimensionBinding[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);

  // Load dimensions + filter configs once + parse the dv's existing
  // bindings. Accepts both shapes:
  //   ["product", "store"]                   — legacy: just refs
  //   [{dimension_ref: "...", filter_config_id?: "..."}]
  //   [{ref: "..."}]                         — older still
  useEffect(() => {
    let cancelled = false;
    Promise.all([api.getDimensions(), api.getFilterConfigs()])
      .then(([dims, fcs]) => {
        if (cancelled) return;
        setAllDimensions(dims);
        setAllFilterConfigs(fcs);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Sync bindings from the dv whenever it reloads.
  useEffect(() => {
    const raw: any[] = Array.isArray(dv.dimensions) ? dv.dimensions : [];
    const out: DimensionBinding[] = raw
      .map((entry) => {
        if (typeof entry === "string") {
          return { dimension_ref: entry, filter_config_id: null };
        }
        if (entry && typeof entry === "object") {
          const ref = entry.dimension_ref ?? entry.ref ?? null;
          if (!ref) return null;
          return {
            dimension_ref: String(ref),
            filter_config_id: entry.filter_config_id
              ? String(entry.filter_config_id)
              : null,
          };
        }
        return null;
      })
      .filter((b): b is DimensionBinding => b !== null);
    setBindings(out);
  }, [dv.id, dv.dimensions]);

  const isApplied = (ref: string) =>
    bindings.some((b) => b.dimension_ref === ref);

  const toggle = (ref: string) => {
    setBindings((prev) => {
      if (prev.some((b) => b.dimension_ref === ref)) {
        return prev.filter((b) => b.dimension_ref !== ref);
      }
      return [...prev, { dimension_ref: ref, filter_config_id: null }];
    });
  };

  const setFc = (ref: string, fcId: string | null) => {
    setBindings((prev) =>
      prev.map((b) =>
        b.dimension_ref === ref ? { ...b, filter_config_id: fcId } : b,
      ),
    );
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveMsg(null);
    try {
      await api.updateDataView(dv.id, { dimensions: bindings });
      setSaveMsg("Saved");
      setTimeout(() => setSaveMsg(null), 2000);
      onReload();
    } catch (e: any) {
      setSaveMsg("Error: " + (e?.message || "Save failed"));
    } finally {
      setSaving(false);
    }
  };

  const fcByDim = new Map<string, any[]>();
  for (const fc of allFilterConfigs) {
    const dim = String(fc.dimension_ref || "");
    if (!fcByDim.has(dim)) fcByDim.set(dim, []);
    fcByDim.get(dim)!.push(fc);
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div>
          <h3 className="text-sm font-medium text-gray-200">Dimension Filters</h3>
          <p className="text-[11px] text-gray-500 mt-0.5">
            Toggle which dimensions apply to this DataView. For each, pick a
            Filter Configuration (the cascading filter panel definition).
          </p>
        </div>
        <div className="flex items-center gap-2">
          {saveMsg && (
            <span
              className={`text-xs ${saveMsg.startsWith("Error") ? "text-red-400" : "text-green-400"}`}
            >
              {saveMsg}
            </span>
          )}
          <button
            onClick={handleSave}
            disabled={saving}
            className="flex items-center gap-1 px-3 py-1.5 text-xs rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
          >
            {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
            Save
          </button>
        </div>
      </div>

      {allDimensions.length === 0 ? (
        <div className="rounded border border-gray-800 p-3 text-xs text-gray-500 italic">
          No dimensions defined yet. Add one in the Dimensions tab.
        </div>
      ) : (
        <div className="border border-gray-800 rounded overflow-hidden">
          <table className="w-full text-sm">
            <thead className="bg-gray-900/50 text-gray-400 text-xs">
              <tr>
                <th className="px-3 py-2 text-left w-10"></th>
                <th className="px-3 py-2 text-left">Dimension</th>
                <th className="px-3 py-2 text-left">Master Table</th>
                <th className="px-3 py-2 text-left">Filter Configuration</th>
              </tr>
            </thead>
            <tbody>
              {allDimensions.map((dim: any) => {
                const ref = String(dim.id);
                const applied = isApplied(ref);
                const binding = bindings.find((b) => b.dimension_ref === ref);
                const fcs = fcByDim.get(ref) || [];
                return (
                  <tr key={ref} className="border-t border-gray-800 hover:bg-gray-900/30">
                    <td className="px-3 py-2">
                      <input
                        type="checkbox"
                        checked={applied}
                        onChange={() => toggle(ref)}
                        title={
                          applied
                            ? "Dimension applies to this DataView"
                            : "Click to apply this dimension"
                        }
                        className="rounded border-gray-600 bg-gray-800 text-blue-500 focus:ring-blue-500"
                      />
                    </td>
                    <td className="px-3 py-2">
                      <div className="flex items-baseline gap-2">
                        <span className="text-gray-200 font-medium">
                          {dim.display_name || dim.id}
                        </span>
                        <span className="text-[10px] text-gray-500 font-mono">{dim.id}</span>
                      </div>
                    </td>
                    <td className="px-3 py-2 text-[11px] text-gray-500 font-mono">
                      {dim.master_table || "—"}
                    </td>
                    <td className="px-3 py-2">
                      {applied ? (
                        fcs.length > 0 ? (
                          <select
                            value={binding?.filter_config_id ?? ""}
                            onChange={(e) =>
                              setFc(ref, e.target.value ? e.target.value : null)
                            }
                            className="w-full px-2 py-1 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                          >
                            <option value="">(none — define one in Filter Configurations)</option>
                            {fcs.map((fc: any) => (
                              <option key={fc.id} value={fc.id}>
                                {fc.display_name || fc.id} · {fc.id}
                              </option>
                            ))}
                          </select>
                        ) : (
                          <span className="text-[11px] text-amber-400 italic">
                            no Filter Configurations defined for this dimension
                          </span>
                        )
                      ) : (
                        <span className="text-[11px] text-gray-600 italic">not applied</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Generate Tab (placeholder)                                        */
/* ------------------------------------------------------------------ */

function GenerateTab({ dv }: { dv: any }) {
  const columns: any[] = dv.columns || [];

  return (
    <div className="space-y-4">
      <h3 className="text-sm font-medium text-gray-200">gRPC Service Generation</h3>

      <div className="rounded border border-gray-800 p-3 space-y-2">
        <div className="grid grid-cols-2 gap-3 text-xs">
          <div>
            <span className="text-gray-500">DataView: </span>
            <span className="text-gray-300">{dv.display_name}</span>
          </div>
          <div>
            <span className="text-gray-500">Columns: </span>
            <span className="text-gray-300">{columns.length}</span>
          </div>
          {dv.contract?.service && (
            <div>
              <span className="text-gray-500">Service: </span>
              <span className="text-gray-300 font-mono">{dv.contract.service}</span>
            </div>
          )}
        </div>
      </div>

      <button
        disabled
        className="flex items-center gap-1.5 px-3 py-1.5 text-xs text-gray-500 border border-gray-700 rounded cursor-not-allowed opacity-50"
      >
        <Code2 size={12} />
        Generate Service
      </button>
      <p className="text-[10px] text-gray-600 italic">Service generation placeholder</p>
    </div>
  );
}
