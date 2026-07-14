---
name: setting-up-on-macos
description: Use when setting up GraphStudio on a fresh macOS machine, onboarding a teammate, or fixing a broken local dev environment — missing prerequisites, cargo build failures from missing protoc or cargo-watch, Bitbucket SSH auth errors, or a missing environment.toml.
---

# Setting Up GraphStudio on macOS

## Overview

End-to-end runbook to take a macOS machine from nothing to a running GraphStudio
dev environment (`npm run dev` → Vite `:5173` + Rust server `:3001`).

**Execution model:** run the steps in order. Every step is **idempotent** — the
check at the top of each step skips work that's already done, so it is safe to
re-run the whole skill. Do NOT stop except at a **🔴 GATE**.

## 🔴 Human-input gates (the only places you must pause)

| Gate | Why a human is required |
|---|---|
| Xcode Command Line Tools | `xcode-select --install` opens a GUI dialog — a human clicks "Install". |
| Homebrew install | The installer prompts for the sudo password interactively. |
| Bitbucket SSH access | The generated public key must be added to the user's Bitbucket account, **and** that account must be granted read access to the private `insideinsight/rust-shared-utils` repo. Nobody but the user/admin can do this. |
| LLM API keys (optional) | Only if the AI agent subsystem is used; the user supplies their own secret. |

Everything else installs automatically without prompting.

## ✅ Verification status

- **Verified** on a fully-set-up Mac: the detection/idempotency check at the top
  of every step (1–8) correctly reports the already-installed state and skips.
  The Node predicate (step 4) and the `environment.toml` rewrite (step 8) were
  each tested against real inputs.
- **Fixed after review**: unconditional `. ~/.cargo/env` (errors under a
  Homebrew Rust), a too-loose Node major check, a hard-coded Apple-Silicon brew
  prefix, and missing Bitbucket host-key enrollment — all corrected below.
- **Not yet verified**: the install branches (`brew install`, rustup, `cargo
  install cargo-watch`, `ssh-keygen`, `npm install`) — never exercised because
  the test machine already had everything. Command structure is correct but not
  execution-tested.
- **Not yet verified**: a clean-machine, end-to-end run and the final `npm run
  dev` launch driven through this skill.

To fully verify, run this on a fresh Mac (or a new user account / VM).

## Steps

Run all shell steps from the repo root — the `GraphStudio/` folder that contains
`package.json`, wherever it was cloned (e.g. `<your-projects>/GraphStudio/`). All
paths below are relative to that folder or use `$HOME`, so nothing is tied to a
specific machine or user.

### 1. Xcode Command Line Tools — 🔴 GATE if missing
```bash
xcode-select -p >/dev/null 2>&1 && echo "CLT present" || xcode-select --install
```
If it triggers the installer, **pause** and ask the human to complete the GUI
dialog, then continue.

### 2. Homebrew — 🔴 GATE if missing
```bash
command -v brew >/dev/null 2>&1 && brew --version || \
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```
The installer asks for the sudo password — **pause** for the human. Then make
sure the shell can find brew. The prefix differs by CPU — `/opt/homebrew` on
Apple Silicon, `/usr/local` on Intel — so detect it rather than hard-coding:
```bash
BREW="$([ -x /opt/homebrew/bin/brew ] && echo /opt/homebrew/bin/brew || echo /usr/local/bin/brew)"
grep -q 'brew shellenv' ~/.zprofile 2>/dev/null || \
  echo "eval \"\$($BREW shellenv)\"" >> ~/.zprofile
eval "$($BREW shellenv)"
```

### 3. Rust toolchain (rustup) — auto
```bash
command -v rustc >/dev/null 2>&1 && rustc --version || \
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# Put cargo on PATH for this shell. `~/.cargo/env` only exists for a rustup
# install — a Homebrew Rust puts cargo in the brew prefix instead, so guard the
# source (unconditional sourcing errors when the file is absent). Use explicit
# `if` blocks so the step still exits 0 when neither path is present (safe to
# paste into a `set -e` script).
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
if [ -d "$HOME/.cargo/bin" ]; then export PATH="$HOME/.cargo/bin:$PATH"; fi
```

### 4. Node.js — auto
Vite (locked version) requires Node **`^20.19.0 || >=22.12.0`**. A loose major
check would pass unsupported versions (20.0–20.18, 21.x, 22.0–22.11), so test the
minor too:
```bash
node -e 'const [a,b]=process.versions.node.split(".").map(Number);process.exit(((a===20&&b>=19)||(a===22&&b>=12)||a>=23)?0:1)' 2>/dev/null \
  && echo "Node $(node --version) OK" || brew install node
```
`brew install node` installs a current LTS (≥ 24) that satisfies the range. If an
older Node is already on PATH from another installer, it may shadow brew's — open
a fresh shell and re-run the check, or `brew link --overwrite node`.

### 5. protoc (Protocol Buffers compiler) — auto
Required by `tonic-build`/`build.rs`; without it the Rust build fails.
```bash
command -v protoc >/dev/null 2>&1 && protoc --version || brew install protobuf
```

### 6. cargo-watch — auto (compiles from source, a minute or two)
`npm run dev` runs `cargo watch`; without it the dev script fails.
```bash
cargo watch --version >/dev/null 2>&1 && cargo watch --version || cargo install cargo-watch
```

### 7. Bitbucket SSH access — 🔴 GATE if not authenticated
Rust deps pull `insideinsight/rust-shared-utils` over SSH. First enroll
Bitbucket's host key so the non-interactive (`BatchMode`) test below can't fail
on an unknown host on a fresh machine:
```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
ssh-keygen -F bitbucket.org >/dev/null 2>&1 || ssh-keyscan bitbucket.org >> ~/.ssh/known_hosts 2>/dev/null
```
Then test:
```bash
ssh -o BatchMode=yes -o ConnectTimeout=6 -T git@bitbucket.org 2>&1 | grep -qi 'authenticated' \
  && echo "Bitbucket SSH OK" || echo "NEEDS SETUP"
```
If it prints `NEEDS SETUP`:
```bash
# Create a key only if none exists
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519 -N "" -C "$(whoami)@$(hostname)"
eval "$(ssh-agent -s)"; ssh-add ~/.ssh/id_ed25519 2>/dev/null
echo "=== Give this public key to the human to add to Bitbucket ==="; cat ~/.ssh/id_ed25519.pub
```
**Pause.** The human must (a) add that key under Bitbucket → Personal settings →
SSH keys, and (b) confirm their Bitbucket account has read access to
`insideinsight/rust-shared-utils` (branch `develop/dev-v4`). Re-run the test
above until it prints `Bitbucket SSH OK` before continuing.

### 8. environment.toml — auto (deterministic default)
The server exits at boot if this file is missing, and `home_path` **must be an
absolute path that already exists** (the server won't create it). Create a
machine-independent data dir under `$HOME` and write it into a fresh config:
```bash
if [ -f environment.toml ]; then
  echo "environment.toml exists — leaving it untouched"
else
  cp environment.toml.example environment.toml
  mkdir -p "$HOME/graphstudio-data"
  # Replace the placeholder home_path with the dir we just created (BSD sed).
  sed -i '' "s|home_path = \"/path/to/your/data\"|home_path = \"$HOME/graphstudio-data\"|" environment.toml
  echo "Wrote home_path=$HOME/graphstudio-data — edit client/app_type/environment to taste."
fi
```
For a brand-new local tenant keep `is_new = true` for the first boot, then remove
that line. Tell the human which identity was set so they can change it if needed.

### 9. Install JS dependencies — auto
```bash
npm install
( cd mcp-server && npm install )
```

### 10. Pre-warm the Rust build — auto (several minutes: DuckDB + Tonic)
```bash
cargo build --manifest-path server/Cargo.toml
```

### 11. LLM API keys — 🔴 GATE, optional
Only needed for the AI agent subsystem. If used, have the human export their key
(e.g. `export ANTHROPIC_API_KEY=...`) in the shell that will run the server.
Otherwise skip.

## Verify

```bash
npm run dev
```
Confirm both processes come up, then in another shell:
```bash
curl -s http://localhost:3001/api/health && echo && echo "server OK"
```
Open http://localhost:5173 — the GraphStudio editor UI should load.

## Common Mistakes

| Symptom | Fix |
|---|---|
| `cargo build` fails resolving `rust-shared-utils` | Step 7 — Bitbucket SSH key/access not set up. |
| Build error mentioning `protoc` / `Could not find protoc` | Step 5 — `brew install protobuf`. |
| `npm run dev` errors on `cargo watch: command not found` | Step 6 — `cargo install cargo-watch`. |
| Server exits immediately with a config error | Step 8 — missing or malformed `environment.toml`; `home_path` must be an existing absolute path. |
| `brew: command not found` after install (Intel or Apple Silicon) | Step 2 — add the detected `brew shellenv` to `~/.zprofile`. |
| Vite errors about unsupported Node engine | Step 4 — Node must be `^20.19.0 \|\| >=22.12.0`; a stale older Node may be shadowing brew's (`brew link --overwrite node`, fresh shell). |
