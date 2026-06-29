// View mode for a saved dashboard:
//   1. Fetch the dashboard (incl. widget cache) on mount.
//   2. Render the layout tree — row/column are flex containers; widget
//      leaves go through WidgetShell.
//   3. For each leaf widget without a cached row, auto-run it once on
//      first open so the dashboard isn't a sea of empty cards.
//   4. Per-widget ↻ refresh + dashboard-level "Refresh all" + Edit + Back.

import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, Edit, RefreshCw, Sparkles } from "lucide-react";
import type { DashboardDetail, DashboardLayout, TreeNode, WidgetCacheRow, WidgetNode } from "./types";
import { EMPTY_LAYOUT } from "./types";
import { dashboardsApi } from "./api";
import type { ModelEntry } from "../api";
import { collectWidgets } from "./tree";
import { WidgetShell, normalize, type WidgetPayload } from "./widgets";

export type WidgetMeta = {
  /** Total wall time including tool dispatch + response parse, ms. */
  wall_ms?: number | null;
  /** Provider-reported LLM latency in ms (`llm_usage.latency_ms`). */
  llm_ms?: number | null;
  /** Derived cost for the run. */
  cost_usd?: number | null;
  /** When this payload landed (ms epoch). */
  fetched_at?: number | null;
  /** True when the payload came from widget_cache, not a fresh run. */
  from_cache?: boolean;
};

type WidgetState = {
  payload: WidgetPayload | null;
  loading: boolean;
  error: string | null;
  meta: WidgetMeta | null;
  /** The effective placeholder values for the LAST run: saved
   *  `widget.placeholder_values` plus whatever override the most
   *  recent click / auto-cascade applied. Surfaced in the widget
   *  header so the user can read "brand: DASH" at a glance and
   *  know what the table or detail card is actually filtered to. */
  filters: Record<string, string>;
};

export function DashboardView(props: {
  dashboardId: string;
  /** Enabled models, fetched once at app boot. Used to populate the
   *  header dropdown that swaps the dashboard's active model. */
  models: ModelEntry[];
  onBack: () => void;
  onEdit: () => void;
}) {
  const [detail, setDetail] = useState<DashboardDetail | null>(null);
  const [fetchErr, setFetchErr] = useState<string | null>(null);
  const [widgets, setWidgets] = useState<Record<string, WidgetState>>({});
  const [refreshingAll, setRefreshingAll] = useState(false);
  // Track which widgets have been auto-run this session so we don't
  // accidentally re-trigger on every render.
  const autoRunFired = useRef<Set<string>>(new Set());

  // Load the dashboard + its cached widgets.
  useEffect(() => {
    let cancelled = false;
    setDetail(null); setFetchErr(null);
    dashboardsApi.get(props.dashboardId)
      .then((d) => {
        if (cancelled) return;
        setDetail(d);
        setWidgets(initialWidgetState(d));
      })
      .catch((e: unknown) => { if (!cancelled) setFetchErr(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, [props.dashboardId]);

  const layout = useMemo(() => parseLayout(detail), [detail]);

  // Auto-fill behavior on dashboard open:
  //  - Top widget (no `drill.from`) with `auto_run_on_open === true`
  //    → kick off a run immediately (runOne's success will cascade).
  //  - Top widget with no cache and `auto_run_on_open !== true`
  //    → stay idle; user must click ↻.
  //  - Top widget with cached payload
  //    → cascade once to drill children so a re-opened dashboard
  //      doesn't sit half-empty. Children with `drill.auto === false`
  //      are skipped by `cascade`.
  useEffect(() => {
    if (!detail || !layout) return;
    const all = collectWidgets(layout);
    for (const w of all) {
      if (w.drill?.from) continue; // downstream — waits for cascade
      const key = `${detail.id}:${w.id}`;
      if (autoRunFired.current.has(key)) continue;
      const cur = widgets[w.id];
      if (!cur || cur.loading) continue;
      if (cur.payload == null) {
        if (w.auto_run_on_open === true) {
          autoRunFired.current.add(key);
          void runOne(detail.id, w);
        }
        continue;
      }
      // Has cache → cascade once to populate idle children.
      const firstPick = firstPickFromPayload(cur.payload);
      const childrenIdle = all.some(
        (c) => c.drill?.from === w.id && widgets[c.id]?.payload == null && !widgets[c.id]?.loading,
      );
      if (firstPick != null && childrenIdle) {
        autoRunFired.current.add(key);
        cascade(w.id, firstPick, { manual: false });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail, layout, widgets]);

  const runOne = async (dashboardId: string, w: WidgetNode, overrides?: Record<string, string>) => {
    // Compute the effective placeholder set for this run = saved
    // values + override. Stored on the widget state so the header
    // can show "brand: DASH" after a drill click.
    const filters: Record<string, string> = {
      ...(w.placeholder_values ?? {}),
      ...(overrides ?? {}),
    };
    setWidgets((prev) => ({
      ...prev,
      [w.id]: { ...(prev[w.id] ?? { payload: null, meta: null, filters: {} }), loading: true, error: null, filters },
    }));
    try {
      const response = await dashboardsApi.runWidget(dashboardId, w.id, overrides);
      // Backend returns `{data, meta}` envelopes; older callers / cache
      // hits still hand back the bare payload. Accept both.
      const { data, meta } = unwrapEnvelope(response);
      const payload = normalize(w.kind, data);
      setWidgets((prev) => ({
        ...prev,
        [w.id]: { payload, loading: false, error: null, meta, filters },
      }));
      // Auto-cascade: pick the first item from this widget's result and
      // fire any drill children whose `drill.auto !== false`. Skip when
      // overrides is non-empty (= this was a drill run itself, not a
      // top-level refresh) so we don't re-fire on a stale first-pick.
      if (!overrides || Object.keys(overrides).length === 0) {
        const firstPick = firstPickFromPayload(payload);
        if (firstPick != null) cascade(w.id, firstPick, { manual: false });
      }
    } catch (e: unknown) {
      setWidgets((prev) => ({
        ...prev,
        [w.id]: {
          payload: prev[w.id]?.payload ?? null,
          loading: false,
          error: e instanceof Error ? e.message : String(e),
          meta: prev[w.id]?.meta ?? null,
          filters,
        },
      }));
    }
  };

  /** Trigger every widget that declares `drill.from === sourceId`
   *  with `<placeholder> = pickedValue`. Used by both manual clicks
   *  and the auto-cascade after a parent finishes loading.
   *
   *  `manual` is true when the user explicitly clicked a parent
   *  item — those always fire regardless of the child's `auto`
   *  flag (the click IS the consent). `manual: false` runs honor
   *  `child.drill.auto`: children with `auto === false` stay idle
   *  until the user clicks. */
  const cascade = (sourceId: string, pickedValue: string, opts: { manual: boolean }) => {
    if (!detail || !layout) return;
    const all = collectWidgets(layout);
    for (const child of all) {
      if (child.drill?.from === sourceId && child.drill.set) {
        if (!opts.manual && child.drill.auto === false) continue;
        const merged = {
          ...(child.placeholder_values ?? {}),
          [child.drill.set]: pickedValue,
        };
        void runOne(detail.id, child, merged);
      }
    }
  };

  const onPick = (sourceId: string) => (label: string) => cascade(sourceId, label, { manual: true });

  /** Runtime placeholder edit: the user typed a new value into the
   *  widget's header chip and pressed Enter. Re-runs THIS widget
   *  with the new value as a transient override (same shape as a
   *  drill click — saved `placeholder_values` stay untouched, so a
   *  page reload returns to the dashboard's defaults). For
   *  permanent changes the user can still edit the widget in
   *  Designer mode and Save. */
  const setPlaceholder = (widgetId: string, name: string, value: string) => {
    if (!detail || !layout) return;
    const all = collectWidgets(layout);
    const w = all.find((x) => x.id === widgetId);
    if (!w) return;
    const current = widgets[widgetId]?.filters ?? w.placeholder_values ?? {};
    const merged = { ...current, [name]: value };
    void runOne(detail.id, w, merged);
  };

  /** Refresh only top-level widgets (no `drill.from`). Their drill
   *  children fall in line automatically via `runOne`'s success-path
   *  cascade, so re-running every widget in parallel — the previous
   *  behavior — was wasteful: it burned an LLM call on every drill
   *  child immediately, then the cascade would overwrite those
   *  results with the first-pick auto-fill anyway. The cleaner
   *  shape: refresh tops, let cascade walk down. Children with
   *  `drill.auto === false` stay put and the user clicks into them
   *  manually, same as the regular auto-cascade rules. */
  const refreshAll = async () => {
    if (!detail || !layout) return;
    const all = collectWidgets(layout);
    const tops = all.filter((w) => !w.drill?.from);
    if (tops.length === 0) return;
    setRefreshingAll(true);
    try {
      // Run tops concurrently; each runOne handles its own state +
      // auto-cascade.
      await Promise.all(tops.map((w) => runOne(detail.id, w)));
    } finally {
      setRefreshingAll(false);
    }
  };

  /** Swap the model used for future widget runs. Optimistically
   *  updates local state so the dropdown is responsive; reverts +
   *  surfaces the error if the PATCH 400s (model not in allowlist).
   *  Cached widget payloads stay put — the user has to ↻ to see the
   *  new model's output. */
  const onModelChange = async (next: string) => {
    if (!detail || next === detail.model) return;
    const prev = detail.model;
    setDetail({ ...detail, model: next });
    try {
      await dashboardsApi.patch(detail.id, { model: next });
    } catch (e: unknown) {
      setDetail((d) => (d ? { ...d, model: prev } : d));
      setFetchErr(e instanceof Error ? e.message : String(e));
      // Auto-clear the banner after a few seconds so it doesn't stick
      // around. The dropdown reverts immediately on its own.
      setTimeout(() => setFetchErr(null), 4000);
    }
  };

  if (fetchErr) {
    return (
      <div className="flex-1 p-6 max-w-3xl mx-auto w-full">
        <div className="border border-rose-300 bg-rose-50 rounded-xl p-4 text-sm text-rose-800">
          <div className="font-medium mb-1">Couldn't load dashboard</div>
          <div className="font-mono text-xs whitespace-pre-wrap">{fetchErr}</div>
        </div>
      </div>
    );
  }
  if (!detail || !layout) {
    return (
      <div className="flex-1 flex items-center justify-center text-sm text-slate-400">Loading…</div>
    );
  }

  return (
    <div className="flex-1 flex flex-col max-w-7xl mx-auto w-full p-6 gap-4">
      <header className="flex items-center gap-3">
        <button onClick={props.onBack} className="text-sm text-slate-500 hover:text-slate-800 inline-flex items-center gap-1">
          <ArrowLeft className="w-3.5 h-3.5" /> back
        </button>
        <h1 className="text-lg font-semibold text-slate-900 flex-1 truncate flex items-center gap-2">
          <span className="w-7 h-7 rounded-lg bg-gradient-to-br from-indigo-500 to-blue-500 flex items-center justify-center text-white">
            <Sparkles className="w-4 h-4" />
          </span>
          {detail.name}
        </h1>
        <ModelPicker
          value={detail.model}
          models={props.models}
          onChange={onModelChange}
        />
        <button
          onClick={refreshAll}
          disabled={refreshingAll}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm border border-slate-200 bg-white hover:border-slate-300 hover:shadow-sm transition disabled:opacity-50"
          title="Refresh every widget"
        >
          <RefreshCw className={`w-3.5 h-3.5 ${refreshingAll ? "animate-spin" : ""}`} />
          Refresh all
        </button>
        <button
          onClick={props.onEdit}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm bg-gradient-to-br from-indigo-500 to-blue-600 text-white hover:from-indigo-600 hover:to-blue-700 shadow-sm transition"
        >
          <Edit className="w-3.5 h-3.5" /> Edit
        </button>
      </header>

      {detail.description && (
        <div className="text-sm text-slate-500">{detail.description}</div>
      )}

      {/* Widgets size to their content; the browser scrolls the page
          when the stack exceeds viewport height. No nested overflow
          containers, no max-heights on widget shells — long tables
          and detail cards display in full. Special wide-format
          renderers (treemap) cap their own max-height so they don't
          grow forever on a single-widget dashboard. */}
      <div className="flex-1">
        <NodeRenderer
          node={layout.root}
          widgets={widgets}
          onRefreshWidget={(w) => runOne(detail.id, w)}
          onPickFor={onPick}
          drillTargets={drillTargets(layout)}
          onSetPlaceholder={setPlaceholder}
        />
      </div>
    </div>
  );
}

// ── Tree renderer ────────────────────────────────────────────────────────

function NodeRenderer(props: {
  node: TreeNode;
  widgets: Record<string, WidgetState>;
  onRefreshWidget: (w: WidgetNode) => void;
  onPickFor: (sourceId: string) => (label: string) => void;
  /** Set of widget ids that act as drill SOURCES — i.e. some other
   *  widget declares `drill.from === <this id>`. Only sources get a
   *  click handler so non-drill widgets stay non-interactive. */
  drillTargets: Set<string>;
  /** Runtime placeholder edit handler. Re-runs the widget with the
   *  new value as a transient override. */
  onSetPlaceholder?: (widgetId: string, name: string, value: string) => void;
}) {
  const { node, widgets, onRefreshWidget, onPickFor, drillTargets, onSetPlaceholder } = props;
  if (node.type === "widget") {
    const ws = widgets[node.id] ?? { payload: null, loading: false, error: null, meta: null, filters: {} };
    const isSource = drillTargets.has(node.id);
    // `flex` ratio applies inside a row (horizontal); inside a column
    // it lets a widget stretch only if siblings are smaller. Default
    // case: each widget sizes to its content. Widgets that want a
    // bigger render area (treemap, heatmap) supply their own
    // min-height inside the renderer.
    return (
      <div className="flex flex-col" style={{ flex: node.span && node.span > 1 ? node.span : 1 }}>
        <WidgetShell
          node={node}
          payload={ws.payload}
          loading={ws.loading}
          error={ws.error}
          meta={ws.meta}
          filters={ws.filters}
          onRefresh={() => onRefreshWidget(node)}
          onPick={isSource ? onPickFor(node.id) : undefined}
          onSetPlaceholder={onSetPlaceholder ? (name, value) => onSetPlaceholder(node.id, name, value) : undefined}
        />
      </div>
    );
  }
  const dir = node.type === "row" ? "flex-row" : "flex-col";
  const gap = "gap-3";
  return (
    <div className={`flex ${dir} ${gap} mb-3`}>
      {node.children.length === 0 ? (
        <div className="text-xs text-slate-400 px-3 py-2 border border-dashed border-slate-200 rounded-lg flex-1">
          Empty {node.type}.
        </div>
      ) : (
        node.children.map((c) => (
          <NodeRenderer
            key={c.id}
            node={c}
            widgets={widgets}
            onRefreshWidget={onRefreshWidget}
            onPickFor={onPickFor}
            drillTargets={drillTargets}
            onSetPlaceholder={onSetPlaceholder}
          />
        ))
      )}
    </div>
  );
}

/** Set of every widget id that's the `drill.from` of some other widget
 *  in the layout. Pre-computed once per layout so the renderer can
 *  decide cheaply whether to attach a click handler. */
function drillTargets(layout: DashboardLayout): Set<string> {
  const out = new Set<string>();
  for (const w of collectWidgets(layout)) {
    if (w.drill?.from) out.add(w.drill.from);
  }
  return out;
}

// ── Helpers ──────────────────────────────────────────────────────────────

function parseLayout(detail: DashboardDetail | null): DashboardLayout | null {
  if (!detail) return null;
  const raw = detail.layout_json;
  if (typeof raw === "string") {
    try { return JSON.parse(raw) as DashboardLayout; } catch { return EMPTY_LAYOUT; }
  }
  if (raw && typeof raw === "object") return raw as DashboardLayout;
  return EMPTY_LAYOUT;
}

/** Extract a sensible "first item" from a widget's payload to seed
 *  its drill children. Bar / pie → `data[0].label`. Markdown table
 *  → first cell of the first non-header data row. Returns `null`
 *  when no obvious pick exists (text payloads, empty tables, parse
 *  failures) — caller treats that as "no cascade". */
function firstPickFromPayload(payload: WidgetPayload | null): string | null {
  if (!payload) return null;
  if (payload.kind === "chart") {
    try {
      const spec = JSON.parse(payload.specJson) as { data?: Array<{ label?: unknown }> };
      const first = spec?.data?.[0];
      if (first && typeof first.label === "string" && first.label.trim()) return first.label.trim();
    } catch { /* ignore */ }
    return null;
  }
  if (payload.kind === "markdown") {
    // Walk the markdown for the first table's first data row.
    // Header separator looks like `| --- | --- |`. Cells before/
    // after the outer pipes are empty after split, so we strip
    // those. Anything matching `:?-+:?` is the separator.
    const lines = payload.text.split("\n");
    let sawSeparator = false;
    for (const raw of lines) {
      const line = raw.trim();
      if (!line.startsWith("|")) {
        if (sawSeparator) return null;
        continue;
      }
      const cells = line
        .split("|")
        .map((c) => c.trim())
        .filter((_, i, arr) => i > 0 && i < arr.length - 1);
      if (cells.length === 0) continue;
      const isSep = cells.every((c) => /^:?-+:?$/.test(c));
      if (isSep) { sawSeparator = true; continue; }
      if (!sawSeparator) continue; // header row before separator
      // First data row. Strip backticks / leading $ etc. and return.
      const key = cells[0].replace(/^`|`$/g, "").trim();
      return key || null;
    }
    return null;
  }
  return null;
}

function initialWidgetState(detail: DashboardDetail): Record<string, WidgetState> {
  const out: Record<string, WidgetState> = {};
  const byNode: Record<string, WidgetCacheRow> = {};
  for (const w of detail.widgets) byNode[w.node_id] = w;
  const layout = parseLayout(detail) ?? EMPTY_LAYOUT;
  for (const w of collectWidgets(layout)) {
    const cached = byNode[w.id];
    out[w.id] = {
      payload: cached ? normalize(w.kind, cached.data_json) : null,
      loading: false,
      error: null,
      // Cache rows carry `fetched_at` but not latency or cost (those
      // belong to the original run, not the replay). Surface what we
      // have so the widget header can show "loaded from cache · 3h ago".
      meta: cached ? { fetched_at: cached.fetched_at, from_cache: true } : null,
      // Cached payload was produced by the widget's saved placeholder
      // values (drill runs skip the cache); surface those so the header
      // shows the current filter context.
      filters: { ...(w.placeholder_values ?? {}) },
    };
  }
  return out;
}

/** Backend's `/widgets/:id/run` returns `{data, meta}`. Cache hits and
 *  older callers still hand back the bare payload. Tolerate both
 *  shapes by detecting the envelope structure. */
function unwrapEnvelope(raw: unknown): { data: unknown; meta: WidgetMeta | null } {
  if (
    raw && typeof raw === "object" &&
    "data" in raw && "meta" in raw &&
    typeof (raw as { meta?: unknown }).meta === "object"
  ) {
    const env = raw as { data: unknown; meta: WidgetMeta };
    return { data: env.data, meta: env.meta };
  }
  return { data: raw, meta: null };
}

/** Header-bar model dropdown. Style mirrors the "Refresh all" button so
 *  the trio (model / Refresh / Edit) reads as one toolbar.
 *
 *  If the dashboard's stored model isn't in the allowlist (e.g. a row
 *  the admin disabled after the dashboard was created), include it as a
 *  disabled option so the user sees what's selected and can pick a
 *  replacement. */
function ModelPicker(props: {
  value: string;
  models: ModelEntry[];
  onChange: (next: string) => void;
}) {
  const inList = props.models.some((m) => m.model === props.value);
  return (
    <label className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-sm border border-slate-200 bg-white hover:border-slate-300 hover:shadow-sm transition">
      <span className="text-slate-400 text-xs">model</span>
      <select
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
        className="bg-transparent outline-none cursor-pointer text-slate-700"
        title="Model used for every future widget run"
      >
        {!inList && (
          <option value={props.value} disabled>
            {props.value} (disabled)
          </option>
        )}
        {props.models.map((m) => (
          <option key={m.model} value={m.model}>
            {m.display_name}
          </option>
        ))}
      </select>
    </label>
  );
}
