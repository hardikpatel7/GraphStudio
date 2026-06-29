import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";
import { DUCKDB_TABLE } from "../schema.js";

const input = z.object({
  sql: z
    .string()
    .min(1)
    .describe(
      "DuckDB SQL. Must SELECT (or WITH/FROM) and must reference the article_selection table. Use single-quoted literals."
    ),
  limit: z.number().int().min(1).max(1000).default(100).describe("Server-side limit (1-1000)."),
  offset: z.number().int().min(0).default(0).describe("Server-side offset."),
});

interface QueryResponse {
  rows: Array<Record<string, unknown>>;
  columns: string[];
  total: number;
  row_count: number;
  duration_ms: number;
}

const FORBIDDEN = /\b(INSERT|UPDATE|DELETE|DROP|TRUNCATE|ALTER|CREATE|ATTACH|COPY|EXPORT|VACUUM|REPLACE|MERGE|GRANT|REVOKE)\b/i;
const SELECT_LEAD = /^\s*(WITH|SELECT|FROM)\b/i;

function validateSql(sql: string): string {
  const trimmed = sql.trim().replace(/;+\s*$/, "");
  if (!SELECT_LEAD.test(trimmed)) {
    throw new Error(
      "query_articles only accepts SELECT / WITH / FROM-first SELECT statements."
    );
  }
  if (FORBIDDEN.test(trimmed)) {
    throw new Error("DDL/DML keywords are not allowed in query_articles.");
  }
  if (trimmed.includes(";")) {
    throw new Error("Multiple statements not allowed — single SELECT only.");
  }
  if (!new RegExp(`\\b${DUCKDB_TABLE}\\b`, "i").test(trimmed)) {
    throw new Error(
      `Query must reference the ${DUCKDB_TABLE} table. Other tables are out of scope in Phase 1.`
    );
  }
  return trimmed;
}

export const queryTool = defineTool({
  name: "query_articles",
  title: "Filtered / aggregated SQL over article_selection",
  destructive: false,
  inputSchema: input,
  description: [
    "Run a single SELECT (or WITH/FROM-first) DuckDB statement against the",
    "article_selection table. This is the primary read tool — use it for any filtered,",
    "aggregated, or grouped query.",
    "",
    "GUARDRAILS:",
    "- SELECT/WITH/FROM only; no DDL/DML.",
    "- One statement (no `;`).",
    "- Must reference the `article_selection` table.",
    "- limit/offset are applied automatically by the server on top of any LIMIT inside the SQL.",
    "",
    "TIPS:",
    "- Exception filters: stockout = oh=0 AND mapped_stores_count>0; overstock = oh>max_stock;",
    "  below-min = oh<min_stock; reserve gap = reserve_quantity>net_available_inventory;",
    "  no eligible stores = mapped_stores_count=0; below WOC = wos<min_woc.",
    "- Brand match: use exact comparison, e.g., brand = 'FILA' (call resolve_filter_values first",
    "  if unsure of casing).",
    "- Hierarchy: l1_name through l5_name.",
    "- Per-DC/store maps (oh_map, au_map, rq_map) are JSON-ish strings — use article_detail",
    "  for parsed views; SQL on them is brittle.",
    "",
    "INPUT: { sql, limit?, offset? }.",
    "",
    "RETURNS: { rows, columns, total, duration_ms, executed_sql }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const sql = validateSql(args.sql);
    const data = await http.post<QueryResponse>("/api/query", {
      sql,
      limit: args.limit,
      offset: args.offset,
    });
    return { ...data, executed_sql: sql };
  },
});
