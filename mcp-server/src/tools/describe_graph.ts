import { z } from "zod";
import { http, HttpError } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    id: z
      .string()
      .min(1)
      .describe("Graph id from list_graphs (e.g. 'bealls-inventory-graph')."),
  })
  .describe("Target a single graph for full description.");

interface GraphSpec {
  id: string;
  display_name: string;
  toml_text: string;
  last_validated_at: string | null;
  error_log: string | null;
  created_at: string;
  updated_at: string;
}

interface GraphStats {
  id: string;
  graph_version: number;
  node_count: number;
  string_count: number;
  kinds: { name: string; hierarchy: string; node_count: number }[];
  metrics: {
    name: string;
    source: string;
    column: string;
    rollup: string;
    is_composite: boolean;
  }[];
  cross_edges: { alias: string; kind_a: string; kind_b: string }[];
}

export const describeGraphTool = defineTool({
  name: "describe_graph",
  title: "Describe a graph — spec + live stats",
  destructive: false,
  inputSchema: input,
  description: [
    "Return everything you need to start working with a graph:",
    "  - its declarative spec (TOML — sources, hierarchies, relations, metrics)",
    "  - its live runtime state (kinds with node counts, metric registry,",
    "    cross-edges, graph_version)",
    "",
    "Call this after list_graphs to learn what hierarchies and metrics a",
    "specific graph offers BEFORE calling graph_node / graph_traverse /",
    "graph_cross_filter on it. The TOML spec is human-readable — read it",
    "directly to discover available kinds (hierarchy levels), the source",
    "tables backing each level, and the rollup function on each metric.",
    "",
    "If the graph hasn't been built since boot, the stats portion will be",
    "absent and a `stats_unavailable_reason` field explains why. The spec",
    "is always returned. A non-empty `error_log` on the spec means the",
    "TOML has validation issues — treat the graph as potentially stale.",
    "",
    "INPUT: { id }.",
    "",
    "RETURNS: { spec, stats?, stats_unavailable_reason? }.",
  ].join("\n"),
  async execute(raw) {
    const { id } = input.parse(raw ?? {});

    // Spec is the source of truth and must succeed. Stats is best-effort —
    // 404 means "graph row exists but no live snapshot yet" (not yet built).
    const specPromise = http.get<GraphSpec>(`/api/graphs/${encodeURIComponent(id)}`);
    const statsPromise = http
      .get<GraphStats>(`/api/graphs/${encodeURIComponent(id)}/stats`)
      .then(
        (s) => ({ ok: true as const, stats: s }),
        (e: unknown) => ({
          ok: false as const,
          reason:
            e instanceof HttpError
              ? e.status === 404
                ? "graph not built yet — no live snapshot"
                : `stats fetch failed: HTTP ${e.status}`
              : e instanceof Error
                ? `stats fetch failed: ${e.message}`
                : `stats fetch failed: ${String(e)}`,
        }),
      );

    const [spec, statsResult] = await Promise.all([specPromise, statsPromise]);

    if (statsResult.ok) {
      return { spec, stats: statsResult.stats };
    }
    return { spec, stats_unavailable_reason: statsResult.reason };
  },
});
