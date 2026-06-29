// Composition tree types. Mirror the layout shape in the agent.db
// `dashboard.layout_json` column (see `server/src/agent/schema.sql`).
//
// Three node kinds: `row` (horizontal flex), `column` (vertical flex), and
// `widget` (leaf). Containers have `children`; widgets have `kind` + saved
// `prompt`. Each node carries a stable `id` so the backend can key
// widget_cache rows on (dashboard_id, node_id).

export type WidgetKind =
  | "kpi"
  | "bar"
  | "line"
  | "pie"
  | "stacked_bar"
  | "bullet"
  | "pareto"
  | "funnel"
  | "gauge"
  | "sparkline"
  | "heatmap"
  | "treemap"
  | "histogram"
  | "slope"
  | "boxplot"
  | "waterfall"
  | "table"
  | "text";

export type RowNode = {
  type: "row";
  id: string;
  children: TreeNode[];
};

export type ColumnNode = {
  type: "column";
  id: string;
  children: TreeNode[];
};

export type WidgetNode = {
  type: "widget";
  id: string;
  kind: WidgetKind;
  title: string;
  /** Inline prompt for standalone widgets. Ignored when `component_id`
   *  is set — the runner resolves the prompt from the component's
   *  template + `placeholder_values` at run time. */
  prompt: string;
  /** Flex weight when sibling of other widgets in a row. Default 1. */
  span?: number;
  /** Optional binding to a reusable component. When set, the widget's
   *  kind + prompt are derived from the component at run time; the
   *  inline `kind` / `prompt` fields are kept for diagnostics but the
   *  runner ignores them. */
  component_id?: string;
  /** Per-placeholder values for the bound component's template. Keys
   *  match the names in `<placeholder>` tokens. Unfilled keys cause the
   *  runner to return an error rather than send a half-rendered prompt. */
  placeholder_values?: Record<string, string>;
  /** Drill-down hook. When set, clicking a bar / pie-slice / table-row
   *  in widget `from` populates this widget's placeholder `set` with
   *  the clicked value, and this widget re-runs with that override.
   *  Chains naturally: A→B drill + B→C drill means clicking in A
   *  updates B; clicking in B updates C.
   *
   *  `auto` (default `true`): when the parent widget loads or
   *  finishes a run, automatically pick its first item and cascade
   *  to this widget. Set `false` for "manual" drill — this widget
   *  only refreshes when the user explicitly clicks a parent item. */
  drill?: { from: string; set: string; auto?: boolean };
  /** Auto-run this widget when the dashboard is opened. Only
   *  meaningful for TOP widgets (no `drill.from`); drill children
   *  honor `drill.auto` instead. Default `false` — top widgets
   *  stay idle until the user clicks ↻, so a freshly-opened
   *  dashboard doesn't burn an LLM call before the user has a
   *  chance to edit the prompt. */
  auto_run_on_open?: boolean;
  /** For `table` widgets only: when this widget is acting as the
   *  source of a drill (a child declares `drill.from === <this>`),
   *  which column's cell from the clicked row becomes the drill
   *  value? 1-based column index. Default = 1 (first column).
   *  Picked by the dashboard designer in the table widget's props
   *  drawer. Lets a 4-column table emit, say, the `brand` column
   *  rather than the `article` column when a row is clicked. */
  drill_emit_column?: number;
};

export type TreeNode = RowNode | ColumnNode | WidgetNode;

export type DashboardLayout = {
  version: 1;
  root: TreeNode;
};

/** Default layout matches the backend's `EMPTY_LAYOUT` constant. */
export const EMPTY_LAYOUT: DashboardLayout = {
  version: 1,
  root: { type: "column", id: "root", children: [] },
};

// ── REST shapes ──────────────────────────────────────────────────────────

export type DashboardSummary = {
  id: string;
  workspace_id: string;
  session_id: string;
  name: string;
  description: string | null;
  /** Model used for widget runs. Mirror of the synthetic session's
   *  `model` column — change via `PATCH { model }`. Always present
   *  (the synthetic session is created with the workspace default,
   *  so this is never null). */
  model: string;
  created_at: number;
  updated_at: number;
};

export type WidgetCacheRow = {
  node_id: string;
  spec_hash: string;
  /** Parsed payload the renderer feeds the widget. Either a chart spec
   *  (chart kinds) or `{ markdown: "..." }` (table/text). May arrive as
   *  a string or pre-parsed object depending on how SQLite serialized it —
   *  the renderer normalizes. */
  data_json: unknown;
  fetched_at: number;
  prompt_id: string | null;
};

export type DashboardDetail = DashboardSummary & {
  /** Same dual-shape rationale as widget_cache data: SQLite TEXT can come
   *  back parsed when its content looks like JSON. */
  layout_json: string | DashboardLayout;
  widgets: WidgetCacheRow[];
};
