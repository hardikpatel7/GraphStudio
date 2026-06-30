// Quick-commerce glossary tailored to the Bolt Basket store_positions DataView.
// Used by the `glossary` tool to ground Claude Code's interpretation of prompts.

export const GLOSSARY: Record<string, string> = {
  OHU:       'On-Hand Units — units physically present and countable in the dark store.',
  OOU:       'On-Order Units — units on inbound replenishment orders not yet received.',
  Available: 'Pickable units = on_hand_units minus reserved_units. The quantity an order can actually draw from right now.',
  Reserved:  'Units committed to in-flight orders that have not been picked yet.',
  DOS:       'Days of Supply — on_hand_units divided by daily_velocity. How many days the current stock will last at the current sales rate.',
  DOC:       'Days of Coverage — available_units divided by daily_velocity. Like DOS but excludes reserved stock.',
  'Fill rate': 'Percentage of ordered line items that were fulfilled from on-hand stock without substitution or cancellation.',
  'Min stock': 'Replenishment trigger level. When available_units drops to or below min_stock, a replenishment order should be raised.',
  'Max stock': 'Upper stocking limit for the dark store / SKU pair. Prevents over-ordering and storage overflow.',
  'Reorder qty': 'Standard order quantity raised when a replenishment trigger fires.',
  'Dark store':  'A micro-fulfilment center (MFC) — a warehouse optimised for rapid picking of online grocery orders, not open to walk-in shoppers.',
  'Service zone': 'The geographic delivery area served by a specific dark store.',
  'Delivery type': 'Speed tier for the delivery slot: express (minutes), standard (hours), or cold-chain (temperature-controlled).',
  Velocity:  'Sales rate. daily_velocity = 7-day rolling unit sales ÷ 7. Drives DOS and replenishment triggers.',
  Stockout:  'Condition where available_units = 0 — no stock left to fulfil orders for this SKU at this dark store.',
  'Low stock': 'available_units is above zero but at or below min_stock. The SKU is at risk of stocking out before replenishment arrives.',
  'Overstock': 'on_hand_units exceeds max_stock. Excess inventory ties up space and cash.',
  'Freshness': 'Shelf-life indicator for perishables. Tracked as days remaining to expiry for the oldest lot in the dark store.',
  'Substitution chain': 'An ordered list of alternative SKUs that can fulfil an order if the primary SKU is out of stock.',
  'Dead SKU': 'A SKU with zero velocity for 14+ days — likely discontinued, seasonal, or mis-catalogued.',
  'Rating':  'Average customer satisfaction score (1–5 stars) for a SKU at a specific dark store, from post-delivery survey responses.',
  'Complaint rate': 'Complaints (wrong item, damaged, missing) as a percentage of delivered orders for this SKU / dark store.',
  Hub:       'Central warehouse that replenishes multiple dark stores. Called a Distribution Center (DC) in traditional retail.',
  Replenishment: 'Inbound stock transfer from hub/warehouse to dark store, triggered when available_units hits min_stock.',
};

/** Case-insensitive lookup index over the glossary keys. */
export const GLOSSARY_INDEX: Map<string, string> = (() => {
  const idx = new Map<string, string>();
  for (const [term, meaning] of Object.entries(GLOSSARY)) {
    idx.set(term.toLowerCase(), meaning);
  }
  return idx;
})();
