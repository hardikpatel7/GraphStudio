// Retail-inventory glossary tailored to the columns in article_selection.
// Used by the `glossary` tool to ground Claude Code's interpretation of prompts.

export interface GlossaryEntry {
  term: string;
  aka: string[];
  meaning: string;
  related_columns?: string[];
}

export const GLOSSARY: GlossaryEntry[] = [
  {
    term: "OH",
    aka: ["on hand", "on-hand", "stock on hand"],
    meaning: "Units physically on hand across DCs and stores.",
    related_columns: ["oh", "oh_map"],
  },
  {
    term: "OO",
    aka: ["on order", "on-order"],
    meaning: "Units on a PO that have been ordered but not yet received.",
    related_columns: ["oo"],
  },
  {
    term: "IT",
    aka: ["in transit", "in-transit"],
    meaning: "Units that have shipped from a source but have not yet arrived at their destination.",
    related_columns: ["it"],
  },
  {
    term: "NAI",
    aka: ["net available inventory", "available to allocate"],
    meaning: "Net available inventory: OH + OO + IT minus reserves and already-allocated units.",
    related_columns: ["net_available_inventory"],
  },
  {
    term: "Reserve",
    aka: ["reserve quantity", "held back"],
    meaning: "Units intentionally held back from allocation (e.g., for safety, future events).",
    related_columns: ["reserve_quantity", "rq_map"],
  },
  {
    term: "Allocated",
    aka: ["allocated units"],
    meaning: "Units already assigned to specific stores by the allocation engine.",
    related_columns: ["allocated_units", "au_map", "last_allocated"],
  },
  {
    term: "WOS",
    aka: ["weeks of supply", "weeks of cover", "current cover"],
    meaning: "Current weeks of supply — how many weeks the on-hand position covers at the recent sales rate.",
    related_columns: ["wos"],
  },
  {
    term: "WOC",
    aka: ["weeks of cover target", "target cover"],
    meaning: "Target weeks-of-cover band. min_woc / max_woc bracket the desired WOS.",
    related_columns: ["min_woc", "max_woc"],
  },
  {
    term: "Min stock",
    aka: ["minimum stock", "reorder point"],
    meaning: "Lower threshold of units below which the article is considered understocked.",
    related_columns: ["min_stock", "min_stock_validator"],
  },
  {
    term: "Max stock",
    aka: ["maximum stock", "overstock ceiling"],
    meaning: "Upper threshold of units above which the article is considered overstocked.",
    related_columns: ["max_stock", "max_stock_validator"],
  },
  {
    term: "APS",
    aka: ["average per-store sales", "avg per store"],
    meaning: "Average sales rate per store carrying the article.",
    related_columns: ["aps"],
  },
  {
    term: "In-stock %",
    aka: ["in stock perc", "service level"],
    meaning: "Percentage of eligible stores with at least one unit available.",
    related_columns: ["in_stock_perc"],
  },
  {
    term: "LW",
    aka: ["last week", "last-week"],
    meaning: "Trailing-week metrics (units sold, revenue, margin).",
    related_columns: ["lw_units", "lw_revenue", "lw_margin"],
  },
  {
    term: "RCL",
    aka: ["rules and constraint layer", "ruleset"],
    meaning: "Business-rules layer governing eligibility, allocation policy, and constraints. The article_selection table is materialized against an RCL snapshot identified by rcl_version.",
    related_columns: ["allocation_rules", "mapped_stores_count"],
  },
  {
    term: "Mapped stores",
    aka: ["eligible stores", "carrying stores"],
    meaning: "Number of stores that RCL permits to carry this article. mapped_stores_count = 0 means no store is eligible to receive it.",
    related_columns: ["mapped_stores_count"],
  },
  {
    term: "DC",
    aka: ["distribution center"],
    meaning: "A warehouse from which stores are replenished. dcs lists DCs holding this article.",
    related_columns: ["dcs"],
  },
  {
    term: "Stockout",
    aka: ["out of stock", "OOS"],
    meaning: "An article that has eligible stores but zero on-hand units (oh = 0 AND mapped_stores_count > 0).",
    related_columns: ["oh", "mapped_stores_count"],
  },
  {
    term: "Overstock",
    aka: ["overstocked", "above max"],
    meaning: "An article with on-hand units above the max_stock policy threshold.",
    related_columns: ["oh", "max_stock"],
  },
  {
    term: "Below min",
    aka: ["understock", "below reorder"],
    meaning: "An article with on-hand units below the min_stock policy threshold.",
    related_columns: ["oh", "min_stock"],
  },
  {
    term: "Reserve gap",
    aka: ["reserve shortfall"],
    meaning: "Reserved quantity exceeds net available inventory — reserve commitments cannot be fully honored from current position.",
    related_columns: ["reserve_quantity", "net_available_inventory"],
  },
  {
    term: "No eligible stores",
    aka: ["unmapped article"],
    meaning: "An article with mapped_stores_count = 0 — RCL does not currently permit any store to carry it.",
    related_columns: ["mapped_stores_count"],
  },
  {
    term: "Dead stock",
    aka: ["non-mover"],
    meaning: "An article holding units but not selling — typical heuristic: oh > 0 AND lw_units = 0.",
    related_columns: ["oh", "lw_units"],
  },
];

export const GLOSSARY_INDEX: Map<string, GlossaryEntry> = (() => {
  const idx = new Map<string, GlossaryEntry>();
  for (const e of GLOSSARY) {
    idx.set(e.term.toLowerCase(), e);
    for (const a of e.aka) idx.set(a.toLowerCase(), e);
  }
  return idx;
})();
