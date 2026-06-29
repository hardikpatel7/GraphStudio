import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const input = z
  .object({
    id: z.string().min(1).describe("DataView id from list_dataviews."),
  })
  .describe("Target a single dataview.");

interface DataViewDetail {
  id: string;
  display_name: string;
  contract: unknown;
  dimensions: unknown;
  columns: Array<{
    name: string;
    type: string;
    sortable?: boolean;
    searchable?: boolean;
    visible?: boolean;
    group?: string;
  }>;
  sort?: unknown;
  cascading_filters?: unknown;
  updated_at?: string;
  [k: string]: unknown;
}

export const describeDataViewTool = defineTool({
  name: "describe_dataview",
  title: "Describe a single dataview",
  destructive: false,
  inputSchema: input,
  description: [
    "Return the full dataview record — columns (with type, sortable,",
    "searchable, visible flags, and group), contract (engine, cache",
    "strategy, supported operations), dimensions, and any cascading filter",
    "configuration.",
    "",
    "Columns can be either:",
    "  - STATIC — declared on the dataview row; the array tells you the",
    "    canonical shape upfront.",
    "  - DYNAMIC — empty array on the record; the actual shape is produced",
    "    by the underlying pipeline at read time. This is a legitimate",
    "    state (e.g. in-memory UAM views, v8 graph-backed dataviews), NOT",
    "    a missing-metadata bug. Use `introspect_dataview` to discover the",
    "    runtime shape without paying for a read.",
    "",
    "The `contract` tells you HOW the data is served — if you need the",
    "actual rows, prefer `dataview_read` over composing SQL by hand.",
    "",
    "INPUT: { id }.",
    "RETURNS: full dataview row (verbatim from SmartStudio).",
  ].join("\n"),
  async execute(raw) {
    const { id } = input.parse(raw ?? {});
    const data = await http.get<DataViewDetail>(`/api/dataviews/${encodeURIComponent(id)}`);
    return data;
  },
});
