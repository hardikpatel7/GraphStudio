# Deploying smartstudio-mcp

For deployments fronted by nginx on Ubuntu and orchestrated by AWX/Ansible. Co-locate one MCP server per SmartStudio tenant on the same host.

## Topology

```
Tenant host (Ubuntu — already running SmartStudio)
┌─────────────────────────────────────────────────────────┐
│ nginx                                                   │
│   /            → SmartStudio frontend (existing)        │
│   /api/*       → SmartStudio backend :3001 (existing)   │
│   /mcp         → smartstudio-mcp     :3101  (NEW)       │
│                                                         │
│ systemd                                                 │
│   smartstudio.service     (existing)                    │
│   smartstudio-mcp.service (NEW)                         │
└─────────────────────────────────────────────────────────┘
```

The MCP server talks to SmartStudio over loopback (`http://127.0.0.1:3001`) — no auth between them, no network hop. Users reach the MCP server through nginx over HTTPS with a bearer token.

## Build a release artifact

From the `mcp-server/` source tree:

```bash
npm run pack:release
# → release/smartstudio-mcp-<version>.tar.gz
# Prints the sha256 to put into mcp_artifact_checksum.
```

The tarball contains `package.json`, `package-lock.json`, production-only `node_modules/`, and the compiled `dist/`. No build tools needed on the target host — just Node.

Upload the tarball to your internal artifact store (Artifactory, Nexus, S3, etc.) and set `mcp_artifact_url` in the Ansible vars to its URL.

## Deploy with AWX

Files of interest in this repo:

```
mcp-server/deploy/
├── README.md                       (this file)
├── systemd/smartstudio-mcp.service (reference unit; the Ansible role templates the deployed copy)
├── env.example                     (env-file shape)
├── nginx/smartstudio-mcp.conf      (reference nginx snippet; same)
└── ansible/
    ├── example-playbook.yml
    └── roles/smartstudio_mcp/
        ├── defaults/main.yml
        ├── handlers/main.yml
        ├── meta/main.yml
        ├── tasks/main.yml
        └── templates/
            ├── env.j2
            ├── smartstudio-mcp.service.j2
            └── nginx-smartstudio-mcp.conf.j2
```

### AWX setup

1. **Project**: point AWX at the repo containing the `smartstudio_mcp` role (this one, or your roles repo if you keep them separate).
2. **Credentials**: create a Vault credential holding `mcp_auth_token`. Generate the value with `openssl rand -hex 32`. One token per tenant.
3. **Inventory**: group your tenant hosts (e.g., `smartstudio_tenants`). Each host should already have SmartStudio installed.
4. **Job Template**:
   - Playbook: `deploy/ansible/example-playbook.yml` (or your own that imports the role)
   - Inventory: as above
   - Credentials: the vault credential from step 2
   - Extra Vars (per-environment overrides):
     ```yaml
     mcp_version: "0.1.0"
     mcp_artifact_url: "https://artifacts.internal/smartstudio-mcp/smartstudio-mcp-0.1.0.tar.gz"
     mcp_artifact_checksum: "sha256:..."
     mcp_smartstudio_url: "http://127.0.0.1:3001"
     ```
5. **Run** — the role is idempotent. A re-run with the same `mcp_version` is a no-op on the install step; only env/systemd/nginx templating runs.

### Updating

Bump `mcp_version`, upload the new tarball, re-run the job template. The role:

1. Installs the new version under `/opt/smartstudio-mcp/<version>/`.
2. Atomically swaps the `current` symlink.
3. Restarts the service.
4. Waits for `/healthz` to return 200.

If `/healthz` doesn't come back, the job fails and the symlink is still pointing at the new version — flip it back manually or re-run with the previous `mcp_version` to roll back. The old version stays on disk under its versioned directory until you clean it up.

## Client configuration

Each user adds an entry to their Claude Code config (`~/.claude.json`):

```jsonc
{
  "mcpServers": {
    "smartstudio-bealls-uat": {
      "type": "http",
      "url": "https://bealls-uat.smartstudio.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${env:SMARTSTUDIO_MCP_TOKEN_BEALLS_UAT}"
      }
    }
  }
}
```

The token comes from AWX (distribute via your normal secret-sharing channel). For multi-tenant access, list each tenant as a separate server entry — they appear as separate tool namespaces in Claude Code.

## Operations

### Logs
```bash
journalctl -u smartstudio-mcp -f
journalctl -u smartstudio-mcp --since "1 hour ago"
```

### Healthcheck
```bash
curl https://<tenant>.example.com/mcp/healthz
# {"ok":true,"smartstudio":"ok","tools":8,"version":"0.1.0"}
```

The `smartstudio` field reports the backend's reachability:
- `ok` — `/api/health` returned 200
- `degraded` — `/api/health` returned non-200
- `unreachable` — connection failed

### Restart
```bash
sudo systemctl restart smartstudio-mcp
```

### Rotate the auth token

1. Generate a new token in AWX (re-roll the vault credential).
2. Re-run the AWX job template — the role re-templates `/etc/smartstudio-mcp/env` and restarts the service.
3. Distribute the new token to users; the old one stops working at restart.

## Resource expectations

The MCP server is essentially an Express process plus an SDK transport — very low footprint.

| Metric | Typical |
|---|---|
| RSS at idle | ~60–90 MB |
| RSS under load | ~120–200 MB |
| CPU | negligible except during JSON serialization of large result sets |
| Open file descriptors | small (one per active session) |

The heavy work happens in SmartStudio's DuckDB. The MCP server is a thin façade.

## Hardening notes

The systemd unit ships with `ProtectSystem=strict`, `PrivateTmp`, `NoNewPrivileges`, etc. Keep these on. The only writable path is `/var/log/smartstudio-mcp`, kept for future log-file output (today everything goes to journald).

The auth token is a static bearer — adequate for an internal-network deployment behind your existing nginx + TLS. If you need per-user attribution, OIDC integration is the natural next step (validate ID-tokens from your SSO inside the auth middleware). Plan ~1 day of work plus IdP coordination.
