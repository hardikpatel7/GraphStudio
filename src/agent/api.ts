// Typed wrappers over /api/agent/*. Single file so the App component can
// import everything cleanly. SSE events use the discriminated union below.

export type Workspace = {
  id: string;
  kind: "inventory" | "item" | "pricing" | "assort" | "plan";
  name: string;
  session_count: number;
  /** Number of tools the agent will expose for this workspace's kind.
   *  Zero = "Backend not yet configured" — the prompt endpoint rejects
   *  submissions for such kinds. Driven by the `workspace_kind_tools` table. */
  tool_count: number;
  config_json: unknown;
  created_at: number;
};

export type Session = {
  id: string;
  workspace_id: string;
  provider: string;
  model: string;
  title: string;
  created_at: number;
  last_active_at: number;
  prompt_count?: number;
  /** Server-derived: `chat` for user-opened sessions, `dashboard`
   *  for synthetic sessions that back a dashboard's widget runs (and
   *  the component-preview holder). Lets the UI render them in two
   *  separate lists instead of mixing them. */
  kind?: "chat" | "dashboard";
};

export type WorkspaceStats = {
  sessions_total: number;
  prompts: { total: number; done: number; errored: number; streaming: number };
  api_calls: { total: number; errors: number; cache_hits: number };
  tokens: { tokens_in_total: number; tokens_out_total: number; avg_latency_ms: number };
  cost_usd_total: number;
  cost_usd_avg: number;
  top_tools: Array<{ tool: string; count: number }>;
};

export type ModelEntry = {
  provider: string;
  model: string;
  display_name: string;
  backend: "rig" | "async_openai";
};

export type Prompt = {
  id: string;
  session_id: string;
  parent_prompt_id: string | null;
  user_text: string;
  model: string;
  status: "streaming" | "done" | "error";
  response_text: string | null;
  /** Captured error message when `status === "error"`. NULL on success or
   *  on pre-migration errored rows. */
  error: string | null;
  started_at: number;
  finished_at: number | null;
  tokens_in: number | null;
  tokens_out: number | null;
  latency_ms: number | null;
  /** Server-derived cost in USD. Null when no pricing_config row covers
   *  the prompt's `started_at`. */
  cost_usd: number | null;
};

export type PromptDetail = {
  prompt: Prompt;
  usage: { tokens_in: number; tokens_out: number; latency_ms: number } | null;
  api_calls: Array<{
    id: number;
    tool_name: string;
    started_at: number;
    duration_ms: number;
    bytes_in: number;
    bytes_out: number;
    status: string;
    error: string | null;
    /** Truncated JSON the model passed as arguments. Can come back as
     *  either a string OR a pre-parsed object — the backend's SQLite layer
     *  auto-parses TEXT columns that look like JSON. The `Section`
     *  component handles both shapes. Null on pre-update rows. */
    args_preview: string | Record<string, unknown> | unknown[] | null;
    /** Same dual-shape rationale as args_preview. */
    response_preview: string | Record<string, unknown> | unknown[] | null;
  }>;
  cost_usd: number | null;
  /** Server-derived per-component cost breakdown so the UI can show how
   *  the total was arrived at: model rate × tokens, per-call base + ms +
   *  bytes-out × tool multiplier, summed. */
  cost_breakdown: PromptCostBreakdown | null;
};

export type PromptCostBreakdown = {
  total_usd: number;
  tokens: {
    model: string;
    tokens_in: number;
    tokens_out: number;
    in_per_1k_usd: number;
    out_per_1k_usd: number;
    rate_found: boolean;
    input_cost_usd: number;
    output_cost_usd: number;
    subtotal_usd: number;
  } | null;
  tokens_subtotal_usd: number;
  calls: Array<{
    api_call_id: number;
    tool: string;
    status: string;
    duration_ms: number;
    bytes_out: number;
    multiplier: number;
    multiplier_key: string;
    base_call_usd: number;
    ms_cost_usd: number;
    bytes_cost_usd: number;
    pre_multiplier_usd: number;
    post_multiplier_usd: number;
  }>;
  calls_subtotal_usd: number;
  weights: {
    per_call_usd: number;
    per_ms_usd: number;
    per_byte_out_usd: number;
    default_multiplier: number;
  };
  pricing_effective_at: number;
};

export type SseEvent =
  | { type: "turn_started"; prompt_id: string; model: string }
  | { type: "text_delta"; text: string }
  | { type: "tool_call_started"; call_id: string; tool: string; args_preview: string }
  | { type: "tool_call_finished"; call_id: string; duration_ms: number; status: string; bytes_out: number }
  | { type: "usage"; tokens_in: number; tokens_out: number }
  | { type: "turn_finished"; prompt_id: string; final_text: string; latency_ms: number }
  | { type: "error"; message: string; retriable: boolean };

const j = async <T,>(p: Promise<Response>): Promise<T> => {
  const r = await p;
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json() as Promise<T>;
};

export const api = {
  listWorkspaces:    ()                                            => j<Workspace[]>(fetch("/api/agent/workspaces")),
  listSessions:      (wsId: string)                                => j<Session[]>(fetch(`/api/agent/workspaces/${wsId}/sessions`)),
  workspaceStats:    (wsId: string)                                => j<WorkspaceStats>(fetch(`/api/agent/workspaces/${wsId}/stats`)),
  createSession:     (wsId: string, body: { model: string; title?: string }) =>
    j<Session>(fetch(`/api/agent/workspaces/${wsId}/sessions`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),
  listModels:        ()                                            => j<ModelEntry[]>(fetch("/api/agent/models")),
  listPrompts:       (sessId: string)                              => j<Prompt[]>(fetch(`/api/agent/sessions/${sessId}/prompts`)),
  promptDetail:      (id: string)                                  => j<PromptDetail>(fetch(`/api/agent/prompts/${id}`)),
  deleteSession:     (sessId: string)                              =>
    fetch(`/api/agent/sessions/${sessId}`, { method: "DELETE" })
      .then(async (r) => { if (!r.ok) throw new Error(`${r.status} ${await r.text()}`); }),
  updateSession:     (sessId: string, body: Partial<{ title: string; model: string }>) =>
    j<Session>(fetch(`/api/agent/sessions/${sessId}`, {
      method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
    })),
};

/**
 * Submit a prompt and consume the SSE stream. Calls `onEvent` for each
 * parsed event; resolves when the stream closes. Errors during fetch
 * resolve via `onEvent({type:"error",...})` so callers handle them
 * uniformly with model errors.
 *
 * fetch + ReadableStream rather than EventSource: EventSource doesn't
 * support POST bodies, and our submit endpoint takes a JSON body.
 */
export async function submitPrompt(
  sessionId: string,
  userText: string,
  onEvent: (ev: SseEvent) => void,
): Promise<void> {
  let resp: Response;
  try {
    resp = await fetch(`/api/agent/sessions/${sessionId}/prompts`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ user_text: userText }),
    });
  } catch (e) {
    onEvent({ type: "error", message: (e as Error).message, retriable: true });
    return;
  }
  if (!resp.ok || !resp.body) {
    onEvent({ type: "error", message: `HTTP ${resp.status}: ${await resp.text()}`, retriable: false });
    return;
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  // SSE frames are separated by "\n\n"; each frame is one or more
  // "data: <json>" lines that we concatenate. Keep-alive newlines
  // between frames are ignored.
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf("\n\n")) >= 0) {
      const frame = buf.slice(0, idx);
      buf = buf.slice(idx + 2);
      const dataLines = frame
        .split("\n")
        .filter((l) => l.startsWith("data:"))
        .map((l) => l.slice(5).trim());
      if (dataLines.length === 0) continue;
      const payload = dataLines.join("\n");
      try {
        onEvent(JSON.parse(payload) as SseEvent);
      } catch {
        // ignore non-JSON keep-alives
      }
    }
  }
}
