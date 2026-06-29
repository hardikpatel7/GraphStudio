import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({
  connection_id: z
    .string()
    .min(1)
    .describe(
      "Id of a connection with type=clickhouse. Get this from list_connections."
    ),
  sql: z
    .string()
    .min(1)
    .describe(
      "ClickHouse SQL. Single SELECT (or WITH/FROM-first) statement against the CH server this connection points at. Use single-quoted string literals. Qualify with database.table when needed."
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
  columns: Array<{ name: string }>;
  total: number;
  // `duration_ms` is the graphstudio-side wall-clock for the full
  // /api/connections/:id/run roundtrip (auth + HTTP + parse +
  // count-query). `client_ms` is just the CH HTTP call from the
  // graphstudio server's perspective (subset of duration_ms).
  // `server_ms` is CH server-side execution time parsed from
  // `X-ClickHouse-Summary` (`elapsed_ns / 1e6`); may be null when
  // CH didn't surface the header. `read_rows` / `read_bytes` come
  // from the same header and report the rows/bytes CH scanned to
  // compute the result.
  duration_ms?: number;
  client_ms?: number;
  server_ms?: number | null;
  read_rows?: number | null;
  read_bytes?: number | null;
}

const FORBIDDEN =
  /\b(INSERT|UPDATE|DELETE|DROP|TRUNCATE|ALTER|CREATE|RENAME|ATTACH|DETACH|OPTIMIZE|GRANT|REVOKE|KILL)\b/i;
const SELECT_LEAD = /^\s*(WITH|SELECT|FROM|EXPLAIN|DESCRIBE)\b/i;

function validateSql(sql: string): string {
  const trimmed = sql.trim().replace(/;+\s*$/, "");
  if (!SELECT_LEAD.test(trimmed)) {
    throw new Error(
      "clickhouse_query only accepts SELECT / WITH / FROM-first / EXPLAIN / DESCRIBE statements."
    );
  }
  if (FORBIDDEN.test(trimmed)) {
    throw new Error(
      "Write keywords are not allowed via this tool. The connection also enforces this server-side when allow_write_access=false."
    );
  }
  if (trimmed.includes(";")) {
    throw new Error("Multiple statements not allowed — single SELECT only.");
  }
  return trimmed;
}

export const clickhouseQueryTool = defineTool({
  name: "clickhouse_query",
  title: "Ad-hoc SQL over ClickHouse",
  destructive: false,
  inputSchema: input,
  description: [
    "Run a single read-only ClickHouse query against a registered CH",
    "connection. Use this when no DataView wraps the slice you need, or",
    "when you want to explore the CH catalog before defining a DataView.",
    "",
    "WORKFLOW:",
    "  1. list_connections → find a connection with type='clickhouse'.",
    "  2. clickhouse_dictionary → discover which databases / tables /",
    "     columns are available, with their comments and types.",
    "  3. clickhouse_query → run your SQL.",
    "",
    "GUARDRAILS:",
    "  - SELECT / WITH / FROM-first / EXPLAIN / DESCRIBE only.",
    "  - No DDL/DML — read-only. The connection's `allow_write_access`",
    "    flag is also enforced server-side as a second guard.",
    "  - One statement (no `;`).",
    "  - limit (default 500, max 5000) and offset are applied server-side",
    "    by wrapping your SQL as a subquery with LIMIT / OFFSET.",
    "",
    "PREFER DataViews WHEN POSSIBLE:",
    "  - If a DataView already wraps the query shape you need, use",
    "    dataview_read instead — DataViews are the documented planner",
    "    contract and benefit from filter / sort / pagination plumbing.",
    "  - clickhouse_query is the escape hatch for exploration and for",
    "    questions that don't fit a pre-built DataView.",
    "",
    "TIMING:",
    "  - `duration_ms` is the full graphstudio-side wall-clock for the",
    "    request (HTTP roundtrip + CH execution + JSON decode).",
    "  - `server_ms` is what ClickHouse itself reports (from",
    "    X-ClickHouse-Summary.elapsed_ns); subtracting it from",
    "    `duration_ms` is roughly network + graphstudio overhead.",
    "  - `read_rows` / `read_bytes` show the scan cost CH paid; useful",
    "    for understanding whether a query is hitting the right index.",
    "",
    "INPUT: { connection_id, sql, limit?, offset? }.",
    "RETURNS: { rows, columns, total, row_count, executed_sql,",
    "          duration_ms, client_ms, server_ms, read_rows, read_bytes }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const sql = validateSql(args.sql);
    const data = await http.post<QueryResponse>(
      `/api/connections/${encodeURIComponent(args.connection_id)}/run`,
      {
        sql,
        engine: "clickhouse",
        limit: args.limit,
        offset: args.offset,
      }
    );
    return {
      rows: data.rows,
      columns: data.columns,
      total: data.total,
      row_count: Array.isArray(data.rows) ? data.rows.length : 0,
      executed_sql: sql,
      duration_ms: data.duration_ms,
      client_ms: data.client_ms,
      server_ms: data.server_ms,
      read_rows: data.read_rows,
      read_bytes: data.read_bytes,
    };
  },
});
