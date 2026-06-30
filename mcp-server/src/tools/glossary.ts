import { z } from "zod";
import { defineTool } from "../tool.js";
import { GLOSSARY, GLOSSARY_INDEX } from "../glossary.js";

const input = z
  .object({
    term: z
      .string()
      .optional()
      .describe("If set, return only entries matching this term/synonym (case-insensitive)."),
  })
  .describe("Optional term filter.");

export const glossaryTool = defineTool({
  name: "glossary",
  title: "Quick-commerce glossary for Bolt Basket store_positions",
  destructive: false,
  inputSchema: input,
  description: [
    "Definitions for quick-commerce / dark-store terms grounded in the store_positions columns.",
    "Covers OHU, OOU, available, reserved, DOS, DOC, fill rate, min/max stock, reorder qty,",
    "dark store, service zone, delivery type, velocity, stockout, low stock, overstock,",
    "freshness, substitution chain, dead SKU, rating, complaint rate, hub, replenishment.",
    "",
    "Call this when the user uses quick-commerce jargon you want to map to specific columns,",
    "or when you need the canonical definition before composing a SQL filter.",
    "",
    "INPUT: { term? } — look up a specific term (case-insensitive).",
    "",
    "RETURNS: { entries: [{ term, meaning }] }.",
  ].join("\n"),
  async execute(raw) {
    const { term } = input.parse(raw ?? {});
    if (term) {
      const meaning = GLOSSARY_INDEX.get(term.trim().toLowerCase());
      if (!meaning) return { entries: [], matched: false };
      return { entries: [{ term: term.trim(), meaning }], matched: true };
    }
    const entries = Object.entries(GLOSSARY).map(([t, meaning]) => ({ term: t, meaning }));
    return { entries, matched: true };
  },
});
