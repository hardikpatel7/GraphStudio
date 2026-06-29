import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    id: z
      .string()
      .min(1)
      .describe("Graph id from list_graphs (e.g. 'bealls-inventory-graph')."),
    kind: z
      .string()
      .min(1)
      .describe(
        "Hierarchy level id from the graph spec (e.g. 'l0', 'l1', 'article', 'store_code'). Call describe_graph first if you don't know what kinds exist."
      ),
    name: z
      .string()
      .min(1)
      .describe(
        "Node identity. The string value at this level (e.g. 'Ladies Footwear' for kind='l1', '88412' for kind='article'). Must match exactly — case-sensitive."
      ),
    include_ancestors: z
      .boolean()
      .optional()
      .describe(
        "Include the spine of strict ancestors as `{<kind>: <name>}`. Default: true — useful for context (which L0/L1/.../L5 owns this article)."
      ),
    include_metrics: z
      .boolean()
      .optional()
      .describe(
        "Include the pre-aggregated metrics at this node as `{<source>.<metric>: <value>}`. Default: true — this is the main reason to call graph_node."
      ),
    include_cross_edges: z
      .boolean()
      .optional()
      .describe(
        "Include bridge-source links to other hierarchies, e.g. articles ↔ stores. Default: false — opt in only when you need them, can be large."
      ),
  })
  .describe("Locate a single node and read what's attached to it.");

interface NodeResponse {
  id: string;
  from: { kind: string; name: string };
  row: {
    id: number;
    kind: string;
    name: string;
    ancestors?: Record<string, string>;
    metrics?: Record<string, unknown>;
    cross_edges?: Record<string, string[]>;
  };
}

export const graphNodeTool = defineTool({
  name: "graph_node",
  title: "Read metrics + context at a graph node",
  destructive: false,
  inputSchema: input,
  description: [
    "Look up one node in a graph by (kind, name) and return its pre-aggregated",
    "metrics plus optional ancestor spine and cross-edges. This is the right",
    "tool for questions like:",
    "  - \"OH at L2=Denim across the network?\"",
    "  - \"How is article 88412 performing?\"",
    "  - \"What's the in-stock % for L1=Ladies Footwear?\"",
    "",
    "Metrics are pre-aggregated bottom-up in the graph (sum/min/max/etc. per",
    "the rollup defined in the TOML spec), so this is an O(1) lookup — no",
    "scan, no SQL. If the metric you want isn't here, call describe_graph",
    "to see the registry, or fall back to duckdb_query.",
    "",
    "Inputs:",
    "  - id: graph id (from list_graphs)",
    "  - kind: hierarchy level (from describe_graph's `stats.kinds[]`)",
    "  - name: node identity at that level (case-sensitive)",
    "  - include_ancestors: default true",
    "  - include_metrics: default true",
    "  - include_cross_edges: default false",
    "",
    "Returns: { id, from: {kind, name}, row: { id, kind, name, ancestors?,",
    "          metrics?, cross_edges? } }.",
    "",
    "Errors:",
    "  - 404 \"graph not built\" — call describe_graph to confirm; build is a",
    "    SmartStudio operation, not an MCP one.",
    "  - 400 \"unknown kind\" — kind isn't in the registry; check describe_graph.",
    "  - 404 \"node not found\" — name doesn't match. The graph indexes exact",
    "    strings, so check spelling / case before retrying.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const body = {
      from: { kind: args.kind, name: args.name },
      project: {
        include_ancestors: args.include_ancestors ?? true,
        include_metrics: args.include_metrics ?? true,
        include_cross_edges: args.include_cross_edges ?? false,
      },
    };
    const data = await http.post<NodeResponse>(
      `/api/graphs/${encodeURIComponent(args.id)}/node`,
      body,
    );
    return data;
  },
});
