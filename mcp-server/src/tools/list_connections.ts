import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({}).describe("No inputs.");

interface ConnectionRow {
  id: string;
  display_name: string;
  type: string;
  is_default: number;
  [k: string]: unknown;
}

export const listConnectionsTool = defineTool({
  name: "list_connections",
  title: "List external database connections",
  destructive: false,
  inputSchema: input,
  description: [
    "Return every registered database connection (Postgres, ClickHouse,",
    "etc.). A connection wraps credentials + host info — sources and",
    "DataViews reference these by id via `connection_ref`.",
    "",
    "Use this when you need a `connection_id` for ClickHouse tools",
    "(clickhouse_dictionary / clickhouse_query). The `type` field tells",
    "you which engine the connection targets:",
    "  - `pg` / `postgres` → use sources + DataViews; no direct ad-hoc",
    "    query tool exists for PG.",
    "  - `clickhouse` → use clickhouse_dictionary (schema discovery)",
    "    and clickhouse_query (ad-hoc SQL).",
    "",
    "INPUT: none.",
    "RETURNS: { count, connections: [{ id, display_name, type, is_default,",
    "          ... }] }.",
  ].join("\n"),
  async execute() {
    const rows = await http.get<ConnectionRow[]>("/api/connections");
    return { count: rows.length, connections: rows };
  },
});
