# smartstudio-mcp

Phase 1 MCP server. Exposes SmartStudio's `article_selection` DataView (46 inventory columns) to Claude Code as 8 typed tools — no new Anthropic key, no SmartStudio backend changes.

## Tools

| Tool | Mutates | Purpose |
|---|---|---|
| `materialize_article_selection` | yes | Run the article-selection materializer (PG → DuckDB) |
| `article_selection_status` | no | Freshness + row count |
| `describe_article_selection` | no | Schema dictionary (46 cols + filter config + dimension) |
| `glossary` | no | Retail-inventory term glossary keyed to columns |
| `resolve_filter_values` | no | Distinct values for the product filter (cascading) |
| `list_articles` | no | Sorted, paginated, unfiltered listing |
| `query_articles` | no | SELECT-only SQL over the `article_selection` DuckDB table |
| `article_detail` | no | Full row for one article, with `*_map` columns parsed |

`materialize_article_selection` is the only write — Claude Code prompts before invoking it.

## Build

```bash
cd mcp-server
npm install
npm run build       # → dist/index.js
```

## Wire into Claude Code

Add to `~/.claude.json` (or project-scoped `.mcp.json` at SmartStudio repo root):

```jsonc
{
  "mcpServers": {
    "smartstudio": {
      "command": "node",
      "args": ["/Users/karthickpachiappan/bb/smartstudio/mcp-server/dist/index.js"],
      "env": { "SMARTSTUDIO_URL": "http://localhost:3001" }
    }
  }
}
```

Restart Claude Code. Tools appear under the `smartstudio` namespace (e.g., `mcp__smartstudio__query_articles`).

## Prerequisites

1. SmartStudio Rust backend running on `:3001` (or wherever `SMARTSTUDIO_URL` points).
2. `[rcl] enabled = true` in `environment.toml` (materializer needs the in-process RCL ruleset).
3. A default PG connection registered in SmartStudio.

## Verification (first session)

In a Claude Code session, ask:

1. "Is the article selection up to date?" → calls `article_selection_status`.
2. "Materialize it now" → Claude Code prompts for `materialize_article_selection`, approve, returns timings + `rcl_version`.
3. "What columns does article_selection have?" → `describe_article_selection`.
4. "What brands exist?" → `resolve_filter_values({context: undefined})` returns L1/L2/.../brand values.
5. "Show me stockouts in brand FILA" → `query_articles` with `WHERE brand='FILA' AND oh=0 AND mapped_stores_count>0`.
6. "Tell me about article XYZ123" → `article_detail`.
7. "Sum OH by brand, top 10" → `query_articles` with GROUP BY.

## Constraints in this phase

- Scoped to one DataView (`dv_article_selection_v7`).
- No filter pass-through on `POST /api/dataviews/{id}/data` for duckdb_table sources — we use `POST /api/query` directly.
- No historical / WoW deltas (no snapshot history yet).
- No V8 article_graph traversal (next phase).
- No authoring tools (next phase).

## Dev

```bash
SMARTSTUDIO_URL=http://localhost:3001 npm run dev          # stdio (local Claude Code)
MCP_AUTH_TOKEN=$(openssl rand -hex 32) \
SMARTSTUDIO_URL=http://localhost:3001 npm run dev:http     # HTTP on :3101 (deployed shape)
```

Stdio banner prints on stderr; stdout is the MCP protocol channel.

## Production deployment

For AWX + Ubuntu + nginx see [`deploy/README.md`](./deploy/README.md). Build a release tarball with `npm run pack:release`, upload it to your artifact store, then run the included Ansible role.
