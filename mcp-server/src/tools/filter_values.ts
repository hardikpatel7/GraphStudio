import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";
import { FILTER_CONFIG_ID } from "../schema.js";

const input = z
  .object({
    context: z
      .record(z.array(z.string()))
      .optional()
      .describe(
        "Parent-column selections, e.g. { l1_name: ['Women'] }. Honored by the cascading rules on the filter config (l1 → l2/l3/l4)."
      ),
  })
  .describe("Optional cascading context.");

interface ResolveValuesResponse {
  columns: Record<string, string[]>;
}

export const filterValuesTool = defineTool({
  name: "resolve_filter_values",
  title: "Distinct values for the product filter (cascading)",
  destructive: false,
  inputSchema: input,
  description: [
    "Return distinct values for each filterable column on the product filter config",
    "(fc_877c14152bf9). Honors cascading: passing context={l1_name:['Women']} narrows",
    "the l2/l3/l4 lists to children of 'Women'.",
    "",
    "Use this to discover what brands, L1s, L2s, etc. actually exist in the data BEFORE",
    "composing a query_articles SQL filter — saves a round trip and avoids guessing.",
    "",
    "Note: values come from the dimension's master_table in DuckDB, not article_selection.",
    "Master_table for the product dimension is `global.product_attributes_filter`.",
    "",
    "INPUT: { context? }.",
    "",
    "RETURNS: { columns: { l1_name: [...], l2_name: [...], brand: [...], ... } }.",
  ].join("\n"),
  async execute(raw) {
    const { context } = input.parse(raw ?? {});
    const body = context ? { context } : {};
    const data = await http.post<ResolveValuesResponse>(
      `/api/filter-configs/${FILTER_CONFIG_ID}/resolve-values`,
      body
    );
    return data;
  },
});
