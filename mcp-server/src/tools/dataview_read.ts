import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const FILTER_OPERATORS = [
  "in",
  "not_in",
  "eq",
  "ne",
  "gt",
  "lt",
  "gte",
  "lte",
  "like",
  "ilike",
  "between",
] as const;

const FilterClause = z.object({
  attribute_name: z
    .string()
    .min(1)
    .describe(
      "Column name to filter on. Must be one of the dataview's declared columns (use describe_dataview to confirm)."
    ),
  operator: z
    .enum(FILTER_OPERATORS)
    .default("in")
    .describe(
      "How to compare. 'in' / 'not_in' take multi-value lists; 'gt' / 'lt' / 'gte' / 'lte' take one numeric; 'eq' / 'ne' / 'like' / 'ilike' take one string; 'between' takes exactly two numerics [lo, hi] inclusive. Like patterns use SQL wildcards: % and _."
    ),
  values: z
    .array(z.string())
    .min(1)
    .describe(
      "Values to match. Strings for string ops, numbers-as-strings (e.g. ['4', '10']) for numeric ops — they're parsed server-side."
    ),
});

const AGG_OPS = ["sum", "avg", "count", "count_distinct", "min", "max"] as const;

const AggregateSpec = z.object({
  column: z
    .string()
    .min(1)
    .describe(
      "Column to aggregate. For op='count' use '*' to count rows; for any other op the column must be a declared dataview column."
    ),
  op: z
    .enum(AGG_OPS)
    .describe(
      "Aggregation operator. count_distinct emits COUNT(DISTINCT col); requires a real column (no '*')."
    ),
  alias: z
    .string()
    .optional()
    .describe(
      "Output column name. Defaults to `<column>_<op>` (e.g. lw_units_sum, brand_distinct). For count(*) the default becomes 'count_all'."
    ),
});

const HavingClause = z.object({
  alias: z
    .string()
    .min(1)
    .describe(
      "Alias to filter on AFTER grouping. Must match either a group_by column name or an aggregate's output alias on this same request."
    ),
  operator: z
    .enum(FILTER_OPERATORS)
    .default("gt")
    .describe("Same operator vocabulary as filters; same numeric/string parsing."),
  values: z
    .array(z.string())
    .min(1)
    .describe("Values to compare against. Numbers-as-strings for numeric ops; 'between' takes 2."),
});

const input = z
  .object({
    id: z.string().min(1).describe("DataView id from list_dataviews."),
    limit: z
      .number()
      .int()
      .min(1)
      .max(5000)
      .optional()
      .describe("Page size. Server default applies if omitted."),
    offset: z
      .number()
      .int()
      .min(0)
      .optional()
      .describe("Page offset. Defaults to 0."),
    sort_col: z
      .string()
      .optional()
      .describe(
        "Column to sort by. Must be one of the dataview's columns (describe_dataview to confirm)."
      ),
    sort_dir: z
      .enum(["asc", "desc"])
      .optional()
      .describe("Sort direction. Defaults to ascending."),
    filters: z
      .array(FilterClause)
      .optional()
      .describe(
        "Server-side filters applied as a WHERE clause on the dataview's source SELECT. Supported for pg_query / duckdb_table / duckdb_query sources; article_graph sources have their own filter pipeline (still honored here, dispatched differently server-side). attribute_name is validated against the dataview's declared columns; numeric operators parse the value as f64. Multiple filters AND together."
      ),
    group_by: z
      .array(z.string().min(1))
      .optional()
      .describe(
        "Group-by columns. When non-empty, the output row grain becomes one row per distinct combination of these columns. Requires `aggregates` to be non-empty. Validated against the dataview's declared columns. Filter clauses (above) apply BEFORE grouping."
      ),
    aggregates: z
      .array(AggregateSpec)
      .optional()
      .describe(
        "Aggregate specs paired with `group_by`. Each spec adds one output column with name = alias (or default `<column>_<op>`). Common shape: [{column:'lw_units', op:'sum'}, {column:'l4w_units', op:'sum'}]."
      ),
    having: z
      .array(HavingClause)
      .optional()
      .describe(
        "Post-group filters (HAVING). Each clause references either a group_by column or an aggregate alias defined on this same request. Use for thresholds like 'stores where sum(oh) > 100' that can't be expressed with pre-group filters."
      ),
    skip_total: z
      .boolean()
      .optional()
      .describe(
        "When true, skip the COUNT(*) and return total=0. Pass true on page/sort changes within the same filter set to avoid recounting; pass false (default) on filter changes."
      ),
  })
  .describe("Paginated, optionally sorted/filtered read of a registered dataview.");

interface DataResponse {
  rows: Array<Record<string, unknown>>;
  total: number;
  columns: Array<{ name: string }>;
  sql?: string;
}

export const dataViewReadTool = defineTool({
  name: "dataview_read",
  title: "Paginated typed read of a dataview's source",
  destructive: false,
  inputSchema: input,
  description: [
    "Read rows from a dataview. The dataview's contract picks the backend",
    "(DuckDB table, PG query, in-memory store, etc.) — you don't pick a SQL",
    "engine, you pick a dataview and get its canonical column shape.",
    "",
    "Use this when:",
    "  - You want the same column projection the planner UI sees, with",
    "    sort/page semantics that match the front-end's experience.",
    "  - The question is \"give me the first N rows of this dataview\" or",
    "    \"sorted by X descending, page 2.\"",
    "  - You don't want to compose SQL or guess which DuckDB table backs",
    "    the dataview — the contract already routes it.",
    "",
    "Prefer this over duckdb_query when a dataview already exposes the",
    "shape you need; it respects the contract (cache strategy, supported",
    "ops) the way the rest of the system reads it.",
    "",
    "Inputs:",
    "  - id: dataview id (from list_dataviews / describe_dataview)",
    "  - limit, offset: pagination",
    "  - sort_col, sort_dir: optional ordering",
    "  - filters: AND-ed predicates applied server-side as WHERE clauses",
    "  - skip_total: optimization for repeat reads with same filter set",
    "",
    "Filter examples:",
    "  - L2=DENIM with stockout: [{attribute_name:'l2_name', operator:'ilike', values:['%DENIM%']},",
    "                              {attribute_name:'stockout', operator:'gt', values:['0']}]",
    "  - Region rollup:           [{attribute_name:'region', operator:'eq', values:['REGION3999']}]",
    "  - Brand list:              [{attribute_name:'brand', operator:'in', values:['FILA','NIKE','ADIDAS']}]",
    "",
    "Group-by + aggregate examples:",
    "  - L1 totals network-wide:  group_by=['l1_name'],",
    "                              aggregates=[{column:'lw_units', op:'sum'},",
    "                                          {column:'l4w_units', op:'sum'}]",
    "  - Region × L1 sales:       group_by=['region', 'l1_name'],",
    "                              aggregates=[{column:'lw_revenue', op:'sum'},",
    "                                          {column:'*', op:'count'}]",
    "  - Brand min/max OH:        group_by=['brand'],",
    "                              aggregates=[{column:'oh', op:'min'}, {column:'oh', op:'max'}]",
    "  - Distinct counts:         group_by=['region'],",
    "                              aggregates=[{column:'article', op:'count_distinct'}]",
    "  - Age range:               filters=[{cfc_age_weeks, between, ['5','20']}]",
    "  - HAVING (post-group):     group_by=['store_code'],",
    "                              aggregates=[{column:'oh', op:'sum'}],",
    "                              having=[{alias:'oh_sum', operator:'gt', values:['1000']}]",
    "  - With filter:             filters narrow the rows BEFORE grouping; HAVING narrows AFTER.",
    "",
    "Returns: { rows, total, columns: [{ name }], sql? }.",
    "  - `sql` is the underlying query string when the backend produced one;",
    "    treat as diagnostic only, don't compose against it.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const body: Record<string, unknown> = {};
    if (args.limit !== undefined) body.limit = args.limit;
    if (args.offset !== undefined) body.offset = args.offset;
    if (args.sort_col !== undefined) body.sort_col = args.sort_col;
    if (args.sort_dir !== undefined) body.sort_dir = args.sort_dir;
    if (args.skip_total !== undefined) body.skip_total = args.skip_total;
    if (args.filters !== undefined && args.filters.length > 0) {
      body.filters = args.filters.map((f) => ({
        attribute_name: f.attribute_name,
        operator: f.operator,
        values: f.values,
      }));
    }
    if (args.group_by !== undefined && args.group_by.length > 0) {
      body.group_by = args.group_by;
    }
    if (args.aggregates !== undefined && args.aggregates.length > 0) {
      body.aggregates = args.aggregates.map((a) => ({
        column: a.column,
        op: a.op,
        ...(a.alias !== undefined ? { alias: a.alias } : {}),
      }));
    }
    if (args.having !== undefined && args.having.length > 0) {
      body.having = args.having.map((h) => ({
        alias: h.alias,
        operator: h.operator,
        values: h.values,
      }));
    }
    const data = await http.post<DataResponse>(
      `/api/dataviews/${encodeURIComponent(args.id)}/data`,
      body,
    );
    return data;
  },
});
