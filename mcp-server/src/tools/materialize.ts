import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({}).describe("No inputs.");

interface MaterializeResponse {
  dataview_id: string;
  rows: number;
  extract_ms: number;
  duckdb_ms: number;
  total_ms: number;
  rcl_version: number | string;
}

export const materializeTool = defineTool({
  name: "materialize_article_selection",
  title: "Materialize article_selection",
  destructive: true,
  inputSchema: input,
  description: [
    "Run GraphStudio's article-selection materializer.",
    "",
    "Reads from the default PostgreSQL connection, resolves the in-process RCL ruleset,",
    "and writes 46 columns into tenant_data.duckdb::article_selection. This is what every",
    "other tool here queries. Run it once at session start (or when data is known stale),",
    "then use status/list/query tools to read.",
    "",
    "RUNS A FULL EXTRACT — may take minutes on first run depending on PG size.",
    "",
    "INPUT: none.",
    "",
    "RETURNS: { dataview_id, rows, extract_ms, duckdb_ms, total_ms, rcl_version }.",
  ].join("\n"),
  async execute() {
    const data = await http.post<MaterializeResponse>("/api/article-selection/materialize");
    return {
      ok: true,
      summary:
        `Materialized ${data.rows.toLocaleString()} rows in ${(data.total_ms / 1000).toFixed(1)}s ` +
        `(extract ${data.extract_ms}ms + duckdb ${data.duckdb_ms}ms). RCL v${data.rcl_version}.`,
      ...data,
    };
  },
});
