// Pure functions over the dashboard composition tree. Keep mutations
// here so the React layer is just dumb dispatch — easier to reason about
// + trivially testable without React.

import type { DashboardLayout, RowNode, ColumnNode, TreeNode, WidgetKind, WidgetNode } from "./types";

// ── id generation ────────────────────────────────────────────────────────

/** Short stable id for new nodes. Not cryptographically random — the
 *  guarantee we need is uniqueness within one dashboard's tree. */
export function makeId(prefix: "n" | "w" = "n"): string {
  const tail = Math.random().toString(36).slice(2, 9);
  return `${prefix}_${tail}`;
}

// ── factories ────────────────────────────────────────────────────────────

export function makeRow(): RowNode {
  return { type: "row", id: makeId(), children: [] };
}

export function makeColumn(): ColumnNode {
  return { type: "column", id: makeId(), children: [] };
}

export function makeWidget(kind: WidgetKind = "kpi"): WidgetNode {
  return {
    type: "widget",
    id: makeId("w"),
    kind,
    title: defaultTitle(kind),
    prompt: "",
  };
}

// ── Layout templates ─────────────────────────────────────────────────────

/** Known layout templates. Add new ids here AND in `makeTemplateRows`. */
export type TemplateId =
  | "2x2"      // two rows × two widgets
  | "2x3"      // two rows × three widgets
  | "1plus3"   // one full-width row + three side-by-side
  | "3plus1"   // three side-by-side + one full-width
  | "1plus2"   // one full-width + two side-by-side
  | "kpi-row"; // single row of four small KPIs

/** Return one or more rows that compose the template. Caller appends
 *  these as children of whichever container the user has selected (or
 *  the root column when nothing is selected). Widgets default to `kpi`
 *  with empty prompts — the user fills them in after stamping. */
export function makeTemplateRows(template: TemplateId): RowNode[] {
  switch (template) {
    case "2x2":     return [row(widgets(2)), row(widgets(2))];
    case "2x3":     return [row(widgets(3)), row(widgets(3))];
    case "1plus3":  return [row(widgets(1)), row(widgets(3))];
    case "3plus1":  return [row(widgets(3)), row(widgets(1))];
    case "1plus2":  return [row(widgets(1)), row(widgets(2))];
    case "kpi-row": return [row(widgets(4, "kpi"))];
  }
}

function row(children: WidgetNode[]): RowNode {
  return { type: "row", id: makeId(), children };
}

function widgets(count: number, kind: WidgetKind = "kpi"): WidgetNode[] {
  return Array.from({ length: count }, () => makeWidget(kind));
}

function defaultTitle(kind: WidgetKind): string {
  switch (kind) {
    case "kpi":         return "New KPI";
    case "bar":         return "New bar chart";
    case "line":        return "New line chart";
    case "pie":         return "New pie chart";
    case "stacked_bar": return "New stacked bar";
    case "bullet":      return "New bullet chart";
    case "pareto":      return "New Pareto";
    case "funnel":      return "New funnel";
    case "gauge":       return "New gauge";
    case "sparkline":   return "New sparkline";
    case "heatmap":     return "New heatmap";
    case "treemap":     return "New treemap";
    case "histogram":   return "New histogram";
    case "slope":       return "New slope chart";
    case "boxplot":     return "New box plot";
    case "waterfall":   return "New waterfall";
    case "table":       return "New table";
    case "text":        return "New note";
  }
}

// ── traversal ────────────────────────────────────────────────────────────

export function findNode(tree: DashboardLayout, id: string): TreeNode | null {
  return findRec(tree.root, id);
}

function findRec(node: TreeNode, id: string): TreeNode | null {
  if (node.id === id) return node;
  if (node.type === "row" || node.type === "column") {
    for (const c of node.children) {
      const found = findRec(c, id);
      if (found) return found;
    }
  }
  return null;
}

/** Find the parent container of `id` (or null if `id` is the root). */
export function findParent(tree: DashboardLayout, id: string): RowNode | ColumnNode | null {
  if (tree.root.id === id) return null;
  return findParentRec(tree.root, id);
}

function findParentRec(node: TreeNode, id: string): RowNode | ColumnNode | null {
  if (node.type !== "row" && node.type !== "column") return null;
  for (const c of node.children) {
    if (c.id === id) return node;
    const inside = findParentRec(c, id);
    if (inside) return inside;
  }
  return null;
}

export function collectWidgets(tree: DashboardLayout): WidgetNode[] {
  const out: WidgetNode[] = [];
  walk(tree.root, (n) => {
    if (n.type === "widget") out.push(n);
  });
  return out;
}

function walk(node: TreeNode, visit: (n: TreeNode) => void): void {
  visit(node);
  if (node.type === "row" || node.type === "column") {
    for (const c of node.children) walk(c, visit);
  }
}

// ── mutations (return a NEW tree; never mutate in place) ─────────────────

/** Replace the subtree at `id` with `replacement`. Returns a new tree. */
export function replaceNode(tree: DashboardLayout, id: string, replacement: TreeNode): DashboardLayout {
  return { ...tree, root: replaceRec(tree.root, id, replacement) };
}

function replaceRec(node: TreeNode, id: string, repl: TreeNode): TreeNode {
  if (node.id === id) return repl;
  if (node.type === "row" || node.type === "column") {
    const next = node.children.map((c) => replaceRec(c, id, repl));
    return { ...node, children: next };
  }
  return node;
}

/** Append `child` to the container at `parentId`. No-op if `parentId`
 *  doesn't exist or isn't a container. */
export function addChild(tree: DashboardLayout, parentId: string, child: TreeNode): DashboardLayout {
  return { ...tree, root: addChildRec(tree.root, parentId, child) };
}

function addChildRec(node: TreeNode, parentId: string, child: TreeNode): TreeNode {
  if (node.id === parentId && (node.type === "row" || node.type === "column")) {
    return { ...node, children: [...node.children, child] };
  }
  if (node.type === "row" || node.type === "column") {
    return { ...node, children: node.children.map((c) => addChildRec(c, parentId, child)) };
  }
  return node;
}

/** Remove the node with id `target`. No-op for the root. */
export function removeNode(tree: DashboardLayout, target: string): DashboardLayout {
  if (tree.root.id === target) return tree;
  return { ...tree, root: removeRec(tree.root, target) };
}

function removeRec(node: TreeNode, target: string): TreeNode {
  if (node.type !== "row" && node.type !== "column") return node;
  return {
    ...node,
    children: node.children
      .filter((c) => c.id !== target)
      .map((c) => removeRec(c, target)),
  };
}

/** Move a node up (-1) or down (+1) within its parent's child list.
 *  Clamps at the ends. */
export function moveNode(tree: DashboardLayout, target: string, direction: -1 | 1): DashboardLayout {
  const parent = findParent(tree, target);
  if (!parent) return tree;
  const idx = parent.children.findIndex((c) => c.id === target);
  if (idx < 0) return tree;
  const newIdx = Math.max(0, Math.min(parent.children.length - 1, idx + direction));
  if (newIdx === idx) return tree;
  const next = parent.children.slice();
  [next[idx], next[newIdx]] = [next[newIdx], next[idx]];
  return replaceNode(tree, parent.id, { ...parent, children: next });
}
