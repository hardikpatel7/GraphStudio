#!/usr/bin/env bash
# Build a release tarball at release/smartstudio-mcp-<version>.tar.gz.
# Tarball layout (after extraction with --strip-components=1):
#   ./package.json
#   ./package-lock.json
#   ./node_modules/   (production only)
#   ./dist/

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
NAME="smartstudio-mcp-${VERSION}"
OUT="release/${NAME}.tar.gz"

echo "[pack] cleaning…"
rm -rf dist release node_modules
mkdir -p release

echo "[pack] installing all deps (incl. dev) for build…"
npm ci

echo "[pack] building TypeScript…"
npm run build

echo "[pack] pruning to production deps…"
rm -rf node_modules
npm ci --omit=dev

echo "[pack] writing tarball -> ${OUT}"
tar -czf "${OUT}" \
  --transform "s,^,${NAME}/," \
  package.json package-lock.json dist node_modules

if command -v sha256sum >/dev/null 2>&1; then
  SUM=$(sha256sum "${OUT}" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  SUM=$(shasum -a 256 "${OUT}" | awk '{print $1}')
else
  SUM="(install sha256sum/shasum to compute)"
fi

echo
echo "[pack] done:"
echo "  artifact: ${OUT}"
echo "  size:     $(du -h "${OUT}" | awk '{print $1}')"
echo "  sha256:   ${SUM}"
echo
echo "Set mcp_artifact_checksum: \"sha256:${SUM}\" in the Ansible inventory."
