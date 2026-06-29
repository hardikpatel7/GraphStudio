import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({
  sql: z
    .string()
    .min(1)
    .describe(
      "DuckDB SQL. Single SELECT (or WITH/FROM-first) statement against any table this tenant's DuckDB exposes. Use single-quoted string literals."
    ),
  limit: z
    .number()
    .int()
    .min(1)
    .max(5000)
    .default(500)
    .describe("Server-side row cap on top of any LIMIT in the SQL itself."),
  offset: z.number().int().min(0).default(0).describe("Server-side offset."),
});

interface QueryResponse {
  rows: Array<Record<string, unknown>>;
  columns: string[];
  total: number;
  row_count: number;
  duration_ms: number;
}

const FORBIDDEN =
  /\b(INSERT|UPDATE|DELETE|DROP|TRUNCATE|ALTER|CREATE|ATTACH|DETACH|COPY|EXPORT|IMPORT|VACUUM|REPLACE|MERGE|GRANT|REVOKE|CALL|PRAGMA)\b/i;
const SELECT_LEAD = /^\s*(WITH|SELECT|FROM)\b/i;

function validateSql(sql: string): string {
  const trimmed = sql.trim().replace(/;+\s*$/, "");
  if (!SELECT_LEAD.test(trimmed)) {
    throw new Error(
      "duckdb_query only accepts SELECT / WITH / FROM-first SELECT statements."
    );
  }
  if (FORBIDDEN.test(trimmed)) {
    throw new Error("DDL/DML keywords are not allowed; this is read-only.");
  }
  if (trimmed.includes(";")) {
    throw new Error("Multiple statements not allowed — single SELECT only.");
  }
  return trimmed;
}

export const duckDbQueryTool = defineTool({
  name: "duckdb_query",
  title: "Ad-hoc SQL over DuckDB — escape hatch when graphs can't answer",
  destructive: false,
  inputSchema: input,
  description: [
    "Run a single read-only DuckDB query. This is the FALLBACK path — reach",
    "for graph tools (graph_node / graph_traverse / graph_cross_filter) first,",
    "because they're constant-time over pre-aggregated metrics. duckdb_query",
    "scans the underlying tables and is slower for the same answer.",
    "",
    "WHEN TO USE:",
    "  - The metric or shape you need isn't pre-aggregated on a graph node.",
    "  - You need a custom GROUP BY, window function, JOIN, or pivot.",
    "  - You need to drill into raw rows under a node you've already located.",
    "  - The graph doesn't model the entity you're asking about.",
    "",
    "GUARDRAILS:",
    "  - SELECT / WITH / FROM-first only.",
    "  - No DDL, DML, ATTACH, COPY, PRAGMA, CALL — read-only.",
    "  - One statement (no `;`).",
    "  - limit (default 500, max 5000) and offset are applied server-side",
    "    on top of any LIMIT inside your SQL.",
    "",
    "DISCOVERY:",
    "  - Use list_sources / describe_source to find which DuckDB tables exist",
    "    and what columns each has.",
    "  - Use list_dataviews / describe_dataview to learn the planner-facing",
    "    column vocabulary.",
    "",
    "FEEDBACK:",
    "  - If you fell back to duckdb_query because a graph couldn't answer,",
    "    submit_feedback(category='data_gap' or 'ergonomics') so the developer",
    "    can decide whether to add the metric / endpoint that would have",
    "    served the question directly.",
    "",
    "INPUT: { sql, limit?, offset? }.",
    "RETURNS: { rows, columns, total, row_count, duration_ms, executed_sql }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const sql = validateSql(args.sql);
    const data = await http.post<QueryResponse>("/api/query", {
      sql,
      limit: args.limit,
      offset: args.offset,
    });
    return {
      ...data,
      executed_sql: sql,
      gap_hint:
        "Fallback SQL path. If this question would naturally fit a graph tool " +
        "(graph_node / graph_traverse / graph_cross_filter) but the graph couldn't " +
        "answer (missing dimension / metric / edge), call submit_feedback before " +
        "responding to the user, with example_question = the planner's actual " +
        "prompt. Skip if this was schema discovery (information_schema, " +
        "duckdb_tables(), describe table) or a genuinely SQL-shaped operation.",
    };
  },
});
