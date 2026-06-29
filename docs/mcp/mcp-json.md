# `.mcp.json` — reference

`.mcp.json` is the config file Claude Code reads to discover Model Context Protocol (MCP) servers. SmartStudio ships an MCP server at `mcp-server/` (8 tools over the `article_selection` DataView); this doc covers the file format itself. For the wiring steps, see [`claude-code-integration.md`](./claude-code-integration.md).

## Scope: where the file lives

| Location | Scope | When to use |
|---|---|---|
| `<repo>/.mcp.json` | Project (one repo) | A teammate who clones this repo should get the MCP automatically once they build the server. **Caveat**: paths must be portable (see below) — that's why this file is gitignored in SmartStudio. |
| `~/.claude.json` | User-global | The MCP follows you across every Claude Code session, regardless of `cwd`. Recommended for personal dev machines. |

Claude Code reads both. Project-scoped entries are layered on top of user-scoped ones.

## Schema

```jsonc
{
  "mcpServers": {
    "<server-name>": {                      // free-form id; appears in tool prefixes
      "command": "<executable>",            // node, python, a binary, etc.
      "args": ["<arg1>", "<arg2>", "..."],
      "env": {                              // process env (optional)
        "KEY": "value"
      }
    }
  }
}
```

That's the whole shape — one top-level `mcpServers` object, keyed by server name. Each value is a process spec.

### Fields

- **`mcpServers.<name>`** — Free-form identifier. Tools registered by this server get prefixed with `mcp__<name>__` in Claude Code (e.g. `mcp__smartstudio__query_articles`). Pick something short and unambiguous.
- **`command`** — The executable. Resolved against `$PATH` if unqualified; absolute paths skip `$PATH` lookup. Common choices: `node`, `python`, `uvx`, `bunx`, or a compiled binary.
- **`args`** — Argv after `command`. Most MCP servers want the entry-point script path here (e.g. the built `dist/index.js`). Each token is a separate array element — Claude Code does not split on spaces.
- **`env`** — Environment variables for the spawned process. **No `${VAR}` interpolation** — values are passed verbatim. If you need a dynamic value, set it in `command` via a wrapper script.

## SmartStudio example

The template lives at `mcp-server/.mcp.json.example`:

```jsonc
{
  "mcpServers": {
    "smartstudio": {
      "command": "node",
      "args": ["/Users/<you>/path/to/smartstudio/mcp-server/dist/index.js"],
      "env": {
        "SMARTSTUDIO_URL": "http://localhost:3001"
      }
    }
  }
}
```

- The `args` path must point at the **built** `dist/index.js` (run `npm run build` in `mcp-server/` first).
- `SMARTSTUDIO_URL` is read by the server (see `mcp-server/src/http.ts`); defaults to `http://localhost:3001` if absent.

## Why this file is gitignored

Two reasons, both about portability:

1. **Absolute paths**. `args` must point at the built `dist/index.js`, and there's no `${repo_root}` expansion. Committing it would bake one developer's home directory into the repo.
2. **Per-developer URL overrides**. A teammate running SmartStudio on a non-default port needs to change `SMARTSTUDIO_URL` without touching git history.

The example template at `mcp-server/.mcp.json.example` is committed; the live `.mcp.json` is not (see the root `.gitignore`).

## Multiple servers

`mcpServers` is a map — add more keys to register more servers. The same Claude Code session can talk to all of them simultaneously.

```jsonc
{
  "mcpServers": {
    "smartstudio": { "command": "node", "args": ["..."] },
    "filesystem":  { "command": "npx",  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] }
  }
}
```

## Common gotchas

- **`No tools available from 'smartstudio'`**. Almost always means `dist/index.js` doesn't exist — you skipped `npm run build`, or `args` points at the wrong path.
- **Server appears but tools are missing**. Stderr from the server is logged to `~/.claude/logs/` — check for startup errors there.
- **Path with spaces**. Wrap the whole path in quotes inside the JSON string, e.g. `"/Users/me/Documents and Settings/.../dist/index.js"`. JSON only cares about the surrounding `"`s.
- **Env vars don't expand**. `"args": ["$REPO/dist/index.js"]` literally passes the string `$REPO/dist/index.js` — Claude Code doesn't expand. Use absolute paths or a wrapper shell script.
- **Edits don't apply**. Claude Code reads `.mcp.json` at startup. Restart the session (or run `/mcp` to re-discover) after editing.
