#!/bin/bash
# 启动 vLLM 服务（diffusiongemma-26B-A4B-it-NVFP4，文本生成模型）
# 注意：该模型不是视觉模型，不能用于 AI 图片打标，仅用于文本任务
# 在 WSL2 中运行：bash start_vllm_diffusiongemma.sh

set -e

MODEL_PATH="/mnt/c/models/diffusiongemma-26B-A4B-it-NVFP4"
VLLM_PORT="${VLLM_PORT:-8000}"
VLLM_HOST="${VLLM_HOST:-0.0.0.0}"

source /root/miniconda3/etc/profile.d/conda.sh
conda activate vllm

export CUDA_HOME="$CONDA_PREFIX"
export PATH="$CONDA_PREFIX/bin:$PATH"
export LD_LIBRARY_PATH="$CONDA_PREFIX/lib:$LD_LIBRARY_PATH"
export VLLM_ENABLE_V1_MULTIPROCESSING=0

echo "[INFO] 启动 vLLM 服务 (diffusiongemma NVFP4)"
echo "[INFO] 模型: $MODEL_PATH"
echo "[INFO] 监听: $VLLM_HOST:$VLLM_PORT"
echo "[WARN] 该模型为文本模型，无法用于图片打标"
echo "[INFO] 按 Ctrl+C 停止服务"
echo ""

vllm serve "$MODEL_PATH" \
  --dtype bfloat16 \
  --trust-remote-code \
  --gpu-memory-utilization 0.90 \
  --max-model-len 4096 \
  --moe-backend marlin \
  --port "$VLLM_PORT" \
  --host "$VLLM_HOST"
