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
  database: z
    .string()
    .optional()
    .describe(
      "Optional. Restrict the dictionary to a single CH database. Pushed into the server-side JOIN so both scan + payload drop proportionally. If the connection has a `default_database` hint and this param is omitted, the server falls back to that hint — see the `database_filter` field in the response for what was effectively used. Pass an explicit value to override the hint."
    ),
  include_columns: z
    .boolean()
    .default(false)
    .describe(
      "Default false — tables-only summary fits 200-table catalogs into ~3.5K tokens. Set true to get columns inline (~50x bigger; will overflow tool-result limits on a real catalog). Better pattern: get the tables list, then drill into one table's columns via clickhouse_query against system.columns."
    ),
  format: z
    .enum(["compact", "json"])
    .default("compact")
    .describe(
      "compact = DDL-ish text (one line per table, columns inline). json = full structured object. Default compact — JSON is ~3-4x more tokens for the same content."
    ),
});

interface Column {
  name: string;
  type: string;
  default_expression?: string;
  comment?: string;
  position?: number;
}

interface Table {
  name: string;
  engine?: string;
  total_rows?: number;
  total_bytes?: number;
  comment?: string;
  columns: Column[];
}

interface Database {
  name: string;
  engine?: string;
  comment?: string;
  tables: Table[];
}

interface DictionaryResponse {
  databases: Database[];
  duration_ms?: number;
  database_filter?: string;
}

/// Pretty an Int / null total_rows / total_bytes; strip the noisy "(0)" /
/// "0" cases that just clutter the output.
function fmtCount(n: number | null | undefined): string {
  if (n == null || n === 0) return "";
  if (n < 1000) return `${n}r`;
  if (n < 1_000_000) return `${Math.round(n / 1000)}Kr`;
  return `${(n / 1_000_000).toFixed(1)}Mr`;
}

function fmtBytes(n: number | null | undefined): string {
  if (n == null || n === 0) return "";
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}K`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)}M`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)}G`;
}

/// Build the compact text representation. One line per table:
///   db.table  [Engine, rows, bytes, "comment"]
///     col1:Type1, col2:Type2, ...
/// Or, when include_columns=false, just the header line.
function toCompact(data: DictionaryResponse, includeColumns: boolean): string {
  const out: string[] = [];
  for (const db of data.databases ?? []) {
    out.push(`# ${db.name}${db.engine ? ` (${db.engine})` : ""} — ${db.tables.length} tables`);
    if (db.comment) out.push(`  # ${db.comment}`);
    for (const t of db.tables ?? []) {
      const meta: string[] = [];
      if (t.engine) meta.push(t.engine);
      const rows = fmtCount(t.total_rows);
      if (rows) meta.push(rows);
      const bytes = fmtBytes(t.total_bytes);
      if (bytes) meta.push(bytes);
      if (t.comment) {
        // CH comments can contain literal newlines / tabs (we've seen
        // them embedded mid-sentence). Collapse so each table stays on
        // one line — that's the whole point of the compact shape.
        const flat = t.comment
          .replace(/[\r\n\t]+/g, " ")
          .replace(/\s{2,}/g, " ")
          .trim();
        meta.push(`"${flat.replace(/"/g, '\\"')}"`);
      }
      const header = `${db.name}.${t.name}${meta.length ? ` [${meta.join(", ")}]` : ""}`;
      if (!includeColumns || t.columns.length === 0) {
        out.push(header);
        continue;
      }
      out.push(`${header}:`);
      // Group columns onto lines of ~120 chars so the compact form
      // still wraps reasonably in any viewer.
      const segments: string[] = t.columns.map((c) => {
        const def = c.default_expression ? `=${c.default_expression}` : "";
        const cmt = c.comment
          ? ` /*${c.comment
              .replace(/[\r\n\t]+/g, " ")
              .replace(/\s{2,}/g, " ")
              .trim()}*/`
          : "";
        return `${c.name}:${c.type}${def}${cmt}`;
      });
      let line = "  ";
      for (const seg of segments) {
        if (line.length > 2 && line.length + seg.length + 2 > 120) {
          out.push(line.replace(/,\s*$/, ""));
          line = "  ";
        }
        line += seg + ", ";
      }
      if (line.length > 2) out.push(line.replace(/,\s*$/, ""));
    }
    out.push("");
  }
  return out.join("\n");
}

export const clickhouseDictionaryTool = defineTool({
  name: "clickhouse_dictionary",
  title: "ClickHouse data dictionary — databases, tables, columns",
  destructive: false,
  inputSchema: input,
  description: [
    "Return the schema landscape of a ClickHouse connection. Use this",
    "BEFORE clickhouse_query to plan SQL — pick the right database /",
    "table names and columns.",
    "",
    "OUTPUT MODES (in order of token cost):",
    "  - compact + include_columns=false (DEFAULT, ~99% smaller than json):",
    "      `arhaus_dev.product_details [ReplacingMergeTree, 628Kr, 119M]`",
    "    207-table arhaus_dev fits in ~14KB / ~3.5K tokens. The pattern:",
    "    call once for the table list, then drill into the few tables",
    "    you actually need via clickhouse_query, e.g.:",
    "      SELECT name, type, comment FROM system.columns",
    "      WHERE database='arhaus_dev' AND table='product_details'",
    "  - compact + include_columns=true:",
    "      Columns inline per table. ~50x bigger — will OVERFLOW the",
    "      tool-result limit on a real catalog (207 tables ≈ 750KB).",
    "      Use only for tiny databases or with `database=<name>` AND a",
    "      narrow expected size.",
    "  - format='json':",
    "      Full structured object with name / type / default / comment /",
    "      position for every column. ~6x bigger than compact. Use only",
    "      when a downstream consumer needs the structure.",
    "",
    "Always pass `database` to scope. The filter is pushed into the",
    "CH-side JOIN so both scan + payload drop proportionally.",
    "",
    "Comments come straight from CH (`CREATE TABLE … COMMENT '…'`),",
    "so well-curated catalogs surface business meaning here. Engine",
    "schemas (`system`, `INFORMATION_SCHEMA`, `information_schema`)",
    "are filtered out.",
    "",
    "INPUT: { connection_id, database?, include_columns?, format? }.",
    "RETURNS (compact): { format: 'compact', text, duration_ms,",
    "          databases_count, tables_count, columns_count }.",
    "RETURNS (json): { format: 'json', databases: [...], duration_ms }.",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const qs = args.database
      ? `?database=${encodeURIComponent(args.database)}`
      : "";
    const data = await http.get<DictionaryResponse>(
      `/api/connections/${encodeURIComponent(args.connection_id)}/dictionary${qs}`
    );
    if (args.database && data.databases.length === 0) {
      return {
        format: args.format,
        text: "",
        duration_ms: data.duration_ms,
        databases_count: 0,
        tables_count: 0,
        columns_count: 0,
        note: `Database '${args.database}' not found on this connection.`,
      };
    }
    const tables_count = data.databases.reduce(
      (n, db) => n + (db.tables?.length ?? 0),
      0
    );
    const columns_count = data.databases.reduce(
      (n, db) =>
        n + db.tables.reduce((m, t) => m + (t.columns?.length ?? 0), 0),
      0
    );
    if (args.format === "json") {
      return {
        format: "json" as const,
        databases: data.databases,
        duration_ms: data.duration_ms,
        database_filter: data.database_filter,
        databases_count: data.databases.length,
        tables_count,
        columns_count,
      };
    }
    const text = toCompact(data, args.include_columns);
    return {
      format: "compact" as const,
      text,
      duration_ms: data.duration_ms,
      database_filter: data.database_filter,
      databases_count: data.databases.length,
      tables_count,
      columns_count,
    };
  },
});
