import { z } from "zod";
import { defineTool } from "../tool.js";
import {
  COLUMNS,
  DATAVIEW_ID,
  DUCKDB_TABLE,
  FILTER_CONFIG,
  PRODUCT_DIMENSION,
} from "../schema.js";

const input = z
  .object({
    group: z
      .enum(["hierarchy", "position", "per_node_map", "performance", "policy", "allocation", "all"])
      .optional()
      .describe("If set, return only columns in this group. Defaults to 'all'."),
  })
  .describe("Optional group filter.");

export const describeTool = defineTool({
  name: "describe_article_selection",
  title: "Schema dictionary for article_selection",
  destructive: false,
  inputSchema: input,
  description: [
    "Return the data dictionary for the article_selection DataView: 46 columns grouped",
    "by hierarchy/position/per-node-map/performance/policy/allocation, plus the attached",
    "filter config (which columns can be filtered, mandatory column, cascading rules)",
    "and the product dimension hierarchy.",
    "",
    "Always call this first when the user references columns or filters by name —",
    "lets you ground their language in the actual schema instead of guessing.",
    "",
    "INPUT: { group? } — narrows to one column group.",
    "",
    "RETURNS: { dataview_id, duckdb_table, columns: [...], filter_config: {...}, dimension: {...} }.",
  ].join("\n"),
  async execute(raw) {
    const { group } = input.parse(raw ?? {});
    const cols = group && group !== "all" ? COLUMNS.filter((c) => c.group === group) : COLUMNS;
    return {
      dataview_id: DATAVIEW_ID,
      duckdb_table: DUCKDB_TABLE,
      column_count: cols.length,
      columns: cols,
      filter_config: FILTER_CONFIG,
      dimension: PRODUCT_DIMENSION,
      notes: [
        "POST /api/dataviews/{id}/data currently ignores `filters` for duckdb_table sources.",
        "Use the `query_articles` tool (SQL-on-DuckDB) for filtered or aggregated reads.",
        "Use `list_articles` for plain sorted, paginated, unfiltered listing.",
      ],
    };
  },
});
