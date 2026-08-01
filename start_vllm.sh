#!/usr/bin/env bash
# 启动 vLLM 0.23.0 服务（unsloth/Qwen3.6-27B-NVFP4，用于 AI 打标）
# 在 WSL2 中运行：bash start_vllm.sh

set -Eeuo pipefail

# 模型路径或 ModelScope/HuggingFace ID
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
MODEL_PATH="${MODEL_PATH:-unsloth/Qwen3.6-27B-NVFP4}"
VLLM_PORT="${VLLM_PORT:-8000}"
VLLM_HOST="${VLLM_HOST:-127.0.0.1}"
VLLM_CONDA_ENV="${VLLM_CONDA_ENV:-vllm}"
LOG_DIR="${LOG_DIR:-$ROOT_DIR/logs}"
VLLM_STATE_FILE="${VLLM_STATE_FILE:-$LOG_DIR/vllm.state.json}"
mkdir -p "$LOG_DIR"

# 同一份启动器只允许一个模型进程，避免模型加载阶段重复启动并耗尽显存。
if command -v flock >/dev/null 2>&1; then
    VLLM_LOCK_FILE="${VLLM_LOCK_FILE:-$LOG_DIR/vllm.lock}"
    exec 9>"$VLLM_LOCK_FILE"
    if ! flock -n 9; then
        echo "[INFO] vLLM 已在运行或加载中，不重复启动"
        exit 0
    fi
fi

# 端口已被监听时保持现有服务，不再杀进程。
if command -v fuser >/dev/null 2>&1 && fuser "${VLLM_PORT}/tcp" >/dev/null 2>&1; then
    echo "[INFO] 端口 $VLLM_PORT 已有服务监听，不重复启动"
    exit 0
fi

# 激活 conda 环境
CONDA_SH="${CONDA_SH:-}"
if [ -z "$CONDA_SH" ]; then
    for candidate in \
        "$HOME/miniconda3/etc/profile.d/conda.sh" \
        "/root/miniconda3/etc/profile.d/conda.sh" \
        "/opt/conda/etc/profile.d/conda.sh"; do
        if [ -f "$candidate" ]; then
            CONDA_SH="$candidate"
            break
        fi
    done
fi
if [ -n "$CONDA_SH" ] && [ -f "$CONDA_SH" ]; then
    # shellcheck source=/dev/null
    source "$CONDA_SH"
elif command -v conda >/dev/null 2>&1; then
    eval "$(conda shell.bash hook)"
else
    echo "[ERROR] 找不到 conda；请安装 vLLM 环境或设置 CONDA_SH" >&2
    exit 1
fi
# 第三方 conda activate.d 脚本可能直接读取尚未定义的 CUDA 环境变量。
set +u
conda activate "$VLLM_CONDA_ENV"
set -u

# Windows 的 TEMP 会被 WSL 映射到 /mnt/c，ZeroMQ 无法在 DrvFs 上创建 IPC socket。
export TMPDIR="${VLLM_TMPDIR:-/tmp}"
export TMP="$TMPDIR"
export TEMP="$TMPDIR"
mkdir -p "$TMPDIR"

# 确保使用 conda 内的 CUDA 工具链
export CUDA_HOME="$CONDA_PREFIX"
export PATH="$CONDA_PREFIX/bin:$PATH"
export LD_LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"

# 使用 ModelScope 下载模型（替代 HuggingFace）
export VLLM_USE_MODELSCOPE=True

# RTX 5090 (SM120) 上 vLLM 0.23.0 默认 Marlin 后端会挂，使用 CUTLASS 后端
# 通过 --linear-backend cutlass 指定，无需设置 VLLM_NVFP4_GEMM_BACKEND

# 减少显存碎片，降低 OOM 概率
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True

# 日志同时输出到终端和文件
LOG_FILE="$LOG_DIR/vllm_$(date +%Y%m%d_%H%M%S).log"
exec > >(tee -a "$LOG_FILE") 2>&1

echo "[INFO] 启动 vLLM 0.23.0 服务"
echo "[INFO] 模型: $MODEL_PATH"
echo "[INFO] 监听: $VLLM_HOST:$VLLM_PORT"
echo "[INFO] 日志文件: $LOG_FILE"
echo "[INFO] 按 Ctrl+C 停止服务"
echo ""

state_temp="$VLLM_STATE_FILE.$$"
printf '{"status":"loading","port":%s,"pid":%s}\n' "$VLLM_PORT" "$$" > "$state_temp"
mv -f "$state_temp" "$VLLM_STATE_FILE"

exec vllm serve "$MODEL_PATH" \
  --trust-remote-code \
  --dtype bfloat16 \
  --quantization compressed-tensors \
  --attention-backend flashinfer \
  --linear-backend cutlass \
  --kv-cache-dtype fp8_e4m3 \
  --gpu-memory-utilization 0.94 \
  --max-model-len 10240 \
  --max-num-seqs 16 \
  --max-num-batched-tokens 4096 \
  --enable-chunked-prefill \
  --enable-prefix-caching \
  --port "$VLLM_PORT" \
  --host "$VLLM_HOST"
