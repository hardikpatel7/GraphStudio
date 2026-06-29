import { useEffect, useState } from "react";
import { ChevronRight, Loader2 } from "lucide-react";
import { useWorkspaceStore } from "@/stores/workspace";
import { RclTrace } from "@/components/RclTrace";
import { api, type GraphTraverseEdge, type GraphTraverseKind } from "@/api/client";

/// Discriminated payload shapes the inspector knows how to render.
interface RclTracePayload {
  kind: "rcl_trace";
  product_code?: string;
  article?: string;
}

interface GraphTraversePayload {
  kind: "graph_traverse";
  from: { kind: GraphTraverseKind; name: string };
  edge: GraphTraverseEdge;
}

function isRclTracePayload(v: any): v is RclTracePayload {
  return v && typeof v === "object" && v.kind === "rcl_trace";
}
function isGraphTraversePayload(v: any): v is GraphTraversePayload {
  return v && typeof v === "object" && v.kind === "graph_traverse";
}

export function InspectorPanel() {
  const inspectorPayload = useWorkspaceStore((s) => s.inspectorPayload);

  const isRcl = isRclTracePayload(inspectorPayload);
  const isTraverse = isGraphTraversePayload(inspectorPayload);

  return (
    <div className="h-full overflow-y-auto p-4 bg-gray-900">
      {/* Title only — close lives on the outer panel header so we don't
          stack two X buttons. */}
      <h2 className="text-sm font-semibold text-gray-100 mb-4">
        {isRcl ? "RCL trace" : isTraverse ? "Graph traversal" : "Inspector"}
      </h2>
      {isRcl ? (
        <RclTrace
          productCode={inspectorPayload.product_code}
          article={inspectorPayload.article}
        />
      ) : isTraverse ? (
        <GraphTraverseView
          from={inspectorPayload.from}
          edge={inspectorPayload.edge}
        />
      ) : (
        <pre className="text-xs text-gray-300 whitespace-pre-wrap break-all">
          {JSON.stringify(inspectorPayload, null, 2)}
        </pre>
      )}
    </div>
  );
}

/// Mini-table for `traverse(from, edge) → rows`. Each row rendered
/// here is itself traversable: clicking a graph-link cell drives
/// `setStep(...)` to walk further. Breadcrumbs at the top let you
/// jump back to any prior step.
function GraphTraverseView({
  from,
  edge,
}: {
  from: { kind: GraphTraverseKind; name: string };
  edge: GraphTraverseEdge;
}) {
  type Step = { from: { kind: GraphTraverseKind; name: string }; edge: GraphTraverseEdge };
  const initial: Step = { from, edge };
  const [stack, setStack] = useState<Step[]>([initial]);
  const [rows, setRows] = useState<Record<string, any>[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reset the stack when the inspector is opened with a new payload.
  useEffect(() => {
    setStack([{ from, edge }]);
  }, [from.kind, from.name, edge]);

  // Fetch whenever the top step changes.
  useEffect(() => {
    const top = stack[stack.length - 1];
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .graphTraverse(top.from, top.edge)
      .then((resp) => {
        if (!cancelled) setRows(resp.rows ?? []);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [stack]);

  const top = stack[stack.length - 1];
  const cols = rows[0] ? Object.keys(rows[0]) : [];

  // Drive the next traversal when a child cell is clicked.
  const drillInto = (kind: GraphTraverseKind, name: string) => {
    const edge = defaultEdgeForKind(kind);
    setStack((s) => [...s, { from: { kind, name }, edge }]);
  };

  return (
    <div className="space-y-3">
      {/* Breadcrumbs */}
      <div className="flex items-center gap-1 text-[11px] flex-wrap">
        {stack.map((step, i) => (
          <span key={i} className="flex items-center gap-1">
            <button
              onClick={() => setStack(stack.slice(0, i + 1))}
              disabled={i === stack.length - 1}
              className={`font-mono ${
                i === stack.length - 1
                  ? "text-gray-200"
                  : "text-blue-400 hover:text-blue-300 hover:underline"
              } disabled:cursor-default`}
              title={`${step.from.kind} ${step.edge}`}
            >
              {step.from.kind.toLowerCase()}={step.from.name}
            </button>
            <span className="text-gray-600">→</span>
            <span className="text-gray-400 italic">{step.edge}</span>
            {i < stack.length - 1 && <ChevronRight size={11} className="text-gray-700" />}
          </span>
        ))}
      </div>

      {loading && (
        <div className="flex items-center gap-2 text-xs text-gray-500">
          <Loader2 size={12} className="animate-spin" />
          Traversing…
        </div>
      )}

      {error && (
        <div className="rounded bg-red-900/30 border border-red-800 px-3 py-2 text-xs text-red-300 font-mono">
          {error}
        </div>
      )}

      {!loading && !error && rows.length === 0 && (
        <div className="text-xs text-gray-500 italic">
          No results from <span className="font-mono">{top.from.kind}={top.from.name}</span> →
          <span className="font-mono"> {top.edge}</span>.
        </div>
      )}

      {!loading && rows.length > 0 && (
        <div className="border border-gray-800 rounded overflow-x-auto">
          <table className="w-full text-xs">
            <thead className="bg-gray-950">
              <tr>
                {cols.map((c) => (
                  <th
                    key={c}
                    className="text-left px-2 py-1.5 font-medium text-gray-500 uppercase text-[10px] tracking-wider"
                  >
                    {c}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => (
                <tr key={i} className="border-t border-gray-800/50 hover:bg-gray-800/30">
                  {cols.map((c) => {
                    const v = row[c];
                    const colKey = c.toLowerCase();
                    const drillKind = INSPECTOR_LINK_COLUMNS[colKey];
                    const renderable = v != null && v !== "";
                    if (drillKind && renderable) {
                      return (
                        <td key={c} className="px-2 py-1 font-mono">
                          <button
                            onClick={() => drillInto(drillKind, String(v))}
                            className="text-blue-400 hover:text-blue-300 hover:underline cursor-pointer"
                            title={`Traverse ${drillKind}=${v} → ${defaultEdgeForKind(drillKind)}`}
                          >
                            {String(v)}
                          </button>
                        </td>
                      );
                    }
                    return (
                      <td key={c} className="px-2 py-1 text-gray-300 font-mono whitespace-nowrap">
                        {v == null || v === "" ? <span className="text-gray-600">—</span> : String(v)}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/// Same column→kind mapping as the DataView preview. Click semantics
/// in the inspector mirror the outer table.
const INSPECTOR_LINK_COLUMNS: Record<string, GraphTraverseKind> = {
  l0_name: "L0",
  l1_name: "L1",
  l2_name: "L2",
  l3_name: "L3",
  l4_name: "L4",
  l5_name: "L5",
  article: "ARTICLE",
  product_code: "PRODUCT_CODE",
  channel: "CHANNEL",
  store_code: "STORE_CODE",
  brand: "BRAND",
  // hierarchy projection rows surface "name" + "level"; clicking name
  // doesn't tell us which kind. Skip for now; users can drill via
  // l1_name/l2_name etc. from article rows.
};

function defaultEdgeForKind(kind: GraphTraverseKind): GraphTraverseEdge {
  switch (kind) {
    case "L0":
    case "L1":
    case "L2":
    case "L3":
    case "L4":
    case "L5":
      return "children";
    case "ARTICLE":
      return "children";
    case "PRODUCT_CODE":
      return "parent";
    case "BRAND":
      return "articles";
    case "CHANNEL":
      return "stores";
    case "STORE_CODE":
      return "parent";
  }
}
