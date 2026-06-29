import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";
import { DUCKDB_TABLE } from "../schema.js";

const input = z
  .object({
    article: z.string().optional().describe("Article (SKU) identifier."),
    ph_code: z.string().optional().describe("Product hierarchy code."),
  })
  .refine((v) => Boolean(v.article || v.ph_code), {
    message: "Provide at least one of `article` or `ph_code`.",
  })
  .describe("Identify a single article by article or ph_code.");

interface QueryResponse {
  rows: Array<Record<string, unknown>>;
  columns: string[];
  total: number;
  duration_ms: number;
}

const MAP_COLUMNS = ["oh_map", "rq_map", "au_map"];

/** Best-effort parse of the JSON-ish *_map columns. Leaves the raw string in place if it doesn't parse. */
function decorateMaps(row: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...row };
  for (const c of MAP_COLUMNS) {
    const raw = row[c];
    if (typeof raw !== "string" || raw.length === 0) continue;
    try {
      out[`${c}__parsed`] = JSON.parse(raw);
    } catch {
      // Some maps may be {"k": v, ...} but with single quotes or k=v syntax — skip if not JSON.
    }
  }
  return out;
}

function escapeLiteral(v: string): string {
  return v.replace(/'/g, "''");
}

export const articleDetailTool = defineTool({
  name: "article_detail",
  title: "Single-article inventory card",
  destructive: false,
  inputSchema: input,
  description: [
    "Fetch the full row for one article from article_selection, with the per-node JSON-ish",
    "columns (oh_map, rq_map, au_map) parsed when they're valid JSON. Use this when the",
    "user asks about a specific article/SKU and wants the full picture.",
    "",
    "INPUT: { article? } or { ph_code? } — at least one required.",
    "",
    "RETURNS: { found, article: row (with *_map__parsed when parseable) }.",
  ].join("\n"),
  async execute(raw) {
    const { article, ph_code } = input.parse(raw);
    const clauses: string[] = [];
    if (article) clauses.push(`article = '${escapeLiteral(article)}'`);
    if (ph_code) clauses.push(`ph_code = '${escapeLiteral(ph_code)}'`);
    const where = clauses.join(" OR ");
    const sql = `SELECT * FROM ${DUCKDB_TABLE} WHERE ${where} LIMIT 1`;
    const data = await http.post<QueryResponse>("/api/query", { sql, limit: 1 });

    const row = data.rows[0];
    if (!row) {
      return { found: false, article: null, executed_sql: sql };
    }
    return {
      found: true,
      article: decorateMaps(row),
      executed_sql: sql,
      duration_ms: data.duration_ms,
    };
  },
});
