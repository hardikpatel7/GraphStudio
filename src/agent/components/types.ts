// Reusable widget component definitions. A component bundles a widget
// kind with a prompt TEMPLATE containing `<placeholder>` tokens. Dashboard
// widgets reference a component_id and supply per-instance values for
// each placeholder.

import type { WidgetKind } from "../dashboards/types";

export type Component = {
  id: string;
  workspace_id: string;
  name: string;
  description: string | null;
  kind: WidgetKind;
  prompt_template: string;
  /** Server-derived list of `{{placeholder}}` names in first-occurrence
   *  order. The client can also compute this from prompt_template with
   *  `extractPlaceholders` — same result. */
  placeholders: string[];
  created_at: number;
  updated_at: number;
};

/** Pull every `{{name}}` placeholder out of a template. Distinct names
 *  in first-occurrence order. Strict grammar — only
 *  `{{[a-zA-Z_][a-zA-Z0-9_]*}}` counts. Switched from `<name>` because
 *  the old syntax collided with literal `<` in SQL / HTML / JSON shape
 *  examples (a documented JSON value like `{"label":"<l1_name>"}`
 *  was being parsed as a required placeholder). */
export function extractPlaceholders(template: string): string[] {
  const out: string[] = [];
  const re = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;
  for (const m of template.matchAll(re)) {
    const name = m[1];
    if (!out.includes(name)) out.push(name);
  }
  return out;
}

/** Substitute placeholder values into a template. Missing values stay
 *  as their `{{token}}` text so the preview shows what's still unfilled. */
export function substitutePlaceholders(template: string, values: Record<string, string>): string {
  return template.replace(/\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g, (match, name) => {
    const v = values[name];
    return v != null && v !== "" ? v : match;
  });
}
