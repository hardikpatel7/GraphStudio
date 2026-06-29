import { z } from "zod";
import { http } from "../http.js";
import { defineTool } from "../tool.js";

const CATEGORIES = [
  "missing_endpoint",
  "data_gap",
  "ergonomics",
  "perf",
  "new_graph",
  "bug",
] as const;

const input = z
  .object({
    category: z.enum(CATEGORIES).describe(
      "Triage bucket. Pick the one that best matches the gap: missing_endpoint (an API call would have answered cleanly), data_gap (the data needed isn't reachable from the LLM's view), ergonomics (answer was reachable but the path was painful — many steps, JSON parsing, etc.), perf (worked but was slow enough to hurt UX), new_graph (a new graph/hierarchy would unlock a class of questions), bug (something behaved incorrectly)."
    ),
    summary: z.string().min(1).describe("One-line title for triage. Aim for ≤80 chars."),
    example_question: z
      .string()
      .optional()
      .describe(
        "The planner's actual prompt that triggered this. The single most useful field — it tells the developer what real users want to do."
      ),
    attempted_path: z
      .array(z.string())
      .optional()
      .describe(
        "Ordered list of MCP tools the LLM tried while answering, e.g. ['describe_graph','graph_node','duckdb_query']. Helps the developer see where the workflow bent."
      ),
    what_was_painful: z
      .string()
      .optional()
      .describe("Plain-English description of what made the path hard."),
    workaround: z
      .string()
      .optional()
      .describe("What the LLM did instead, if anything. Empty if the question couldn't be answered."),
    proposed_solution: z
      .string()
      .optional()
      .describe(
        "Optional. The LLM's suggestion for what would have made this easier — a new endpoint, a column to add to DuckDB, a metric on a graph, etc."
      ),
  })
  .describe("A structured feedback entry to file against GraphStudio's roadmap.");

interface CreateResponse {
  id: string;
  duration_ms: number;
}

export const submitFeedbackTool = defineTool({
  name: "submit_feedback",
  title: "File a feedback entry against GraphStudio",
  destructive: false,
  inputSchema: input,
  description: [
    "Record a structured note about something the LLM couldn't answer cleanly,",
    "or answered only via a painful path. The developer reviews these to decide",
    "what GraphStudio capability to build next — so this is the LLM's primary",
    "channel for shaping the backlog.",
    "",
    "WHEN TO CALL:",
    "  - You fell back to duckdb_query because the graph couldn't answer.",
    "  - You composed many tool calls where one should have sufficed.",
    "  - You parsed map / JSON-ish columns to derive per-cell metrics.",
    "  - You returned an estimate instead of a direct answer.",
    "  - Data was stale or missing for a real planner question.",
    "",
    "WHEN NOT TO CALL:",
    "  - Routine answers that worked cleanly via graph_node / describe.",
    "  - Speculative ideas not tied to a real question you just handled.",
    "  Filing noise dilutes the signal — only file when the friction was real.",
    "",
    "Always include `example_question` if you have it — it's the strongest",
    "prioritization signal the developer sees.",
    "",
    "INPUT: { category, summary, example_question?, attempted_path?,",
    "         what_was_painful?, workaround?, proposed_solution? }",
    "RETURNS: { id, duration_ms }",
  ].join("\n"),
  async execute(raw) {
    const args = input.parse(raw ?? {});
    const data = await http.post<CreateResponse>("/api/feedback", args);
    return {
      ok: true,
      summary: `Filed ${args.category} (${data.id}): ${args.summary}`,
      ...data,
    };
  },
});
