import { useState, useEffect, useCallback, useRef } from "react";
import {
  Save, Pencil, X, Play, Loader2, CheckCircle2, XCircle,
  ChevronDown, ChevronRight, Eye, Plus, ArrowUp, ArrowDown, Trash2, Hash, GitBranch,
} from "lucide-react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";
import { useActivePipelineRun, ACTIVE_POLL_INTERVAL_MS } from "@/hooks/useActivePipelineRun";

interface SharedPipelineWorkspaceProps {
  pipelineId: string;
}

type StepStatus = "pending" | "running" | "success" | "failed" | "skipped";
type StepResult = {
  status: StepStatus;
  row_count?: number;
  duration_ms?: number;
  message?: string;
  phase?: string;
  startedAt?: number;
};

const TYPE_BADGE_STYLES: Record<string, string> = {
  pg_extract: "bg-blue-900/50 text-blue-400 border-blue-800",
  duckdb_table: "bg-emerald-900/50 text-emerald-400 border-emerald-800",
  duckdb_query: "bg-amber-900/50 text-amber-400 border-amber-800",
  duckdb_sql: "bg-amber-900/50 text-amber-400 border-amber-800",
  grpc_call: "bg-violet-900/50 text-violet-400 border-violet-800",
  pipeline_ref: "bg-pink-900/50 text-pink-400 border-pink-800",
  bq_export: "bg-orange-900/50 text-orange-400 border-orange-800",
  loop: "bg-green-900/50 text-green-400 border-green-800",
  gcs_download: "bg-teal-900/50 text-teal-400 border-teal-800",
  custom_rust: "bg-indigo-900/50 text-indigo-400 border-indigo-800",
};

function typeBadgeStyle(type: string): string {
  return TYPE_BADGE_STYLES[type] || "bg-gray-800 text-gray-400 border-gray-700";
}

function flattenPipeline(nodes: any[], depth = 0, stepCounter = { n: 0 }): any[] {
  const result: any[] = [];
  for (const n of nodes) {
    stepCounter.n++;
    result.push({ ...n, _depth: depth, _stepNum: stepCounter.n });
    if (n.children?.length) result.push(...flattenPipeline(n.children, depth + 1, stepCounter));
  }
  return result;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60000).toFixed(1)}m`;
}

function formatRowCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

/** Hook: ticks every second while any step is running, returns current timestamp for elapsed calc */
function useRunningTimer(stepResults: Map<string, StepResult>) {
  const [now, setNow] = useState(Date.now());
  const hasRunning = Array.from(stepResults.values()).some(r => r.status === "running");
  useEffect(() => {
    if (!hasRunning) return;
    const id = setInterval(() => setNow(Date.now()), 200);
    return () => clearInterval(id);
  }, [hasRunning]);
  return now;
}

// ---- "Test Query" + "Preview" controls for pg_extract steps ----
function PgQueryTester({ query }: { query: string }) {
  const [busy, setBusy] = useState<null | "count" | "preview">(null);
  const [count, setCount] = useState<{ ok: boolean; count?: number; ms?: number; msg?: string } | null>(null);
  const [preview, setPreview] = useState<{ rows: any[]; columns: string[]; ms: number; limited: number | null } | null>(null);
  const [previewErr, setPreviewErr] = useState<string | null>(null);
  const [limitInput, setLimitInput] = useState<string>("50");

  const callApi = async (path: string, body: any) => {
    const res = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const json = await res.json();
    if (!res.ok) throw new Error(json.error || res.statusText);
    return json;
  };

  const handleCount = async () => {
    if (!query.trim()) return setCount({ ok: false, msg: "Enter a query first" });
    setBusy("count"); setCount(null);
    try {
      const r = await callApi("/api/pipeline/test-pg-query", { query });
      setCount({ ok: true, count: r.count, ms: r.duration_ms });
    } catch (e: any) {
      setCount({ ok: false, msg: e.message || "Request failed" });
    } finally { setBusy(null); }
  };

  const handlePreview = async () => {
    if (preview || previewErr) { setPreview(null); setPreviewErr(null); return; }
    if (!query.trim()) return setPreviewErr("Enter a query first");
    const limit = Math.max(0, Number.parseInt(limitInput || "0", 10) || 0);
    setBusy("preview");
    try {
      const r = await callApi("/api/pipeline/preview-pg-query", { query, limit });
      setPreview({ rows: r.rows, columns: r.columns, ms: r.duration_ms, limited: r.limit ?? null });
    } catch (e: any) {
      setPreviewErr(e.message || "Request failed");
    } finally { setBusy(null); }
  };

  return (
    <div className="space-y-2 mt-1">
      <div className="flex items-center gap-3 flex-wrap">
        <button
          onClick={handleCount}
          disabled={!!busy || !query.trim()}
          className="flex items-center gap-1.5 px-2.5 py-1 text-[11px] rounded bg-gray-800 border border-gray-700 hover:border-gray-600 hover:bg-gray-700 text-gray-200 disabled:opacity-50 transition-colors"
        >
          {busy === "count" ? <Loader2 size={11} className="animate-spin" /> : <Hash size={11} />}
          Count
        </button>
        <button
          onClick={handlePreview}
          disabled={!!busy || (!preview && !previewErr && !query.trim())}
          className={`flex items-center gap-1.5 px-2.5 py-1 text-[11px] rounded border hover:border-gray-600 hover:bg-gray-700 text-gray-200 disabled:opacity-50 transition-colors ${
            preview || previewErr
              ? "bg-blue-900/40 border-blue-800 text-blue-300"
              : "bg-gray-800 border-gray-700"
          }`}
        >
          {busy === "preview"
            ? <Loader2 size={11} className="animate-spin" />
            : (preview || previewErr) ? <ChevronDown size={11} /> : <Eye size={11} />}
          {preview || previewErr ? "Hide Preview" : "Preview"}
        </button>
        <div className="flex items-center gap-1">
          <span className="text-[10px] text-gray-500">limit</span>
          <input
            type="number"
            min={0}
            value={limitInput}
            onChange={(e) => setLimitInput(e.target.value)}
            className="w-16 px-1.5 py-0.5 text-[11px] rounded bg-gray-950 border border-gray-800 text-gray-200 font-mono focus:outline-none focus:border-blue-500"
            title="0 = no limit"
          />
          <span className="text-[10px] text-gray-600">(0 = all)</span>
        </div>
        {count?.ok && (
          <span className="text-[10px] text-green-400 font-mono">{(count.count ?? 0).toLocaleString()} rows · {count.ms}ms</span>
        )}
      </div>
      {count && !count.ok && (
        <pre className="text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words bg-red-950/30 border border-red-900/60 rounded px-2 py-1.5 max-h-60 overflow-auto">{count.msg}</pre>
      )}

      {previewErr && (
        <pre className="text-[10px] text-red-400 font-mono whitespace-pre-wrap break-words bg-red-950/30 border border-red-900/60 rounded px-2 py-1.5 max-h-60 overflow-auto">{previewErr}</pre>
      )}

      {preview && (
        <div className="border border-gray-800 rounded overflow-hidden">
          <div className="px-2 py-1 bg-gray-900 border-b border-gray-800 text-[10px] text-gray-500 flex justify-between">
            <span>{preview.rows.length} rows · {preview.columns.length} columns · {preview.ms}ms</span>
            <span className="text-gray-600">{preview.limited ? `limit ${preview.limited}` : "no limit"}</span>
          </div>
          <div className="overflow-auto max-h-[260px]">
            <table className="w-full text-[10px] font-mono">
              <thead className="bg-gray-900 sticky top-0">
                <tr>
                  {preview.columns.map((c) => (
                    <th key={c} className="px-2 py-1 text-left text-gray-400 border-b border-gray-800 font-medium whitespace-nowrap">{c}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {preview.rows.map((r, i) => (
                  <tr key={i} className="hover:bg-gray-900/50">
                    {preview.columns.map((c) => (
                      <td key={c} className="px-2 py-0.5 text-gray-300 border-b border-gray-800/50 whitespace-nowrap max-w-[280px] truncate" title={String(r[c] ?? "")}>
                        {r[c] === null || r[c] === undefined
                          ? <span className="text-gray-600 italic">null</span>
                          : typeof r[c] === "object"
                            ? JSON.stringify(r[c])
                            : String(r[c])}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

// ---- Click-to-rename pipeline title. ----
// Renders the display_name as a heading; clicking switches to an input.
// Enter or blur saves via api.updateSharedPipeline; Esc cancels. Empty name
// is rejected (server-side `display_name required` rules apply too).
function PipelineTitleEditor({
  pipelineId,
  displayName,
  onChanged,
}: {
  pipelineId: string;
  displayName: string;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(displayName);
  const [busy, setBusy] = useState(false);
  // Keep the draft in sync if the row reloads with a new name.
  useEffect(() => { if (!editing) setDraft(displayName); }, [displayName, editing]);

  const commit = async () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (!trimmed || trimmed === displayName) {
      setDraft(displayName);
      return;
    }
    setBusy(true);
    try {
      await api.updateSharedPipeline(pipelineId, { display_name: trimmed });
      onChanged();
    } catch (e: any) {
      alert("Rename failed: " + (e?.message || "Unknown error"));
      setDraft(displayName);
    } finally {
      setBusy(false);
    }
  };

  if (editing) {
    return (
      <input
        autoFocus
        type="text"
        value={draft}
        disabled={busy}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") { setDraft(displayName); setEditing(false); }
        }}
        className="text-lg font-semibold text-gray-100 bg-gray-900 border border-blue-700 rounded px-1.5 py-0.5 focus:outline-none focus:ring-1 focus:ring-blue-500 w-full"
      />
    );
  }
  return (
    <h1
      className="text-lg font-semibold text-gray-100 truncate cursor-text hover:bg-gray-900 rounded px-1.5 py-0.5 -mx-1.5 -my-0.5"
      title="Click to rename"
      onClick={() => setEditing(true)}
    >
      {displayName}
    </h1>
  );
}

// ---- Click-to-edit pipeline description. ----
// Renders a placeholder when empty; clicking switches to a textarea.
// Blur or Cmd/Ctrl+Enter saves; Esc reverts. Empty save clears.
function PipelineDescriptionEditor({
  pipelineId,
  description,
  onChanged,
}: {
  pipelineId: string;
  description: string;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(description);
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (!editing) setDraft(description); }, [description, editing]);

  const commit = async () => {
    setEditing(false);
    if (draft === description) return;
    setBusy(true);
    try {
      await api.updateSharedPipeline(pipelineId, { description: draft });
      onChanged();
    } catch (e: any) {
      alert("Description save failed: " + (e?.message || "Unknown error"));
      setDraft(description);
    } finally {
      setBusy(false);
    }
  };

  if (editing) {
    return (
      <textarea
        autoFocus
        rows={3}
        value={draft}
        disabled={busy}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) commit();
          else if (e.key === "Escape") { setDraft(description); setEditing(false); }
        }}
        placeholder="What this pipeline does, when to use it, what it depends on…"
        className="mt-1 w-full text-xs text-gray-300 bg-gray-900 border border-blue-700 rounded px-2 py-1.5 focus:outline-none focus:ring-1 focus:ring-blue-500 resize-y"
      />
    );
  }
  return (
    <div
      onClick={() => setEditing(true)}
      title="Click to edit"
      className={`mt-1 text-xs leading-relaxed cursor-text rounded px-2 py-1 -mx-2 hover:bg-gray-900 ${
        description ? "text-gray-400" : "text-gray-600 italic"
      }`}
    >
      {description || "Add description…"}
    </div>
  );
}

// ---- Inline editor for pipeline-level settings (trigger + placement). ----
// Phase 2/3 of misty-hinton. Sits above the step list; both fields auto-save.
// Trigger has a fixed-shape dropdown for the simple kinds (manual /
// rcl_change); Cdc + Scheduled + Composed shapes are detected and shown
// read-only with a "edit via API" note (their JSON shape is more involved
// than a one-line picker can capture).
function PipelineSettingsBar({
  pipelineId,
  row,
  onChanged,
}: {
  pipelineId: string;
  row: any;
  onChanged: () => void;
}) {
  const placement: string = row?.placement || "duck_db_only";
  const triggerJson: string = typeof row?.trigger === "string"
    ? row.trigger
    : JSON.stringify(row?.trigger || { kind: "manual" });
  let triggerKind = "manual";
  try {
    const parsed = JSON.parse(triggerJson);
    triggerKind = parsed?.kind || "manual";
  } catch { /* keep manual */ }

  const isSimpleKind = triggerKind === "manual" || triggerKind === "rcl_change";
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const persist = async (data: Record<string, any>) => {
    setBusy(true);
    setErr(null);
    try {
      await api.updateSharedPipeline(pipelineId, data);
      onChanged();
    } catch (e: any) {
      setErr(e?.message || "Save failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mt-2 flex items-center gap-4 text-[11px]">
      <div className="flex items-center gap-1.5">
        <span className="text-gray-500">Trigger:</span>
        {isSimpleKind ? (
          <select
            disabled={busy}
            value={triggerKind}
            onChange={(e) => persist({ trigger: { kind: e.target.value } })}
            className="px-1.5 py-0.5 rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
            title="Manual = run via UI/API only. RCL change = run when the in-process RuleStore is replaced."
          >
            <option value="manual">Manual</option>
            <option value="rcl_change">RCL change</option>
          </select>
        ) : (
          <span
            className="px-1.5 py-0.5 rounded bg-gray-900 border border-gray-800 text-indigo-300 cursor-help"
            title={`Complex trigger (${triggerKind}). Edit via PUT /api/pipelines/${pipelineId} body.trigger to keep nested config intact.`}
          >
            {triggerKind} <span className="text-gray-500">(edit via API)</span>
          </span>
        )}
      </div>

      <div className="flex items-center gap-1.5">
        <span className="text-gray-500">Placement:</span>
        <select
          disabled={busy}
          value={placement}
          onChange={(e) => persist({ placement: e.target.value })}
          className="px-1.5 py-0.5 rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
          title="duck_db_only = output lives in DuckDB. duck_db_and_in_memory = also rehydrate the in-memory store after each run (for low-latency RPC consumers like the article_selection gRPC)."
        >
          <option value="duck_db_only">DuckDB only</option>
          <option value="duck_db_and_in_memory">DuckDB + in-memory</option>
        </select>
      </div>

      {busy && <span className="text-gray-500">saving…</span>}
      {err && <span className="text-red-400">{err}</span>}
    </div>
  );
}

// ---- Inline editor for a single step's config + label ----
function StepConfigForm({ step, onChange, onBlur }: {
  step: any;
  onChange: (patch: { label?: string; config?: Record<string, any> }) => void;
  onBlur: () => void;
}) {
  const cfg = step.config || {};
  const txt = "w-full px-2 py-1 text-[11px] rounded bg-gray-950 border border-gray-800 text-gray-200 font-mono focus:outline-none focus:border-blue-500";
  const ta  = `${txt} resize-y min-h-[60px]`;
  const lbl = "block text-[10px] text-gray-500 font-medium mb-0.5";

  const fieldText = (key: string, label: string, placeholder?: string) => (
    <div>
      <span className={lbl}>{label}</span>
      <input
        type="text"
        value={cfg[key] ?? ""}
        placeholder={placeholder}
        onChange={(e) => onChange({ config: { [key]: e.target.value } })}
        onBlur={onBlur}
        className={txt}
      />
    </div>
  );
  const fieldArea = (key: string, label: string, placeholder?: string) => (
    <div className="col-span-2">
      <span className={lbl}>{label}</span>
      <textarea
        value={cfg[key] ?? ""}
        placeholder={placeholder}
        onChange={(e) => onChange({ config: { [key]: e.target.value } })}
        onBlur={onBlur}
        className={ta}
        rows={4}
      />
    </div>
  );
  const fieldSelect = (key: string, label: string, options: string[]) => (
    <div>
      <span className={lbl}>{label}</span>
      <select
        value={cfg[key] ?? options[0]}
        onChange={(e) => { onChange({ config: { [key]: e.target.value } }); }}
        onBlur={onBlur}
        className={txt}
      >
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </select>
    </div>
  );

  return (
    <div className="grid grid-cols-2 gap-x-3 gap-y-2">
      {/* Label is common to all */}
      <div className="col-span-2">
        <span className={lbl}>Label</span>
        <input
          type="text"
          value={step.label ?? ""}
          onChange={(e) => onChange({ label: e.target.value })}
          onBlur={onBlur}
          className={txt + " text-gray-100 font-sans"}
        />
      </div>

      {step.type === "pg_extract" && (
        <>
          {fieldSelect("target", "Target", ["parquet", "duckdb"])}
          {(cfg.target === "duckdb" || cfg.target === "memory")
            ? fieldText("table_name", "Table Name", "my_table")
            : fieldText("output_path", "Output Path", "{PARQUET_HOME}/dataset/")}
          {fieldText("tracking_column", "Tracking Column (optional)", "updated_at")}
          {/* Partitioned extract: when both `partition_column` and
              `partition_values_sql` are set, the runner enumerates
              distinct values once and runs a parallel COPY per value
              with `{partition_value}` substituted into the query.
              Output is Hive-laid-out parquet; the load step reads
              with `hive_partitioning=true`. */}
          {fieldText("partition_column", "Partition Column (optional)", "l1_name")}
          {fieldArea(
            "partition_values_sql",
            "Partition Values SQL (required when Partition Column is set)",
            "SELECT DISTINCT l1_name FROM inventory_smart.ph_master WHERE l1_name IS NOT NULL"
          )}
          {fieldArea(
            "query",
            "SQL Query (when partitioning, use the partition column name as placeholder, e.g. {l1_name})",
            "SELECT * FROM ... WHERE l1_name = {l1_name}"
          )}
          <div className="col-span-2">
            <PgQueryTester query={cfg.query ?? ""} />
          </div>
        </>
      )}
      {step.type === "duckdb_query" && (
        <>
          {fieldText("table_name", "Output Table (optional)", "result_table")}
          <div /> {/* spacer */}
          {fieldArea("sql", "SQL", "CREATE TABLE foo AS SELECT ...")}
        </>
      )}
      {step.type === "duckdb_table" && (
        <>
          {fieldText("table_name", "Table Name", "my_table")}
          {fieldText("parquet_path", "Parquet Path", "{PARQUET_HOME}/dataset/")}
        </>
      )}
      {step.type === "bq_export" && (
        <>
          {fieldText("dataset", "BQ Dataset / Source", "project.dataset.table")}
          {fieldText("path", "GCS Path", "gs://bucket/dataset/")}
          {fieldArea("query", "Query (optional)", "SELECT * FROM `project.dataset.table`")}
        </>
      )}
      {step.type === "gcs_download" && (
        <>
          {fieldText("gcs_path", "GCS Source", "gs://bucket/dataset/")}
          {fieldText("local_path", "Local Destination", "{PARQUET_HOME}/dataset/")}
        </>
      )}
      {step.type === "grpc_call" && (
        <>
          {fieldText("service", "Service", "rcl-resolution")}
          {fieldText("method", "Method", "Resolve")}
          {fieldArea("payload", "Payload (JSON)", '{ "key": "value" }')}
        </>
      )}
      {step.type === "loop" && (
        <>
          {fieldText("partition_col", "Partition Column", "store_code")}
          {fieldText("table_name", "Source Table (for partition values)", "stores")}
        </>
      )}
      {step.type === "custom_rust" && (
        <>
          <div>
            <span className={lbl}>Assembly ID</span>
            <input
              type="text"
              value={cfg.assembly_id ?? ""}
              readOnly
              className={txt + " text-indigo-300 cursor-not-allowed opacity-80"}
              title="Assembly ids are registered server-side; not editable from the UI."
            />
          </div>
          {fieldText("output_table", "Output Table", "article_selection")}
        </>
      )}
      {step.type === "run_pipeline" && (
        <RunPipelineStepConfig
          value={cfg.pipeline_id ?? ""}
          onChange={(pipelineId) => {
            onChange({ config: { ...(cfg as any), pipeline_id: pipelineId } });
          }}
          onBlur={onBlur}
        />
      )}
    </div>
  );
}

/// Picker for the `run_pipeline` step's child pipeline. Loads the saved
/// pipelines list once and renders a dropdown — operators see "human
/// label · id" so the same id-based wire format works regardless of
/// rename. Dropping straight into a free-text id box is fine if the
/// list fails (eg. bad network).
function RunPipelineStepConfig({
  value,
  onChange,
  onBlur,
}: {
  value: string;
  onChange: (pipelineId: string) => void;
  onBlur: () => void;
}) {
  const [pipelines, setPipelines] = useState<{ id: string; display_name?: string }[]>([]);
  const [loadFailed, setLoadFailed] = useState(false);
  useEffect(() => {
    api.getSharedPipelines()
      .then((rows: any[]) => setPipelines(Array.isArray(rows) ? rows : []))
      .catch(() => setLoadFailed(true));
  }, []);
  const lbl = "block text-[9px] uppercase tracking-wider text-gray-500 mb-0.5";
  const txt = "w-full px-2 py-1 text-[11px] rounded bg-gray-950 border border-gray-800 text-gray-200 font-mono focus:outline-none focus:border-blue-500";
  return (
    <div>
      <span className={lbl}>Pipeline to run</span>
      {loadFailed ? (
        <input
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onBlur={onBlur}
          placeholder="pl_v7_extracts"
          className={txt}
        />
      ) : (
        <select
          value={value}
          onChange={(e) => { onChange(e.target.value); }}
          onBlur={onBlur}
          className={txt}
        >
          <option value="">— pick a pipeline —</option>
          {pipelines.map((p) => (
            <option key={p.id} value={p.id}>
              {p.display_name || p.id} · {p.id}
            </option>
          ))}
        </select>
      )}
      <p className="text-[10px] text-gray-500 mt-1">
        The chosen pipeline runs as a single step. It honors its own saved Parallel /
        Trigger / Placement settings; the parent's run-mode flags don't propagate.
      </p>
    </div>
  );
}

// ---- Pipeline tree mutation helpers (immutable) ----
function pipelineUpdate(nodes: any[], id: string, fn: (n: any) => any): any[] {
  return nodes.map((n) => {
    if (n.id === id) return fn(n);
    if (Array.isArray(n.children)) return { ...n, children: pipelineUpdate(n.children, id, fn) };
    return n;
  });
}
function pipelineDelete(nodes: any[], id: string): any[] {
  return nodes
    .filter((n) => n.id !== id)
    .map((n) => (Array.isArray(n.children) ? { ...n, children: pipelineDelete(n.children, id) } : n));
}
function pipelineAddChild(nodes: any[], parentId: string, child: any): any[] {
  return nodes.map((n) => {
    if (n.id === parentId) {
      const children = Array.isArray(n.children) ? [...n.children, child] : [child];
      return { ...n, children };
    }
    if (Array.isArray(n.children)) return { ...n, children: pipelineAddChild(n.children, parentId, child) };
    return n;
  });
}
function pipelineMove(nodes: any[], id: string, dir: -1 | 1): any[] {
  const idx = nodes.findIndex((n) => n.id === id);
  if (idx >= 0) {
    const next = idx + dir;
    if (next < 0 || next >= nodes.length) return nodes;
    const out = [...nodes];
    [out[idx], out[next]] = [out[next], out[idx]];
    return out;
  }
  return nodes.map((n) =>
    Array.isArray(n.children) ? { ...n, children: pipelineMove(n.children, id, dir) } : n,
  );
}

const STEP_TEMPLATES: { type: string; label: string; desc: string; config: Record<string, any>; children?: boolean }[] = [
  { type: "pg_extract",   label: "PG → Parquet",  desc: "Query PostgreSQL, write parquet",     config: { query: "", target: "parquet", output_path: "" } },
  // CH → DuckDB Table uses the generic `custom_rust` step with the
  // `ch_extract` assembly registered in pipeline_assemblies.rs. The
  // assembly resolves the connection by id, runs the SQL via the CH
  // HTTP interface, and writes rows into a DuckDB table via
  // read_json_auto from a temp NDJSON file. No parquet round-trip;
  // CH→parquet is a follow-up.
  { type: "custom_rust",  label: "CH → DuckDB Table", desc: "Query ClickHouse, materialize into a DuckDB table", config: { assembly_id: "ch_extract", connection_ref: "", sql: "", target_table: "" } },
  { type: "duckdb_query", label: "DuckDB SQL",    desc: "Run SQL against DuckDB",              config: { sql: "" } },
  { type: "duckdb_table", label: "DuckDB Table",  desc: "Register parquet/CSV as a table",     config: { table_name: "", parquet_path: "" } },
  { type: "bq_export",    label: "BQ Export",     desc: "EXPORT DATA from BigQuery to GCS",    config: { dataset: "", path: "" } },
  { type: "gcs_download", label: "GCS Download",  desc: "Pull parquet from GCS to local",      config: { gcs_path: "" } },
  { type: "grpc_call",    label: "gRPC Call",     desc: "Call a generated gRPC service",       config: { service: "", method: "" } },
  { type: "loop",         label: "Loop",          desc: "Iterate over a partition column",     config: { partition_col: "" }, children: true },
  { type: "run_pipeline", label: "Run pipeline",  desc: "Invoke another saved pipeline as a step", config: { pipeline_id: "" } },
];

// Pipeline step type → data_source.type matchers (for connection overrides display).
const STEP_TO_TYPES: Record<string, string[]> = {
  pg_extract: ["pg", "postgres"],
  bq_export:  ["bq", "bigquery"],
  // custom_rust steps with assembly_id=ch_extract resolve a CH
  // connection via config.connection_ref — the connection-override
  // UI doesn't apply, but we still keep the mapping for any future
  // step that wants to filter connection pickers by type.
};

export function SharedPipelineWorkspace({ pipelineId }: SharedPipelineWorkspaceProps) {
  const [pipelineRow, setPipelineRow] = useState<any>(null);
  const [error, setError] = useState<string | null>(null);

  // Local pipeline tree (mirrors pipelineRow.pipeline). Stored as a flat array of step nodes.
  const [pipeline, setPipeline] = useState<any[]>([]);
  // Synthetic nodes the server appends to the tree echo at run time —
  // currently the post-extract `_merge_to_tenant` step. Kept separate from
  // `pipeline` so the saved row isn't polluted with run-only rows. Cleared
  // when a fresh run starts.
  const [transientSteps, setTransientSteps] = useState<any[]>([]);

  const flatSteps = flattenPipeline([...pipeline, ...transientSteps]);

  // Run-mode flags (replace the old quiet / normal / detailed dropdown).
  //   captureTransfer = byte-level ticking from each step's reader/writer.
  //                     Tells the UI "how much data has flowed so far".
  //   measureProgress = pre-COUNT(*) + row totals + ETA. Knowing the
  //                     denominator costs an extra COUNT round-trip but
  //                     lets the progress bar show % done and ETA.
  // Mapping back to the wire enum the backend already understands:
  //   captureTransfer=false, measureProgress=false → quiet
  //   captureTransfer=true,  measureProgress=false → normal
  //   *,                      measureProgress=true  → detailed
  const [captureTransfer, setCaptureTransfer] = useState(true);
  const [measureProgress, setMeasureProgress] = useState(false);
  const runMode: "quiet" | "normal" | "detailed" = measureProgress
    ? "detailed"
    : captureTransfer
      ? "normal"
      : "quiet";

  // Execution mode: sequence (default) runs tasks one after another; parallel
  // wraps them in a single Step::Group{Parallel} on the backend so they fan
  // out concurrently. Persisted on the pipeline row so a parent pipeline
  // calling this one inherits the choice.
  const [parallel, setParallel] = useState(false);
  // Re-seed from the pipeline row whenever it loads / reloads.
  useEffect(() => {
    setParallel(pipelineRow?.execution === "parallel");
  }, [pipelineRow?.execution]);

  // Skip mechanism
  const [skippedSteps, setSkippedSteps] = useState<Set<string>>(new Set());
  const toggleSkip = (id: string) => {
    setSkippedSteps((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const selectAll = () => setSkippedSteps(new Set());
  const deselectAll = () => setSkippedSteps(new Set(flatSteps.map((s) => s.id)));

  const willRun = flatSteps.length - skippedSteps.size;

  // Execution state
  const [stepResults, setStepResults] = useState<Map<string, StepResult>>(new Map());
  const [running, setRunning] = useState(false);
  const [pipelineStatus, setPipelineStatus] = useState<"idle" | "running" | "success" | "failed">("idle");
  const [totalTime, setTotalTime] = useState<number | null>(null);
  // Wall-clock timestamp when this tab kicked off the run. Used as a
  // fallback to estimate `totalTime` if the SSE never delivers
  // `pipeline_done` (proxy timeout, tab sleep, etc.) — see the
  // reconciliation effect below.
  const runStartedAtRef = useRef<number | null>(null);
  // Failure detail surfaced from the SSE pipeline_done event when status=failed.
  // Without this, "Failed in 0ms" rendered with no context — the actual reason
  // (e.g. "another pipeline run is still active server-side") was dropped.
  const [failureMessage, setFailureMessage] = useState<string | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);

  // Server-side active run snapshot (polled). When it points at THIS
  // pipelineId, expose Cancel even if local `running` is false — covers
  // the case where the user navigates back to a workspace whose run was
  // started in a previous browser session, so we never received the
  // local `running=true` from the run trigger.
  const activeRun = useActivePipelineRun();
  const serverRunningHere = activeRun?.pipeline_id === pipelineId;

  // Reconcile local `running` against the server's view. If we set
  // `running = true` at click time and the SSE later closed without
  // delivering `pipeline_done` (proxy idle timeout, tab sleep, network
  // blip), the UI used to wedge with Cancel showing forever. The
  // active-run poll is the backstop: when the server says no run is in
  // flight AND we still think one is, finalize the local state. We
  // can't tell whether the run succeeded or failed from this signal —
  // it just means "server is idle." Treat it as success since the
  // alternative ("failed") would falsely flag a completed run; if it
  // really failed, the activity log retains the truth.
  //
  // The grace window is sized to one full poll interval. The current
  // `activeRun === null` reading was taken at the most recent tick
  // (somewhere in the last [0, ACTIVE_POLL_INTERVAL_MS] window), so
  // waiting another full interval guarantees a *fresh* idle reading
  // before we act on it — defends against a race where the run finishes
  // between two ticks and `pipeline_done` is still in flight on the
  // SSE. If pipeline_done arrives first, the SSE handler clears
  // `running` and this effect's cleanup cancels the timer.
  useEffect(() => {
    if (!running) return;
    if (activeRun !== null) return; // poll says some run is active — wait
    const timer = window.setTimeout(() => {
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
        eventSourceRef.current = null;
      }
      // Best-effort total time: subtract the recorded run-start
      // timestamp from "now" minus the grace window we just waited.
      // Doesn't match the server's exact total_ms (it's wall-clock
      // including the grace), but at least the user sees *something*
      // instead of a blank "Completed" badge.
      if (runStartedAtRef.current != null) {
        const elapsed = Date.now() - runStartedAtRef.current - ACTIVE_POLL_INTERVAL_MS;
        setTotalTime(Math.max(0, elapsed));
      }
      setRunning(false);
      setPipelineStatus("success");
      finalizeRunning("success");
    }, ACTIVE_POLL_INTERVAL_MS);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running, activeRun]);

  const now = useRunningTimer(stepResults);

  // Saving
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);

  // Workspace store — used after delete to clear the selection so the
  // empty-state placeholder takes over.
  const select = useWorkspaceStore((s) => s.select);

  // Mirrors the ConnectionWorkspace delete pattern: prompt + DELETE +
  // clear selection. The Sidebar refetches on its own when a list-relevant
  // event arrives, but we also clear selection so the workspace doesn't
  // render against a stale id between the API call and the next list refresh.
  const handleDeletePipeline = async () => {
    const label = pipelineRow?.display_name || pipelineId;
    if (!window.confirm(`Delete pipeline "${label}" (${pipelineId})? This cannot be undone.`)) return;
    setDeleting(true);
    try {
      await api.deleteSharedPipeline(pipelineId);
      select(null);
    } catch (e: any) {
      alert("Delete failed: " + (e?.message || "unknown"));
      setDeleting(false);
    }
  };

  // Show step details toggle
  const [showDetails, setShowDetails] = useState(false);
  const [expandedSteps, setExpandedSteps] = useState<Set<string>>(new Set());
  const toggleExpand = (id: string) => {
    setExpandedSteps((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  // Connection overrides collapsible
  const [connOpen, setConnOpen] = useState(false);
  const [dataSources, setDataSources] = useState<any[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [addingChildOf, setAddingChildOf] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setError(null);
      const row = await api.getSharedPipeline(pipelineId);
      setPipelineRow(row);
      const raw = row?.pipeline;
      const arr = typeof raw === "string" ? JSON.parse(raw || "[]") : (Array.isArray(raw) ? raw : []);
      setPipeline(Array.isArray(arr) ? arr : []);
    } catch (e: any) {
      setError(e.message || "Failed to load shared pipeline");
    }
  }, [pipelineId]);

  useEffect(() => {
    setPipelineRow(null);
    setPipeline([]);
    load();
  }, [load]);

  // Reset skip and results when pipeline changes
  useEffect(() => {
    setSkippedSteps(new Set());
    setStepResults(new Map());
    setPipelineStatus("idle");
    setTotalTime(null);
    setTransientSteps([]);
    setFailureMessage(null);
  }, [pipelineId]);

  useEffect(() => {
    if (!connOpen) return;
    api.getDataSources().then(setDataSources).catch(() => setDataSources([]));
  }, [connOpen]);

  const resolveDefault = (stepType: string) => {
    const types = STEP_TO_TYPES[stepType] || [];
    const matching = dataSources.filter((d) => types.includes(d.type));
    const def = matching.find((d) => d.is_default) || matching[0];
    return def ? { name: def.display_name || def.id, id: def.id, isDefault: !!def.is_default } : null;
  };

  // ---- Mutations: update local state, then persist ----
  const persist = async (newPipeline: any[]) => {
    setPipeline(newPipeline);
    setSaving(true);
    try {
      await api.updateSharedPipeline(pipelineId, { pipeline: newPipeline });
    } catch (e: any) {
      alert("Save failed: " + (e?.message || "Unknown error"));
    } finally {
      setSaving(false);
    }
  };

  const handleAddStep = async (tpl: typeof STEP_TEMPLATES[number], parentId?: string) => {
    const id = `step_${crypto.randomUUID().replace(/-/g, "").slice(0, 8)}`;
    const newNode: any = { id, type: tpl.type, label: tpl.label, config: tpl.config };
    if (tpl.children) newNode.children = [];
    const newPipeline = parentId ? pipelineAddChild(pipeline, parentId, newNode) : [...pipeline, newNode];
    await persist(newPipeline);
    setAddOpen(false);
    setAddingChildOf(null);
  };

  const handleDeleteStep = (id: string) => {
    if (!confirm("Delete this step (and its children)?")) return;
    persist(pipelineDelete(pipeline, id));
  };
  const handleMoveStep = (id: string, dir: -1 | 1) => {
    persist(pipelineMove(pipeline, id, dir));
  };
  const handleUpdateStep = (id: string, patch: { label?: string; config?: Record<string, any> }) => {
    const newPipeline = pipelineUpdate(pipeline, id, (n) => {
      const next = { ...n };
      if (patch.label !== undefined) next.label = patch.label;
      if (patch.config) next.config = { ...(n.config || {}), ...patch.config };
      return next;
    });
    setPipeline(newPipeline);
  };
  const commit = () => persist(pipeline);

  const savePipeline = async () => {
    setSaving(true);
    try {
      await api.updateSharedPipeline(pipelineId, { pipeline });
      await load();
    } catch (e: any) {
      alert("Save failed: " + (e.message || "Unknown error"));
    } finally {
      setSaving(false);
    }
  };

  const cancelPipeline = async () => {
    // Server-side cancel: drops the active run's PG COPY stream future and
    // calls DuckDB::interrupt() on any in-flight statement. Returns 409 if
    // no active run (e.g. already finished) — ignore that case.
    try {
      await fetch("/api/pipelines/cancel", { method: "POST" });
    } catch {
      // Network issue — fall through to local-only cleanup.
    }
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
      eventSourceRef.current = null;
    }
    setRunning(false);
    setPipelineStatus("failed");
    setStepResults((prev) => {
      const next = new Map(prev);
      for (const [k, v] of next) {
        if (v.status === "running") next.set(k, { ...v, status: "failed", message: "Cancelled" });
      }
      return next;
    });
  };

  // Mark any still-"running" steps as the given terminal status. Used when the SSE
  // closes (cleanly or otherwise) so step spinners/elapsed timers don't tick forever.
  const finalizeRunning = (terminal: "success" | "failed", message?: string) => {
    setStepResults((prev) => {
      const next = new Map(prev);
      for (const [k, v] of next) {
        if (v.status === "running") {
          next.set(k, { ...v, status: terminal, message: message ?? v.message });
        }
      }
      return next;
    });
  };

  const runPipeline = (opts?: { parallelOverride?: boolean }) => {
    // The two run buttons set `parallel` and call this in the same
    // tick — the closure may still see the old value, so we accept an
    // explicit override that wins over the state read.
    const useParallel = opts?.parallelOverride ?? parallel;

    setExpandedSteps(new Set());
    setShowDetails(false);

    setRunning(true);
    setPipelineStatus("running");
    setTotalTime(null);
    runStartedAtRef.current = Date.now();
    setStepResults(new Map());
    setTransientSteps([]);
    setFailureMessage(null);

    const qp = new URLSearchParams();
    if (skippedSteps.size > 0) qp.set("skip", Array.from(skippedSteps).join(","));
    if (runMode !== "normal") qp.set("mode", runMode);
    if (useParallel) qp.set("execution", "parallel");
    const connParam = qp.toString() ? `?${qp.toString()}` : "";

    const es = new EventSource(`/api/pipelines/${pipelineId}/tree-stream${connParam}`);
    eventSourceRef.current = es;

    es.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);

        if (data.type === "tree") {
          // Server may append synthetic runtime-only nodes (e.g. the
          // `_merge_to_tenant` step that runs after all pg_extracts). Pluck
          // any node whose id is not in the saved pipeline and surface it
          // through `transientSteps` so flatSteps renders a row for it.
          const nodes: any[] = Array.isArray(data.nodes) ? data.nodes : [];
          const savedIds = new Set(pipeline.map((n: any) => n.id));
          const synthetic = nodes.filter((n) => !savedIds.has(n.id));
          if (synthetic.length > 0) setTransientSteps(synthetic);
        } else if (data.type === "node_event") {
          if (data.status === "start") {
            setStepResults((prev) => {
              const next = new Map(prev);
              next.set(data.node_id, { status: "running", startedAt: Date.now() });
              return next;
            });
          } else if (data.status === "progress") {
            setStepResults((prev) => {
              const existing = prev.get(data.node_id);
              // A progress event after success/failed is stale (a late
              // ticker tick from the rust-shared-utils parallel COPY
              // ticker can arrive after the step's `success` event).
              // Don't downgrade a finished step back to running — that's
              // what was leaving "fetched 0 B so far" stuck on completed
              // pg_extract rows.
              if (existing?.status === "success" || existing?.status === "failed") {
                return prev;
              }
              const next = new Map(prev);
              next.set(data.node_id, { ...existing, status: "running", phase: data.phase || data.message });
              return next;
            });
          } else if (data.status === "success" || data.status === "failed") {
            setStepResults((prev) => {
              const next = new Map(prev);
              next.set(data.node_id, {
                status: data.status,
                message: data.message,
                row_count: data.row_count,
                duration_ms: data.duration_ms,
                // phase explicitly omitted — completed rows shouldn't
                // carry the last running-state phase text. The success
                // checkmark + duration + row count is the completion
                // signal; phase column stays empty.
              });
              return next;
            });
            if (data.status === "failed" && data.node_type !== "loop_iteration") {
              setPipelineStatus("failed");
              setRunning(false);
              // Mark any sibling steps that were spinning before the
              // executor aborted as "failed" too. Without this, parallel
              // pipelines leave neighbouring steps in `running` state
              // forever after the SSE closes (no pipeline_done arrives).
              finalizeRunning("failed", "Aborted by failed sibling step");
              es.close();
              eventSourceRef.current = null;
            }
          }
        } else if (data.type === "step_start") {
          const step = flatSteps[data.index];
          if (step) {
            setStepResults((prev) => {
              const next = new Map(prev);
              next.set(step.id, { status: "running", startedAt: Date.now() });
              return next;
            });
          }
        } else if (data.type === "step_done") {
          const step = flatSteps[data.index];
          if (step) {
            setStepResults((prev) => {
              const next = new Map(prev);
              next.set(step.id, {
                status: data.status,
                message: data.message,
                row_count: data.row_count,
                duration_ms: data.duration_ms,
              });
              return next;
            });
          }
          if (data.status === "failed") {
            setPipelineStatus("failed");
            setRunning(false);
            finalizeRunning("failed", "Aborted by failed sibling step");
            es.close();
            eventSourceRef.current = null;
          }
        } else if (data.type === "pipeline_done") {
          setPipelineStatus(data.status);
          setTotalTime(data.total_time_ms);
          setFailureMessage(data.status === "failed" ? (data.message || null) : null);
          setRunning(false);
          finalizeRunning(data.status === "success" ? "success" : "failed");
          eventSourceRef.current = null;
          es.close();
        } else if (data.type === "error") {
          setPipelineStatus("failed");
          setRunning(false);
          finalizeRunning("failed", data.message || "Pipeline error");
          es.close();
          eventSourceRef.current = null;
        }
      } catch { /* ignore parse errors */ }
    };

    es.onerror = () => {
      if (es.readyState === EventSource.CLOSED) {
        setPipelineStatus((prev) => (prev === "success" ? "success" : "failed"));
        setRunning(false);
        finalizeRunning("failed", "Connection lost");
        eventSourceRef.current = null;
        es.close();
      }
    };
  };

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
        eventSourceRef.current = null;
      }
    };
  }, []);

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

  if (!pipelineRow) {
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
        <div className="flex items-center gap-2 min-w-0">
          <GitBranch size={18} className="text-teal-400 shrink-0" />
          <div className="min-w-0">
            <PipelineTitleEditor
              pipelineId={pipelineId}
              displayName={pipelineRow.display_name || "Shared Pipeline"}
              onChanged={load}
            />
          </div>
        </div>
        {/* Free-form description right under the title. Click to edit;
            blur or Enter to save. Leave empty to clear. Used to capture
            intent + caveats — e.g. "production-only PG path; will fail
            on dev replicas without asv2_* MVs". */}
        <PipelineDescriptionEditor
          pipelineId={pipelineId}
          description={pipelineRow.description || ""}
          onChanged={load}
        />
        {/* Phase 2/3 controls: trigger + placement live on the pipeline row,
            not on individual steps — surface them inline so they're editable
            without dropping to the API. */}
        <PipelineSettingsBar
          pipelineId={pipelineId}
          row={pipelineRow}
          onChanged={load}
        />
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-5 py-4">
        <div className="space-y-3">
          {/* Toolbar */}
          <div className="flex items-center justify-between flex-wrap gap-2">
            <div className="flex items-center gap-3">
              <h3 className="text-sm font-medium text-gray-200">
                Pipeline <span className="text-gray-500">&mdash; {flatSteps.length} step{flatSteps.length !== 1 ? "s" : ""}</span>
              </h3>
              {flatSteps.length > 0 && (
                <span className="text-[10px] text-gray-500">
                  {skippedSteps.size > 0 ? (
                    <>
                      <span className="text-amber-400 font-medium">{skippedSteps.size} skipped</span>
                      {" · "}
                      {willRun} of {flatSteps.length} will run
                    </>
                  ) : (
                    <>All {flatSteps.length} steps selected</>
                  )}
                </span>
              )}
              {totalTime != null && pipelineStatus !== "idle" && (
                <span className={`text-[10px] font-medium ${pipelineStatus === "success" ? "text-green-400" : "text-red-400"}`}>
                  {pipelineStatus === "success" ? "Completed" : "Failed"} in {formatDuration(totalTime)}
                </span>
              )}
            </div>
            {failureMessage && (
              <div className="mt-2 px-3 py-2 rounded bg-red-950/50 border border-red-900 text-xs text-red-300">
                {failureMessage}
              </div>
            )}
            <div className="flex items-center gap-2">
              {flatSteps.length > 0 && (
                <>
                  <button
                    onClick={() => setShowDetails(!showDetails)}
                    className={`flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded border transition-colors ${
                      showDetails
                        ? "bg-blue-900/50 text-blue-400 border-blue-800"
                        : "text-gray-500 border-gray-700 hover:text-gray-300 hover:border-gray-600"
                    }`}
                  >
                    <Eye size={10} />
                    Details
                  </button>
                  <span className="text-gray-700">|</span>
                  <button onClick={selectAll} className="text-[10px] text-blue-400 hover:text-blue-300 transition-colors">
                    Select All
                  </button>
                  <button onClick={deselectAll} className="text-[10px] text-gray-500 hover:text-gray-300 transition-colors">
                    Deselect All
                  </button>
                </>
              )}
              <button
                onClick={() => setAddOpen((v) => !v)}
                className={`flex items-center gap-1 px-2.5 py-1 text-xs rounded border transition-colors ${
                  addOpen
                    ? "bg-gray-800 text-gray-200 border-gray-600"
                    : "text-gray-300 hover:text-gray-100 border-gray-700 hover:border-gray-600"
                }`}
                title="Add a step to the pipeline"
              >
                <Plus size={12} />
                Add Step
              </button>
              <button
                onClick={savePipeline}
                disabled={saving}
                className="flex items-center gap-1 px-2.5 py-1 text-xs text-blue-400 hover:text-blue-300 border border-blue-800 rounded hover:border-blue-700 bg-blue-950/50 transition-colors disabled:opacity-50"
              >
                <Save size={12} />
                {saving ? "Saving..." : "Save"}
              </button>
              {/* Export — server returns the pipeline JSON with a
                  Content-Disposition: attachment header so the browser's
                  download flow fires from a regular anchor click. */}
              <a
                href={api.exportSharedPipelineUrl(pipelineId)}
                download={`${pipelineId}.pipeline.json`}
                className="flex items-center gap-1 px-2.5 py-1 text-xs text-gray-300 hover:text-gray-100 border border-gray-700 rounded hover:border-gray-600 bg-gray-900 transition-colors"
                title="Download this pipeline as a JSON file"
              >
                Export
              </a>
              {/* Import (replace) — replace this pipeline's definition with
                  an exported JSON file. Mode = "replace" so the server
                  overwrites the current row at this id. */}
              <label
                className="flex items-center gap-1 px-2.5 py-1 text-xs text-gray-300 hover:text-gray-100 border border-gray-700 rounded hover:border-gray-600 bg-gray-900 transition-colors cursor-pointer"
                title="Replace this pipeline's definition from a previously-exported JSON file"
              >
                Import (replace)
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
                      await api.importSharedPipeline(data, "replace", pipelineId);
                      await load();
                    } catch (err: any) {
                      alert("Import failed: " + (err?.message || "invalid JSON"));
                    }
                  }}
                />
              </label>
              {/* Delete the entire pipeline. Mirrors ConnectionWorkspace's
                  destructive-action button (red + trash icon). Disabled
                  while a run is in flight so the user can't pull the rug
                  on the executor mid-pipeline. */}
              <button
                onClick={handleDeletePipeline}
                disabled={deleting || running}
                title="Delete this pipeline"
                className="flex items-center gap-1 px-2.5 py-1 text-xs text-red-300 hover:text-red-200 border border-red-800 rounded hover:border-red-700 bg-red-950/50 transition-colors disabled:opacity-50"
              >
                {deleting ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
                Delete
              </button>
              {running || serverRunningHere ? (
                <button
                  onClick={cancelPipeline}
                  className="flex items-center gap-1.5 px-3 py-1 text-xs text-red-400 border border-red-800 rounded bg-red-950/50 hover:bg-red-900/50 transition-colors"
                >
                  <XCircle size={12} />
                  Cancel
                </button>
              ) : (
                <div className="flex items-center gap-3">
                  <button
                    onClick={() => runPipeline()}
                    disabled={flatSteps.length === 0 || willRun === 0}
                    className="flex items-center gap-1.5 px-3 py-1 text-xs text-green-400 border border-green-800 rounded bg-green-950/50 hover:bg-green-900/50 transition-colors disabled:opacity-40"
                    title={parallel ? "Run all tasks concurrently." : "Run all tasks in sequence."}
                  >
                    <Play size={12} />
                    Run
                  </button>
                  {/* Parallel is persisted onto the pipeline row so a
                      parent pipeline calling this one (via the
                      run_pipeline step type) inherits the chosen mode
                      without a query-param override. */}
                  <label
                    className="flex items-center gap-1.5 text-[10px] text-gray-300 cursor-pointer"
                    title="Run all tasks concurrently. Saved on the pipeline so parent pipelines invoking this one inherit the choice."
                  >
                    <input
                      type="checkbox"
                      checked={parallel}
                      onChange={async (e) => {
                        const next = e.target.checked;
                        setParallel(next);
                        try {
                          await api.updateSharedPipeline(pipelineId, {
                            execution: next ? "parallel" : "sequence",
                          });
                          load();
                        } catch {
                          /* leave the toggle showing local state on save failure */
                        }
                      }}
                      className="rounded border-gray-700 bg-gray-900 text-green-500 focus:ring-green-700"
                    />
                    Parallel
                  </label>
                  <label
                    className="flex items-center gap-1.5 text-[10px] text-gray-300 cursor-pointer"
                    title="Emit byte-level events as each step reads/writes data — drives the live throughput readout in the run log."
                  >
                    <input
                      type="checkbox"
                      checked={captureTransfer}
                      onChange={(e) => setCaptureTransfer(e.target.checked)}
                      className="rounded border-gray-700 bg-gray-900 text-green-500 focus:ring-green-700"
                    />
                    Live throughput
                  </label>
                  <label
                    className="flex items-center gap-1.5 text-[10px] text-gray-300 cursor-pointer"
                    title="Pre-count rows so % done and ETA can be computed. Adds a COUNT(*) per step."
                  >
                    <input
                      type="checkbox"
                      checked={measureProgress}
                      onChange={(e) => setMeasureProgress(e.target.checked)}
                      className="rounded border-gray-700 bg-gray-900 text-green-500 focus:ring-green-700"
                    />
                    Show ETA
                  </label>
                </div>
              )}
            </div>
          </div>

          {/* Add Step panel */}
          {addOpen && (
            <div className="rounded border border-gray-700 bg-gray-900 p-3 space-y-2">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-gray-200">
                  {addingChildOf ? `Add child of ${addingChildOf}` : "Add a step"}
                </span>
                <button
                  onClick={() => { setAddOpen(false); setAddingChildOf(null); }}
                  className="text-gray-500 hover:text-gray-300"
                >
                  <X size={14} />
                </button>
              </div>
              <div className="grid grid-cols-2 gap-2">
                {STEP_TEMPLATES.map((tpl) => (
                  <button
                    key={tpl.type}
                    onClick={() => handleAddStep(tpl, addingChildOf || undefined)}
                    disabled={saving}
                    className="text-left p-2 rounded border border-gray-800 hover:border-gray-600 hover:bg-gray-800/50 transition-colors disabled:opacity-50"
                  >
                    <div className="flex items-center gap-2">
                      <span className={`text-[10px] px-1.5 py-0.5 rounded border font-medium ${typeBadgeStyle(tpl.type)}`}>{tpl.type}</span>
                      <span className="text-xs text-gray-200">{tpl.label}</span>
                    </div>
                    <div className="text-[10px] text-gray-500 mt-0.5">{tpl.desc}</div>
                  </button>
                ))}
              </div>
              {!addingChildOf && (
                <p className="text-[10px] text-gray-600 mt-1">
                  New steps are appended to the end of the pipeline. Use the <Plus size={10} className="inline" /> on a Loop step to add a child.
                </p>
              )}
            </div>
          )}

          {/* Connection Overrides (collapsible) */}
          <div className="rounded border border-gray-800">
            <button
              onClick={() => setConnOpen(!connOpen)}
              className="w-full flex items-center gap-2 px-3 py-2 text-xs text-gray-400 hover:text-gray-300 transition-colors"
            >
              {connOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
              <span className="font-medium">Connection Overrides</span>
              <span className="text-[10px] text-gray-600">runtime connection remapping</span>
            </button>
            {connOpen && (
              <div className="px-3 pb-3 border-t border-gray-800">
                <p className="text-[10px] text-gray-600 mt-2 mb-2">
                  Override which database connection each step type uses at execution time.
                  Steps with explicit connections will use those; others use the default for their type.
                </p>
                <div className="grid grid-cols-2 gap-2">
                  {["pg_extract", "bq_export"].map((type) => {
                    const def = resolveDefault(type);
                    return (
                      <div key={type} className="flex items-center gap-2">
                        <span className={`text-[10px] px-1.5 py-0.5 rounded border font-medium ${typeBadgeStyle(type)}`}>{type}</span>
                        <span className="text-[10px] text-gray-500">&rarr;</span>
                        {def ? (
                          <span className="text-[10px] text-gray-300 font-mono truncate">
                            {def.name}{def.isDefault ? " ★" : ""}
                          </span>
                        ) : (
                          <span className="text-[10px] text-gray-600 italic">no connection configured</span>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
          </div>

          {/* Step list */}
          {flatSteps.length > 0 ? (
            <div className="space-y-0.5">
              {flatSteps.map((step) => {
                const isSkipped = skippedSteps.has(step.id);
                const result = stepResults.get(step.id);
                const status: StepStatus = isSkipped ? "skipped" : result?.status || "pending";
                const deps = step.config?.depends_on || [];

                const isExpanded = showDetails || expandedSteps.has(step.id);

                return (
                  <div
                    key={step.id}
                    className={`rounded bg-gray-900 border border-gray-800 transition-opacity ${
                      isSkipped ? "opacity-40" : ""
                    }`}
                    style={{ marginLeft: step._depth * 20 }}
                  >
                    {/* Main row */}
                    <div className="flex items-center gap-1.5 px-2 py-1.5">
                      {pipelineStatus !== "idle" && (
                        <span className="shrink-0 flex items-center justify-center w-[16px]">
                          {status === "pending" && (
                            <span className="w-2.5 h-2.5 rounded-full bg-gray-700 border border-gray-600" title="Pending" />
                          )}
                          {status === "running" && (
                            <Loader2 size={13} className="text-blue-400 animate-spin" strokeWidth={2.5} />
                          )}
                          {status === "success" && (
                            <CheckCircle2 size={13} className="text-green-400" strokeWidth={2.5} />
                          )}
                          {status === "failed" && (
                            <XCircle size={13} className="text-red-400" strokeWidth={2.5} />
                          )}
                          {status === "skipped" && (
                            <span className="text-[9px] text-gray-600">—</span>
                          )}
                        </span>
                      )}

                      <button
                        onClick={() => toggleExpand(step.id)}
                        className={`shrink-0 p-0.5 rounded transition-colors ${
                          expandedSteps.has(step.id)
                            ? "text-blue-400 hover:text-blue-300"
                            : "text-gray-600 hover:text-gray-400"
                        }`}
                        title={expandedSteps.has(step.id) ? "Close editor" : "Edit step"}
                      >
                        <Pencil size={11} />
                      </button>

                      <button
                        onClick={() => toggleExpand(step.id)}
                        className="text-gray-600 hover:text-gray-400 shrink-0 w-3"
                      >
                        {isExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                      </button>

                      <input
                        type="checkbox"
                        checked={!isSkipped}
                        onChange={() => toggleSkip(step.id)}
                        className="w-3 h-3 rounded border-gray-600 bg-gray-800 text-blue-500 focus:ring-blue-600 focus:ring-offset-0 cursor-pointer shrink-0"
                      />

                      <span className="text-[10px] font-mono text-gray-500 w-5 text-right shrink-0">
                        {step._stepNum}
                      </span>

                      <span className={`text-[10px] px-1.5 py-0.5 rounded border font-medium shrink-0 ${typeBadgeStyle(step.type)}`}>
                        {step.type}
                      </span>

                      <span className="text-xs text-gray-300 font-mono truncate min-w-0 flex-1">
                        {step.label || step.id}
                      </span>

                      {!isExpanded && step.config?.output_path && (
                        <span className="text-[9px] px-1.5 py-0.5 bg-gray-800 text-gray-500 rounded font-mono shrink-0 hidden sm:inline-block max-w-[200px] truncate">
                          &rarr; {step.config.output_path}
                        </span>
                      )}
                      {!isExpanded && step.config?.table_name && (
                        <span className="text-[9px] px-1.5 py-0.5 bg-emerald-900/30 text-emerald-500 rounded font-mono shrink-0 hidden sm:inline-block max-w-[200px] truncate">
                          {step.config.table_name}
                        </span>
                      )}

                      <span className="shrink-0 w-[140px] text-right truncate">
                        {status === "running" && result?.phase && (
                          <span className="text-[9px] text-blue-400 font-medium animate-pulse">{result.phase}</span>
                        )}
                      </span>

                      <span className="shrink-0 w-[50px] text-right">
                        {status === "running" && result?.startedAt && (
                          <span className="text-[10px] text-blue-300 font-mono tabular-nums">
                            {formatDuration(now - result.startedAt)}
                          </span>
                        )}
                        {status !== "running" && result?.duration_ms != null && (
                          <span className="text-[10px] text-gray-400 font-mono tabular-nums">
                            {formatDuration(result.duration_ms)}
                          </span>
                        )}
                      </span>

                      <span className="shrink-0 w-[70px] text-right">
                        {result?.row_count != null && result.row_count > 0 && (
                          <span className="text-[10px] text-gray-400 font-mono">
                            {formatRowCount(result.row_count)} rows
                          </span>
                        )}
                      </span>

                      <span className="shrink-0 flex items-center gap-0.5 ml-1">
                        <button
                          onClick={() => handleMoveStep(step.id, -1)}
                          title="Move up"
                          className="p-0.5 text-gray-600 hover:text-gray-300 rounded"
                        >
                          <ArrowUp size={11} />
                        </button>
                        <button
                          onClick={() => handleMoveStep(step.id, 1)}
                          title="Move down"
                          className="p-0.5 text-gray-600 hover:text-gray-300 rounded"
                        >
                          <ArrowDown size={11} />
                        </button>
                        {(step.type === "loop" || Array.isArray(step.children)) && (
                          <button
                            onClick={() => { setAddingChildOf(step.id); setAddOpen(true); }}
                            title="Add child step"
                            className="p-0.5 text-gray-600 hover:text-blue-400 rounded"
                          >
                            <Plus size={11} />
                          </button>
                        )}
                        <button
                          onClick={() => handleDeleteStep(step.id)}
                          title="Delete step"
                          className="p-0.5 text-gray-600 hover:text-red-400 rounded"
                        >
                          <Trash2 size={11} />
                        </button>
                      </span>
                    </div>

                    {/* Expandable detail section: editable config */}
                    {isExpanded && (
                      <div className="px-3 pb-3 pt-1 ml-8 border-t border-gray-800/50 space-y-2">
                        <StepConfigForm
                          step={step}
                          onChange={(patch) => handleUpdateStep(step.id, patch)}
                          onBlur={commit}
                        />
                        {deps.length > 0 && (
                          <div className="text-[10px] text-gray-500">
                            depends_on: <span className="font-mono text-gray-400">{deps.join(", ")}</span>
                          </div>
                        )}
                        {result?.message && (
                          <div className={`mt-1 text-[10px] font-mono px-2 py-1 rounded ${
                            result.status === "failed"
                              ? "bg-red-950/50 border border-red-900 text-red-400"
                              : "bg-gray-950 border border-gray-800 text-gray-400"
                          }`}>
                            {result.message}
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}

              {/* Error messages for failed steps */}
              {Array.from(stepResults.entries())
                .filter(([, r]) => r.status === "failed" && r.message)
                .map(([id, r]) => {
                  const step = flatSteps.find((s) => s.id === id);
                  return (
                    <div key={`err-${id}`} className="mx-3 mt-1 px-3 py-2 rounded bg-red-950/50 border border-red-900 text-xs text-red-400">
                      <span className="font-medium">{step?.label || id}:</span> {r.message}
                    </div>
                  );
                })}
            </div>
          ) : (
            <div className="text-center py-12 text-gray-600">
              <p className="text-sm">No pipeline steps defined</p>
              <p className="text-xs mt-1">Click <span className="font-medium text-gray-300">Add Step</span> above to add the first step.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
