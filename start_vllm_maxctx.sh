#!/bin/bash
# 测试 vLLM 在当前显卡上能支持的最大上下文吞吐量
# max_model_len 设为 vLLM 估算的最大值附近（约 12k）

set -e

MODEL_PATH="${MODEL_PATH:-unsloth/Qwen3.8-27B-NVFP4}"
VLLM_PORT="${VLLM_PORT:-8000}"
VLLM_HOST="${VLLM_HOST:-0.0.0.0}"
MAX_MODEL_LEN="${MAX_MODEL_LEN:-10240}"

source /root/miniconda3/etc/profile.d/conda.sh
conda activate vllm

export CUDA_HOME="$CONDA_PREFIX"
export PATH="$CONDA_PREFIX/bin:$PATH"
export LD_LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib:$LD_LIBRARY_PATH"
export LIBRARY_PATH="/usr/lib/wsl/lib:$CONDA_PREFIX/lib:$LIBRARY_PATH"

export VLLM_USE_MODELSCOPE=True
export VLLM_NVFP4_GEMM_BACKEND=marlin
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True

echo "[INFO] 启动 vLLM 服务（max_model_len=$MAX_MODEL_LEN）"
echo "[INFO] 模型: $MODEL_PATH"
echo "[INFO] 监听: $VLLM_HOST:$VLLM_PORT"
echo ""

vllm serve "$MODEL_PATH" \
  --trust-remote-code \
  --dtype bfloat16 \
  --quantization compressed-tensors \
  --attention-backend flashinfer \
  --kv-cache-dtype fp8_e4m3 \
  --gpu-memory-utilization 0.94 \
  --max-model-len "$MAX_MODEL_LEN" \
  --max-num-seqs 1 \
  --max-num-batched-tokens 4096 \
  --enable-chunked-prefill \
  --no-enable-prefix-caching \
  --enforce-eager \
  --port "$VLLM_PORT" \
  --host "$VLLM_HOST"
