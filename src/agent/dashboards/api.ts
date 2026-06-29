// REST wrappers for the dashboard endpoints. Mirrors the same `j` helper
// pattern as src/agent/api.ts so error handling stays uniform.

import type { DashboardDetail, DashboardLayout, DashboardSummary } from "./types";

const j = async <T,>(p: Promise<Response>): Promise<T> => {
  const r = await p;
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json() as Promise<T>;
};

export const dashboardsApi = {
  list:    (workspaceId: string) =>
    j<DashboardSummary[]>(fetch(`/api/agent/workspaces/${workspaceId}/dashboards`)),

  create:  (workspaceId: string, body: { name: string; description?: string }) =>
    j<DashboardSummary>(fetch(`/api/agent/workspaces/${workspaceId}/dashboards`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),

  get:     (id: string) =>
    j<DashboardDetail>(fetch(`/api/agent/dashboards/${id}`)),

  patch:   (id: string, body: Partial<{ name: string; description: string | null; layout_json: DashboardLayout; model: string }>) =>
    j<DashboardSummary>(fetch(`/api/agent/dashboards/${id}`, {
      method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),

  delete:  (id: string) =>
    fetch(`/api/agent/dashboards/${id}`, { method: "DELETE" })
      .then(async (r) => { if (!r.ok) throw new Error(`${r.status} ${await r.text()}`); }),

  /** Run a single widget's prompt. Returns the parsed payload
   *  (chart spec or `{ markdown }`). When `overrides` is empty (no
   *  args, or empty object) the run caches into widget_cache; when
   *  non-empty (drill-down) the cache is skipped so the saved
   *  payload stays the un-drilled view. */
  runWidget: (dashboardId: string, nodeId: string, overrides?: Record<string, string>) =>
    j<unknown>(fetch(`/api/agent/dashboards/${dashboardId}/widgets/${nodeId}/run`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ placeholder_overrides: overrides ?? {} }),
    })),

  /** Refresh every leaf widget in the tree, or just under `subtreeId`. */
  refresh:   (dashboardId: string, subtreeId?: string) =>
    j<{ ran: number; errors: Array<{ node_id: string; error: string }> }>(
      fetch(`/api/agent/dashboards/${dashboardId}/refresh`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(subtreeId ? { subtree_id: subtreeId } : {}),
      })
    ),
};
