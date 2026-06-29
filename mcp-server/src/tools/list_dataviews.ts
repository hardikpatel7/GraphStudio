import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z.object({}).describe("No inputs.");

interface DataViewRow {
  id: string;
  display_name: string;
  contract: unknown;
  dimensions: unknown;
  columns: Array<{ name: string; type: string; visible?: boolean }>;
  updated_at?: string;
  [k: string]: unknown;
}

export const listDataViewsTool = defineTool({
  name: "list_dataviews",
  title: "List dataviews",
  destructive: false,
  inputSchema: input,
  description: [
    "Return every dataview registered in SmartStudio. A dataview is a",
    "shaped, contract-bound table view — it lists the columns the planner",
    "will see, plus a contract describing how the data is served (gRPC",
    "engine, cache strategy, allowed operations) and any cascading filter",
    "configuration.",
    "",
    "Use this to discover what analytical tables are exposed to consumers,",
    "and the canonical column names the planner expects. Pair with",
    "describe_dataview when you need the full contract / column shape for",
    "a specific one.",
    "",
    "INPUT: none.",
    "RETURNS: { count, dataviews: [{ id, display_name, columns, ... }] }.",
  ].join("\n"),
  async execute() {
    const rows = await http.get<DataViewRow[]>("/api/dataviews");
    return { count: rows.length, dataviews: rows };
  },
});
