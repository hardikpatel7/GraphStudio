import { useEffect, useState } from "react";
import {
  Search,
  ChevronUp,
  ChevronsUp,
  ChevronDown,
  Loader2,
  ArrowRight,
  CornerDownRight,
  CircleAlert,
  Plus,
} from "lucide-react";

// Shapes from /api/graphs/:id/stats and /traverse.
interface StatsLite {
  kinds: { name: string; hierarchy: string; node_count: number }[];
  cross_edges?: { alias: string; kind_a: string; kind_b: string }[];
}

interface TraverseRow {
  id: number;
  kind: string;
  name: string;
  ancestors?: Record<string, string>;
  metrics?: Record<string, unknown>;
}

interface TraverseResponse {
  id: string;
  from: { kind: string; name: string };
  rows: TraverseRow[];
  total: number;
  offset: number;
  limit: number;
}

/// One paginated edge result. `total` is the full subtree size; we
/// page through it 50 at a time.
interface PagedList {
  rows: TraverseRow[];
  total: number;
  loadingMore: boolean;
}

interface Props {
  graphId: string;
  stats: StatsLite | null;
}

type Edge =
  | { kind: "children" }
  | { kind: "parent" }
  | { kind: "ancestors" }
  | { kind: "descendants_of_kind"; of: string }
  | { kind: "cross_edge"; alias: string };

function edgeToBody(e: Edge): unknown {
  switch (e.kind) {
    case "children":
    case "parent":
    case "ancestors":
      return e.kind;
    case "descendants_of_kind":
      return { descendants_of_kind: e.of };
    case "cross_edge":
      return { cross_edge: e.alias };
  }
}

/// One node "focused" in the explore view. We re-fetch when the user
/// drills somewhere — each drill replaces the focus with the new
/// node.
interface Focus {
  kind: string;
  name: string;
}

const PAGE_SIZE = 50;

/// Drill into the live v2 graph snapshot. Renders breadcrumbs +
/// neighbor lists for the currently-focused node. Children + each
/// cross-edge list paginate on demand — initial fetch is PAGE_SIZE
/// rows; "Load more" requests the next page (server-side slice, so
/// large subtrees don't ship a megabyte of JSON on the first call).
export function ExplorePane({ graphId, stats }: Props) {
  const firstKind = stats?.kinds.find((k) => k.name !== "__root__")?.name ?? "";
  const [seedKind, setSeedKind] = useState<string>(firstKind);
  const [seedName, setSeedName] = useState<string>("");

  const [focus, setFocus] = useState<Focus | null>(null);
  const [focusRow, setFocusRow] = useState<TraverseRow | null>(null);
  const [children, setChildren] = useState<PagedList | null>(null);
  const [parents, setParents] = useState<TraverseRow[]>([]); // 0 or 1, no paging
  const [ancestors, setAncestors] = useState<TraverseRow[]>([]); // bounded by spine depth
  const [crossEdgeResults, setCrossEdgeResults] = useState<Record<string, PagedList>>(
    {},
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!seedKind && firstKind) setSeedKind(firstKind);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [firstKind]);

  /// One /traverse round-trip. Accepts optional limit/offset; the
  /// server defaults to limit=1000 when omitted, but we always send
  /// PAGE_SIZE so the response stays predictable.
  const traverse = async (
    from: Focus,
    edge: Edge,
    opts: { limit?: number; offset?: number } = {},
  ): Promise<TraverseResponse> => {
    const r = await fetch(
      `/api/graphs/${encodeURIComponent(graphId)}/traverse`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          from,
          edge: edgeToBody(edge),
          project: { include_ancestors: true, include_metrics: true },
          limit: opts.limit,
          offset: opts.offset,
        }),
      },
    );
    if (!r.ok) throw new Error(await r.text());
    return r.json();
  };

  /// Fetch just the focused node — `/traverse` with edge=ancestors
  /// skips the node itself by design (matches v1 traversal
  /// semantics), so we need a dedicated endpoint to get the focus
  /// row's metrics + ancestors filled in.
  const fetchFocusRow = async (next: Focus): Promise<TraverseRow | null> => {
    const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}/node`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        from: next,
        project: { include_ancestors: true, include_metrics: true },
      }),
    });
    if (!r.ok) return null;
    const data = await r.json();
    return data.row ?? null;
  };

  /// Fetch the full neighbor set for `next`. Children + cross-edges
  /// fetch only the first page; parent + ancestors fetch
  /// everything (always small). Also fetch the focus row itself so
  /// the header can show its metrics.
  const refocus = async (next: Focus) => {
    setLoading(true);
    setError(null);
    setFocusRow(null);
    setChildren(null);
    setParents([]);
    setAncestors([]);
    setCrossEdgeResults({});
    setFocus(next);
    try {
      const [fRow, cResp, pResp, aResp] = await Promise.all([
        fetchFocusRow(next).catch(() => null),
        traverse(next, { kind: "children" }, { limit: PAGE_SIZE, offset: 0 }).catch(
          () => null,
        ),
        // Parent: at most 1 row; no paging needed.
        traverse(next, { kind: "parent" }, { limit: PAGE_SIZE, offset: 0 }).catch(
          () => null,
        ),
        // Ancestors: bounded by spine depth (~10 for bealls); send a
        // generous limit so we never paginate them.
        traverse(next, { kind: "ancestors" }, { limit: 100, offset: 0 }).catch(
          () => null,
        ),
      ]);
      setFocusRow(fRow);
      setChildren(
        cResp ? { rows: cResp.rows, total: cResp.total, loadingMore: false } : null,
      );
      setParents(pResp?.rows ?? []);
      setAncestors(aResp?.rows ?? []);
      // Cross-edge first pages — only fetch those whose endpoints
      // include the focused kind (others guaranteed to return zero).
      const relevant = (stats?.cross_edges ?? []).filter(
        (ce) => ce.kind_a === next.kind || ce.kind_b === next.kind,
      );
      const crossResults: Record<string, PagedList> = {};
      await Promise.all(
        relevant.map(async (ce) => {
          const resp = await traverse(
            next,
            { kind: "cross_edge", alias: ce.alias },
            { limit: PAGE_SIZE, offset: 0 },
          ).catch(() => null);
          if (resp && resp.total > 0) {
            crossResults[ce.alias] = {
              rows: resp.rows,
              total: resp.total,
              loadingMore: false,
            };
          }
        }),
      );
      setCrossEdgeResults(crossResults);
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setLoading(false);
    }
  };

  /// Fetch the next page for a paginated list and append the rows.
  const loadMoreChildren = async () => {
    if (!focus || !children || children.loadingMore) return;
    setChildren({ ...children, loadingMore: true });
    try {
      const resp = await traverse(
        focus,
        { kind: "children" },
        { limit: PAGE_SIZE, offset: children.rows.length },
      );
      setChildren({
        rows: [...children.rows, ...resp.rows],
        total: resp.total,
        loadingMore: false,
      });
    } catch (e: any) {
      setError(e?.message ?? String(e));
      setChildren({ ...children, loadingMore: false });
    }
  };

  const loadMoreCrossEdge = async (alias: string) => {
    if (!focus) return;
    const cur = crossEdgeResults[alias];
    if (!cur || cur.loadingMore) return;
    setCrossEdgeResults({ ...crossEdgeResults, [alias]: { ...cur, loadingMore: true } });
    try {
      const resp = await traverse(
        focus,
        { kind: "cross_edge", alias },
        { limit: PAGE_SIZE, offset: cur.rows.length },
      );
      setCrossEdgeResults({
        ...crossEdgeResults,
        [alias]: {
          rows: [...cur.rows, ...resp.rows],
          total: resp.total,
          loadingMore: false,
        },
      });
    } catch (e: any) {
      setError(e?.message ?? String(e));
      setCrossEdgeResults({
        ...crossEdgeResults,
        [alias]: { ...cur, loadingMore: false },
      });
    }
  };

  const start = () => {
    const name = seedName.trim();
    if (!seedKind || !name) return;
    void refocus({ kind: seedKind, name });
  };

  if (!stats) {
    return (
      <div className="px-3 py-4 text-[11px] text-gray-500 space-y-1.5">
        <div className="flex items-start gap-1.5">
          <CircleAlert size={12} className="mt-0.5 shrink-0 text-amber-400" />
          <span>
            Graph isn't built yet. Click <span className="text-gray-300">Build</span> at the
            top to materialize the snapshot, then come back here to traverse it.
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="p-3 space-y-3">
      {/* Seed form */}
      <section className="rounded border border-gray-800 bg-gray-900/30 p-2.5 space-y-1.5">
        <div className="text-[10px] uppercase tracking-wider text-gray-500">Start at</div>
        <div className="flex items-center gap-1.5">
          <select
            value={seedKind}
            onChange={(e) => setSeedKind(e.target.value)}
            className="text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
          >
            {stats.kinds
              .filter((k) => k.name !== "__root__")
              .map((k) => (
                <option key={k.name} value={k.name}>
                  {k.name} ({k.node_count.toLocaleString()})
                </option>
              ))}
          </select>
          <input
            type="text"
            placeholder="Node name (e.g., 30-bls or A1234)"
            value={seedName}
            onChange={(e) => setSeedName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") start();
            }}
            className="flex-1 min-w-0 text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500"
          />
          <button
            onClick={start}
            disabled={!seedKind || !seedName.trim() || loading}
            className="flex items-center gap-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
          >
            {loading ? <Loader2 size={11} className="animate-spin" /> : <Search size={11} />}
            Go
          </button>
        </div>
      </section>

      {error && (
        <div className="rounded border border-red-900/60 bg-red-950/30 p-2.5 text-[11px] text-red-300">
          <pre className="font-mono whitespace-pre-wrap break-words">{error}</pre>
        </div>
      )}

      {focus && (
        <>
          {/* Breadcrumb */}
          {ancestors.length > 0 && (
            <section className="text-[11px] text-gray-400">
              {[...ancestors].reverse().map((a, i) => (
                <span key={`${a.kind}.${a.name}`}>
                  <button
                    className="hover:text-gray-200 hover:underline"
                    onClick={() => refocus({ kind: a.kind, name: a.name })}
                  >
                    <span className="text-gray-500 font-mono">{a.kind}:</span>{" "}
                    <span>{a.name}</span>
                  </button>
                  {i < ancestors.length - 1 && (
                    <ArrowRight size={10} className="inline mx-1 text-gray-600" />
                  )}
                </span>
              ))}
              {ancestors.length > 0 && (
                <ArrowRight size={10} className="inline mx-1 text-gray-600" />
              )}
              <span className="text-gray-200">
                <span className="text-gray-500 font-mono">{focus.kind}:</span> {focus.name}
              </span>
            </section>
          )}

          {/* Focus header */}
          <section className="rounded border border-blue-900/60 bg-blue-950/20 p-2.5 space-y-2">
            <div className="flex items-baseline gap-2">
              <span className="text-[10px] uppercase tracking-wider text-blue-300">
                Focused
              </span>
              <span className="text-xs font-mono text-blue-200">{focus.kind}</span>
              <span className="text-sm text-gray-100 font-medium">{focus.name}</span>
            </div>

            {/* Rolled-up metrics for the focused node. At hierarchy
                ancestor levels these are the post-rollup totals
                across the subtree; at leaf levels they're the
                originally-attached values. Numbers are formatted with
                locale separators; non-finite values fall back to "—". */}
            {focusRow?.metrics && Object.keys(focusRow.metrics).length > 0 && (
              <div>
                <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
                  Metrics (rolled up)
                </div>
                <table className="w-full text-[11px]">
                  <tbody className="divide-y divide-blue-900/30">
                    {Object.entries(focusRow.metrics).map(([name, value]) => (
                      <tr key={name}>
                        <td className="py-0.5 font-mono text-gray-400">{name}</td>
                        <td className="py-0.5 text-right tabular-nums text-gray-100">
                          {formatMetric(value)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          {/* Parent */}
          {parents.length > 0 && (
            <RowList
              title="Parent"
              icon={<ChevronUp size={11} />}
              rows={parents}
              total={parents.length}
              onClick={(r) => refocus({ kind: r.kind, name: r.name })}
            />
          )}

          {/* Ancestors */}
          {ancestors.length > 0 && (
            <RowList
              title={`Ancestors (${ancestors.length})`}
              icon={<ChevronsUp size={11} />}
              rows={ancestors}
              total={ancestors.length}
              onClick={(r) => refocus({ kind: r.kind, name: r.name })}
            />
          )}

          {/* Children — paginated */}
          {children && children.total > 0 && (
            <RowList
              title={`Children (${children.rows.length.toLocaleString()} of ${children.total.toLocaleString()})`}
              icon={<ChevronDown size={11} />}
              rows={children.rows}
              total={children.total}
              loadingMore={children.loadingMore}
              onLoadMore={children.rows.length < children.total ? loadMoreChildren : undefined}
              onClick={(r) => refocus({ kind: r.kind, name: r.name })}
            />
          )}

          {/* Cross-edge neighbors — paginated per bridge */}
          {Object.entries(crossEdgeResults).map(([alias, pl]) => (
            <RowList
              key={alias}
              title={`${alias} (${pl.rows.length.toLocaleString()} of ${pl.total.toLocaleString()})`}
              icon={<CornerDownRight size={11} />}
              rows={pl.rows}
              total={pl.total}
              loadingMore={pl.loadingMore}
              onLoadMore={pl.rows.length < pl.total ? () => loadMoreCrossEdge(alias) : undefined}
              onClick={(r) => refocus({ kind: r.kind, name: r.name })}
            />
          ))}

          {!loading &&
            parents.length === 0 &&
            ancestors.length === 0 &&
            (!children || children.total === 0) &&
            Object.keys(crossEdgeResults).length === 0 && (
              <div className="text-[11px] text-gray-500 px-1">
                No neighbors found — the node might not exist for this kind, or it's a
                leaf with no children / cross-edges.
              </div>
            )}
        </>
      )}
    </div>
  );
}

// ─── Helpers ──────────────────────────────────────────────────────────────

function RowList({
  title,
  icon,
  rows,
  total,
  loadingMore,
  onLoadMore,
  onClick,
}: {
  title: string;
  icon: React.ReactNode;
  rows: TraverseRow[];
  total: number;
  loadingMore?: boolean;
  onLoadMore?: () => void;
  onClick: (r: TraverseRow) => void;
}) {
  return (
    <section className="rounded border border-gray-800 bg-gray-900/30">
      <div className="px-2.5 py-1 border-b border-gray-800 text-[10px] uppercase tracking-wider text-gray-500 flex items-center gap-1.5">
        {icon}
        <span>{title}</span>
      </div>
      <ul className="divide-y divide-gray-900/60 max-h-80 overflow-auto">
        {rows.map((r) => {
          const metric = r.metrics ? topMetric(r.metrics) : null;
          return (
            <li
              key={`${r.kind}.${r.id}`}
              className="px-2.5 py-1 hover:bg-gray-900/60 cursor-pointer flex items-baseline gap-2 text-[11px]"
              onClick={() => onClick(r)}
            >
              <span className="text-gray-500 font-mono w-16 shrink-0">{r.kind}</span>
              <span className="text-gray-200 truncate flex-1">{r.name}</span>
              {metric && (
                <span className="text-gray-500 font-mono tabular-nums whitespace-nowrap">
                  {metric.label}: {metric.value}
                </span>
              )}
            </li>
          );
        })}
        {/* "Load more" lives inside the scroll container so it stays
            anchored to the bottom of the list. Disabled while a
            previous fetch is in flight. */}
        {onLoadMore && (
          <li className="px-2.5 py-1 sticky bottom-0 bg-gray-900/80 backdrop-blur-sm">
            <button
              onClick={onLoadMore}
              disabled={loadingMore}
              className="w-full flex items-center justify-center gap-1.5 text-[11px] px-2 py-1 rounded bg-gray-800 hover:bg-gray-700 text-gray-300 disabled:opacity-50"
            >
              {loadingMore ? (
                <Loader2 size={11} className="animate-spin" />
              ) : (
                <Plus size={11} />
              )}
              Load {Math.min(50, total - rows.length).toLocaleString()} more
            </button>
          </li>
        )}
      </ul>
    </section>
  );
}

/// Format a metric value for display. Numbers get locale separators
/// (10852 → 10,852). Sets/lists serialize as arrays — render as a
/// comma-separated string. Null / NaN / inf collapse to "—".
function formatMetric(v: unknown): string {
  if (v == null) return "—";
  if (typeof v === "number") {
    return Number.isFinite(v) ? v.toLocaleString() : "—";
  }
  if (typeof v === "string") return v;
  if (typeof v === "boolean") return v ? "true" : "false";
  if (Array.isArray(v)) {
    if (v.length === 0) return "[]";
    if (v.length <= 5) return v.map((x) => String(x)).join(", ");
    return `${v.slice(0, 5).join(", ")} … (+${v.length - 5} more)`;
  }
  return JSON.stringify(v);
}

/// Pick a small representative metric for the row's right column.
function topMetric(m: Record<string, unknown>): { label: string; value: string } | null {
  const keys = Object.keys(m);
  if (keys.length === 0) return null;
  const preferred = ["inventory.oh", "inv.oh", "oh"];
  const pick = preferred.find((k) => k in m) ?? keys[0];
  const raw = m[pick];
  const num = typeof raw === "number" ? raw : Number(raw ?? 0);
  if (!Number.isFinite(num)) return null;
  return {
    label: pick.split(".").pop() ?? pick,
    value: num.toLocaleString(),
  };
}
