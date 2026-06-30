import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";
import { DUCKDB_TABLE } from "../schema.js";

const input = z
  .object({
    sku_code: z.string().describe("SKU identifier to look up."),
    dark_store_id: z
      .string()
      .optional()
      .describe("Optional dark store ID to scope the lookup to a specific location."),
  })
  .describe("Identify a single SKU position by sku_code and optional dark_store_id.");

interface QueryResponse {
  rows: Array<Record<string, unknown>>;
  columns: string[];
  total: number;
  duration_ms: number;
}

function escapeLiteral(v: string): string {
  return v.replace(/'/g, "''");
}

export const articleDetailTool = defineTool({
  name: "product_detail",
  title: "Single-SKU dark-store position card",
  destructive: false,
  inputSchema: input,
  description: [
    "Fetch the full row for one product/SKU from store_positions.",
    "Use when the user asks about a specific SKU at a specific dark store.",
    "",
    "INPUT: { sku_code } — required. { dark_store_id? } — optional, filters to one dark store.",
    "",
    "RETURNS: { found, product: row, executed_sql }.",
  ].join("\n"),
  async execute(raw) {
    const { sku_code, dark_store_id } = input.parse(raw);
    const clauses: string[] = [`sku_code = '${escapeLiteral(sku_code)}'`];
    if (dark_store_id) {
      clauses.push(`dark_store_id = '${escapeLiteral(dark_store_id)}'`);
    }
    const where = clauses.join(" AND ");
    const sql = `SELECT * FROM ${DUCKDB_TABLE} WHERE ${where} LIMIT 1`;
    const data = await http.post<QueryResponse>("/api/query", { sql, limit: 1 });

    const row = data.rows[0];
    if (!row) {
      return { found: false, product: null, executed_sql: sql };
    }
    return {
      found: true,
      product: row,
      executed_sql: sql,
      duration_ms: data.duration_ms,
    };
  },
});
