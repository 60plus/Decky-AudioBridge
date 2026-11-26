#!/bin/sh
# Type-check and build the Decky frontend with Docker, run the Python tests,
# then package the plugin. Output: out/AudioBridge-<version>.zip and .sha256
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/src" -v audiobridge-pnpm:/pnpm-store -w /work node:22-trixie-slim sh -c '
  set -e
  cp -r /src/package.json /src/pnpm-lock.yaml /src/tsconfig.json /src/rollup.config.js /src/.npmrc /src/src /src/plugin.json /work/
  corepack enable >/dev/null 2>&1
  corepack prepare pnpm@9.15.9 --activate >/dev/null 2>&1
  pnpm config set store-dir /pnpm-store >/dev/null
  pnpm install --frozen-lockfile >/dev/null
  pnpm run typecheck
  pnpm run build
  mkdir -p /src/dist
  cp dist/index.js /src/dist/index.js'
MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/src:ro" -w /tmp -e PYTHONDONTWRITEBYTECODE=1 python:3.11-slim sh -c '
  cp -r /src/py_modules /src/tests /src/main.py /tmp/ && python -m unittest discover -s tests -p "test_*.py"'
MSYS_NO_PATHCONV=1 docker run --rm -v "$ROOT:/src" -w /src -e PYTHONDONTWRITEBYTECODE=1 python:3.11-slim \
  python -I scripts/package.py
