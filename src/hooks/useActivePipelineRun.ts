import { useEffect, useState } from "react";
import { api } from "@/api/client";

export type ActivePipelineRun = { pipeline_id: string; ran_for_ms: number };

/** How often `useActivePipelineRun` re-fetches `/api/pipelines/active`. */
export const ACTIVE_POLL_INTERVAL_MS = 5000;

/**
 * Polls `GET /api/pipelines/active` every {@link ACTIVE_POLL_INTERVAL_MS}
 * and returns the in-flight run (or `null` if idle).
 *
 * Shared by the sidebar (per-row spinner badge) and the pipeline workspace
 * (so Cancel stays available even when the user navigates back to a workspace
 * whose run was started in a previous browser session — local `running`
 * state is false there, but the server still holds the lock).
 */
export function useActivePipelineRun(): ActivePipelineRun | null {
  const [active, setActive] = useState<ActivePipelineRun | null>(null);
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const r = await api.getActivePipelineRun();
        if (cancelled) return;
        if (r && r.pipeline_id) {
          setActive({ pipeline_id: r.pipeline_id, ran_for_ms: r.ran_for_ms ?? 0 });
        } else {
          setActive(null);
        }
      } catch {
        if (!cancelled) setActive(null);
      }
    };
    tick();
    const id = window.setInterval(tick, ACTIVE_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);
  return active;
}

export function formatRunDuration(ms: number): string {
  if (ms < 60_000) return `${Math.floor(ms / 1000)}s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  return `${m}m ${s}s`;
}
