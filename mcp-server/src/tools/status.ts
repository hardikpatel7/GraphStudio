import { z } from "zod";
import { http, HttpError } from "../http.js";
import { defineTool } from "../tool.js";
import { SOURCE_ID, DUCKDB_TABLE } from "../schema.js";

const input = z.object({}).describe("No inputs.");

interface SourceRow {
  id: string;
  display_name: string;
  kind: string;
  status: string;
  last_populated_at: string | null;
  target_table: string | null;
  updated_at: string;
}

interface QueryResponse {
  rows: Array<Record<string, unknown>>;
  total: number;
}

export const statusTool = defineTool({
  name: "article_selection_status",
  title: "Article-selection freshness & row count",
  destructive: false,
  inputSchema: input,
  description: [
    "Report whether the article_selection table is populated, when, and how many rows.",
    "",
    "Always call this before answering inventory questions on a fresh session — if status",
    "is 'not_yet_populated' or row_count is 0, call materialize_article_selection first.",
    "",
    "INPUT: none.",
    "",
    "RETURNS: { status, last_populated_at, row_count, table_present }.",
  ].join("\n"),
  async execute() {
    let source: SourceRow | null = null;
    try {
      source = await http.get<SourceRow>(`/api/sources/${SOURCE_ID}`);
    } catch (err) {
      if (err instanceof HttpError && err.status === 404) {
        source = null;
      } else {
        throw err;
      }
    }

    let rowCount = 0;
    let tablePresent = false;
    try {
      const q = await http.post<QueryResponse>("/api/query", {
        sql: `SELECT COUNT(*) AS n FROM ${DUCKDB_TABLE}`,
        limit: 1,
      });
      const first = q.rows[0];
      const n = first && typeof first.n === "number" ? first.n : Number(first?.n ?? 0);
      rowCount = Number.isFinite(n) ? n : 0;
      tablePresent = true;
    } catch {
      // table doesn't exist yet — not_yet_populated
    }

    return {
      source_id: SOURCE_ID,
      table: DUCKDB_TABLE,
      status: source?.status ?? "missing",
      last_populated_at: source?.last_populated_at ?? null,
      row_count: rowCount,
      table_present: tablePresent,
      hint:
        rowCount === 0
          ? "Call materialize_article_selection to populate the table."
          : "Ready for queries.",
    };
  },
});
