# graphstudio-mcp

MCP server exposing GraphStudio's data layer to Claude Code as typed tools — 
no new Anthropic key required.

## Tools

| Tool | Mutates | Purpose |
|---|---|---|
| `list_dataviews` | no | List all DataViews in the running tenant |
| `describe_dataview` | no | Schema dictionary for a DataView (columns + filter config) |
| `introspect_dataview` | no | Runtime schema introspection |
| `dataview_read` | no | Sorted, filtered, paginated read |
| `resolve_filter_values` | no | Distinct values for a named filter config |
| `query_duckdb` | no | SELECT-only SQL over tenant DuckDB |
| `list_graphs` | no | List all graphs in the tenant |
| `graph_traverse` | no | Walk edges in a graph snapshot |
| `graph_cross_filter` | no | Filter a graph snapshot, return attribute distincts |
| `product_detail` | no | Full row for one product/SKU, with JSON map columns parsed |
| `glossary` | no | Domain-specific term glossary for this tenant |
| `materialize_dataview` | yes | Run a DataView's source materializer |
| `dataview_status` | no | Freshness + row count for a materialized DataView |

`materialize_dataview` is the only write — Claude Code prompts before invoking it.

## Build

```bash
cd mcp-server
npm install
npm run build       # → dist/index.js
```

## Wire into Claude Code

Add to `~/.claude.json` (or project-scoped `.mcp.json` at repo root):

```jsonc
{
  "mcpServers": {
    "graphstudio": {
      "command": "node",
      "args": ["/path/to/GraphStudio/mcp-server/dist/index.js"],
      "env": { "SMARTSTUDIO_URL": "http://localhost:3001" }
    }
  }
}
```

Restart Claude Code. Tools appear under the `graphstudio` namespace.

## Prerequisites

1. GraphStudio Rust backend running on `:3001` (or wherever `SMARTSTUDIO_URL` points).
2. A default PG connection registered in GraphStudio (for materializers).

## Verification (first session)

In a Claude Code session, ask:

1. "What DataViews does this tenant have?" → calls `list_dataviews`
2. "Describe the store_positions DataView" → calls `describe_dataview`
3. "Show me the top 10 rows sorted by on_hand_units" → calls `dataview_read`
4. "What dark stores are available?" → calls `resolve_filter_values`
5. "Query the store positions for dark store DS001" → calls `query_duckdb`

## Dev

```bash
SMARTSTUDIO_URL=http://localhost:3001 npm run dev          # stdio
MCP_AUTH_TOKEN=$(openssl rand -hex 32) \
SMARTSTUDIO_URL=http://localhost:3001 npm run dev:http     # HTTP on :3101
```
