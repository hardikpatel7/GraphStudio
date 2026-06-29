import { useEffect, useMemo, useState } from "react";
import { MemoryStick, RefreshCw, Loader2 } from "lucide-react";

/// Memory stats explorer for the V8 in-memory ArticleGraph.
///
/// Hits POST /api/graph/articles/memory-stats and renders a heuristic
/// breakdown — per-section bytes plus a stacked bar so the dominant
/// contributors are obvious at a glance. Numbers are estimates; Rust's
/// stdlib doesn't expose live allocator telemetry per object, so we
/// sum mem::size_of × count plus rough overhead for HashMap buckets
/// and Arc<str> headers.

interface MemoryStats {
  graph_version: number;
  rule_pointers_version: number;
  duration_ms: number;
  node_struct_size_bytes: number;

  nodes: {
    by_kind: { kind: string; count: number; bytes_struct: number; bytes_heap: number; bytes_total: number }[];
    total_count: number;
    total_bytes: number;
  };
  strings: {
    count: number;
    total_chars: number;
    struct_bytes: number;
    heap_bytes: number;
    total_bytes: number;
  };
  by_kind_index: {
    kinds: number;
    entries: number;
    total_bytes: number;
  };
  cross_indices: {
    breakdown: { name: string; entries: number; value_total?: number; bytes_total: number }[];
    total_bytes: number;
  };
  psm: {
    priorities: number;
    rule_dim_entries: number;
    products_with_rcl_hash: number;
    inner_hash_entries_total: number;
    total_bytes: number;
  };
  grand_total_bytes: number;
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
function fmtCount(n: number): string {
  return n.toLocaleString();
}

const SECTION_COLORS: Record<string, string> = {
  Nodes: "bg-blue-600",
  Strings: "bg-green-600",
  "by_kind index": "bg-amber-600",
  "Cross indices": "bg-purple-600",
  PSM: "bg-rose-600",
};

/// One tab on the Memory workspace — either the singleton hand-coded
/// article graph (`state.legacy_graph`) or a metadata-driven graph
/// snapshot from `state.graphs[id]`. The hand-coded variant uses a
/// different POST endpoint and has no graph_id; spec-built entries
/// each have their own id from the `graphs` SQLite table.
type GraphTab =
  | { kind: "articles"; label: string }
  | { kind: "spec"; id: string; label: string };

const ARTICLES_TAB: GraphTab = { kind: "articles", label: "Article Graph" };

export function MemoryWorkspace() {
  const [tabs, setTabs] = useState<GraphTab[]>([ARTICLES_TAB]);
  const [activeKey, setActiveKey] = useState<string>("articles");
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Resolve the tab object for the currently-selected key. Keyed by
  // `articles` for the hand-coded graph and `spec:<id>` for each
  // metadata-driven entry.
  const activeTab = useMemo<GraphTab | undefined>(
    () =>
      tabs.find((t) =>
        t.kind === "articles" ? activeKey === "articles" : activeKey === `spec:${t.id}`,
      ),
    [tabs, activeKey],
  );

  // Discover spec-built graphs at mount. The hand-coded "Article
  // Graph" tab is always present; spec-built entries come from the
  // `graphs` SQLite table (only those built since boot will actually
  // return stats on click — others 404).
  useEffect(() => {
    void (async () => {
      try {
        const r = await fetch("/api/graphs", { headers: { "Content-Type": "application/json" } });
        if (!r.ok) return;
        const rows: { id: string; display_name?: string }[] = await r.json();
        const specTabs: GraphTab[] = rows.map((row) => ({
          kind: "spec",
          id: row.id,
          label: row.display_name ?? row.id,
        }));
        setTabs([ARTICLES_TAB, ...specTabs]);
      } catch {
        // Network error → Article-Graph-only tab. Memory tab remains usable.
      }
    })();
  }, []);

  const load = async (tab: GraphTab) => {
    setLoading(true);
    setError(null);
    setStats(null);
    try {
      const url =
        tab.kind === "articles"
          ? "/api/graph/articles/memory-stats"
          : `/api/graphs/${encodeURIComponent(tab.id)}/memory-stats`;
      const r = await fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
      const text = await r.text();
      let parsed: any = null;
      try {
        parsed = JSON.parse(text);
      } catch {
        throw new Error(`non-JSON response (${r.status}): ${text.slice(0, 200)}`);
      }
      if (!r.ok) throw new Error(parsed?.error ?? text);
      setStats(parsed as MemoryStats);
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setLoading(false);
    }
  };

  // Initial fetch + refetch on tab change.
  useEffect(() => {
    if (activeTab) void load(activeTab);
  }, [activeTab?.kind, activeTab && (activeTab.kind === "spec" ? activeTab.id : "articles")]);

  const topLevelSections = useMemo(() => {
    if (!stats) return [];
    return [
      { name: "Nodes", bytes: stats.nodes.total_bytes },
      { name: "Strings", bytes: stats.strings.total_bytes },
      { name: "by_kind index", bytes: stats.by_kind_index.total_bytes },
      { name: "Cross indices", bytes: stats.cross_indices.total_bytes },
      { name: "PSM", bytes: stats.psm.total_bytes },
    ].sort((a, b) => b.bytes - a.bytes);
  }, [stats]);

  return (
    <div className="h-full flex flex-col bg-gray-950 text-gray-200">
      {/* Header */}
      <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-3">
        <MemoryStick size={14} className="text-blue-400" />
        <h1 className="text-sm font-medium text-gray-100">Article Graph · Memory</h1>
        <span className="text-[10px] text-gray-500">heuristic — sums struct sizes + estimated heap overhead</span>
        <div className="ml-auto flex items-center gap-3">
          {stats && (
            <span className="text-[10px] text-gray-500">
              graph v{stats.graph_version} · ruleset v{stats.rule_pointers_version} · scanned in {stats.duration_ms} ms
            </span>
          )}
          <button
            onClick={() => activeTab && void load(activeTab)}
            disabled={loading || !activeTab}
            className="flex items-center gap-1 text-[11px] px-2 py-1 rounded bg-gray-900 border border-gray-800 text-gray-300 hover:bg-gray-800 disabled:opacity-50"
          >
            {loading ? <Loader2 size={11} className="animate-spin" /> : <RefreshCw size={11} />}
            Refresh
          </button>
        </div>
      </div>

      {/* Tab strip — one tab per graph source (the hand-coded
          "Article Graph" + every entry in state.graphs). Clicking
          switches the active source; the breakdown below re-fetches
          against that source's endpoint. */}
      <div className="px-4 pt-2 border-b border-gray-800 flex items-center gap-1 overflow-x-auto">
        {tabs.map((tab) => {
          const key = tab.kind === "articles" ? "articles" : `spec:${tab.id}`;
          const active = key === activeKey;
          return (
            <button
              key={key}
              onClick={() => setActiveKey(key)}
              className={
                "text-[11px] px-3 py-1.5 rounded-t border-b-2 transition-colors " +
                (active
                  ? "text-gray-100 border-blue-500 bg-gray-900"
                  : "text-gray-400 border-transparent hover:text-gray-200 hover:bg-gray-900/40")
              }
            >
              {tab.label}
            </button>
          );
        })}
      </div>

      <div className="flex-1 overflow-auto p-4 space-y-5">
        {error && (
          <div className="rounded border border-red-900/60 bg-red-950/30 p-3 text-xs text-red-300">
            {error}
            {error.includes("not built") && (
              <span className="block mt-1 text-red-200">
                Run pipeline <code className="text-amber-300">pl_build_article_graph</code> from the Pipelines tab first.
              </span>
            )}
          </div>
        )}

        {!stats && !error && loading && (
          <div className="text-xs text-gray-500 flex items-center gap-2">
            <Loader2 size={12} className="animate-spin" /> Walking the graph…
          </div>
        )}

        {stats && (
          <>
            {/* Grand total + stacked bar */}
            <section>
              <div className="flex items-baseline gap-2 mb-2">
                <h2 className="text-[10px] uppercase tracking-wider font-semibold text-gray-400">Total</h2>
                <span className="text-2xl font-semibold text-gray-100 tabular-nums">
                  {fmtBytes(stats.grand_total_bytes)}
                </span>
              </div>
              <StackedBar
                segments={topLevelSections.map((s) => ({
                  label: s.name,
                  value: s.bytes,
                  color: SECTION_COLORS[s.name] || "bg-gray-500",
                }))}
                total={stats.grand_total_bytes}
              />
              <div className="flex flex-wrap gap-3 mt-2 text-[11px]">
                {topLevelSections.map((s) => (
                  <span key={s.name} className="flex items-center gap-1.5">
                    <span className={`inline-block w-2 h-2 rounded-sm ${SECTION_COLORS[s.name] || "bg-gray-500"}`} />
                    <span className="text-gray-300">{s.name}</span>
                    <span className="text-gray-500 tabular-nums">{fmtBytes(s.bytes)}</span>
                    <span className="text-gray-600 tabular-nums">
                      {((s.bytes / stats.grand_total_bytes) * 100).toFixed(1)}%
                    </span>
                  </span>
                ))}
              </div>
            </section>

            {/* Nodes by kind */}
            <Section title="Nodes" subtitle={`${fmtCount(stats.nodes.total_count)} nodes · struct ${stats.node_struct_size_bytes} B each`}>
              <table className="w-full text-xs">
                <thead className="text-[10px] uppercase tracking-wider text-gray-500">
                  <tr className="border-b border-gray-800">
                    <th className="text-left py-1.5 px-2">Kind</th>
                    <th className="text-right py-1.5 px-2">Count</th>
                    <th className="text-right py-1.5 px-2">Struct</th>
                    <th className="text-right py-1.5 px-2">Heap</th>
                    <th className="text-right py-1.5 px-2">Total</th>
                    <th className="text-left py-1.5 px-2 w-[200px]">Share</th>
                  </tr>
                </thead>
                <tbody>
                  {stats.nodes.by_kind.map((row) => {
                    const pct = stats.nodes.total_bytes > 0 ? (row.bytes_total / stats.nodes.total_bytes) * 100 : 0;
                    return (
                      <tr key={row.kind} className="border-b border-gray-900/60 hover:bg-gray-900/40">
                        <td className="py-1 px-2 font-mono text-gray-200">{row.kind}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-300">{fmtCount(row.count)}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-400">{fmtBytes(row.bytes_struct)}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-400">{fmtBytes(row.bytes_heap)}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-200 font-medium">{fmtBytes(row.bytes_total)}</td>
                        <td className="py-1 px-2">
                          <BarRow pct={pct} color="bg-blue-600" />
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </Section>

            {/* Strings */}
            <Section title="String pool" subtitle={`${fmtCount(stats.strings.count)} interned · ${fmtCount(stats.strings.total_chars)} chars`}>
              <KeyValue rows={[
                ["Distinct strings", fmtCount(stats.strings.count)],
                ["Total chars (sum of lengths)", fmtCount(stats.strings.total_chars)],
                ["Vec<Arc<str>> overhead", fmtBytes(stats.strings.struct_bytes)],
                ["Arc heap (str bytes + 16 B header each)", fmtBytes(stats.strings.heap_bytes)],
                ["Subtotal", fmtBytes(stats.strings.total_bytes)],
              ]} />
            </Section>

            {/* by_kind index */}
            <Section title="by_kind index" subtitle="Per-kind StrId → NodeId for fast `find by name` lookups.">
              <KeyValue rows={[
                ["Kinds (HashMap count)", fmtCount(stats.by_kind_index.kinds)],
                ["Total entries across maps", fmtCount(stats.by_kind_index.entries)],
                ["Estimated bytes", fmtBytes(stats.by_kind_index.total_bytes)],
              ]} />
            </Section>

            {/* Cross indices */}
            <Section title="Cross indices" subtitle="Cross-cutting maps that don't fit the parent→child spine.">
              <table className="w-full text-xs">
                <thead className="text-[10px] uppercase tracking-wider text-gray-500">
                  <tr className="border-b border-gray-800">
                    <th className="text-left py-1.5 px-2">Map</th>
                    <th className="text-right py-1.5 px-2">Entries</th>
                    <th className="text-right py-1.5 px-2">Total values</th>
                    <th className="text-right py-1.5 px-2">Bytes</th>
                    <th className="text-left py-1.5 px-2 w-[200px]">Share</th>
                  </tr>
                </thead>
                <tbody>
                  {stats.cross_indices.breakdown.map((row) => {
                    const pct = stats.cross_indices.total_bytes > 0
                      ? (row.bytes_total / stats.cross_indices.total_bytes) * 100
                      : 0;
                    return (
                      <tr key={row.name} className="border-b border-gray-900/60 hover:bg-gray-900/40">
                        <td className="py-1 px-2 font-mono text-gray-200">{row.name}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-300">{fmtCount(row.entries)}</td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-400">
                          {row.value_total != null ? fmtCount(row.value_total) : "—"}
                        </td>
                        <td className="py-1 px-2 text-right tabular-nums text-gray-200 font-medium">{fmtBytes(row.bytes_total)}</td>
                        <td className="py-1 px-2">
                          <BarRow pct={pct} color="bg-purple-600" />
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </Section>

            {/* PSM */}
            <Section title="PSM resolver" subtitle="Module-101 priority chain + per-product rcl_hash lookup.">
              <KeyValue rows={[
                ["Priority chain entries", fmtCount(stats.psm.priorities)],
                ["rule_dim entries", fmtCount(stats.psm.rule_dim_entries)],
                ["Products with rcl_hash", fmtCount(stats.psm.products_with_rcl_hash)],
                ["Inner-hash entries (sum)", fmtCount(stats.psm.inner_hash_entries_total)],
                ["Subtotal", fmtBytes(stats.psm.total_bytes)],
              ]} />
            </Section>
          </>
        )}
      </div>
    </div>
  );
}

function Section({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="flex items-baseline gap-2 mb-2">
        <h2 className="text-[10px] uppercase tracking-wider font-semibold text-gray-400">{title}</h2>
        {subtitle && <span className="text-[11px] text-gray-500">{subtitle}</span>}
      </div>
      <div className="rounded border border-gray-800 bg-gray-900/40 overflow-hidden">{children}</div>
    </section>
  );
}

function KeyValue({ rows }: { rows: [string, string][] }) {
  return (
    <div className="divide-y divide-gray-900/60 text-xs">
      {rows.map(([k, v]) => (
        <div key={k} className="flex items-center justify-between px-3 py-1.5">
          <span className="text-gray-400">{k}</span>
          <span className="text-gray-200 tabular-nums font-medium">{v}</span>
        </div>
      ))}
    </div>
  );
}

function StackedBar({
  segments,
  total,
}: {
  segments: { label: string; value: number; color: string }[];
  total: number;
}) {
  const safe = total > 0 ? total : 1;
  return (
    <div className="flex h-3 rounded overflow-hidden border border-gray-800 bg-gray-900">
      {segments.map((s) => {
        const pct = (s.value / safe) * 100;
        if (pct < 0.1) return null;
        return (
          <div
            key={s.label}
            className={`${s.color} h-full`}
            style={{ width: `${pct}%` }}
            title={`${s.label} · ${fmtBytes(s.value)} · ${pct.toFixed(1)}%`}
          />
        );
      })}
    </div>
  );
}

function BarRow({ pct, color }: { pct: number; color: string }) {
  return (
    <div className="h-1.5 rounded-full bg-gray-800 overflow-hidden">
      <div className={`h-full ${color}`} style={{ width: `${Math.min(100, pct)}%` }} />
    </div>
  );
}
