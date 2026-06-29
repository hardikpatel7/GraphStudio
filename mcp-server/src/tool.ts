import type { ZodTypeAny } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";

export interface Tool<S extends ZodTypeAny = ZodTypeAny> {
  name: string;
  title: string;
  description: string;
  inputSchema: S;
  /** Whether this tool mutates state (gates Claude Code permission prompt). */
  destructive: boolean;
  execute(input: unknown): Promise<unknown>;
}

export function defineTool<S extends ZodTypeAny>(t: Tool<S>): Tool<S> {
  return t;
}

export function toJsonSchema(schema: ZodTypeAny): Record<string, unknown> {
  const s = zodToJsonSchema(schema, { target: "openApi3" }) as Record<string, unknown>;
  // MCP servers expect a plain JSON Schema "object"; ensure $schema isn't leaked.
  delete s.$schema;
  return s;
}
