#!/usr/bin/env bash
set -e
DIR="$(dirname "$0")"
PORT="${PORT:-8888}"

# Dev mode: debug build (fast compile), skips frontend if dist/ exists
if [ ! -d "$DIR/frontend/dist" ]; then
    echo "[dev] Building frontend..."
    (cd "$DIR/frontend" && npm ci --silent && npm run build) || exit 1
fi

echo "[dev] Starting backend (debug) on :${PORT}..."
cd "$DIR/backend" && exec cargo run --locked
