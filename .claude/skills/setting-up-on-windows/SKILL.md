---
name: setting-up-on-windows
description: Use when setting up GraphStudio on a fresh Windows machine, onboarding a teammate on Windows, or fixing a broken local dev environment — missing prerequisites, cargo build failures from a missing MSVC linker, Perl, or protoc, missing cargo-watch, Bitbucket SSH auth errors, or a missing/misconfigured environment.toml.
---

# Setting Up GraphStudio on Windows

## Overview

End-to-end runbook to take a Windows 10/11 (x64) machine from nothing to a
running GraphStudio dev environment (`npm run dev` → Vite `:5173` + Rust server
`:3001`).

**Shell:** run every command in **PowerShell**. Steps that install machine-wide
tools or change services need an **elevated (Administrator)** PowerShell — those
are marked **(elevated)**. Works on both Windows PowerShell 5.1 and PowerShell
7+; version-sensitive commands handle both.

**Execution model:** run the steps in order. Every step is **idempotent** — the
check at the top skips work already done, so re-running the whole skill is safe.
Each installer aborts on a non-zero exit code (see the `Install-WinGet` helper),
so a failure stops the run instead of silently continuing. Do NOT stop except at
a **🔴 GATE**.

## 🔴 Human-input gates (the only places you must pause)

| Gate | Why a human is required |
|---|---|
| winget missing | If `winget` isn't present it must be installed from the Microsoft Store ("App Installer") — a GUI action. This is a hard stop, not a warning. |
| Visual Studio C++ Build Tools | Large install with a UAC prompt; the "Desktop development with C++" workload is required for Rust's MSVC linker. |
| winget UAC prompts | Machine-wide installs raise a UAC elevation prompt to confirm. |
| Bitbucket SSH access | The generated public key must be added to the user's Bitbucket account, **and** that account must be granted read access to the private `insideinsight/rust-shared-utils` repo. Nobody else can do this. |
| LLM API keys (optional) | Only if the AI agent subsystem is used; the user supplies their own secret. |

## ⚠️ Verification status

This runbook was authored and reviewed on macOS, cross-checked against the
project's real `package.json`, `vite.config.ts`, `server/Cargo.toml`, and
`Cargo.lock`. The command *structure* and the dependency reasoning (MSVC, Perl
for vendored OpenSSL, protoc, Node range) are grounded in those files, but the
PowerShell has **not been executed on a real Windows machine**. Treat winget
package IDs as "confirm with `winget search <name>` if an install fails," and
run this end-to-end on a clean Windows VM to fully verify.

## Preflight

Run all shell steps from the repo root — the `GraphStudio` folder that contains
`package.json`. **Clone to a short path** (e.g. `C:\dev\GraphStudio`, not a deep
`Documents\...` tree): the Rust build unpacks large native sources
(DuckDB, OpenSSL, aws-lc) that can hit Windows' path-length limit.

Enable long paths and define two helpers used throughout. Run the registry line
in an **(elevated)** shell once:
```powershell
# (elevated) allow >260-char paths for the native build
New-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
  -Name LongPathsEnabled -Value 1 -PropertyType DWORD -Force | Out-Null

# Refresh PATH in the CURRENT session from the machine + user environment.
# winget updates the stored PATH but not the live shell, so call this after installs.
function Update-SessionPath {
  $env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' +
              [Environment]::GetEnvironmentVariable('Path','User')
}

# Install via winget, auto-accepting agreements, failing loudly, then refresh PATH.
function Install-WinGet($id) {
  winget install --id $id -e --source winget `
    --accept-package-agreements --accept-source-agreements
  if ($LASTEXITCODE -ne 0) { throw "winget install $id failed (exit $LASTEXITCODE)" }
  Update-SessionPath
}
```

## Steps

### 1. winget (App Installer) — 🔴 GATE if missing
```powershell
if (Get-Command winget -ErrorAction SilentlyContinue) { winget --version } else {
  throw "winget not found. Install 'App Installer' from the Microsoft Store, reopen PowerShell, and re-run." }
```

### 2. Git — auto
```powershell
if (Get-Command git -ErrorAction SilentlyContinue) { git --version } else { Install-WinGet 'Git.Git' }
git config --global core.longpaths true
```

### 3. Visual Studio C++ Build Tools (MSVC toolchain) — 🔴 GATE
Rust's default `stable-msvc` toolchain needs the MSVC linker, and the vendored
OpenSSL build (step 6) runs `nmake` from this toolset. Detect the actual VC
component with `vswhere` (a directory check gives false positives and misses
Community/Pro/Enterprise):
```powershell
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVC = (Test-Path $vswhere) -and (& $vswhere -latest -products * `
  -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if ($hasVC) { "MSVC C++ tools present: $hasVC" } else {
  winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget `
    --accept-package-agreements --accept-source-agreements `
    --override "--wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
}
```
**Pause** for the UAC/installer. The `--add ...Workload.VCTools` flag installs the
workload non-interactively; if the GUI appears, ensure **Desktop development with
C++** is selected. Then **verify** (re-run the `vswhere` check above — it must now
print an install path). Rust and `openssl-src` locate `link.exe`/`nmake` from
this toolset automatically, so they don't need to be on `PATH`.

### 4. Rust toolchain (rustup) — auto
```powershell
if (Get-Command rustc -ErrorAction SilentlyContinue) { rustc --version } else { Install-WinGet 'Rustlang.Rustup' }
$env:Path += ";$env:USERPROFILE\.cargo\bin"   # this session
rustup default stable-msvc
```

### 5. Node.js — auto
Vite (locked version) requires Node **`^20.19.0 || >=22.12.0`**. A loose major
check would pass unsupported versions (20.0–20.18, 21.x, 22.0–22.11), so test the
minor too:
```powershell
$nodeOK = $false
if (Get-Command node -ErrorAction SilentlyContinue) {
  node -e "const [a,b]=process.versions.node.split('.').map(Number);process.exit(((a===20&&b>=19)||(a===22&&b>=12)||a>=23)?0:1)"
  $nodeOK = ($LASTEXITCODE -eq 0)
}
if ($nodeOK) { "Node $(node --version) OK" } else { Install-WinGet 'OpenJS.NodeJS.LTS'; Update-SessionPath }
```

### 6. Perl — auto (required for the vendored OpenSSL build)
`connectorx` (in `server/Cargo.toml`, `src_postgres` feature) pulls
`postgres-openssl` → `openssl-sys` with the **vendored** OpenSSL, whose build
script runs `perl` to configure. macOS ships Perl; Windows does not, so install
Strawberry Perl or `cargo build` fails partway through compiling `openssl-sys`:
```powershell
if (Get-Command perl -ErrorAction SilentlyContinue) { perl --version | Select-Object -First 2 } else {
  Install-WinGet 'StrawberryPerl.StrawberryPerl' }
```

### 7. protoc (Protocol Buffers compiler) — auto (confirm the ID)
Required by `server\build.rs`/`tonic-build`; without it the Rust build fails.
```powershell
if (Get-Command protoc -ErrorAction SilentlyContinue) { protoc --version } else {
  # If this id is stale, run `winget search protobuf` and use the current one.
  Install-WinGet 'Google.Protobuf'
}
```
Fallbacks if winget lacks it: `choco install protoc` (Chocolatey),
`scoop install protobuf` (Scoop), or download a `protoc-*-win64.zip` from the
protobuf GitHub releases and add its `bin\` to `PATH`.

### 8. cargo-watch — auto (compiles from source, a minute or two)
`npm run dev` runs `cargo watch`; without it the dev script fails.
```powershell
if (cargo watch --version 2>$null) { cargo watch --version } else { cargo install cargo-watch }
```

### 9. Bitbucket SSH access — 🔴 GATE if not authenticated
Rust deps pull `insideinsight/rust-shared-utils` over SSH.

**a.** Ensure the OpenSSH client exists (it's an *optional* Windows feature, not
guaranteed). In an **(elevated)** shell if it needs installing:
```powershell
if (-not (Get-Command ssh -ErrorAction SilentlyContinue)) {
  Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0   # (elevated)
}
```
**b.** Start the agent (**elevated**, once):
```powershell
Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent
```
**c.** Enroll Bitbucket's host key so the non-interactive test can't fail on an
unknown host:
```powershell
$sshDir = "$env:USERPROFILE\.ssh"; New-Item -ItemType Directory -Force -Path $sshDir | Out-Null
if (-not (ssh-keygen -F bitbucket.org 2>$null)) { ssh-keyscan bitbucket.org 2>$null | Add-Content "$sshDir\known_hosts" }
```
**d.** Test:
```powershell
$r = ssh -o BatchMode=yes -o ConnectTimeout=6 -T git@bitbucket.org 2>&1
if ("$r" -match 'authenticated') { "Bitbucket SSH OK" } else { "NEEDS SETUP" }
```
If it prints `NEEDS SETUP`, create a key. The empty-passphrase argument differs
between PowerShell versions (5.1 drops a bare `''`), so pick the right form:
```powershell
$key = "$env:USERPROFILE\.ssh\id_ed25519"
if (-not (Test-Path "$key.pub")) {
  $empty = if ($PSVersionTable.PSVersion.Major -ge 6) { '' } else { '""' }
  ssh-keygen -t ed25519 -f $key -N $empty -C "$env:USERNAME@$env:COMPUTERNAME"
}
ssh-add $key
Write-Host "=== Give this public key to the human to add to Bitbucket ==="
Get-Content "$key.pub"
```
**Pause.** The human must (a) add that key under Bitbucket → Personal settings →
SSH keys, and (b) confirm read access to `insideinsight/rust-shared-utils`
(branch `develop/dev-v4`). Re-run the test (step **d**) until it prints
`Bitbucket SSH OK`.

### 10. environment.toml — auto (deterministic, forward-slash path)
The server exits at boot if this file is missing, and `home_path` **must be an
absolute path that already exists**. Create a data dir and write it in with
forward slashes (backslashes need escaping in TOML):
```powershell
if (Test-Path environment.toml) { "environment.toml exists — leaving it untouched" } else {
  Copy-Item environment.toml.example environment.toml
  $data = "$env:USERPROFILE\graphstudio-data"
  New-Item -ItemType Directory -Force -Path $data | Out-Null
  $fwd = $data -replace '\\','/'
  (Get-Content environment.toml) -replace '^home_path = .*', "home_path = `"$fwd`"" | Set-Content environment.toml
  "Wrote home_path=$fwd — edit client/app_type/environment to taste."
}
```
For a brand-new local tenant keep `is_new = true` for the first boot, then remove
that line. Tell the human which identity was set.

### 11. Install JS dependencies — auto
```powershell
npm ci
Push-Location mcp-server; npm ci; Pop-Location
```
(`npm ci` is the clean, lockfile-exact install; use `npm install` if `npm ci`
complains that the lockfile is out of sync.)

### 12. Pre-warm the Rust build — auto (several minutes: DuckDB + OpenSSL + Tonic)
```powershell
cargo build --locked --manifest-path server/Cargo.toml
```

### 13. LLM API keys — 🔴 GATE, optional
Only for the AI agent subsystem. If used, have the human set their key in the
shell that runs the server: `$env:ANTHROPIC_API_KEY="..."`. Otherwise skip.

## Verify

### Dev mode (primary — matches Mac)
```powershell
npm run dev
```
Confirm both processes start, then in another PowerShell:
```powershell
curl.exe -s http://localhost:3001/api/health; "`nserver OK"
Start-Process "http://localhost:5173"   # opens the editor UI in the browser
```

### Claude Desktop preview
`.claude/launch.json` is cross-platform (launches `npx vite` on `:5173`, no
hard-coded paths), so the in-app preview works on Windows once `npx` is on
`PATH` (steps 5 & 11). No Windows-specific change needed.

### Production build/preview (optional)
```powershell
npm run build       # tsc + vite build → dist/
npm run preview     # serves the production bundle on http://localhost:4173
```

## Common Mistakes

| Symptom | Fix |
|---|---|
| winget not recognized | Preflight/step 1 — install "App Installer" from the Microsoft Store, reopen PowerShell. |
| `cargo build` fails resolving `rust-shared-utils` | Step 9 — Bitbucket SSH key/access, agent, or host key not set up. |
| `error: linker 'link.exe' not found` | Step 3 — install VS C++ Build Tools (verify with `vswhere`); ensure `rustup default stable-msvc`. |
| Build fails compiling `openssl-sys` / "perl … not found" / Configure error | Step 6 — install Strawberry Perl, then reopen PowerShell (or `Update-SessionPath`). |
| OpenSSL build complains about `nasm` | Install NASM (`winget install NASM.NASM` or from nasm.us) and reopen PowerShell, then rebuild. |
| Build error mentioning `protoc` / `Could not find protoc` | Step 7 — install protobuf (confirm the winget id or use choco/scoop). |
| `npm run dev` errors on `cargo watch` not recognized | Step 8 — `cargo install cargo-watch`. |
| Vite errors about unsupported Node engine | Step 5 — Node must be `^20.19.0 \|\| >=22.12.0`. |
| Server exits with a config error | Step 10 — missing `environment.toml`, or a `home_path` with backslashes / a folder that doesn't exist. Use forward slashes; the dir must pre-exist. |
| `rustc`/`cargo`/`node`/`perl` "not recognized" right after install | PATH not refreshed — run `Update-SessionPath` or open a fresh shell. |
| `ssh` or `ssh-add` not recognized / agent not running | Step 9 — install the OpenSSH Client capability and start `ssh-agent` (elevated). |
| Build fails on long/nested paths | Preflight — enable `LongPathsEnabled` and re-clone to a short path like `C:\dev\GraphStudio`. |
