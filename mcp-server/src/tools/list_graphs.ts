import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({}).describe("No inputs.");

interface GraphRow {
  id: string;
  display_name: string;
  last_validated_at: string | null;
  error_log: string | null;
  created_at: string;
  updated_at: string;
}

export const listGraphsTool = defineTool({
  name: "list_graphs",
  title: "List graphs available on GraphStudio",
  destructive: false,
  inputSchema: input,
  description: [
    "Return every graph defined on this GraphStudio instance.",
    "",
    "A graph is an in-memory, traversal-optimized index over the underlying",
    "analytical tables. Each graph defines its own hierarchies (e.g., product",
    "L0..L5..article..product_code; store/DC spine) and pre-aggregated metrics",
    "(OH, OO, sell-through, WOS, etc.). Subsequent calls (describe_graph,",
    "graph_node, graph_traverse, graph_cross_filter) target a specific graph",
    "by its `id`.",
    "",
    "Call this once near the start of a session so you know what's available;",
    "the result is stable across the session and worth caching in your reasoning.",
    "",
    "INPUT: none.",
    "",
    "RETURNS: array of { id, display_name, last_validated_at, error_log,",
    "                    created_at, updated_at }.",
    "",
    "Notes:",
    "  - `id` is the stable handle you pass to other graph_* tools.",
    "  - `last_validated_at` null means the spec hasn't been validated yet.",
    "  - `error_log` non-null/non-empty means the spec has validation issues —",
    "    the graph may still be usable but treat its outputs with care.",
  ].join("\n"),
  async execute() {
    const rows = await http.get<GraphRow[]>("/api/graphs");
    return {
      count: rows.length,
      graphs: rows,
    };
  },
});
