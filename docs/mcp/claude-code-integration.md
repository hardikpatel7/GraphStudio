# Integrate `smartstudio-mcp` with Claude Code

End-to-end setup: build the MCP server, wire it into Claude Code, verify the tools load, and exercise them. For the underlying file format see [`mcp-json.md`](./mcp-json.md).

## Prerequisites

1. **Claude Code** installed and working (`claude --version`).
2. **SmartStudio backend** running on `:3001` (or wherever you'll point `SMARTSTUDIO_URL`):
   ```bash
   npm run dev:server:watch       # rebuild + restart on src/** changes
   # or for a fixed binary
   cargo run --manifest-path server/Cargo.toml
   ```
3. **Node 20+** for the MCP server (`node --version`).
4. **RCL enabled** in `environment.toml` (`[rcl] enabled = true`) — the materializer needs the in-process RCL ruleset.
5. **A default PG connection** registered in SmartStudio (only the legacy materializer needs this; the V7 DuckDB path doesn't, but seeding still expects a default).

## 1. Build the MCP server

```bash
cd mcp-server
npm install
npm run build         # writes dist/index.js
```

You should see `dist/index.js` after the build. Sanity check:

```bash
node dist/index.js < /dev/null
# expected: a one-line banner on stderr, then the process waits on stdin
# (MCP servers speak JSON-RPC over stdio). Ctrl-C to exit.
```

## 2. Pick a config scope

Two valid locations — see [`mcp-json.md`](./mcp-json.md) for the tradeoff:

- **Project-scoped** — `<repo-root>/.mcp.json`. Only active when Claude Code is launched from this repo.
- **User-global** — `~/.claude.json`. Active everywhere.

SmartStudio's repo root has `.mcp.json` gitignored, so project-scoped config is per-developer. Pick whichever fits your workflow.

## 3. Write the config

Copy the template:

```bash
cp mcp-server/.mcp.json.example .mcp.json
```

Edit the path to match your machine:

```jsonc
{
  "mcpServers": {
    "smartstudio": {
      "command": "node",
      "args": ["/absolute/path/to/smartstudio/mcp-server/dist/index.js"],
      "env": {
        "SMARTSTUDIO_URL": "http://localhost:3001"
      }
    }
  }
}
```

The `args` path **must** be absolute. Variables like `$HOME` or `$PWD` are not expanded (see gotchas in [`mcp-json.md`](./mcp-json.md)).

## 4. Restart Claude Code

Existing sessions don't auto-pick-up the new server. Quit and relaunch, or in an active session:

```
/mcp
```

This re-discovers servers and prints their status. You should see `smartstudio` listed with its tool count.

## 5. Verify the tools loaded

In a fresh session, ask Claude to list MCP tools or just probe:

> "What MCP tools do you have access to?"

You should see all eight under the `smartstudio` namespace:

| Tool                              | Mutates |
| --------------------------------- | ------- |
| `mcp__smartstudio__materialize_article_selection` | yes |
| `mcp__smartstudio__article_selection_status`      | no  |
| `mcp__smartstudio__describe_article_selection`    | no  |
| `mcp__smartstudio__glossary`                      | no  |
| `mcp__smartstudio__resolve_filter_values`         | no  |
| `mcp__smartstudio__list_articles`                 | no  |
| `mcp__smartstudio__query_articles`                | no  |
| `mcp__smartstudio__article_detail`                | no  |

(See `mcp-server/README.md` for what each one does.)

## 6. First-session smoke test

Walk through these prompts in order — each exercises a different tool. The materialize step is the only write; Claude Code will ask before invoking it.

1. *"Is the article selection up to date?"* — `article_selection_status`.
2. *"Materialize it now"* — approves `materialize_article_selection`; returns timings + RCL version.
3. *"What columns does article_selection have?"* — `describe_article_selection`.
4. *"What brands exist?"* — `resolve_filter_values`.
5. *"Show me stockouts in brand FILA"* — `query_articles` (`WHERE brand='FILA' AND oh=0 AND mapped_stores_count>0`).
6. *"Tell me about article XYZ123"* — `article_detail`.
7. *"Sum OH by brand, top 10"* — `query_articles` with `GROUP BY`.

If all seven respond cleanly, the integration is healthy.

## Troubleshooting

### Server doesn't appear in `/mcp`

- Confirm `.mcp.json` is at the right location (repo root or `~/.claude.json`).
- Validate JSON: `python3 -m json.tool .mcp.json` or `jq . .mcp.json`.
- Check Claude Code logs at `~/.claude/logs/` — there's usually a parse error there.

### Server appears but tools are missing

- Stderr from the server lands in `~/.claude/logs/` — open the latest file and grep for `smartstudio`. Common causes: `dist/index.js` not built yet, `SMARTSTUDIO_URL` unreachable, port collision.
- Run the server standalone to surface errors immediately:
  ```bash
  SMARTSTUDIO_URL=http://localhost:3001 node /abs/path/to/mcp-server/dist/index.js < /dev/null
  ```

### `materialize_article_selection` fails with "no default PG data source"

Register a PG connection in SmartStudio and mark it default. UI: Connections workspace → toggle "default" on a `type=pg` row. Or via SQL:

```sql
UPDATE connections SET is_default = 1 WHERE id = '<your-pg-conn-id>';
```

### Tools work but data looks stale

`materialize_article_selection` triggers a rebuild of the `article_selection` DuckDB table. Re-run it (or call `article_selection_status` to see the timestamp).

### Editing `.mcp.json` doesn't take effect

Claude Code reads it at startup. Either run `/mcp` to re-discover or restart the session.

## Related docs

- [`mcp-json.md`](./mcp-json.md) — `.mcp.json` file format reference.
- [`../../mcp-server/README.md`](../../mcp-server/README.md) — server-side tool catalogue, prereqs, dev workflow.
- [`prompts/`](./prompts/) — analysis transcripts and outputs from earlier MCP sessions.
