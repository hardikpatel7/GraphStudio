// Leaf widget renderer. Dispatches by `kind`:
//   - kpi/bar/line/pie → reuse `ChartBlock` (it accepts a raw JSON string)
//   - table/text       → render via the markdown renderer
//
// Inputs come in two shapes the renderer normalizes:
//   - `data` from runWidget API: object/array (parsed JSON)
//   - `data` from get-dashboard's widget_cache: may be string OR object
//     because the backend's SQLite layer auto-parses TEXT that looks like
//     JSON.

import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { CheckCircle2, Loader2, RefreshCw, XCircle } from "lucide-react";
import { ChartBlock } from "../charts";
import type { WidgetKind, WidgetNode } from "./types";

/** Normalize a server-supplied payload into the renderer's view of it. */
export type WidgetPayload =
  | { kind: "chart"; specJson: string }
  | { kind: "markdown"; text: string }
  | { kind: "empty" }
  | { kind: "error"; message: string };

export function normalize(widgetKind: WidgetKind, raw: unknown): WidgetPayload {
  if (raw == null) return { kind: "empty" };

  // Markdown widgets: backend stores `{ markdown: "..." }`.
  if (widgetKind === "table" || widgetKind === "text") {
    if (typeof raw === "string") {
      try {
        const parsed = JSON.parse(raw) as { markdown?: string };
        if (parsed && typeof parsed.markdown === "string") return { kind: "markdown", text: parsed.markdown };
        return { kind: "markdown", text: raw };
      } catch {
        return { kind: "markdown", text: raw };
      }
    }
    if (typeof raw === "object" && raw !== null && "markdown" in raw) {
      const md = (raw as { markdown: unknown }).markdown;
      if (typeof md === "string") return { kind: "markdown", text: md };
    }
    return { kind: "error", message: "Unexpected markdown widget shape" };
  }

  // Chart widgets: ChartBlock takes a raw JSON string.
  if (typeof raw === "string") {
    return { kind: "chart", specJson: raw };
  }
  try {
    return { kind: "chart", specJson: JSON.stringify(raw) };
  } catch {
    return { kind: "error", message: "Couldn't serialize chart spec" };
  }
}

// ── Widget shell ─────────────────────────────────────────────────────────

export type WidgetMeta = {
  wall_ms?: number | null;
  llm_ms?: number | null;
  cost_usd?: number | null;
  fetched_at?: number | null;
  from_cache?: boolean;
  /** Backing prompt row id; lets future UIs deep-link into the
   *  prompt-detail drawer. */
  prompt_id?: string | null;
  /** Count of tool calls the agent made during this run. */
  tool_calls_total?: number | null;
  /** How many of those tool calls returned an error. The agent often
   *  retries transparently and still produces a chart; surfacing this
   *  in the footer lets the user see "the underlying SQL retried 3×"
   *  instead of believing the run was clean. */
  tool_errors?: number | null;
  /** Sample message from the first failed tool call (truncated). */
  first_tool_error?: string | null;
};

/** Header chip showing the current value of one placeholder. When
 *  `editable` is true the chip toggles into an inline input on click;
 *  Enter commits the new value (calls `onCommit`), Escape or blur
 *  without change cancels. Used for runtime placeholder edits without
 *  touching the dashboard's saved layout. */
function FilterChip({
  name, value, editable, onCommit,
}: {
  name: string;
  value: string;
  editable: boolean;
  onCommit: (next: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(value);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => { setDraft(value); }, [value]);
  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  const commit = () => {
    setOpen(false);
    const next = draft.trim();
    if (next && next !== value) onCommit(next);
  };
  const cancel = () => {
    setOpen(false);
    setDraft(value);
  };

  if (open) {
    return (
      <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10.5px] font-mono bg-white border border-indigo-300 ring-1 ring-indigo-200">
        <span className="text-indigo-400">{name}:</span>
        <input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); commit(); }
            else if (e.key === "Escape") { e.preventDefault(); cancel(); }
          }}
          onBlur={commit}
          className="bg-transparent outline-none font-mono text-[10.5px] text-indigo-700 min-w-[80px] max-w-[220px]"
          size={Math.max(8, draft.length + 1)}
        />
      </span>
    );
  }
  return (
    <button
      type="button"
      disabled={!editable}
      onClick={() => editable && setOpen(true)}
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10.5px] font-mono bg-indigo-50 text-indigo-700 border border-indigo-100 ${editable ? "cursor-text hover:border-indigo-300 hover:bg-indigo-100/60" : ""}`}
      title={editable ? `Click to change <${name}> at runtime (transient — doesn't save).` : `Placeholder <${name}> resolved to: ${value}`}
    >
      <span className="text-indigo-400">{name}:</span>
      <span className="font-medium truncate max-w-[160px]">{value}</span>
      {editable && <span className="text-indigo-300 text-[9px]">✎</span>}
    </button>
  );
}

export function WidgetShell({
  node, payload, loading, error, meta, filters, onRefresh, onPick, onSetPlaceholder,
}: {
  node: WidgetNode;
  payload: WidgetPayload | null;
  loading: boolean;
  error: string | null;
  /** Telemetry from the last run (fresh `{wall_ms, llm_ms, cost_usd,
   *  fetched_at}` after a refresh, or `{fetched_at, from_cache:true}`
   *  when the payload came from widget_cache on initial load). */
  meta?: WidgetMeta | null;
  /** Effective placeholder values for the rendered payload (saved
   *  `placeholder_values` + the most recent drill override).
   *  Surfaced as chips under the title so the user can read
   *  "brand: DASH · article: 108054813-100" at a glance — without
   *  this they can't tell which slice of a drill chain a widget
   *  is currently showing. */
  filters?: Record<string, string>;
  onRefresh?: () => void;
  /** Drill-down callback. Bar/pie clicks fire with the bar label;
   *  table-row clicks fire with the first cell's text. Wired up
   *  upstream when another widget declares `drill.from === node.id`. */
  onPick?: (label: string) => void;
  /** Runtime placeholder edit. Clicking a filter chip opens an
   *  inline input; pressing Enter calls this with the new value.
   *  Re-runs the widget with the override (transient — doesn't
   *  modify the saved layout). */
  onSetPlaceholder?: (name: string, value: string) => void;
}) {
  const filterEntries = filters
    ? Object.entries(filters).filter(([, v]) => v != null && String(v).trim() !== "")
    : [];
  // Header tooltip: assemble whatever context the widget knows about
  // itself — title, kind, the prompt that produced the payload, the
  // currently-resolved placeholders, and the source prompt_id when
  // available. Lets the user hover any part of the header to see
  // "what am I looking at?" without leaving view mode.
  const headerTooltip = buildHeaderTooltip(node, filterEntries, meta);
  return (
    // `flex-1` here is what makes a widget actually fill its slot in
    // a row (so a row of 5 KPIs all show at the same height even when
    // one has a hint and another doesn't). The slot itself is
    // `flex flex-col` with a `flex: span` weight from NodeRenderer,
    // so flex-1 on the shell stretches vertically inside the slot.
    <div className="border border-slate-200 rounded-xl bg-white shadow-sm flex flex-col flex-1" title={headerTooltip}>
      <div className="px-3 py-2 border-b border-slate-100 flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0 flex-wrap">
          <span
            className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-mono text-slate-500 bg-slate-100 flex-shrink-0"
            title={kindHint(node.kind)}
          >
            {node.kind}
          </span>
          <div className="font-medium text-slate-800 text-sm truncate" title={node.title}>{node.title}</div>
          {filterEntries.map(([k, v]) => (
            <FilterChip
              key={k}
              name={k}
              value={v}
              editable={!!onSetPlaceholder}
              onCommit={(next) => { if (onSetPlaceholder && next !== v) onSetPlaceholder(k, next); }}
            />
          ))}
        </div>
        <div className="flex items-center gap-1 flex-shrink-0">
          {/* When the refresh button is present it doubles as the loading
              indicator (spinning RefreshCw on `loading`). Only render the
              standalone Loader2 when no refresh button exists (e.g. the
              DashboardEdit preview pane) — otherwise the header showed
              two side-by-side spinners during a run. */}
          {loading && !onRefresh && <Loader2 className="w-3.5 h-3.5 text-slate-400 animate-spin" />}
          {!loading && error && <XCircle className="w-3.5 h-3.5 text-rose-500" />}
          {!loading && !error && payload && payload.kind !== "empty" && (
            <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500" />
          )}
          {onRefresh && (
            <button
              onClick={onRefresh}
              disabled={loading}
              className="rounded p-1 text-slate-400 hover:text-indigo-600 hover:bg-indigo-50 transition disabled:opacity-40"
              title="Refresh this widget"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin text-indigo-500" : ""}`} />
            </button>
          )}
        </div>
      </div>
      <div className="p-3 flex-1 min-h-[80px]">
        <WidgetBody payload={payload} loading={loading} error={error} onPick={onPick} emitColumn={node.drill_emit_column} />
      </div>
      {meta && (payload || error) && !loading && (
        <div className="px-3 py-1.5 border-t border-slate-100 text-[10.5px] text-slate-500 flex items-center gap-2.5 flex-wrap font-mono">
          {meta.fetched_at != null && (
            <span title={new Date(meta.fetched_at).toLocaleString()}>
              {meta.from_cache ? "cached · " : ""}{fmtRelative(meta.fetched_at)}
            </span>
          )}
          {meta.wall_ms != null && <span>{formatMs(meta.wall_ms)}</span>}
          {meta.llm_ms != null && meta.wall_ms != null && meta.llm_ms !== meta.wall_ms && (
            <span className="text-slate-400">(LLM {formatMs(meta.llm_ms)})</span>
          )}
          {meta.cost_usd != null && meta.cost_usd > 0 && (
            <span>${meta.cost_usd.toFixed(4)}</span>
          )}
          {meta.tool_calls_total != null && meta.tool_calls_total > 0 && (
            <span className="text-slate-400">
              {meta.tool_calls_total} tool call{meta.tool_calls_total === 1 ? "" : "s"}
            </span>
          )}
          {meta.tool_errors != null && meta.tool_errors > 0 && (
            <span
              className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10.5px] text-amber-700 bg-amber-50 border border-amber-200"
              title={meta.first_tool_error ? `First error — ${meta.first_tool_error}` : "Underlying tool calls returned errors; the agent retried."}
            >
              ⚠ {meta.tool_errors} tool error{meta.tool_errors === 1 ? "" : "s"}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function fmtRelative(ts: number): string {
  const delta = Date.now() - ts;
  if (delta < 0) return "just now";
  const s = Math.floor(delta / 1000);
  if (s < 5)  return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/** Compose the multi-line header hover-tooltip. Lets a viewer hover
 *  any widget chrome and see title, kind, current filters, the
 *  resolved prompt, and the underlying prompt_id without going to
 *  edit mode. */
function buildHeaderTooltip(
  node: WidgetNode,
  filterEntries: Array<[string, string]>,
  meta: WidgetMeta | null | undefined,
): string {
  const lines: string[] = [];
  lines.push(`${node.title}  [${node.kind}]`);
  if (filterEntries.length) {
    lines.push(
      "Filters: " + filterEntries.map(([k, v]) => `${k}=${v}`).join(", "),
    );
  }
  // Truncate the prompt so the OS tooltip stays readable; full
  // text is available in the editor's prompt textarea.
  if (node.prompt && node.prompt.trim()) {
    const p = node.prompt.length > 600 ? node.prompt.slice(0, 600) + "…" : node.prompt;
    lines.push("Prompt:");
    lines.push(p);
  } else if (node.component_id) {
    lines.push(`(backed by component ${node.component_id})`);
  }
  if (meta?.prompt_id) lines.push(`prompt_id: ${meta.prompt_id}`);
  return lines.join("\n");
}

/** One-line description of each widget kind. Shown on the kind chip
 *  so a new user can hover "pareto" or "waterfall" to learn what
 *  it's for without leaving the page. */
function kindHint(kind: WidgetKind): string {
  switch (kind) {
    case "kpi":         return "Single headline number (optionally with a delta + sparkline).";
    case "bar":         return "Horizontal bar chart — top N by some value.";
    case "line":        return "Line chart — values over an ordered axis.";
    case "pie":         return "Donut / pie — composition with ≤6 slices.";
    case "stacked_bar": return "One bar per row, each split into series — composition per category.";
    case "bullet":      return "Actual-vs-target reads (e.g. OH vs policy max).";
    case "pareto":      return "Bar + cumulative line — 80/20 concentration.";
    case "funnel":      return "Narrowing steps — filter cascades / triage flows.";
    case "gauge":       return "Half-arc gauge for service-level / fill-rate reads.";
    case "sparkline":   return "Compact trend with no axes — direction only.";
    case "heatmap":     return "2D matrix coloured by value (e.g. L1 × DC).";
    case "treemap":     return "Hierarchical composition sized by value.";
    case "histogram":   return "Distribution shape — bins + counts.";
    case "slope":       return "Two-period comparison — entity lines from A to B.";
    case "boxplot":     return "Distribution per category — 5-number summary.";
    case "waterfall":   return "Running total decomposition (start → ±steps → end).";
    case "table":       return "Markdown table — rows + columns.";
    case "text":        return "Markdown prose — short notes / narrative.";
  }
}

function WidgetBody({
  payload, loading, error, onPick, emitColumn,
}: {
  payload: WidgetPayload | null;
  loading: boolean;
  error: string | null;
  onPick?: (label: string) => void;
  /** For `table` widgets: 1-based index of the column to emit on
   *  row-click. Default 1 (first column). Lets a 4-column results
   *  table emit, say, `brand` instead of `article` when a row is
   *  clicked. Bar / pie / pareto / funnel etc. don't have multiple
   *  columns to choose from — they always emit the bar's label. */
  emitColumn?: number;
}) {
  if (error) {
    return (
      <div className="rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs text-rose-700">
        <div className="font-medium">Widget run failed</div>
        <div className="mt-1 whitespace-pre-wrap break-words">{error}</div>
      </div>
    );
  }
  if (loading && !payload) {
    return (
      <div className="flex items-center gap-2 text-xs text-slate-400">
        <Loader2 className="w-3.5 h-3.5 animate-spin" /> Running prompt…
      </div>
    );
  }
  if (!payload || payload.kind === "empty") {
    return <div className="text-xs text-slate-400">Click ↻ to run this widget for the first time.</div>;
  }
  if (payload.kind === "chart") {
    return <ChartBlock raw={payload.specJson} onPick={onPick} />;
  }
  if (payload.kind === "markdown") {
    // When `onPick` is wired and we're rendering a markdown table,
    // each <tr> in the body becomes clickable — the first cell's
    // visible text is the drill key (article id, store code, etc.).
    // Header rows are skipped naturally because the GFM renderer
    // emits them in <thead>, not <tbody>.
    const tdGetText = (node: unknown): string => {
      if (typeof node === "string") return node;
      if (typeof node === "number") return String(node);
      if (Array.isArray(node)) return node.map(tdGetText).join("");
      if (node && typeof node === "object" && "props" in (node as Record<string, unknown>)) {
        const p = (node as { props?: { children?: unknown } }).props;
        return tdGetText(p?.children);
      }
      return "";
    };
    return (
      <div className="text-sm text-slate-800 leading-relaxed [&>*:first-child]:mt-0 [&>*:last-child]:mb-0 space-y-2.5">
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          components={{
            table: ({ children }) => (
              <div className="overflow-x-auto rounded-lg border border-slate-200 bg-white">
                <table className="min-w-full text-xs">{children}</table>
              </div>
            ),
            thead: ({ children }) => <thead className="bg-slate-50 text-slate-700">{children}</thead>,
            tbody: ({ children }) => <tbody className="divide-y divide-slate-100">{children}</tbody>,
            th:    ({ children }) => <th className="px-3 py-2 text-left font-semibold border-b border-slate-200">{children}</th>,
            td:    ({ children }) => <td className="px-3 py-2 align-top text-slate-700 font-mono text-[12px]">{children}</td>,
            tr:    ({ children }) => {
              if (!onPick) return <tr>{children}</tr>;
              // children is the list of <td> children. The designer
              // picks which column's cell is the drill value via the
              // source widget's `drill_emit_column` (1-based; default
              // 1 = first column). Out-of-range silently clamps to
              // first, so a table that came back with fewer columns
              // than configured doesn't break the chain.
              const arr = Array.isArray(children) ? children : [children];
              const cells = arr.filter((c) => c && typeof c === "object" && "props" in c);
              if (cells.length === 0) return <tr>{children}</tr>;
              const idx = Math.max(0, Math.min(cells.length - 1, (emitColumn ?? 1) - 1));
              const picked = cells[idx] as { props: { children: unknown } };
              const key = tdGetText(picked.props.children).trim();
              if (!key) return <tr>{children}</tr>;
              return (
                <tr
                  onClick={() => onPick(key)}
                  className="cursor-pointer hover:bg-indigo-50/60 transition"
                  title={`Drill into ${key} (column ${idx + 1})`}
                >
                  {children}
                </tr>
              );
            },
            code:  ({ children }) => <code className="px-1.5 py-0.5 rounded bg-slate-100 text-slate-800 font-mono text-[12px]">{children}</code>,
            ul:    ({ children }) => <ul className="list-disc pl-5 space-y-1">{children}</ul>,
            ol:    ({ children }) => <ol className="list-decimal pl-5 space-y-1">{children}</ol>,
          }}
        >
          {payload.text}
        </ReactMarkdown>
      </div>
    );
  }
  if (payload.kind === "error") {
    return <div className="text-xs text-rose-600">{payload.message}</div>;
  }
  return null;
}
