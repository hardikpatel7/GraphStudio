// Components tab inside the workspace panel. Lists workspace components
// (reusable widget definitions with `<placeholder>` templates), supports
// inline create, expand-to-edit, delete with confirm. Placeholders are
// highlighted live as the user types the template so they can see what
// they're committing to.

import { useEffect, useState } from "react";
import {
  CheckCircle2, ChevronDown, ChevronRight, Clock, Edit, Eye, EyeOff, Loader2,
  Plus, Save, Trash2, X,
} from "lucide-react";
import type { WidgetKind } from "../dashboards/types";
import { WidgetShell, normalize, type WidgetPayload, type WidgetMeta } from "../dashboards/widgets";
import { componentsApi } from "./api";
import type { Component } from "./types";
import { extractPlaceholders } from "./types";

const KINDS: WidgetKind[] = ["kpi", "bar", "line", "pie", "table", "text"];

export function ComponentsTab({ workspaceId }: { workspaceId: string }) {
  const [items, setItems] = useState<Component[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const refresh = () => {
    componentsApi.list(workspaceId).then(setItems).catch(console.error);
  };
  useEffect(refresh, [workspaceId]);

  return (
    <div>
      <div className="flex items-baseline gap-3 mb-3">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider">Components</h2>
        <span className="text-xs text-slate-400">{items.length} defined</span>
        <button
          onClick={() => { setCreating(true); setEditingId(null); }}
          className="ml-auto inline-flex items-center gap-1.5 px-3 py-1.5 bg-gradient-to-br from-indigo-500 to-blue-600 text-white rounded-lg text-sm font-medium hover:from-indigo-600 hover:to-blue-700 shadow-sm transition"
        >
          <Plus className="w-3.5 h-3.5" /> New component
        </button>
      </div>

      <p className="text-xs text-slate-500 mb-3">
        A component is a widget definition with a prompt template. Use{" "}
        <code className="font-mono bg-slate-100 px-1 py-0.5 rounded">{"{{placeholder}}"}</code> tokens
        in the template — when a dashboard widget is bound to this component, it
        will supply values for each placeholder.
      </p>

      {creating && (
        <div className="mb-3">
          <ComponentForm
            workspaceId={workspaceId}
            mode="create"
            initial={blankComponent()}
            onSave={async (vals) => {
              await componentsApi.create(workspaceId, {
                name: vals.name,
                description: vals.description ?? undefined,
                kind: vals.kind,
                prompt_template: vals.prompt_template,
              });
              setCreating(false);
              refresh();
            }}
            onCancel={() => setCreating(false)}
          />
        </div>
      )}

      <div className="grid gap-2">
        {items.length === 0 && !creating ? (
          <div className="text-sm text-slate-400 py-6 text-center border border-dashed border-slate-200 rounded-2xl bg-white/60">
            No components yet — click "New component" to define one.
          </div>
        ) : (
          items.map((c) => (
            editingId === c.id ? (
              <ComponentForm
                key={c.id}
                workspaceId={workspaceId}
                mode="edit"
                initial={c}
                onSave={async (vals) => {
                  await componentsApi.patch(c.id, vals);
                  setEditingId(null);
                  refresh();
                }}
                onCancel={() => setEditingId(null)}
              />
            ) : (
              <ComponentRow
                key={c.id}
                workspaceId={workspaceId}
                component={c}
                onEdit={() => { setEditingId(c.id); setCreating(false); }}
                onDeleted={refresh}
              />
            )
          ))
        )}
      </div>
    </div>
  );
}

// ── Row (collapsed view) ─────────────────────────────────────────────────

function ComponentRow(props: {
  workspaceId: string;
  component: Component;
  onEdit: () => void;
  onDeleted: () => void;
}) {
  const c = props.component;
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [showPreview, setShowPreview] = useState(false);

  const onDelete = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirming) { setConfirming(true); return; }
    setDeleting(true);
    try {
      await componentsApi.delete(c.id);
      props.onDeleted();
    } catch (err) {
      console.error(err);
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  };

  return (
    <div className="group border border-slate-200 rounded-xl bg-white hover:border-slate-300 hover:shadow-sm transition">
      <div className="flex items-start gap-3 p-3.5">
        <div className="w-9 h-9 rounded-lg bg-slate-100 text-slate-500 flex items-center justify-center flex-shrink-0">
          <span className="text-[10px] font-mono">{c.kind}</span>
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-baseline gap-2 flex-wrap">
            <div className="font-medium text-slate-900 truncate">{c.name}</div>
            {c.placeholders.length > 0 && (
              <div className="text-[11px] text-slate-500 inline-flex items-center gap-1">
                {c.placeholders.length} placeholder{c.placeholders.length === 1 ? "" : "s"}:
                {c.placeholders.map((p) => (
                  <code key={p} className="font-mono bg-indigo-50 text-indigo-700 px-1 py-0.5 rounded">
                    {`{{${p}}}`}
                  </code>
                ))}
              </div>
            )}
          </div>
          {c.description && (
            <div className="text-xs text-slate-500 mt-0.5">{c.description}</div>
          )}
          <pre className="mt-2 font-mono text-[11px] text-slate-600 bg-slate-50 rounded px-2 py-1.5 whitespace-pre-wrap break-words max-h-24 overflow-y-auto">
            {highlightPlaceholders(c.prompt_template)}
          </pre>
          <div className="flex items-center gap-3 mt-1 text-[11px] text-slate-400">
            <span className="inline-flex items-center gap-1"><Clock className="w-3 h-3" /> {timeAgo(c.updated_at)}</span>
            <button
              onClick={(e) => { e.stopPropagation(); setShowPreview((v) => !v); }}
              className="inline-flex items-center gap-1 text-indigo-600 hover:text-indigo-800"
            >
              {showPreview ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3" />}
              {showPreview ? "hide preview" : "preview"}
            </button>
          </div>
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          {!confirming && (
            <button
              onClick={(e) => { e.stopPropagation(); props.onEdit(); }}
              className="rounded-md px-2 py-1 text-xs text-slate-400 hover:text-indigo-600 hover:bg-indigo-50 opacity-0 group-hover:opacity-100 transition"
              title="Edit component"
            >
              <Edit className="w-3.5 h-3.5" />
            </button>
          )}
          <button
            onClick={onDelete}
            disabled={deleting}
            className={[
              "rounded-md px-2 py-1 text-xs transition flex items-center gap-1",
              confirming
                ? "bg-rose-600 text-white hover:bg-rose-700"
                : "text-slate-400 hover:text-rose-600 hover:bg-rose-50 opacity-0 group-hover:opacity-100",
            ].join(" ")}
            title={confirming ? "Click again to confirm delete" : "Delete component"}
          >
            {deleting ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Trash2 className="w-3.5 h-3.5" />}
            {confirming && <span>delete?</span>}
          </button>
          {!confirming && <ChevronRight className="w-4 h-4 text-slate-300" />}
        </div>
      </div>
      {showPreview && (
        <div className="border-t border-slate-200 px-3.5 py-3 bg-slate-50/50">
          <PreviewPanel
            workspaceId={props.workspaceId}
            kind={c.kind}
            promptTemplate={c.prompt_template}
            placeholders={c.placeholders}
          />
        </div>
      )}
    </div>
  );
}

// Highlight `{{placeholder}}` tokens in the preview pre. We render
// alternating runs of plain text and highlighted spans rather than
// mutating the string.
function highlightPlaceholders(template: string): React.ReactNode[] {
  const re = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;
  const out: React.ReactNode[] = [];
  let last = 0;
  let i = 0;
  for (const m of template.matchAll(re)) {
    const start = m.index ?? 0;
    if (start > last) out.push(template.slice(last, start));
    out.push(
      <span key={i++} className="bg-indigo-100 text-indigo-800 rounded px-0.5">
        {m[0]}
      </span>
    );
    last = start + m[0].length;
  }
  if (last < template.length) out.push(template.slice(last));
  return out;
}

// ── Form (create + edit) ─────────────────────────────────────────────────

type FormValues = {
  name: string;
  description: string | null;
  kind: WidgetKind;
  prompt_template: string;
};

function ComponentForm(props: {
  workspaceId: string;
  mode: "create" | "edit";
  initial: FormValues;
  onSave: (vals: FormValues) => Promise<void>;
  onCancel: () => void;
}) {
  const [vals, setVals] = useState<FormValues>(props.initial);
  const [saving, setSaving] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const placeholders = extractPlaceholders(vals.prompt_template);

  const save = async () => {
    setSaving(true);
    try { await props.onSave(vals); }
    catch (e) { console.error(e); }
    finally { setSaving(false); }
  };

  return (
    <div className="border border-indigo-200 rounded-xl bg-white p-4 shadow-sm">
      <div className="flex items-center gap-2 mb-3">
        <Edit className="w-4 h-4 text-indigo-500" />
        <div className="font-medium text-slate-900">
          {props.mode === "create" ? "New component" : "Edit component"}
        </div>
        <button
          onClick={props.onCancel}
          className="ml-auto text-slate-400 hover:text-slate-700 p-1 rounded-md hover:bg-slate-100 transition"
          title="Cancel"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      <div className="grid grid-cols-2 gap-3 mb-3">
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Name</span>
          <input
            value={vals.name}
            onChange={(e) => setVals({ ...vals, name: e.target.value })}
            placeholder="Top 10 articles by metric"
            className="border border-slate-200 rounded-md px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200"
          />
        </label>
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Kind</span>
          <select
            value={vals.kind}
            onChange={(e) => setVals({ ...vals, kind: e.target.value as WidgetKind })}
            className="border border-slate-200 rounded-md px-2 py-1.5 text-sm bg-white"
          >
            {KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
          </select>
        </label>
      </div>

      <label className="flex flex-col gap-1 text-xs mb-3">
        <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Description (optional)</span>
        <input
          value={vals.description ?? ""}
          onChange={(e) => setVals({ ...vals, description: e.target.value || null })}
          placeholder="What this component shows"
          className="border border-slate-200 rounded-md px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200"
        />
      </label>

      <label className="flex flex-col gap-1 text-xs mb-2">
        <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">
          Prompt template — wrap variables in <code className="font-mono bg-slate-100 px-1 rounded">{"<name>"}</code>
        </span>
        <textarea
          value={vals.prompt_template}
          onChange={(e) => setVals({ ...vals, prompt_template: e.target.value })}
          rows={5}
          placeholder="Show me top 10 articles by {{metric}} for {{brand}}."
          className="border border-slate-200 rounded-md px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-indigo-200 resize-y"
        />
      </label>

      <div className="mb-3 text-xs">
        {placeholders.length === 0 ? (
          <span className="text-slate-400">No placeholders detected yet.</span>
        ) : (
          <span className="text-slate-600 inline-flex items-center gap-1 flex-wrap">
            <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500" />
            Placeholders:
            {placeholders.map((p) => (
              <code key={p} className="font-mono bg-indigo-50 text-indigo-700 px-1 py-0.5 rounded">
                {`{{${p}}}`}
              </code>
            ))}
          </span>
        )}
      </div>

      <div className="flex items-center gap-2 flex-wrap">
        <button
          onClick={save}
          disabled={saving || !vals.name.trim() || !vals.prompt_template.trim()}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm bg-gradient-to-br from-indigo-500 to-blue-600 text-white hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
        >
          {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Save className="w-3.5 h-3.5" />}
          {props.mode === "create" ? "Create" : "Save"}
        </button>
        <button
          onClick={() => setPreviewOpen((v) => !v)}
          disabled={!vals.prompt_template.trim()}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm border border-slate-200 bg-white hover:border-slate-300 disabled:opacity-50 transition"
        >
          {previewOpen ? <ChevronDown className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
          {previewOpen ? "Hide preview" : "Preview"}
        </button>
        <button onClick={props.onCancel} className="text-sm text-slate-500 hover:text-slate-800 px-2 py-1">
          Cancel
        </button>
      </div>

      {previewOpen && (
        <div className="mt-3 border-t border-slate-200 pt-3">
          <PreviewPanel
            workspaceId={props.workspaceId}
            kind={vals.kind}
            promptTemplate={vals.prompt_template}
            placeholders={placeholders}
          />
        </div>
      )}
    </div>
  );
}

// ── PreviewPanel — fill placeholder values, run preview, render result ──

/**
 * Shared preview surface — used inline on saved component rows AND inside
 * the create/edit form. Maintains its own placeholder-value state so
 * users can experiment without committing the values to a real dashboard.
 */
function PreviewPanel(props: {
  workspaceId: string;
  kind: WidgetKind;
  promptTemplate: string;
  placeholders: string[];
}) {
  const { workspaceId, kind, promptTemplate, placeholders } = props;
  const [values, setValues] = useState<Record<string, string>>(() => {
    const seed: Record<string, string> = {};
    for (const p of placeholders) seed[p] = "";
    return seed;
  });
  // Keep state aligned with the placeholder list (changes when the
  // template is edited live in the form).
  useEffect(() => {
    setValues((cur) => {
      const next: Record<string, string> = {};
      for (const p of placeholders) next[p] = cur[p] ?? "";
      return next;
    });
  }, [placeholders.join("|")]);

  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [payload, setPayload] = useState<WidgetPayload | null>(null);
  const [meta, setMeta] = useState<WidgetMeta | null>(null);

  const canRun = placeholders.every((p) => (values[p] ?? "").trim().length > 0);

  const run = async () => {
    setRunning(true); setError(null);
    try {
      const raw = await componentsApi.preview(workspaceId, {
        kind, prompt_template: promptTemplate, placeholder_values: values,
      });
      // Backend wraps responses as `{data, meta}` for fresh runs;
      // tolerate the legacy bare-payload shape.
      const env = raw as { data?: unknown; meta?: WidgetMeta };
      const data = (env && env.data !== undefined) ? env.data : raw;
      setPayload(normalize(kind, data));
      setMeta((env && env.meta) ? env.meta : null);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div>
      <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium mb-2">
        Preview
      </div>
      {placeholders.length > 0 ? (
        <div className="space-y-1.5 mb-2">
          {placeholders.map((name) => (
            <label key={name} className="flex items-center gap-2 text-xs">
              <code className="font-mono bg-indigo-50 text-indigo-700 px-1.5 py-0.5 rounded text-[11px] flex-shrink-0 w-32 truncate">
                {`{{${name}}}`}
              </code>
              <input
                value={values[name] ?? ""}
                onChange={(e) => setValues({ ...values, [name]: e.target.value })}
                placeholder={`value for ${name}`}
                className="flex-1 border border-slate-200 rounded-md px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200"
              />
            </label>
          ))}
        </div>
      ) : (
        <div className="text-xs text-slate-500 mb-2">
          No placeholders — run with the template as-is.
        </div>
      )}
      <button
        onClick={run}
        disabled={running || !canRun}
        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm bg-gradient-to-br from-indigo-500 to-blue-600 text-white hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
      >
        {running ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Eye className="w-3.5 h-3.5" />}
        {running ? "Running…" : "Run preview"}
      </button>
      {!canRun && placeholders.length > 0 && (
        <span className="ml-2 text-[11px] text-slate-400">Fill every placeholder to enable.</span>
      )}

      {(payload || error) && (
        <div className="mt-3">
          <WidgetShell
            node={{
              type: "widget",
              id: "_preview",
              kind,
              title: "Preview",
              prompt: promptTemplate,
            }}
            payload={payload}
            loading={running}
            error={error}
            meta={meta}
          />
        </div>
      )}
    </div>
  );
}

function blankComponent(): FormValues {
  return { name: "", description: null, kind: "kpi", prompt_template: "" };
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}
