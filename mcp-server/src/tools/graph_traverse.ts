import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const EDGES = [
  "children",
  "parent",
  "ancestors",
  "descendants_of_kind",
  "cross_edge",
] as const;

const input = z
  .object({
    id: z
      .string()
      .min(1)
      .describe("Graph id from list_graphs."),
    kind: z
      .string()
      .min(1)
      .describe("Source-node hierarchy level (from describe_graph)."),
    name: z
      .string()
      .min(1)
      .describe("Source-node identity (exact match, case-sensitive)."),
    edge: z
      .enum(EDGES)
      .describe(
        "Which edge to walk from the source node:\n" +
          "  - 'children': one level down in the same hierarchy.\n" +
          "  - 'parent': one level up.\n" +
          "  - 'ancestors': every strict ancestor up to root.\n" +
          "  - 'descendants_of_kind': all descendant nodes of a specific kind (e.g. all articles under an L2). Requires target_kind.\n" +
          "  - 'cross_edge': follow a bridge to another hierarchy (e.g. articles → stores). Requires cross_edge_alias."
      ),
    target_kind: z
      .string()
      .optional()
      .describe(
        "Required when edge='descendants_of_kind'. The kind of descendants to return (e.g. 'article')."
      ),
    cross_edge_alias: z
      .string()
      .optional()
      .describe(
        "Required when edge='cross_edge'. The bridge alias (see describe_graph stats.cross_edges[].alias)."
      ),
    include_ancestors: z
      .boolean()
      .optional()
      .describe(
        "Include each returned row's ancestor spine. Default: false (the source is shared, ancestors are usually redundant)."
      ),
    include_metrics: z
      .boolean()
      .optional()
      .describe(
        "Include pre-aggregated metrics on each returned row. Default: true — this is usually why you're traversing."
      ),
    include_cross_edges: z
      .boolean()
      .optional()
      .describe(
        "Include each row's cross-edges. Default: false (can be heavy)."
      ),
    offset: z
      .number()
      .int()
      .min(0)
      .optional()
      .describe("Pagination start. Default: 0."),
    limit: z
      .number()
      .int()
      .min(1)
      .optional()
      .describe(
        "Pagination size. Default: 1000 (server cap). Traversals against fat parents can yield 10k+ rows — page through if needed."
      ),
  })
  .describe("Walk an edge from a source node and project the destination set.");

interface TraverseResponse {
  id: string;
  from: { kind: string; name: string };
  rows: Array<{
    id: number;
    kind: string;
    name: string;
    ancestors?: Record<string, string>;
    metrics?: Record<string, unknown>;
    cross_edges?: Record<string, string[]>;
  }>;
  total: number;
  offset: number;
  limit: number;
}

export const graphTraverseTool = defineTool({
  name: "graph_traverse",
  title: "Walk a graph edge and project the destination set",
  destructive: false,
  inputSchema: input,
  description: [
    "Walk one edge from a source node and return the destination nodes,",
    "each projected with its metrics (default), optional ancestors, and",
    "optional cross-edges. Use this for subtree-style questions:",
    "  - \"All articles under L2=Denim\" → edge='descendants_of_kind', target_kind='article'.",
    "  - \"All children of L1=Ladies Footwear\" → edge='children'.",
    "  - \"Which stores does article 88412 reach?\" → edge='cross_edge', cross_edge_alias=<bridge>.",
    "  - \"What L0/L1/.../L5 owns this article?\" → edge='ancestors'.",
    "",
    "Pre-aggregated metrics travel with each row, so a single call returns",
    "both the entity set AND its values — no follow-up scan needed for the",
    "common case.",
    "",
    "Pagination: server defaults to 1000 rows. For a fat parent (e.g. L0 →",
    "all articles ≈ 48k), page through with offset/limit; total is in the",
    "response so you know when you're done.",
    "",
    "Result shape: { id, from, rows: [{ id, kind, name, metrics?, ancestors?,",
    "                cross_edges? }], total, offset, limit }.",
    "",
    "Errors mirror graph_node — 404 if the graph isn't built, 400 on unknown",
    "kind, 404 if the source name doesn't resolve. For descendants_of_kind",
    "and cross_edge, an invalid target_kind / cross_edge_alias returns 400.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});

    let wireEdge: unknown;
    if (args.edge === "descendants_of_kind") {
      if (!args.target_kind) {
        throw new Error("edge='descendants_of_kind' requires target_kind");
      }
      wireEdge = { descendants_of_kind: args.target_kind };
    } else if (args.edge === "cross_edge") {
      if (!args.cross_edge_alias) {
        throw new Error("edge='cross_edge' requires cross_edge_alias");
      }
      wireEdge = { cross_edge: args.cross_edge_alias };
    } else {
      wireEdge = args.edge;
    }

    const body: Record<string, unknown> = {
      from: { kind: args.kind, name: args.name },
      edge: wireEdge,
      project: {
        include_ancestors: args.include_ancestors ?? false,
        include_metrics: args.include_metrics ?? true,
        include_cross_edges: args.include_cross_edges ?? false,
      },
    };
    if (args.offset !== undefined) body.offset = args.offset;
    if (args.limit !== undefined) body.limit = args.limit;

    const data = await http.post<TraverseResponse>(
      `/api/graphs/${encodeURIComponent(args.id)}/traverse`,
      body,
    );
    return data;
  },
});
