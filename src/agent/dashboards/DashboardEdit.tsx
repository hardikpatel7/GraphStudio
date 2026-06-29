// Designer for a dashboard's composition tree.
//
// Two-pane layout:
//   - Left rail: outline tree (every node, indented). Selecting a node
//     enables the Add/Move/Delete toolbar at the top. Selecting a widget
//     also opens the props form on the right.
//   - Right: live preview that mirrors the view mode but with each
//     widget showing its title + prompt rather than running. The "Run
//     this widget" button in the props form fires a one-off run + caches.
//
// Save commits a single PATCH with (name, description, layout_json).

import { useEffect, useState } from "react";
import {
  ArrowLeft, ArrowDown, ArrowUp, ChevronRight, Loader2, MessageSquare, Pencil,
  Plus, Save, Trash2,
} from "lucide-react";
import type { DashboardDetail, DashboardLayout, TreeNode, WidgetKind, WidgetNode } from "./types";
import { EMPTY_LAYOUT } from "./types";
import { dashboardsApi } from "./api";
import {
  addChild, collectWidgets, findNode, findParent, makeColumn, makeRow, makeTemplateRows, makeWidget, moveNode, removeNode, replaceNode,
} from "./tree";
import type { TemplateId } from "./tree";
import { componentsApi } from "../components/api";
import type { Component } from "../components/types";
import { extractPlaceholders, substitutePlaceholders } from "../components/types";
import { WidgetShell, normalize, type WidgetPayload } from "./widgets";

const KINDS: WidgetKind[] = [
  "kpi", "bar", "line", "pie",
  "stacked_bar", "bullet", "pareto", "funnel",
  "gauge", "sparkline",
  "heatmap", "treemap", "histogram", "slope",
  "boxplot", "waterfall",
  "table", "text",
];

export function DashboardEdit(props: { dashboardId: string; onDone: () => void }) {
  const [detail, setDetail] = useState<DashboardDetail | null>(null);
  const [layout, setLayout] = useState<DashboardLayout>(EMPTY_LAYOUT);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [previewRuns, setPreviewRuns] = useState<Record<string, { payload: WidgetPayload | null; loading: boolean; error: string | null }>>({});
  // Workspace components for the "+ From component" picker. Fetched
  // after the dashboard loads (we need workspace_id from the detail).
  const [components, setComponents] = useState<Component[]>([]);
  const [showComponentPicker, setShowComponentPicker] = useState(false);

  useEffect(() => {
    let cancelled = false;
    dashboardsApi.get(props.dashboardId)
      .then((d) => {
        if (cancelled) return;
        setDetail(d);
        setName(d.name);
        setDescription(d.description ?? "");
        setLayout(parseLayout(d));
        // Now that we know which workspace this dashboard lives in,
        // pull its components so the toolbar's "From component"
        // dropdown has something to show.
        componentsApi.list(d.workspace_id)
          .then((cs) => { if (!cancelled) setComponents(cs); })
          .catch(console.error);
      })
      .catch(console.error);
    return () => { cancelled = true; };
  }, [props.dashboardId]);

  const selected = selectedId ? findNode(layout, selectedId) : null;
  const selectedIsContainer = !!selected && (selected.type === "row" || selected.type === "column");

  // ── Mutations ──────────────────────────────────────────────────────────

  const insert = (factory: () => TreeNode) => {
    const newNode = factory();
    // If a container is selected, insert as child. Otherwise insert as
    // sibling of the selected node (or under root if nothing selected).
    if (!selectedId) {
      setLayout((l) => addChild(l, l.root.id, newNode));
    } else if (selectedIsContainer) {
      setLayout((l) => addChild(l, selectedId, newNode));
    } else {
      // Sibling: insert into the parent of the selected node.
      const parent = findParent(layout, selectedId);
      if (parent) setLayout((l) => addChild(l, parent.id, newNode));
      else setLayout((l) => addChild(l, l.root.id, newNode));
    }
    setSelectedId(newNode.id);
  };

  /** Insert a widget bound to the chosen component. The widget's
   *  kind/title default to the component's; the inline `prompt` stays
   *  empty (the runner resolves it from the component template +
   *  placeholder_values). Initial placeholder_values are empty strings
   *  keyed off the template's placeholders — the user fills them in
   *  via the props drawer. */
  const insertFromComponent = (component: Component) => {
    const phValues: Record<string, string> = {};
    for (const p of component.placeholders) phValues[p] = "";
    const widget = makeWidget(component.kind);
    widget.title = component.name;
    widget.prompt = ""; // standalone field unused for component-backed widgets
    widget.component_id = component.id;
    widget.placeholder_values = phValues;

    if (!selectedId) {
      setLayout((l) => addChild(l, l.root.id, widget));
    } else if (selectedIsContainer) {
      setLayout((l) => addChild(l, selectedId, widget));
    } else {
      const parent = findParent(layout, selectedId);
      setLayout((l) => addChild(l, parent ? parent.id : l.root.id, widget));
    }
    setSelectedId(widget.id);
    setShowComponentPicker(false);
  };

  /** Stamp a layout template — appends its rows to the selected container
   *  (or the root column when nothing's selected). Widgets in the
   *  template default to `kpi` with empty prompts; the user fills them
   *  in afterward. */
  const insertTemplate = (template: TemplateId) => {
    const rows = makeTemplateRows(template);
    // Resolve target container once so all rows land in the same place.
    let containerId = layout.root.id;
    if (selectedId) {
      if (selectedIsContainer) {
        containerId = selectedId;
      } else {
        const parent = findParent(layout, selectedId);
        if (parent) containerId = parent.id;
      }
    }
    setLayout((l) => {
      let next = l;
      for (const r of rows) next = addChild(next, containerId, r);
      return next;
    });
    // Select the first row so the user can immediately see where it
    // landed and start filling in widgets.
    if (rows[0]) setSelectedId(rows[0].id);
  };

  const onDeleteSelected = () => {
    if (!selectedId || selectedId === layout.root.id) return;
    setLayout((l) => removeNode(l, selectedId));
    setSelectedId(null);
  };

  const onMove = (dir: -1 | 1) => {
    if (!selectedId) return;
    setLayout((l) => moveNode(l, selectedId, dir));
  };

  const updateSelectedWidget = (mut: (w: WidgetNode) => WidgetNode) => {
    if (!selected || selected.type !== "widget") return;
    setLayout((l) => replaceNode(l, selected.id, mut(selected)));
  };

  // ── Save ───────────────────────────────────────────────────────────────

  const save = async () => {
    setSaving(true);
    try {
      await dashboardsApi.patch(props.dashboardId, {
        name: name.trim() || "Untitled",
        description: description.trim() ? description.trim() : null,
        layout_json: layout,
      });
      props.onDone();
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  };

  // ── Preview run ────────────────────────────────────────────────────────

  const runPreview = async (w: WidgetNode) => {
    setPreviewRuns((p) => ({ ...p, [w.id]: { payload: p[w.id]?.payload ?? null, loading: true, error: null } }));
    try {
      // Save first so the backend has the latest prompt/kind before running.
      await dashboardsApi.patch(props.dashboardId, { layout_json: layout });
      const data = await dashboardsApi.runWidget(props.dashboardId, w.id);
      setPreviewRuns((p) => ({ ...p, [w.id]: { payload: normalize(w.kind, data), loading: false, error: null } }));
    } catch (e: unknown) {
      setPreviewRuns((p) => ({ ...p, [w.id]: { payload: p[w.id]?.payload ?? null, loading: false, error: e instanceof Error ? e.message : String(e) } }));
    }
  };

  if (!detail) {
    return (
      <div className="flex-1 flex items-center justify-center text-sm text-slate-400">
        <Loader2 className="w-4 h-4 animate-spin mr-2" /> Loading designer…
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col max-w-7xl mx-auto w-full p-6 gap-3">
      <header className="flex items-center gap-3 flex-wrap">
        <button onClick={props.onDone} className="text-sm text-slate-500 hover:text-slate-800 inline-flex items-center gap-1">
          <ArrowLeft className="w-3.5 h-3.5" /> back
        </button>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Dashboard name"
          className="flex-1 min-w-[200px] font-semibold text-lg text-slate-900 bg-transparent border-b border-transparent hover:border-slate-200 focus:border-indigo-300 focus:outline-none py-1"
        />
        <button
          onClick={save}
          disabled={saving}
          className="inline-flex items-center gap-1.5 px-4 py-1.5 rounded-lg text-sm bg-gradient-to-br from-indigo-500 to-blue-600 text-white hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
        >
          {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-3.5 h-3.5" />}
          Save
        </button>
      </header>

      <input
        value={description}
        onChange={(e) => setDescription(e.target.value)}
        placeholder="Description (optional)"
        className="text-sm text-slate-600 bg-transparent border-b border-transparent hover:border-slate-200 focus:border-indigo-300 focus:outline-none py-1"
      />

      {/* Toolbar */}
      <div className="border border-slate-200 rounded-lg bg-white px-2 py-1.5 flex items-center gap-1 flex-wrap text-xs">
        <button onClick={() => insert(makeRow)}    className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1"><Plus className="w-3 h-3" />Row</button>
        <button onClick={() => insert(makeColumn)} className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1"><Plus className="w-3 h-3" />Column</button>
        <button onClick={() => insert(() => makeWidget("kpi"))} className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1"><Plus className="w-3 h-3" />Widget</button>
        <div className="relative">
          <button
            onClick={() => setShowComponentPicker((v) => !v)}
            disabled={components.length === 0}
            className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1 disabled:opacity-40"
            title={components.length === 0 ? "No components defined in this workspace yet" : "Insert a widget backed by a saved component"}
          >
            <Plus className="w-3 h-3" />From component
          </button>
          {showComponentPicker && components.length > 0 && (
            <div className="absolute z-20 mt-1 left-0 w-72 max-h-64 overflow-y-auto bg-white border border-slate-200 rounded-lg shadow-lg p-1">
              {components.map((c) => (
                <button
                  key={c.id}
                  onClick={() => insertFromComponent(c)}
                  className="w-full text-left px-2 py-1.5 rounded hover:bg-indigo-50 flex items-center gap-2"
                >
                  <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-mono text-slate-500 bg-slate-100 flex-shrink-0">
                    {c.kind}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm text-slate-900 truncate">{c.name}</div>
                    {c.placeholders.length > 0 && (
                      <div className="text-[10px] text-slate-500 truncate">
                        {c.placeholders.map((p) => `{{${p}}}`).join(" · ")}
                      </div>
                    )}
                  </div>
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="w-px h-5 bg-slate-200 mx-1" />
        <button onClick={() => onMove(-1)} disabled={!selectedId || selectedId === layout.root.id} className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1 disabled:opacity-40">
          <ArrowUp className="w-3 h-3" />
        </button>
        <button onClick={() => onMove(1)} disabled={!selectedId || selectedId === layout.root.id} className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1 disabled:opacity-40">
          <ArrowDown className="w-3 h-3" />
        </button>
        <button onClick={onDeleteSelected} disabled={!selectedId || selectedId === layout.root.id} className="px-2 py-1 rounded hover:bg-rose-50 hover:text-rose-700 inline-flex items-center gap-1 disabled:opacity-40">
          <Trash2 className="w-3 h-3" /> Delete
        </button>
        <div className="ml-auto text-[11px] text-slate-400">
          {selected
            ? <>Selected: <span className="font-mono">{selected.type}</span> · <span className="font-mono">{selected.id}</span></>
            : <>Nothing selected — actions target the root column</>
          }
        </div>
      </div>

      {/* Layout templates — stamp common shapes instead of building
          widget-by-widget. Each preview is a small SVG mock so the
          user can recognize the shape at a glance. */}
      <div className="border border-slate-200 rounded-lg bg-white px-2 py-1.5 flex items-center gap-1.5 flex-wrap text-xs">
        <span className="text-[10px] uppercase tracking-wider text-slate-500 font-medium mr-1">Layouts</span>
        <TemplateButton id="2x2"     label="2 × 2"     onClick={() => insertTemplate("2x2")} />
        <TemplateButton id="2x3"     label="2 × 3"     onClick={() => insertTemplate("2x3")} />
        <TemplateButton id="1plus3"  label="1 + 3"     onClick={() => insertTemplate("1plus3")} />
        <TemplateButton id="3plus1"  label="3 + 1"     onClick={() => insertTemplate("3plus1")} />
        <TemplateButton id="1plus2"  label="1 + 2"     onClick={() => insertTemplate("1plus2")} />
        <TemplateButton id="kpi-row" label="KPI row"   onClick={() => insertTemplate("kpi-row")} />
      </div>

      {/* Two-pane body */}
      <div className="flex-1 grid grid-cols-12 gap-3 min-h-0">
        <div className="col-span-3 border border-slate-200 rounded-lg bg-white overflow-y-auto p-2">
          <OutlineNode
            node={layout.root}
            selectedId={selectedId}
            onSelect={setSelectedId}
            depth={0}
          />
        </div>
        <div className="col-span-9 border border-slate-200 rounded-lg bg-slate-50/40 overflow-y-auto p-3">
          <PreviewNode
            node={layout.root}
            selectedId={selectedId}
            onSelect={setSelectedId}
            previewRuns={previewRuns}
          />
          {selected && selected.type === "widget" && (
            <PropsDrawer
              widget={selected as WidgetNode}
              siblings={collectWidgets(layout).filter((w) => w.id !== selected.id)}
              onChange={updateSelectedWidget}
              onRun={() => runPreview(selected as WidgetNode)}
              running={!!previewRuns[selected.id]?.loading}
              components={components}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// ── Outline (left rail) ──────────────────────────────────────────────────

function OutlineNode(props: {
  node: TreeNode;
  selectedId: string | null;
  onSelect: (id: string) => void;
  depth: number;
}) {
  const { node, selectedId, onSelect, depth } = props;
  const selected = node.id === selectedId;
  const isContainer = node.type === "row" || node.type === "column";
  const Icon = node.type === "widget" ? MessageSquare : ChevronRight;
  return (
    <div>
      <button
        onClick={() => onSelect(node.id)}
        className={[
          "w-full text-left flex items-center gap-1.5 px-1.5 py-1 rounded text-xs",
          selected ? "bg-indigo-100 text-indigo-900" : "hover:bg-slate-100 text-slate-700",
        ].join(" ")}
        style={{ paddingLeft: `${depth * 12 + 6}px` }}
      >
        <Icon className="w-3 h-3 flex-shrink-0" />
        {node.type === "widget"
          ? <span className="truncate"><span className="font-mono text-[10px] text-slate-500 mr-1">{node.kind}</span>{(node as WidgetNode).title}</span>
          : <span className="font-mono text-[10px]">{node.type}</span>
        }
      </button>
      {isContainer && (
        <div>
          {(node as { children: TreeNode[] }).children.map((c) => (
            <OutlineNode key={c.id} node={c} selectedId={selectedId} onSelect={onSelect} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

// ── Preview pane ─────────────────────────────────────────────────────────

function PreviewNode(props: {
  node: TreeNode;
  selectedId: string | null;
  onSelect: (id: string) => void;
  previewRuns: Record<string, { payload: WidgetPayload | null; loading: boolean; error: string | null }>;
}) {
  const { node, selectedId, onSelect, previewRuns } = props;
  if (node.type === "widget") {
    const w = node as WidgetNode;
    const run = previewRuns[w.id];
    const selected = w.id === selectedId;
    return (
      <div
        onClick={(e) => { e.stopPropagation(); onSelect(w.id); }}
        className={`cursor-pointer ${selected ? "ring-2 ring-indigo-300 rounded-xl" : ""}`}
        style={{ flex: w.span && w.span > 1 ? w.span : 1 }}
      >
        <WidgetShell
          node={w}
          payload={run?.payload ?? null}
          loading={!!run?.loading}
          error={run?.error ?? null}
        />
      </div>
    );
  }
  const dir = node.type === "row" ? "flex-row" : "flex-col";
  const selected = node.id === selectedId;
  return (
    <div
      onClick={(e) => { e.stopPropagation(); onSelect(node.id); }}
      className={[
        "p-2 rounded-lg cursor-pointer",
        selected ? "bg-indigo-50/60 ring-1 ring-indigo-200" : "hover:bg-slate-100/50",
      ].join(" ")}
    >
      <div className="text-[10px] uppercase tracking-wider text-slate-400 mb-1.5 font-mono">{node.type}</div>
      <div className={`flex ${dir} gap-3`}>
        {node.children.length === 0 ? (
          <div className="text-xs text-slate-400 px-3 py-2 border border-dashed border-slate-200 rounded-lg flex-1">
            Empty {node.type} — select and use the toolbar to add children.
          </div>
        ) : (
          node.children.map((c) => (
            <PreviewNode key={c.id} node={c} selectedId={selectedId} onSelect={onSelect} previewRuns={previewRuns} />
          ))
        )}
      </div>
    </div>
  );
}

// ── Props drawer for the selected widget ─────────────────────────────────

function PropsDrawer(props: {
  widget: WidgetNode;
  /** Every other widget in the dashboard. Source list for the
   *  drill-section's `from` picker (a widget can only drill from a
   *  sibling, not itself). */
  siblings: WidgetNode[];
  onChange: (mut: (w: WidgetNode) => WidgetNode) => void;
  onRun: () => void;
  running: boolean;
  components: Component[];
}) {
  const { widget, siblings, onChange, onRun, running, components } = props;
  const boundComponent = widget.component_id
    ? components.find((c) => c.id === widget.component_id) ?? null
    : null;
  const isComponentBacked = !!widget.component_id;

  // Compute placeholder list. Prefer the live component (in case the
  // template was edited after this widget was added — picks up new
  // placeholders without manual re-binding). Falls back to whatever the
  // widget already has.
  const placeholders = boundComponent
    ? extractPlaceholders(boundComponent.prompt_template)
    : Object.keys(widget.placeholder_values ?? {});

  const phValues = widget.placeholder_values ?? {};
  const resolvedPreview = boundComponent
    ? substitutePlaceholders(boundComponent.prompt_template, phValues)
    : "";

  const canRun = isComponentBacked
    ? !!boundComponent && placeholders.every((p) => (phValues[p] ?? "").trim().length > 0)
    : widget.prompt.trim().length > 0;

  return (
    <div className="mt-4 border border-indigo-200 rounded-xl bg-white p-4 shadow-sm">
      <div className="flex items-center gap-2 mb-3">
        <Pencil className="w-4 h-4 text-indigo-500" />
        <div className="font-medium text-slate-900">Widget properties</div>
        {isComponentBacked && (
          <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium bg-indigo-100 text-indigo-700">
            from component
          </span>
        )}
        <span className="font-mono text-[10px] text-slate-400 ml-auto">{widget.id}</span>
      </div>

      {/* Title is always editable. Kind picker is hidden when the widget
          is component-backed (the component owns the kind). */}
      <div className={`grid ${isComponentBacked ? "grid-cols-1" : "grid-cols-2"} gap-3 mb-3`}>
        {!isComponentBacked && (
          <label className="flex flex-col gap-1 text-xs">
            <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Kind</span>
            <select
              value={widget.kind}
              onChange={(e) => onChange((w) => ({ ...w, kind: e.target.value as WidgetKind }))}
              className="border border-slate-200 rounded-md px-2 py-1.5 text-sm bg-white"
            >
              {KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
            </select>
          </label>
        )}
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Title</span>
          <input
            value={widget.title}
            onChange={(e) => onChange((w) => ({ ...w, title: e.target.value }))}
            className="border border-slate-200 rounded-md px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200"
          />
        </label>
      </div>

      {isComponentBacked ? (
        <ComponentBackedFields
          widget={widget}
          component={boundComponent}
          placeholders={placeholders}
          resolvedPreview={resolvedPreview}
          onChange={onChange}
        />
      ) : (
        <label className="flex flex-col gap-1 text-xs mb-3">
          <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Prompt</span>
          <textarea
            value={widget.prompt}
            onChange={(e) => onChange((w) => ({ ...w, prompt: e.target.value }))}
            rows={4}
            placeholder="e.g. What's the total written_sales_dollars for FY26, Jewelry only?"
            className="border border-slate-200 rounded-md px-2 py-1.5 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-indigo-200 resize-y"
          />
        </label>
      )}

      {/* Table widgets are the only chart kind where the row has
          multiple columns the click can come from. The designer
          picks which column's cell becomes the drill value via
          `drill_emit_column` (1-based). Other kinds always emit
          the bar/slice/step label — no choice to make. */}
      {widget.kind === "table" && (
        <label className="flex flex-col gap-1 text-xs mb-3">
          <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">
            On row click, emit cell from column
          </span>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min={1}
              value={widget.drill_emit_column ?? 1}
              onChange={(e) => {
                const n = Math.max(1, Number(e.target.value) || 1);
                onChange((w) => ({ ...w, drill_emit_column: n }));
              }}
              className="border border-slate-200 rounded-md px-2 py-1.5 text-sm w-20 focus:outline-none focus:ring-2 focus:ring-indigo-200"
            />
            <span className="text-[11px] text-slate-400">
              1-based. Default 1 = first column. The cell's text becomes the drill value when a downstream widget drills from this one.
            </span>
          </div>
        </label>
      )}

      <label className="flex flex-col gap-1 text-xs mb-3">
        <span className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Span (flex weight when in a row)</span>
        <input
          type="number"
          min={1}
          value={widget.span ?? 1}
          onChange={(e) => {
            const n = Math.max(1, Number(e.target.value) || 1);
            onChange((w) => ({ ...w, span: n }));
          }}
          className="border border-slate-200 rounded-md px-2 py-1.5 text-sm w-24 focus:outline-none focus:ring-2 focus:ring-indigo-200"
        />
      </label>

      {/* Drill / open-behavior editor. Two modes:
            - widget.drill === undefined  → "Open behavior" toggle
              (auto_run_on_open) + an "Add drill" button if there's
              at least one other widget that could be the parent.
            - widget.drill !== undefined  → fully editable drill
              spec: pick the parent (`from`), pick which placeholder
              this widget receives (`set`), toggle auto-cascade, or
              remove the drill entirely. */}
      <DrillPanel widget={widget} siblings={siblings} placeholders={placeholders} onChange={onChange} />

      <div className="flex items-center gap-2">
        <button
          onClick={onRun}
          disabled={running || !canRun}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-sm bg-gradient-to-br from-indigo-500 to-blue-600 text-white hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
        >
          {running ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Save className="w-3.5 h-3.5" />}
          Save & run
        </button>
        <span className="text-[11px] text-slate-400">
          {isComponentBacked && !canRun
            ? "Fill in every placeholder to enable run."
            : "Saves the dashboard, then runs this widget to populate the preview."}
        </span>
      </div>
    </div>
  );
}

/**
 * The fields shown in the props drawer when a widget is bound to a
 * component: read-only template view + per-placeholder inputs + a
 * resolved-prompt preview. The kind picker is hidden (component owns it).
 */
/**
 * Editable drill section inside the props drawer.
 *
 *  - When `widget.drill` is unset and there's at least one sibling
 *    widget to drill from, show an "Add drill" affordance. Above it
 *    sits the simpler "Auto-run on open" toggle (since this is a
 *    candidate top-level widget).
 *  - When `widget.drill` is set, show editable controls:
 *      • dropdown picking which sibling widget is the `from`
 *      • text input for the placeholder name (`set`)
 *      • auto-cascade checkbox (existing behavior)
 *      • "Remove drill" button to convert this back into a top-level
 *        widget.
 */
function DrillPanel({
  widget, siblings, placeholders, onChange,
}: {
  widget: WidgetNode;
  siblings: WidgetNode[];
  /** Placeholder names parsed from this widget's prompt (or the
   *  bound component's template). Drill `set` MUST match one of
   *  these to have any effect — the click value gets written to
   *  `placeholder_values[set]` and substituted at run time. */
  placeholders: string[];
  onChange: (mut: (w: WidgetNode) => WidgetNode) => void;
}) {
  const hasDrill = !!widget.drill;
  const canDrill = siblings.length > 0;

  const startDrill = () => {
    const first = siblings[0];
    if (!first) return;
    // Default `set` to the first placeholder found in the prompt /
    // template, so the chain works without manual config. Empty
    // string when no placeholder exists yet — the user has to add
    // a `{{name}}` token to the prompt before the drill is useful.
    const defaultSet = placeholders[0] ?? "";
    onChange((w) => ({
      ...w,
      drill: { from: first.id, set: defaultSet, auto: true },
      // Drill children don't auto-run on open — the cascade fills
      // them in. Clear any prior open flag so the two modes don't
      // conflict.
      auto_run_on_open: false,
    }));
  };

  const removeDrill = () => {
    onChange((w) => {
      const next = { ...w };
      delete next.drill;
      return next;
    });
  };

  if (!hasDrill) {
    return (
      <div className="mb-3 border border-slate-200 rounded-md bg-slate-50/60 px-3 py-2">
        <div className="flex items-center justify-between mb-1.5">
          <div className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Open behavior</div>
          {canDrill && (
            <button
              type="button"
              onClick={startDrill}
              className="text-[11px] text-indigo-600 hover:text-indigo-800 inline-flex items-center gap-1"
              title="Make this widget cascade from another widget's click"
            >
              <Plus className="w-3 h-3" /> Add drill
            </button>
          )}
        </div>
        <label className="flex items-center gap-2 text-xs cursor-pointer">
          <input
            type="checkbox"
            checked={widget.auto_run_on_open === true}
            onChange={(e) => {
              const checked = e.target.checked;
              onChange((w) => ({ ...w, auto_run_on_open: checked }));
            }}
            className="accent-indigo-600"
          />
          <span className="text-slate-700">
            Auto-run on open
            <span className="ml-2 text-[11px] text-slate-400">
              {widget.auto_run_on_open === true
                ? "fires the prompt as soon as the dashboard loads"
                : "manual only — user clicks ↻ to run"}
            </span>
          </span>
        </label>
        {!canDrill && (
          <div className="mt-1.5 text-[10.5px] text-slate-400">
            Add at least one other widget to enable drill-down.
          </div>
        )}
      </div>
    );
  }

  // Editable drill section.
  const drill = widget.drill!;
  return (
    <div className="mb-3 border border-slate-200 rounded-md bg-slate-50/60 px-3 py-2">
      <div className="flex items-center justify-between mb-1.5">
        <div className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Drill</div>
        <button
          type="button"
          onClick={removeDrill}
          className="text-[11px] text-slate-500 hover:text-rose-600 inline-flex items-center gap-1"
          title="Remove the drill binding"
        >
          <Trash2 className="w-3 h-3" /> Remove
        </button>
      </div>

      <div className="grid grid-cols-2 gap-2 mb-2">
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-slate-500 text-[10px] uppercase tracking-wider">From</span>
          <select
            value={drill.from}
            onChange={(e) => {
              const next = e.target.value;
              onChange((w) => ({ ...w, drill: w.drill ? { ...w.drill, from: next } : w.drill }));
            }}
            className="border border-slate-200 rounded-md px-2 py-1 text-xs bg-white font-mono"
          >
            {siblings.map((s) => (
              <option key={s.id} value={s.id}>
                {s.kind} · {s.title || s.id}
              </option>
            ))}
            {/* If `from` was set to a widget that has since been
                deleted, show it as a dangling option so the user can
                see the broken state and re-pick. */}
            {!siblings.find((s) => s.id === drill.from) && (
              <option value={drill.from}>{drill.from} (missing)</option>
            )}
          </select>
        </label>

        <label className="flex flex-col gap-1 text-xs">
          <span className="text-slate-500 text-[10px] uppercase tracking-wider">Set placeholder</span>
          {placeholders.length > 0 ? (
            <select
              value={drill.set}
              onChange={(e) => {
                const next = e.target.value;
                onChange((w) => ({ ...w, drill: w.drill ? { ...w.drill, set: next } : w.drill }));
              }}
              className="border border-slate-200 rounded-md px-2 py-1 text-xs bg-white font-mono"
            >
              {/* Drill `set` must match a `{{name}}` token in the
                  prompt body. Offering only the parsed placeholders
                  (plus a "(missing)" fallback when the saved value
                  isn't one) prevents typos that silently disable
                  the cascade. */}
              {placeholders.map((p) => (
                <option key={p} value={p}>{`{{${p}}}`}</option>
              ))}
              {drill.set && !placeholders.includes(drill.set) && (
                <option value={drill.set}>{`{{${drill.set}}} (not in prompt)`}</option>
              )}
            </select>
          ) : (
            <div className="text-[11px] text-slate-500 italic">
              Add a <code className="font-mono bg-slate-100 px-1 rounded">{"{{name}}"}</code> token
              to the prompt above first.
            </div>
          )}
        </label>
      </div>

      <label className="flex items-center gap-2 text-xs cursor-pointer">
        <input
          type="checkbox"
          checked={drill.auto !== false}
          onChange={(e) => {
            const checked = e.target.checked;
            onChange((w) => ({
              ...w,
              drill: w.drill ? { ...w.drill, auto: checked } : w.drill,
            }));
          }}
          className="accent-indigo-600"
        />
        <span className="text-slate-700">
          Auto-cascade
          <span className="ml-2 text-[11px] text-slate-400">
            {drill.auto !== false
              ? "fills from parent's first item on load + after each parent run"
              : "manual only — user must click a parent item"}
          </span>
        </span>
      </label>
    </div>
  );
}

function ComponentBackedFields(props: {
  widget: WidgetNode;
  component: Component | null;
  placeholders: string[];
  resolvedPreview: string;
  onChange: (mut: (w: WidgetNode) => WidgetNode) => void;
}) {
  const { widget, component, placeholders, resolvedPreview, onChange } = props;
  return (
    <div className="mb-3">
      {component ? (
        <div className="mb-3">
          <div className="text-slate-500 uppercase tracking-wider text-[10px] font-medium mb-1">
            Component &mdash; <span className="font-mono normal-case tracking-normal text-slate-700">{component.name}</span>
          </div>
          <pre className="font-mono text-[11px] text-slate-600 bg-slate-50 rounded px-2 py-1.5 whitespace-pre-wrap break-words max-h-28 overflow-y-auto">
            {component.prompt_template}
          </pre>
        </div>
      ) : (
        <div className="mb-3 text-xs text-rose-600">
          Bound component is no longer available. Delete this widget or rebind it from the toolbar.
        </div>
      )}

      {placeholders.length > 0 ? (
        <div className="mb-3 space-y-2">
          <div className="text-slate-500 uppercase tracking-wider text-[10px] font-medium">Placeholder values</div>
          {placeholders.map((name) => (
            <label key={name} className="flex items-center gap-2 text-xs">
              <code className="font-mono bg-indigo-50 text-indigo-700 px-1.5 py-0.5 rounded text-[11px] flex-shrink-0">
                {`{{${name}}}`}
              </code>
              <input
                value={(widget.placeholder_values ?? {})[name] ?? ""}
                onChange={(e) => onChange((w) => ({
                  ...w,
                  placeholder_values: { ...(w.placeholder_values ?? {}), [name]: e.target.value },
                }))}
                placeholder={`value for ${name}`}
                className="flex-1 border border-slate-200 rounded-md px-2 py-1.5 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200"
              />
            </label>
          ))}
        </div>
      ) : (
        <div className="mb-3 text-xs text-slate-400">
          This component has no placeholders — it'll run with the template as-is.
        </div>
      )}

      <div>
        <div className="text-slate-500 uppercase tracking-wider text-[10px] font-medium mb-1">Resolved prompt preview</div>
        <pre className="font-mono text-[11px] text-slate-700 bg-indigo-50/40 border border-indigo-100 rounded px-2 py-1.5 whitespace-pre-wrap break-words max-h-28 overflow-y-auto">
          {resolvedPreview}
        </pre>
      </div>
    </div>
  );
}

/**
 * Toolbar button for a layout template. Shows a tiny SVG mock of the
 * cells so the user can recognize the shape without reading the label.
 * Stamping the template appends its rows to the currently selected
 * container (or the root column when nothing is selected).
 */
function TemplateButton({
  id, label, onClick,
}: {
  id: TemplateId;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={`Stamp ${label} into the current container`}
      className="px-2 py-1 rounded hover:bg-slate-100 inline-flex items-center gap-1.5 border border-transparent hover:border-slate-200 transition"
    >
      <TemplateIcon id={id} />
      <span>{label}</span>
    </button>
  );
}

/** 36×20 SVG mock of the template's cell grid. */
function TemplateIcon({ id }: { id: TemplateId }) {
  const W = 36;
  const H = 20;
  const STROKE = "#94a3b8";
  const FILL   = "#e0e7ff";
  // Each entry = list of rows; each row = list of cell widths (flex).
  const ROWS: Record<TemplateId, number[][]> = {
    "2x2":     [[1, 1], [1, 1]],
    "2x3":     [[1, 1, 1], [1, 1, 1]],
    "1plus3":  [[1], [1, 1, 1]],
    "3plus1":  [[1, 1, 1], [1]],
    "1plus2":  [[1], [1, 1]],
    "kpi-row": [[1, 1, 1, 1]],
  };
  const rows = ROWS[id];
  const rowH = H / rows.length;
  return (
    <svg width={W} height={H} className="flex-shrink-0">
      {rows.map((row, rIdx) => {
        const total = row.reduce((s, w) => s + w, 0);
        let x = 0;
        return row.map((w, cIdx) => {
          const cellW = (w / total) * W;
          const rect = (
            <rect
              key={`${rIdx}-${cIdx}`}
              x={x + 0.75}
              y={rIdx * rowH + 0.75}
              width={cellW - 1.5}
              height={rowH - 1.5}
              fill={FILL}
              stroke={STROKE}
              strokeWidth={0.75}
              rx={1.5}
            />
          );
          x += cellW;
          return rect;
        });
      })}
    </svg>
  );
}

function parseLayout(detail: DashboardDetail): DashboardLayout {
  const raw = detail.layout_json;
  if (typeof raw === "string") {
    try { return JSON.parse(raw) as DashboardLayout; } catch { return EMPTY_LAYOUT; }
  }
  if (raw && typeof raw === "object") return raw as DashboardLayout;
  return EMPTY_LAYOUT;
}
