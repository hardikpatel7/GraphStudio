import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { TOOLS } from "./register.js";
import { toJsonSchema } from "./tool.js";
import { HttpError } from "./http.js";

/**
 * Build a configured Server instance with all 8 tools registered.
 * Transport binding (stdio vs HTTP) is the caller's responsibility.
 */
export function createMcpServer(): Server {
  const server = new Server(
    { name: "smartstudio-mcp", version: "0.1.0" },
    { capabilities: { tools: {} } }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: TOOLS.map((t) => ({
      name: t.name,
      title: t.title,
      description: t.description,
      inputSchema: toJsonSchema(t.inputSchema),
      annotations: {
        readOnlyHint: !t.destructive,
        destructiveHint: t.destructive,
        idempotentHint: !t.destructive,
        openWorldHint: false,
      },
    })),
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const tool = TOOLS.find((t) => t.name === req.params.name);
    if (!tool) {
      return {
        isError: true,
        content: [{ type: "text", text: `Unknown tool: ${req.params.name}` }],
      };
    }
    try {
      const result = await tool.execute(req.params.arguments ?? {});
      return {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
      };
    } catch (err) {
      const message =
        err instanceof HttpError
          ? `HTTP ${err.status}: ${err.message}\n${typeof err.body === "string" ? err.body : JSON.stringify(err.body)}`
          : err instanceof Error
            ? err.message
            : String(err);
      return {
        isError: true,
        content: [{ type: "text", text: `Tool '${tool.name}' failed:\n${message}` }],
      };
    }
  });

  return server;
}
