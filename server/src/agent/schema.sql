-- SmartStudio Agent — SQLite schema. Applied via `CREATE TABLE IF NOT EXISTS`
-- so re-running on an existing DB is a no-op. Additive evolution goes through
-- `AgentDb::run_migrations`.

CREATE TABLE IF NOT EXISTS workspace (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('inventory','item','pricing','assort','plan')),
  name        TEXT NOT NULL,
  config_json TEXT NOT NULL DEFAULT '{}',
  created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS session (
  id              TEXT PRIMARY KEY,
  workspace_id    TEXT NOT NULL REFERENCES workspace(id),
  provider        TEXT NOT NULL,
  model           TEXT NOT NULL,
  title           TEXT,
  provider_state  TEXT,
  -- Pre-discovered schema overview injected into every turn's system prompt.
  -- Populated by `agent::schema::discover` at session creation (or lazily on
  -- the first prompt when the pre-warm hasn't finished). NULL until then.
  schema_hint     TEXT,
  created_at      INTEGER NOT NULL,
  last_active_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_session_ws ON session(workspace_id, last_active_at DESC);

CREATE TABLE IF NOT EXISTS prompt (
  id                TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES session(id),
  parent_prompt_id  TEXT REFERENCES prompt(id),
  user_text         TEXT NOT NULL,
  model             TEXT NOT NULL,
  status            TEXT NOT NULL,
  response_text     TEXT,
  -- Captured at the moment the run failed (Rig error chain, max-turns,
  -- context overflow, …). NULL on successful prompts. Surfaced in the
  -- prompt-detail drawer so replaying an old session still shows why a
  -- prior prompt errored — without this the row says only "errored".
  error             TEXT,
  started_at        INTEGER NOT NULL,
  finished_at       INTEGER
);
CREATE INDEX IF NOT EXISTS ix_prompt_session ON prompt(session_id, started_at);
CREATE INDEX IF NOT EXISTS ix_prompt_parent  ON prompt(parent_prompt_id);

CREATE TABLE IF NOT EXISTS response_chunk (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  prompt_id  TEXT NOT NULL REFERENCES prompt(id),
  seq        INTEGER NOT NULL,
  kind       TEXT NOT NULL,
  payload    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_chunk_prompt ON response_chunk(prompt_id, seq);

CREATE TABLE IF NOT EXISTS llm_usage (
  prompt_id   TEXT PRIMARY KEY REFERENCES prompt(id),
  model       TEXT NOT NULL,
  tokens_in   INTEGER NOT NULL,
  tokens_out  INTEGER NOT NULL,
  latency_ms  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS api_call (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  prompt_id       TEXT NOT NULL REFERENCES prompt(id),
  tool_name       TEXT NOT NULL,
  started_at      INTEGER NOT NULL,
  duration_ms     INTEGER NOT NULL,
  bytes_in        INTEGER NOT NULL,
  bytes_out       INTEGER NOT NULL,
  status          TEXT NOT NULL,
  error           TEXT,
  -- Truncated copies of the JSON args the model sent and the JSON result
  -- the tool returned. Useful for the prompt-detail drawer so the user
  -- can audit what the agent actually queried. Bounded in `meter::hook`
  -- to keep the row size sane.
  args_preview    TEXT,
  response_preview TEXT
);
CREATE INDEX IF NOT EXISTS ix_api_call_prompt ON api_call(prompt_id);
CREATE INDEX IF NOT EXISTS ix_api_call_tool   ON api_call(tool_name, started_at);

CREATE TABLE IF NOT EXISTS pricing_config (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  effective_from  INTEGER NOT NULL,
  weights         TEXT NOT NULL,
  notes           TEXT
);
CREATE INDEX IF NOT EXISTS ix_pricing_eff ON pricing_config(effective_from DESC);

CREATE TABLE IF NOT EXISTS model_allowlist (
  provider     TEXT NOT NULL,
  model        TEXT NOT NULL,
  display_name TEXT NOT NULL,
  backend      TEXT NOT NULL DEFAULT 'rig'
                 CHECK (backend IN ('rig','async_openai')),
  enabled      INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (provider, model)
);

-- Reusable widget components. A component bundles a widget kind with a
-- prompt TEMPLATE that contains `<placeholder>` tokens. Dashboard widgets
-- can reference a component by id and supply per-instance values for the
-- placeholders, so the same component can be reused with different brand
-- / metric / period parameters.
CREATE TABLE IF NOT EXISTS component (
  id              TEXT PRIMARY KEY,
  workspace_id    TEXT NOT NULL REFERENCES workspace(id),
  name            TEXT NOT NULL,
  description     TEXT,
  kind            TEXT NOT NULL,                -- widget kind: kpi|bar|line|pie|table|text
  prompt_template TEXT NOT NULL,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_component_ws ON component(workspace_id, updated_at DESC);

-- Persisted dashboards. Each row owns a `layout_json` (composition tree of
-- rows/columns/widget leaves) and a synthetic `session_id` — every widget
-- run is a prompt in that session so cost tracking + the prompt-detail
-- drawer come for free.
CREATE TABLE IF NOT EXISTS dashboard (
  id              TEXT PRIMARY KEY,
  workspace_id    TEXT NOT NULL REFERENCES workspace(id),
  session_id      TEXT NOT NULL REFERENCES session(id),
  name            TEXT NOT NULL,
  description     TEXT,
  layout_json     TEXT NOT NULL DEFAULT '{"version":1,"root":{"type":"column","id":"root","children":[]}}',
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_dashboard_ws ON dashboard(workspace_id, updated_at DESC);

-- Per-widget cached result. Keyed on (dashboard_id, node_id) — `node_id`
-- is the widget node's id inside the layout tree. `spec_hash` covers
-- (kind, prompt) so editing either invalidates the cache automatically.
-- `data_json` holds the parsed payload the renderer feeds the widget
-- (chart spec for kpi/bar/line/pie; raw markdown string for table/text).
-- `prompt_id` back-points to the prompt row, so the UI can deep-link
-- into the existing prompt-detail drawer for debugging a widget.
CREATE TABLE IF NOT EXISTS widget_cache (
  dashboard_id    TEXT NOT NULL REFERENCES dashboard(id),
  node_id         TEXT NOT NULL,
  spec_hash       TEXT NOT NULL,
  data_json       TEXT NOT NULL,
  fetched_at      INTEGER NOT NULL,
  prompt_id       TEXT REFERENCES prompt(id),
  PRIMARY KEY (dashboard_id, node_id)
);

-- Maps each workspace kind to the agent tool names it should expose.
-- One row per allowed (kind, tool_name) pair. Seeded on boot with defaults
-- (see `agent::config::seed_workspace_kind_tools`); admin overrides via
-- direct SQL or a future PATCH route. The agent's `tools::for_kind` reads
-- this table at agent-build time and only instantiates the listed tools.
CREATE TABLE IF NOT EXISTS workspace_kind_tools (
  kind      TEXT NOT NULL CHECK (kind IN ('inventory','item','pricing','assort','plan')),
  tool_name TEXT NOT NULL,
  PRIMARY KEY (kind, tool_name)
);
