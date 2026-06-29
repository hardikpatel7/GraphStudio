#!/usr/bin/env node
import express, { type Request, type Response } from "express";
import { randomUUID } from "node:crypto";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { fetch } from "undici";
import { createMcpServer } from "./server.js";
import { http } from "./http.js";
import { TOOLS } from "./register.js";

const PORT = Number(process.env.MCP_PORT ?? 3101);
const HOST = process.env.MCP_HOST ?? "127.0.0.1";
const AUTH_TOKEN = process.env.MCP_AUTH_TOKEN ?? "";

if (AUTH_TOKEN.length < 16) {
  process.stderr.write(
    "[smartstudio-mcp] FATAL: MCP_AUTH_TOKEN env var is required (min 16 chars).\n"
  );
  process.exit(1);
}

function authOk(header: string | undefined): boolean {
  if (!header || !header.startsWith("Bearer ")) return false;
  const given = header.slice("Bearer ".length);
  if (given.length !== AUTH_TOKEN.length) return false;
  // Constant-time-ish compare to avoid trivial timing leaks.
  let diff = 0;
  for (let i = 0; i < given.length; i++) {
    diff |= given.charCodeAt(i) ^ AUTH_TOKEN.charCodeAt(i);
  }
  return diff === 0;
}

const app = express();
app.use(express.json({ limit: "1mb" }));

// Health probe — intentionally no auth so the load balancer and systemd
// can call it. Reports SmartStudio reachability so a degraded backend
// shows up as a degraded MCP.
app.get("/healthz", async (_req: Request, res: Response) => {
  let ss: "ok" | "degraded" | "unreachable" = "unreachable";
  try {
    const r = await fetch(`${http.baseUrl}/api/health`);
    ss = r.ok ? "ok" : "degraded";
  } catch {
    ss = "unreachable";
  }
  res.json({ ok: true, smartstudio: ss, tools: TOOLS.length, version: "0.1.0" });
});

// Bearer-token gate on the MCP endpoint.
app.use("/mcp", (req: Request, res: Response, next) => {
  if (!authOk(req.header("authorization"))) {
    res.status(401).json({ error: "unauthorized" });
    return;
  }
  next();
});

// Streamable-HTTP MCP transport. We use session-tracking so that multi-turn
// Claude Code conversations keep the same Server instance across requests.
const transports = new Map<string, StreamableHTTPServerTransport>();

app.all("/mcp", async (req: Request, res: Response) => {
  const sessionId = req.header("mcp-session-id") ?? undefined;
  let transport: StreamableHTTPServerTransport | undefined = sessionId
    ? transports.get(sessionId)
    : undefined;

  if (!transport) {
    transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: () => randomUUID(),
      onsessioninitialized: (sid: string) => {
        transports.set(sid, transport!);
      },
    });
    transport.onclose = () => {
      const sid = transport!.sessionId;
      if (sid) transports.delete(sid);
    };
    const server = createMcpServer();
    await server.connect(transport);
  }

  await transport.handleRequest(req, res, req.body);
});

const listener = app.listen(PORT, HOST, () => {
  process.stderr.write(
    `[smartstudio-mcp] http listening on ${HOST}:${PORT}. SMARTSTUDIO_URL=${http.baseUrl}. ${TOOLS.length} tools.\n`
  );
});

function shutdown(signal: string) {
  process.stderr.write(`[smartstudio-mcp] received ${signal}, shutting down.\n`);
  listener.close(() => process.exit(0));
  // Hard exit if listener takes too long.
  setTimeout(() => process.exit(1), 5000).unref();
}
process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));
