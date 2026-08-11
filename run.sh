#!/usr/bin/env bash
set -Eeuo pipefail

# ── Config ──────────────────────────────────────────────────────────────────
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
FRONTEND_DIR="$ROOT_DIR/frontend"
BACKEND_DIR="$ROOT_DIR/backend"
BACKEND_BIN="$BACKEND_DIR/target/release/danbooru-download-tool-pro"
LOCK_STAMP="$FRONTEND_DIR/node_modules/.danbooru-launcher-package-lock.json"

export HOST="${HOST:-127.0.0.1}"
export PORT="${PORT:-8888}"
export DATA_DIR="${DATA_DIR:-$ROOT_DIR}"
export STATIC_DIR="${STATIC_DIR:-$FRONTEND_DIR/dist}"
# The application starts without allocating GPU memory.  Set START_VLLM=1 only
# for unattended launches; normal interactive loading is done from Settings.
START_VLLM="${START_VLLM:-0}"
VLLM_PORT="${VLLM_PORT:-8000}"
VLLM_PID_FILE="${VLLM_PID_FILE:-$DATA_DIR/vllm.pid}"
VLLM_LOG_DIR="${VLLM_LOG_DIR:-$DATA_DIR/logs}"
VLLM_BOOT_LOG="${VLLM_BOOT_LOG:-$VLLM_LOG_DIR/vllm-launcher.log}"
VLLM_STATE_FILE="${VLLM_STATE_FILE:-$VLLM_LOG_DIR/vllm.state.json}"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; NC='\033[0m'
log()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()   { echo -e "${GREEN}[OK]${NC}    $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC}  $*"; }
err()  { echo -e "${RED}[ERR]${NC}  $*"; exit 1; }

start_vllm_background() {
    if [ "$START_VLLM" = "0" ]; then
        warn "vLLM auto-start disabled by START_VLLM=0"
        return
    fi
    if [ "$START_VLLM" != "1" ]; then
        warn "START_VLLM must be 0 or 1; skipping vLLM auto-start"
        return
    fi
    if [ ! -f "$ROOT_DIR/start_vllm.sh" ]; then
        warn "start_vllm.sh not found; the main application will continue without vLLM"
        return
    fi
    if [ -f "$VLLM_PID_FILE" ]; then
        local recorded_pid recorded_port
        read -r recorded_pid recorded_port < "$VLLM_PID_FILE" || true
        if [ -n "$recorded_pid" ] && kill -0 "$recorded_pid" 2>/dev/null; then
            VLLM_PORT="${recorded_port:-$VLLM_PORT}"
            export VLLM_BASE_URL="http://127.0.0.1:${VLLM_PORT}/v1"
            ok "vLLM is already running or loading (PID $recorded_pid; port $VLLM_PORT)"
            return
        fi
        rm -f "$VLLM_PID_FILE"
    fi

    local selection action selected_port port_state preferred_port
    preferred_port="$VLLM_PORT"
    if ! selection="$(node "$ROOT_DIR/scripts/select-vllm-port.mjs" "$VLLM_PORT" "$VLLM_STATE_FILE")"; then
        warn "Unable to select a vLLM port; the main application will continue without vLLM"
        return
    fi
    IFS=: read -r action selected_port port_state <<< "$selection"
    VLLM_PORT="$selected_port"
    export VLLM_BASE_URL="http://127.0.0.1:${VLLM_PORT}/v1"
    if [ "$action" = "ready" ]; then
        ok "vLLM is already available on 127.0.0.1:${VLLM_PORT}"
        return
    fi
    if [ "$action" = "loading" ]; then
        ok "vLLM is already loading on 127.0.0.1:${VLLM_PORT}"
        return
    fi
    if [ "$port_state" = "conflict" ]; then
        warn "Port $preferred_port belongs to another service; vLLM will use port $selected_port"
        log "vLLM will use 127.0.0.1:${selected_port}; the backend endpoint was updated for this session"
    fi

    mkdir -p "$VLLM_LOG_DIR" || {
        warn "Cannot create vLLM log directory: $VLLM_LOG_DIR"
        return
    }
    log "Starting vLLM in the background on 127.0.0.1:${VLLM_PORT}..."
    nohup env \
        VLLM_HOST="${VLLM_HOST:-127.0.0.1}" \
        VLLM_PORT="$VLLM_PORT" \
        LOG_DIR="$VLLM_LOG_DIR" \
        VLLM_STATE_FILE="$VLLM_STATE_FILE" \
        "$ROOT_DIR/start_vllm.sh" >>"$VLLM_BOOT_LOG" 2>&1 &
    local vllm_pid=$!
    if ! printf '%s %s\n' "$vllm_pid" "$VLLM_PORT" > "$VLLM_PID_FILE"; then
        warn "Cannot write vLLM PID file: $VLLM_PID_FILE"
    fi
    sleep 1
    if kill -0 "$vllm_pid" 2>/dev/null; then
        ok "vLLM is loading in the background (PID $vllm_pid; log: $VLLM_BOOT_LOG)"
    else
        rm -f "$VLLM_PID_FILE"
        warn "vLLM failed to start; inspect $VLLM_BOOT_LOG. The main application will continue."
    fi
}

# ── Check deps ──────────────────────────────────────────────────────────────
command -v cargo  &>/dev/null || err "Rust (cargo) not found — install https://rustup.rs"
command -v node   &>/dev/null || err "Node.js not found — install nodejs"
command -v npm    &>/dev/null || err "npm not found"
node -e 'const [major, minor] = process.versions.node.split(".").map(Number); const supported = (major === 20 && minor >= 19) || (major >= 22 && (major > 22 || minor >= 12)); process.exit(supported ? 0 : 1)' \
    || err "Node.js 20.19+ or 22.12+ is required"

# START_VLLM defaults to 0 so opening the application never starts a model.
# It remains available for unattended launches through START_VLLM=1.
start_vllm_background

# ── Frontend ────────────────────────────────────────────────────────────────
if [ ! -d "$FRONTEND_DIR/node_modules" ] \
    || [ ! -f "$LOCK_STAMP" ] \
    || ! cmp -s "$FRONTEND_DIR/package-lock.json" "$LOCK_STAMP"; then
    log "Installing frontend dependencies..."
    (cd "$FRONTEND_DIR" && npm ci --silent) || err "npm ci failed"
    cp "$FRONTEND_DIR/package-lock.json" "$LOCK_STAMP" || err "Cannot write dependency stamp"
else
    ok "Frontend dependencies are up to date"
fi

if [ ! -d "$FRONTEND_DIR/dist" ]; then
    log "Building frontend (first time)..."
else
    log "Building frontend..."
fi
(cd "$FRONTEND_DIR" && npm run build) || err "Frontend build failed"
ok "Frontend built → $FRONTEND_DIR/dist"

# ── Backend ─────────────────────────────────────────────────────────────────
if [ ! -f "$BACKEND_DIR/target/release/danbooru-download-tool-pro" ]; then
    log "Building backend (release, first time — may take a few minutes)..."
else
    log "Building backend (release)..."
fi
(cd "$BACKEND_DIR" && cargo build --release --locked) || err "Backend build failed"
ok "Backend built"

# ── Run ─────────────────────────────────────────────────────────────────────
echo ""
log "Starting server on http://${HOST}:${PORT}"
echo ""
exec "$BACKEND_BIN"
