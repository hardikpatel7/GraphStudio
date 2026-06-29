import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    id: z.string().min(1).describe("Source id from list_sources."),
  })
  .describe("Target a single source.");

interface SourceDetail {
  id: string;
  display_name: string;
  kind: string;
  status: string;
  last_populated_at: string | null;
  target_table: string | null;
  primary_key: string[];
  config: unknown;
  cdc_enabled: number;
  connection_ref: string;
  created_at: string;
  updated_at: string;
  [k: string]: unknown;
}

export const describeSourceTool = defineTool({
  name: "describe_source",
  title: "Describe a single data source",
  destructive: false,
  inputSchema: input,
  description: [
    "Return the full record for a source — kind, status, last_populated_at,",
    "target DuckDB table, primary key, CDC state, and any kind-specific",
    "config. This is the freshness-and-shape lookup the LLM uses before",
    "deciding to query the source's target table via duckdb_query.",
    "",
    "Key fields for routing:",
    "  - `target_table`: the DuckDB table you'd actually SELECT from.",
    "  - `status`: 'populated' means the table exists and has rows;",
    "             anything else means treat the source as not yet usable.",
    "  - `last_populated_at`: when the data was refreshed. Use to judge",
    "             whether the snapshot is fresh enough for the question.",
    "",
    "INPUT: { id }.",
    "RETURNS: full source row (verbatim from GraphStudio).",
  ].join("\n"),
  async execute(raw) {
    const { id } = input.parse(raw ?? {});
    const data = await http.get<SourceDetail>(`/api/sources/${encodeURIComponent(id)}`);
    return data;
  },
});
