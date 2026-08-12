#!/usr/bin/env bash
# Stops only the vLLM process recorded by this application's launcher.

set -Eeuo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
VLLM_STATE_FILE="${VLLM_STATE_FILE:-$ROOT_DIR/logs/vllm.state.json}"

finish_not_running() {
    rm -f -- "$VLLM_STATE_FILE"
    printf '%s\n' 'VLLM_UNLOAD_STATE=not_running'
}

if [ ! -r "$VLLM_STATE_FILE" ]; then
    finish_not_running
    exit 0
fi

pid="$(sed -nE 's/.*"pid"[[:space:]]*:[[:space:]]*([0-9]+).*/\1/p' "$VLLM_STATE_FILE" | head -n 1)"
if ! [[ "$pid" =~ ^[1-9][0-9]*$ ]]; then
    echo '[ERROR] vLLM 状态文件不包含有效 PID' >&2
    exit 2
fi

if ! kill -0 "$pid" 2>/dev/null; then
    finish_not_running
    exit 0
fi

command_line="$(ps -p "$pid" -o args= 2>/dev/null || true)"
case "$command_line" in
  *'vllm serve'*|*'vllm.entrypoints.'*|*'/vllm/'*'serve'*) ;;
  *)
    echo '[ERROR] 状态文件记录的 PID 不是 vLLM 进程，已拒绝停止' >&2
    exit 2
    ;;
esac

if ! kill -TERM "$pid" 2>/dev/null; then
    finish_not_running
    exit 0
fi
for _ in $(seq 1 20); do
    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f -- "$VLLM_STATE_FILE"
        printf '%s\n' 'VLLM_UNLOAD_STATE=stopped'
        exit 0
    fi
    sleep 0.5
done

if ! kill -KILL "$pid" 2>/dev/null; then
    finish_not_running
    exit 0
fi
for _ in $(seq 1 10); do
    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f -- "$VLLM_STATE_FILE"
        printf '%s\n' 'VLLM_UNLOAD_STATE=stopped'
        exit 0
    fi
    sleep 0.2
done

echo '[ERROR] vLLM 进程未能停止' >&2
exit 1
