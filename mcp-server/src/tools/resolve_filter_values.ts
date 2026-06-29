import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    filter_config_id: z
      .string()
      .min(1)
      .describe(
        "Filter config id (e.g. 'fc_877c14152bf9'). Filter configs live in GraphStudio's metadata — `duckdb_query(\"SELECT id, display_name, dimension_ref FROM filter_configs\")` if you need to discover one."
      ),
    context: z
      .record(z.array(z.string()))
      .optional()
      .describe(
        "Optional parent-column selections used to narrow the cascading children, e.g. { l1_name: ['Apparel'] } — l2/l3/l4 then only return children of 'Apparel'. Omit to get the full unrestricted set for every column."
      ),
  })
  .describe("Resolve dropdown distincts for each filterable column on a filter config.");

interface ResolveValuesResponse {
  columns: Record<string, string[]>;
}

export const resolveFilterValuesTool = defineTool({
  name: "resolve_filter_values",
  title: "Distinct values per filterable column on a filter config",
  destructive: false,
  inputSchema: input,
  description: [
    "Return distinct values for each filterable column declared on a filter",
    "config, honoring the config's cascading rules. If `context` is passed,",
    "the children of those parent selections are narrowed; otherwise every",
    "column returns its full distinct set.",
    "",
    "Use this when you want the planner-facing dropdown vocabulary — exactly",
    "the values the UI would show — for a known filter config. Values are",
    "read from the dimension's master_table, not from the analytical fact",
    "tables, so they reflect the canonical spelling/casing the rest of the",
    "system expects.",
    "",
    "Alternatives:",
    "  - For graph-resident hierarchy enumeration (e.g. all L1 values in",
    "    the product hierarchy), graph_traverse from a parent node is",
    "    usually cheaper and graph-native.",
    "  - For multi-axis filter intersection that returns the surviving",
    "    candidate set + distincts, use graph_cross_filter.",
    "",
    "INPUT: { filter_config_id, context? }.",
    "RETURNS: { columns: { <col_name>: [<distinct values>], ... } }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const body = args.context ? { context: args.context } : {};
    const data = await http.post<ResolveValuesResponse>(
      `/api/filter-configs/${encodeURIComponent(args.filter_config_id)}/resolve-values`,
      body,
    );
    return data;
  },
});
