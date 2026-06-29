import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({}).describe("No inputs.");

interface SourceRow {
  id: string;
  display_name: string;
  kind: string;
  status: string;
  last_populated_at: string | null;
  target_table: string | null;
  updated_at: string;
  [k: string]: unknown;
}

export const listSourcesTool = defineTool({
  name: "list_sources",
  title: "List analytical data sources",
  destructive: false,
  inputSchema: input,
  description: [
    "Return every source registered in GraphStudio. A source is a named",
    "feed that backs an analytical table — each one has a `kind` describing",
    "how it produces data (pg_query, duckdb_query, etc.), a `target_table`",
    "(when populated, the DuckDB table to query), and a `status` indicating",
    "whether it's been populated.",
    "",
    "Use this when you need to find which DuckDB table holds a particular",
    "slice of data — pair with describe_source for the column-level detail",
    "and freshness.",
    "",
    "INPUT: none.",
    "RETURNS: { count, sources: [{ id, display_name, kind, status,",
    "          last_populated_at, target_table, ... }] }.",
  ].join("\n"),
  async execute() {
    const rows = await http.get<SourceRow[]>("/api/sources");
    return { count: rows.length, sources: rows };
  },
});
