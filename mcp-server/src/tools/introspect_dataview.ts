import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    id: z.string().min(1).describe("DataView id from list_dataviews."),
  })
  .describe("Target a single dataview for runtime-shape introspection.");

interface IntrospectResponse {
  source: unknown;
  columns: Array<{ name: string; type: string }>;
  engine: string;
}

export const introspectDataViewTool = defineTool({
  name: "introspect_dataview",
  title: "Resolve a dataview's runtime column shape",
  destructive: false,
  inputSchema: input,
  description: [
    "Return the live column shape of a dataview — `{ name, type }` per",
    "column — without paying for a data read.",
    "",
    "When to use:",
    "  - describe_dataview returned an empty `columns` array. That means",
    "    the shape is pipeline-driven, not statically declared. This tool",
    "    asks the backend what columns the source actually emits today.",
    "  - You want type information (VARCHAR / BIGINT / DOUBLE / etc.) to",
    "    inform downstream SQL or filtering.",
    "  - You want to confirm the column shape hasn't drifted before",
    "    composing a dataview_read query against it.",
    "",
    "The endpoint resolves the source's `kind` and dispatches accordingly:",
    "pg_query does a prepared-statement schema lookup, article_graph and",
    "uam_* read from in-memory projection registries, duckdb_table runs",
    "the underlying SELECT in a no-row mode against the tenant DuckDB.",
    "",
    "INPUT: { id }.",
    "RETURNS: { source, columns: [{ name, type }], engine }.",
  ].join("\n"),
  async execute(raw) {
    const { id } = input.parse(raw ?? {});
    const data = await http.post<IntrospectResponse>(
      `/api/dataviews/${encodeURIComponent(id)}/introspect-source`,
      {},
    );
    return data;
  },
});
