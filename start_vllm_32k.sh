#!/bin/bash
# 启动 vLLM 0.23.0 服务，测试 32k 上下文极限

set -e

MODEL_PATH="${MODEL_PATH:-unsloth/Qwen3.6-27B-NVFP4}"
VLLM_PORT="${VLLM_PORT:-8000}"
VLLM_HOST="${VLLM_HOST:-0.0.0.0}"

source /root/miniconda3/etc/profile.d/conda.sh
conda activate vllm

export CUDA_HOME="$CONDA_PREFIX"
export PATH="$CONDA_PREFIX/bin:$PATH"
export LD_LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib:$LD_LIBRARY_PATH"
export LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib:$LIBRARY_PATH"

export VLLM_USE_MODELSCOPE=True
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True

# 如果端口已被占用，先释放（避免上次异常退出后遗留 vLLM 进程）
if command -v fuser >/dev/null 2>&1; then
    echo "[INFO] 检查端口 $VLLM_PORT 是否被占用..."
    fuser -k "${VLLM_PORT}/tcp" >/dev/null 2>&1 && echo "[INFO] 已释放端口 $VLLM_PORT" || true
    sleep 1
fi

echo "[INFO] 启动 vLLM 0.23.0 服务（max_model_len=32768）"
echo "[INFO] 模型: $MODEL_PATH"
echo "[INFO] 监听: $VLLM_HOST:$VLLM_PORT"
echo ""

vllm serve "$MODEL_PATH" \
  --trust-remote-code \
  --dtype bfloat16 \
  --quantization compressed-tensors \
  --attention-backend flashinfer \
  --linear-backend cutlass \
  --kv-cache-dtype fp8_e4m3 \
  --gpu-memory-utilization 0.94 \
  --max-model-len 32768 \
  --max-num-seqs 4 \
  --max-num-batched-tokens 4096 \
  --enable-chunked-prefill \
  --enable-prefix-caching \
  --port "$VLLM_PORT" \
  --host "$VLLM_HOST"
