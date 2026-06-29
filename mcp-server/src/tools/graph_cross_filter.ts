import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const OPERATORS = [
  "in",
  "eq",
  "ne",
  "gt",
  "lt",
  "gte",
  "lte",
  "like",
  "ilike",
  "not_in",
] as const;

const FilterClause = z.object({
  attribute_name: z
    .string()
    .min(1)
    .describe("Column / attribute name to filter on (e.g. 'l2', 'brand', 'channel')."),
  values: z
    .array(z.string())
    .min(1)
    .describe("Values to match (e.g. ['Denim', 'Swim']). Use strings even for numeric values."),
  operator: z
    .enum(OPERATORS)
    .optional()
    .describe(
      "How to compare. Default: 'in' (membership). Most filters are 'in' / 'not_in'."
    ),
});

const input = z
  .object({
    id: z.string().min(1).describe("Graph id from list_graphs."),
    target_kind: z
      .string()
      .optional()
      .describe(
        "Which kind of node the candidate set should be over. Default: 'article'. Use 'store_code' to get the matching store set, etc."
      ),
    filters: z
      .array(FilterClause)
      .describe(
        "Constraints to apply, ANDed together. Each clause narrows the candidate set."
      ),
    attributes: z
      .array(z.string().min(1))
      .describe(
        "Which attribute names to project distinct values for, after filtering. E.g. ['brand', 'l3'] returns each surviving brand and l3 value."
      ),
  })
  .describe("Intersect filters across the graph; project distincts of the surviving set.");

interface CrossFilterResponse {
  id: string;
  target_kind: string;
  count: number;
  status: boolean;
  data: Record<string, string[]>;
  message: string;
}

export const graphCrossFilterTool = defineTool({
  name: "graph_cross_filter",
  title: "Multi-axis filter intersection over a graph",
  destructive: false,
  inputSchema: input,
  description: [
    "Apply AND-ed filters across the graph (over any hierarchy levels and",
    "cross-edges), then project distinct values of the requested attributes",
    "for the surviving candidate set.",
    "",
    "Use this for narrowing questions:",
    "  - \"Which brands exist under L1=Ladies AND channel=Online?\"",
    "      filters=[{l1: ['Ladies']}, {channel: ['Online']}], attributes=['brand']",
    "  - \"Which stores serve denim AND are in DC2's network?\"",
    "      target_kind='store_code', filters=[{l2: ['Denim']}, {dc: ['DC2']}],",
    "      attributes=['store_code']",
    "",
    "What you get back is the *set* and its *distinct projected attributes*,",
    "NOT per-row metrics. If you also need OH/sales per surviving entity, follow",
    "up with graph_traverse (descendants_of_kind) or graph_node per item.",
    "",
    "target_kind defaults to 'article' — the candidate set is articles unless",
    "you ask for another leaf kind (most commonly 'store_code').",
    "",
    "Returns: { id, target_kind, count, data: { attribute_name -> [distinct values] } }",
    "  - count: number of surviving candidate nodes",
    "  - data[attr]: the distinct values that attr takes across the surviving set",
    "",
    "If `count` is 0 your filters are inconsistent (no overlap). If it's the",
    "graph's total, your filters didn't narrow anything — recheck names/case.",
    "Filter attribute_name and value strings must match the graph's interned",
    "strings exactly; describe_graph + a small graph_traverse call are good",
    "ways to discover the canonical spellings.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});

    // Wire-faithful FilterPayload. dimension/filter_type are required by the
    // upstream schema but the v2 resolver doesn't read them on this path
    // (see CrossFilterWorkspace.tsx — same hardcoded defaults there). UAM
    // fields are stripped — the MCP doesn't speak that layer.
    const body = {
      attributes: args.attributes.map((name) => ({
        attribute_name: name,
        dimension: "product",
        filter_type: "non-cascaded",
      })),
      filters: args.filters.map((f) => ({
        attribute_name: f.attribute_name,
        operator: f.operator ?? "in",
        values: f.values,
      })),
      is_urm_filter: false,
    };

    const qs = args.target_kind
      ? `?target_kind=${encodeURIComponent(args.target_kind)}`
      : "";
    const data = await http.post<CrossFilterResponse>(
      `/api/graphs/${encodeURIComponent(args.id)}/cross-filter${qs}`,
      body,
    );
    if (data.count === 0) {
      return {
        ...data,
        gap_hint:
          "Empty result. Two possibilities: (a) your filter values don't match " +
          "the graph's interned strings (case / spelling — check describe_graph " +
          "or a small graph_traverse), or (b) the intersection is genuinely empty. " +
          "If you've already verified the values and still expect this filter to " +
          "produce results, the underlying dimension or edge may not be modeled — " +
          "call submit_feedback with example_question = the planner's actual prompt.",
      };
    }
    return data;
  },
});
