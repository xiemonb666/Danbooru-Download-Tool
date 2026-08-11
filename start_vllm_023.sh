#!/bin/bash
# 启动 vLLM 0.23.0 服务，尝试使用 CUTLASS NVFP4 线性后端

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

echo "[INFO] 启动 vLLM 0.23.0 服务"
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
  --max-model-len 10240 \
  --max-num-seqs 16 \
  --max-num-batched-tokens 4096 \
  --enable-chunked-prefill \
  --enable-prefix-caching \
  --port "$VLLM_PORT" \
  --host "$VLLM_HOST"
