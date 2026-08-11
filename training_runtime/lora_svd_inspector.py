"""Model-agnostic LoRA singular-value diagnostics.

The desktop backend invokes this script with an isolated training Python.  It
intentionally knows only the safetensors container and common factor-pair
conventions: architecture-specific training code is not imported here.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

import torch
from safetensors import safe_open


ALGORITHM_VERSION = "lora-svd-qr-v1"
ENERGY_THRESHOLDS = (0.95, 0.99, 0.999)
NUMERICAL_RANK_EPSILON = 1e-6


@dataclass(frozen=True)
class PairConvention:
    name: str
    down_suffix: str
    up_suffix: str


PAIR_CONVENTIONS = (
    PairConvention("kohya", ".lora_down.weight", ".lora_up.weight"),
    PairConvention("peft", ".lora_A.weight", ".lora_B.weight"),
    PairConvention("generic", ".lora_down", ".lora_up"),
    PairConvention("generic", ".lora_A", ".lora_B"),
)


def finite(value: float) -> float:
    return value if math.isfinite(value) else 0.0


def json_safe(value: Any) -> Any:
    """Return a JSON-compatible value without unpaired Unicode surrogates.

    Safetensors metadata and tensor names can contain byte-preserving surrogate
    escapes on Windows.  Python can print those bytes, but Rust's JSON parser
    correctly rejects them as invalid UTF-8.  Replace only invalid code points
    and preserve normal Chinese/Unicode metadata for the UI and JSON export.
    """
    if isinstance(value, str):
        return value.encode("utf-8", errors="replace").decode("utf-8")
    if isinstance(value, list):
        return [json_safe(item) for item in value]
    if isinstance(value, tuple):
        return [json_safe(item) for item in value]
    if isinstance(value, dict):
        return {
            json_safe(key) if isinstance(key, str) else str(key): json_safe(item)
            for key, item in value.items()
        }
    return value


def json_for_stdout(value: Any) -> str:
    """Serialize IPC output as ASCII-only JSON.

    A selected Windows Conda runtime can inherit a GBK console code page.  By
    escaping non-ASCII JSON characters, the child-to-Rust pipe is byte-stable
    UTF-8/ASCII regardless of that inherited code page; serde_json restores the
    original Unicode strings when the backend parses it.
    """
    return json.dumps(json_safe(value), ensure_ascii=True, allow_nan=False)


def json_from_utf8_bytes(data: bytes) -> dict[str, Any]:
    """Decode the backend request without inheriting a Windows console code page."""
    value = json.loads(data.decode("utf-8"))
    if not isinstance(value, dict):
        raise ValueError("SVD 分析请求必须是 JSON 对象")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def numeric(value: Any) -> float | None:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    return parsed if math.isfinite(parsed) else None


def metadata_subset(metadata: dict[str, str]) -> dict[str, str]:
    keys = (
        "modelspec.architecture",
        "modelspec.title",
        "ss_base_model_version",
        "ss_network_dim",
        "ss_network_alpha",
        "ss_network_module",
        "ss_steps",
        "ss_epoch",
        "ss_output_name",
    )
    return {key: str(metadata[key]) for key in keys if key in metadata}


def architecture_for(metadata: dict[str, str], keys: Iterable[str]) -> str:
    """Identify only mutually consistent, model-specific evidence.

    Captions or arbitrary metadata must never participate here.  A declared
    ModelSpec architecture is strongest evidence, but a model-specific tensor
    namespace that contradicts it is reported as a conflict instead of being
    silently guessed.  Generic ``lora_unet`` keys are deliberately only a
    final fallback because both SD 1.x and SDXL use them.
    """

    def model_hints(source: str) -> set[str]:
        source = source.lower()
        found: set[str] = set()
        if "qwen" in source and "image" in source:
            found.add("Qwen Image")
        if "hunyuan" in source and "image" in source:
            found.add("HunyuanImage-2.1")
        if "lumina" in source:
            found.add("Lumina Image 2.0")
        if "sd3" in source or "stable-diffusion-3" in source:
            found.add("SD3 / SD3.5")
        if "sdxl" in source or "stable-diffusion-xl" in source:
            found.add("SDXL")
        if "flux" in source:
            found.add("FLUX.1")
        # Weight namespaces commonly use underscores (for example
        # ``lora_transformer_anima_block``); underscores are word characters
        # in Python regex, so ``\b`` alone would miss them.  Requiring a
        # namespace delimiter still avoids matching captions such as
        # ``animal`` when this helper is used on trusted metadata fields.
        if re.search(r"(?:^|[\s._/-])anima(?:$|[\s._/-])", source):
            found.add("Anima")
        return found

    declared = model_hints(str(metadata.get("modelspec.architecture", "")))
    metadata_hints = model_hints(" ".join(
        str(metadata.get(key, ""))
        for key in ("ss_base_model_version", "ss_sd_model_name", "modelspec.title")
    ))
    key_hints = model_hints(" ".join(keys))
    all_hints = declared | metadata_hints | key_hints
    if len(all_hints) > 1:
        return "Unknown / conflict"
    if all_hints:
        return next(iter(all_hints))
    if "lora_unet" in " ".join(keys).lower():
        return "Stable Diffusion family"
    return "Unknown"


def component_for(base: str) -> str:
    lowered = base.lower()
    if "text_encoder" in lowered or "lora_te" in lowered or "qwen" in lowered:
        return "Text encoder"
    if "unet" in lowered:
        return "UNet"
    if "transformer" in lowered or "dit" in lowered:
        return "Transformer / DiT"
    if "vae" in lowered:
        return "VAE"
    return base.split(".", 1)[0].split("_", 2)[0] or "Other"


def threshold_ranks(energy: torch.Tensor) -> dict[str, int]:
    if not energy.numel() or float(energy[-1]) <= 0:
        return {"energy_95": 0, "energy_99": 0, "energy_999": 0}
    result: dict[str, int] = {}
    for label, threshold in zip(("energy_95", "energy_99", "energy_999"), ENERGY_THRESHOLDS):
        result[label] = int(torch.searchsorted(energy, threshold, right=False).item()) + 1
    return result


def classify(rank: int, ranks: dict[str, int], tail_energy_20: float) -> str | None:
    if rank <= 0:
        return None
    ratio = ranks["energy_99"] / rank
    if ratio <= 0.5:
        return "compression_headroom"
    if ratio <= 0.75:
        return "compressible"
    if ratio >= 0.95 and tail_energy_20 >= 0.05:
        return "saturation_signal"
    return "well_utilized"


def pair_candidates(keys: list[str]) -> tuple[list[tuple[str, PairConvention]], set[str]]:
    key_set = set(keys)
    pairs: list[tuple[str, PairConvention]] = []
    consumed: set[str] = set()
    for convention in PAIR_CONVENTIONS:
        for key in keys:
            if not key.endswith(convention.down_suffix):
                continue
            base = key[: -len(convention.down_suffix)]
            up_key = base + convention.up_suffix
            if up_key not in key_set or key in consumed:
                continue
            pairs.append((base, convention))
            consumed.add(key)
            consumed.add(up_key)
    return pairs, consumed


def alpha_for(handle: Any, base: str, rank: int, metadata: dict[str, str], device: torch.device) -> float:
    for key in (base + ".alpha", base + ".lora_alpha", base + ".alpha.weight"):
        if key not in handle.keys():
            continue
        tensor = handle.get_tensor(key).detach().to(device="cpu", dtype=torch.float32)
        if tensor.numel() == 1:
            value = numeric(tensor.item())
            if value is not None:
                return value
    return numeric(metadata.get("ss_network_alpha")) or float(rank)


def analyse_pair(
    handle: Any,
    base: str,
    convention: PairConvention,
    metadata: dict[str, str],
    device: torch.device,
) -> tuple[dict[str, Any] | None, str | None]:
    down = handle.get_tensor(base + convention.down_suffix)
    up = handle.get_tensor(base + convention.up_suffix)
    if down.ndim < 2 or up.ndim < 2:
        return None, "LoRA 因子必须至少为二维张量"
    rank = int(down.shape[0])
    if rank <= 0 or int(up.shape[1]) != rank:
        return None, "上下因子的 rank 维度不匹配"
    # Canonical convolution LoRA has an up factor shaped [out, rank, 1, 1].
    # Spatial up factors need a model-specific contraction and are not silently
    # flattened because that would produce a misleading rank conclusion.
    if up.numel() != int(up.shape[0]) * rank:
        return None, "非标准空间 up 因子，当前通用分析器不支持"
    try:
        left = up.detach().to(device=device, dtype=torch.float32).reshape(int(up.shape[0]), rank)
        right = down.detach().to(device=device, dtype=torch.float32).reshape(rank, -1)
        if not torch.isfinite(left).all() or not torch.isfinite(right).all():
            return None, "因子包含 NaN 或 Infinity"
        # ΔW = B A.  QR(B)=QbRb and QR(Aᵀ)=QaRa, so its non-zero singular
        # values equal svdvals(Rb Raᵀ).  This avoids materialising ΔW.
        _, r_left = torch.linalg.qr(left, mode="reduced")
        _, r_right = torch.linalg.qr(right.transpose(0, 1), mode="reduced")
        core = r_left @ r_right.transpose(0, 1)
        alpha = alpha_for(handle, base, rank, metadata, device)
        scale = alpha / rank
        singular = torch.linalg.svdvals(core).detach().to(device="cpu", dtype=torch.float64)
        singular = singular.mul(abs(scale))
    except RuntimeError as error:
        return None, f"SVD 计算失败：{error}"
    if not singular.numel() or float(singular[0]) <= 0:
        return None, "ΔW 为零矩阵，无法形成有效奇异值谱"
    energy = singular.square()
    total_energy = float(energy.sum())
    cumulative = torch.cumsum(energy, dim=0).div(total_energy)
    numerical_rank = int((singular > singular[0] * NUMERICAL_RANK_EPSILON).sum())
    tail_count = max(1, math.ceil(int(singular.numel()) * 0.2))
    tail_energy_20 = finite(float(energy[-tail_count:].sum()) / total_energy)
    ranks = threshold_ranks(cumulative)
    return {
        "id": base,
        "component": component_for(base),
        "rank": rank,
        "alpha": finite(alpha),
        "scale": finite(scale),
        "numerical_rank": numerical_rank,
        "stable_rank": finite(total_energy / float(singular[0].square())),
        "tail_energy_20": tail_energy_20,
        "effective_rank": ranks,
        "energy": finite(total_energy),
        "flag": classify(rank, ranks, tail_energy_20),
        "singular_values": [finite(float(value)) for value in singular],
    }, None


def global_uniform_rank(modules: list[dict[str, Any]]) -> tuple[dict[str, int], float]:
    total = sum(float(module["energy"]) for module in modules)
    max_rank = max((int(module["rank"]) for module in modules), default=0)
    if total <= 0 or max_rank <= 0:
        return {"energy_95": 0, "energy_99": 0, "energy_999": 0}, 0.0
    retained: list[float] = []
    for candidate_rank in range(1, max_rank + 1):
        energy = 0.0
        for module in modules:
            singular = module["singular_values"]
            energy += sum(value * value for value in singular[:candidate_rank])
        retained.append(energy / total)
    ranks = {}
    for label, threshold in zip(("energy_95", "energy_99", "energy_999"), ENERGY_THRESHOLDS):
        ranks[label] = next((index + 1 for index, value in enumerate(retained) if value >= threshold), max_rank)
    return ranks, retained[-1] if retained else 0.0


def nonstandard_adapter_format(keys: Iterable[str]) -> str | None:
    """Return an explicitly unsupported adapter algorithm, if present."""
    source = " ".join(keys).lower()
    if "hada_" in source or "loha" in source:
        return "LoHa"
    if "lokr_" in source:
        return "LoKr"
    if "dora_" in source or "dora" in source:
        return "DoRA"
    return None


def nonstandard_report(
    item: dict[str, Any],
    path: Path,
    metadata: dict[str, str],
    keys: list[str],
    excluded: list[dict[str, str]],
    adapter_format: str,
    started: float,
    device_reason: str,
) -> dict[str, Any]:
    """Report LoHa/LoKr without fabricating a standard-LoRA SVD result."""
    step = numeric(metadata.get("ss_steps"))
    zero_ranks = {"energy_95": 0, "energy_99": 0, "energy_999": 0}
    return {
        "id": hashlib.sha256(str(path).encode("utf-8")).hexdigest()[:20],
        "label": str(item.get("label") or path.stem),
        "path": str(path),
        "file_size_bytes": path.stat().st_size,
        "sha256": sha256(path),
        "modified_at": int(path.stat().st_mtime),
        "step": int(step) if step is not None else None,
        "architecture": architecture_for(metadata, keys),
        "format": f"{adapter_format} (standard LoRA SVD not applicable)",
        "svd_applicable": False,
        "coverage": {"analyzed_modules": 0, "candidate_modules": 0, "unsupported_modules": len(excluded)},
        "rank_distribution": {"minimum": 0, "maximum": 0, "modal": 0, "uniform": False},
        "effective_rank": zero_ranks,
        "current_rank_energy": 0.0,
        "tail_energy_20": 0.0,
        "verdict": "partial_evidence",
        "verdict_message": (
            f"检测到 {adapter_format} 的非标准矩阵结构；它使用"
            f"{'Hadamard' if adapter_format == 'LoHa' else 'Kronecker'} 组合，"
            "标准 LoRA 的 ΔW QR-SVD 不适用，因此未生成 rank 结论。"
        ),
        "metadata": metadata_subset(metadata),
        "excluded": excluded,
        "modules": [],
        "global_singular_values": [],
        "global_cumulative_energy": [],
        "analysis_duration_ms": int((time.perf_counter() - started) * 1000),
        "device_reason": device_reason,
    }


def model_report(item: dict[str, Any], device: torch.device, device_reason: str) -> dict[str, Any]:
    path = Path(str(item["path"])).expanduser().resolve()
    if path.suffix.lower() != ".safetensors":
        raise ValueError(f"仅支持 .safetensors LoRA：{path}")
    if not path.is_file():
        raise ValueError(f"LoRA 文件不存在或不是常规文件：{path}")
    started = time.perf_counter()
    with safe_open(str(path), framework="pt", device="cpu") as handle:
        keys = list(handle.keys())
        metadata = dict(handle.metadata() or {})
        candidates, _ = pair_candidates(keys)
        modules: list[dict[str, Any]] = []
        excluded: list[dict[str, str]] = []
        formats: set[str] = set()
        for base, convention in candidates:
            formats.add(convention.name)
            result, reason = analyse_pair(handle, base, convention, metadata, device)
            if result is None:
                excluded.append({"id": base, "reason": reason or "未知分析错误"})
            else:
                modules.append(result)
        unsupported_tokens = ("hada_", "lokr_", "dora_", "loha_")
        for key in keys:
            if any(token in key.lower() for token in unsupported_tokens):
                base = key.rsplit(".", 1)[0]
                if not any(entry["id"] == base for entry in excluded):
                    excluded.append({"id": base, "reason": "检测到非标准 LoRA 适配器，未纳入通用 SVD"})
    if not modules:
        adapter_format = nonstandard_adapter_format(keys)
        if adapter_format:
            return nonstandard_report(item, path, metadata, keys, excluded, adapter_format, started, device_reason)
        raise ValueError("没有可分析的 LoRA 因子对；请确认这是标准 LoRA safetensors 文件")
    modules.sort(key=lambda module: (-float(module["energy"]), module["id"]))
    ranks = [int(module["rank"]) for module in modules]
    rank_counts = {rank: ranks.count(rank) for rank in set(ranks)}
    modal_rank = max(rank_counts, key=lambda rank: (rank_counts[rank], rank))
    total_energy = sum(float(module["energy"]) for module in modules)
    combined_singular = sorted((value for module in modules for value in module["singular_values"]), reverse=True)
    combined_energy = sum(value * value for value in combined_singular)
    cumulative_energy: list[float] = []
    running = 0.0
    for value in combined_singular:
        running += value * value
        cumulative_energy.append(finite(running / combined_energy))
    effective_rank, current_rank_energy = global_uniform_rank(modules)
    tail_energy = finite(sum(float(module["energy"]) * float(module["tail_energy_20"]) for module in modules) / total_energy)
    uniform = len(rank_counts) == 1
    coverage = len(modules) / max(1, len(modules) + len(excluded))
    if not uniform or coverage < 0.9:
        verdict = "partial_evidence"
        verdict_message = "rank 不统一或部分模块未覆盖；请以模块级结果和验证样图复核。"
    else:
        verdict = classify(modal_rank, effective_rank, tail_energy) or "partial_evidence"
        verdict_message = {
            "compression_headroom": "99% 能量可由明显更低的 rank 保留，存在较高压缩余量。",
            "compressible": "99% 能量可由较低 rank 保留，建议做较低 rank 的消融验证。",
            "well_utilized": "奇异值能量分布与当前 rank 基本匹配。",
            "saturation_signal": "尾部方向仍有实质能量，当前 rank 可能接近容量边界。",
        }.get(verdict, "证据不足以给出统一 rank 判断。")
    selected_metadata = metadata_subset(metadata)
    step = numeric(metadata.get("ss_steps"))
    return {
        "id": hashlib.sha256(str(path).encode("utf-8")).hexdigest()[:20],
        "label": str(item.get("label") or path.stem),
        "path": str(path),
        "file_size_bytes": path.stat().st_size,
        "sha256": sha256(path),
        "modified_at": int(path.stat().st_mtime),
        "step": int(step) if step is not None else None,
        "architecture": architecture_for(metadata, keys),
        "format": "+".join(sorted(formats)) or "unknown",
        "svd_applicable": True,
        "coverage": {
            "analyzed_modules": len(modules),
            "candidate_modules": len(candidates),
            "unsupported_modules": len(excluded),
        },
        "rank_distribution": {
            "minimum": min(ranks), "maximum": max(ranks), "modal": modal_rank, "uniform": uniform,
        },
        "effective_rank": effective_rank,
        "current_rank_energy": finite(current_rank_energy),
        "tail_energy_20": tail_energy,
        "verdict": verdict,
        "verdict_message": verdict_message,
        "metadata": selected_metadata,
        "excluded": excluded,
        "modules": modules,
        "global_singular_values": [finite(value) for value in combined_singular],
        "global_cumulative_energy": cumulative_energy,
        "analysis_duration_ms": int((time.perf_counter() - started) * 1000),
        "device_reason": device_reason,
    }


def comparison_for(reports: list[dict[str, Any]]) -> dict[str, Any] | None:
    if len(reports) < 2:
        return None
    architectures = {report["architecture"] for report in reports}
    formats = {report["format"] for report in reports}
    applicable = all(bool(report.get("svd_applicable", True)) for report in reports)
    comparable = applicable and len(architectures) == 1 and len(formats) == 1
    reason = (
        "权重格式与架构一致，可按训练顺序比较。" if comparable
        else "检测到不同架构、非标准适配器或 LoRA 格式；仅并列展示，不解释为同一训练轨迹。"
    )
    return {
        "comparable": comparable,
        "reason": reason,
        "checkpoints": [{
            "id": report["id"], "label": report["label"], "step": report["step"],
            "effective_rank": report["effective_rank"],
            "rank_utilization": finite(report["effective_rank"]["energy_99"] / max(1, report["rank_distribution"]["modal"])),
            "tail_energy_20": report["tail_energy_20"],
        } for report in reports],
    }


def resolve_device(requested: str) -> tuple[torch.device, str]:
    if requested.startswith("cuda") and torch.cuda.is_available():
        try:
            device = torch.device(requested)
            torch.empty(1, device=device)
            return device, "自动选择空闲 CUDA 设备"
        except (RuntimeError, AssertionError):
            pass
    return torch.device("cpu"), "使用 CPU：CUDA 不可用、显存不足或被工作台任务占用"


def run(request: dict[str, Any], requested_device: str) -> dict[str, Any]:
    files = request.get("files")
    if not isinstance(files, list) or not 1 <= len(files) <= 5:
        raise ValueError("一次分析必须选择 1 到 5 个 LoRA 文件")
    device, reason = resolve_device(requested_device)
    started = time.perf_counter()
    reports = [model_report(item, device, reason) for item in files]
    reports.sort(key=lambda report: (report["step"] is None, report["step"] or report["modified_at"], report["label"]))
    return {
        "algorithm_version": ALGORITHM_VERSION,
        "reports": reports,
        "comparison": comparison_for(reports),
        "execution": {
            "device": str(device), "reason": reason,
            "duration_ms": int((time.perf_counter() - started) * 1000),
            "fallback": requested_device.startswith("cuda") and device.type != "cuda",
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--device", default="cpu")
    args = parser.parse_args()
    try:
        request = json_from_utf8_bytes(sys.stdin.buffer.read())
        print(json_for_stdout(run(request, args.device)))
        return 0
    except Exception as error:  # backend turns this into a structured API failure
        print(json_for_stdout({"error": str(error)}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
