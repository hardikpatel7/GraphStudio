// REST wrappers for component CRUD. Mirrors the dashboards api shape.

import type { WidgetKind } from "../dashboards/types";
import type { Component } from "./types";

const j = async <T,>(p: Promise<Response>): Promise<T> => {
  const r = await p;
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json() as Promise<T>;
};

export const componentsApi = {
  list:   (workspaceId: string) =>
    j<Component[]>(fetch(`/api/agent/workspaces/${workspaceId}/components`)),

  create: (workspaceId: string, body: {
    name: string;
    description?: string;
    kind: WidgetKind;
    prompt_template: string;
  }) =>
    j<Component>(fetch(`/api/agent/workspaces/${workspaceId}/components`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),

  patch:  (id: string, body: Partial<{
    name: string;
    description: string | null;
    kind: WidgetKind;
    prompt_template: string;
  }>) =>
    j<Component>(fetch(`/api/agent/components/${id}`, {
      method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),

  delete: (id: string) =>
    fetch(`/api/agent/components/${id}`, { method: "DELETE" })
      .then(async (r) => { if (!r.ok) throw new Error(`${r.status} ${await r.text()}`); }),

  /** Ad-hoc preview: runs a (kind, template, placeholder_values) through
   *  the same pipeline as dashboard widgets, returns the renderer-ready
   *  payload (chart spec or `{markdown}`). No component row required;
   *  no widget_cache write. */
  preview: (workspaceId: string, body: {
    kind: WidgetKind;
    prompt_template: string;
    placeholder_values: Record<string, string>;
  }) =>
    j<unknown>(fetch(`/api/agent/workspaces/${workspaceId}/components/preview`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),
};
