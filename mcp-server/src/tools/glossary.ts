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
  title: "Retail-inventory glossary for this DataView",
  destructive: false,
  inputSchema: input,
  description: [
    "Definitions for retail-inventory terms grounded in the article_selection columns.",
    "Covers OH, OO, IT, NAI, reserve, allocated, WOS, WOC, min/max stock, APS, in-stock %,",
    "LW metrics, RCL, mapped stores, DC, and the standard exception rules (stockout,",
    "overstock, below-min, reserve gap, no-eligible-stores, dead stock).",
    "",
    "Call this when the user uses retail jargon you want to map to specific columns,",
    "or when you need the canonical definition before composing a SQL filter.",
    "",
    "INPUT: { term? } — look up a specific term (matches on term + synonyms).",
    "",
    "RETURNS: { entries: [{ term, aka, meaning, related_columns }] }.",
  ].join("\n"),
  async execute(raw) {
    const { term } = input.parse(raw ?? {});
    if (term) {
      const e = GLOSSARY_INDEX.get(term.trim().toLowerCase());
      return { entries: e ? [e] : [], matched: Boolean(e) };
    }
    return { entries: GLOSSARY, matched: true };
  },
});
