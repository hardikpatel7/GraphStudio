import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";
import { COLUMN_NAMES, DATAVIEW_ID } from "../schema.js";

const input = z.object({
  limit: z.number().int().min(1).max(500).default(50).describe("Max rows (1-500)."),
  offset: z.number().int().min(0).default(0).describe("Rows to skip."),
  sort_col: z
    .string()
    .optional()
    .describe("Column to sort by. Must be a known article_selection column."),
  sort_dir: z.enum(["ASC", "DESC"]).optional().describe("Sort direction (default ASC)."),
  skip_total: z.boolean().optional().describe("Skip the COUNT(*) companion."),
});

interface DataResponse {
  rows: Array<Record<string, unknown>>;
  columns: Array<{ name: string }>;
  total: number;
  duration_ms: number;
  sql?: string;
}

export const listTool = defineTool({
  name: "list_articles",
  title: "List articles (sorted, paginated, unfiltered)",
  destructive: false,
  inputSchema: input,
  description: [
    "Plain sorted/paginated list of articles from the article_selection DataView.",
    "Use this when the user wants a top-N by some column (e.g., 'top 10 by oh desc'),",
    "or when paginating through the catalog. No filtering — for filtered reads use",
    "query_articles instead.",
    "",
    "INPUT: { limit?, offset?, sort_col?, sort_dir?, skip_total? }.",
    "",
    "RETURNS: { rows, columns, total, duration_ms }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    if (args.sort_col && !COLUMN_NAMES.has(args.sort_col)) {
      throw new Error(
        `Unknown sort_col '${args.sort_col}'. Call describe_article_selection for the valid column list.`
      );
    }
    const body: Record<string, unknown> = {
      limit: args.limit,
      offset: args.offset,
    };
    if (args.sort_col) body.sort_col = args.sort_col;
    if (args.sort_dir) body.sort_dir = args.sort_dir;
    if (args.skip_total) body.skip_total = true;

    const data = await http.post<DataResponse>(`/api/dataviews/${DATAVIEW_ID}/data`, body);
    return data;
  },
});
