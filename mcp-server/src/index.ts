#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createMcpServer } from "./server.js";
import { http } from "./http.js";
import { TOOLS } from "./register.js";

async function main() {
  const server = createMcpServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stderr.write(
    `[smartstudio-mcp] stdio connected. SMARTSTUDIO_URL=${http.baseUrl}. ${TOOLS.length} tools registered.\n`
  );
}

main().catch((err) => {
  process.stderr.write(
    `[smartstudio-mcp] fatal: ${err instanceof Error ? err.stack ?? err.message : String(err)}\n`
  );
  process.exit(1);
});
