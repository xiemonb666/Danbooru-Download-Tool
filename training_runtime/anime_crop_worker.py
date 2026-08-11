"""One-shot, stdin/stdout JSON worker for conservative anime smart crops.

The Rust process validates every media path before passing it here.  This worker
only reads images and emits a single JSON document; it never writes into a
media library and does not expose a network service.  Hugging Face/ONNX model
caches are owned by the selected Python runtime.
"""

from __future__ import annotations

import inspect
import json
import os
import sys
import tempfile
import traceback
import contextlib
from pathlib import Path
from typing import Any, Callable


HEAD_MODEL = "head_detect_v2.0_x_yv11"
_DOWNLOAD_PATCHED = False
_POSE_MODELS: dict[str, Any] = {}
_CUDA_DLL_DIRECTORIES: list[Any] = []
_CUDA_DLL_READY = False
_POSE_LAST_ERROR: str | None = None


def prepare_cuda_runtime() -> None:
    """Make the selected Conda environment's CUDA DLLs visible before ORT.

    On Windows, provider discovery can report CUDA even when an ONNX session
    cannot load cuBLAS/cuDNN.  Torch registers these DLL directories as an
    import side effect; register them explicitly so the crop worker does not
    depend on import order and can reject a real CPU fallback.
    """
    global _CUDA_DLL_READY
    if os.name != "nt" or _CUDA_DLL_READY:
        return
    roots = [
        Path(sys.prefix) / "Lib" / "site-packages" / "torch" / "lib",
        Path(sys.prefix) / "Library" / "bin",
    ]
    for directory in roots:
        if directory.is_dir():
            handle = os.add_dll_directory(str(directory))
            _CUDA_DLL_DIRECTORIES.append(handle)
    _CUDA_DLL_READY = True


def warmup_marker() -> Path:
    cache_root = Path(os.environ.get("CROP_MODEL_CACHE", Path.home() / ".cache" / "danbooru-anime-crop"))
    return cache_root / "warmup-complete.json"


def mark_models_warmed(device: str) -> None:
    marker = warmup_marker()
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps({"device": device, "models": HEAD_MODEL}), encoding="utf-8")


def emit(value: dict[str, Any]) -> int:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()
    return 0


def module_version(name: str) -> str | None:
    try:
        from importlib.metadata import version

        return version(name)
    except Exception:
        return None


def gpu_status(device: str) -> dict[str, Any]:
    prepare_cuda_runtime()
    import torch
    import onnxruntime as ort

    result: dict[str, Any] = {
        "onnxruntime": module_version("onnxruntime-gpu") or module_version("onnxruntime"),
        "providers": ort.get_available_providers(),
        "cuda_provider": "CUDAExecutionProvider" in ort.get_available_providers(),
        "tensorrt_provider": "TensorrtExecutionProvider" in ort.get_available_providers(),
        "device": device,
        "gpu": None,
    }
    try:
        index = int(device.rsplit(":", 1)[-1])
        cuda = bool(torch.cuda.is_available() and torch.cuda.device_count() > index)
        result["torch_cuda"] = cuda
        if cuda:
            props = torch.cuda.get_device_properties(index)
            result["gpu"] = {
                "index": index,
                "name": props.name,
                "memory_total_mib": int(props.total_memory / 1024 / 1024),
            }
    except Exception as error:
        result["torch_cuda"] = False
        result["torch_error"] = str(error)
    return result


def cuda_session_probe() -> dict[str, Any]:
    """Open the real head detector and prove that its first provider is CUDA."""
    try:
        install_hub_download_fallback()
        from huggingface_hub import hf_hub_download
        from imgutils.utils.onnxruntime import open_onnx_model

        model = hf_hub_download(
            repo_id="deepghs/anime_head_detection",
            repo_type="model",
            filename=f"{HEAD_MODEL}/model.onnx",
        )
        session = open_onnx_model(model, mode="CUDAExecutionProvider")
        providers = session.get_providers()
        return {
            "ready": bool(providers and providers[0] == "CUDAExecutionProvider"),
            "providers": providers,
        }
    except Exception as error:
        return {"ready": False, "providers": [], "error": str(error)}


def dependency_health(device: str) -> dict[str, Any]:
    status = gpu_status(device)
    status["imgutils"] = module_version("dghs-imgutils")
    status["rtmlib"] = module_version("rtmlib")
    missing: list[str] = []
    if status["imgutils"] != "0.19.0":
        missing.append("dghs-imgutils==0.19.0")
    if not status["rtmlib"]:
        missing.append("rtmlib")
    if not status["cuda_provider"] or not status.get("torch_cuda"):
        missing.append("ONNX Runtime CUDA provider / CUDA GPU")
    if not missing:
        probe = cuda_session_probe()
        status["detector_session"] = probe
        if not probe["ready"]:
            missing.append("动漫检测 ONNX CUDA session")
    status["dependencies_ready"] = not missing
    status["models_ready"] = warmup_marker().is_file()
    status["ready"] = status["dependencies_ready"] and status["models_ready"]
    status["missing"] = missing
    return status


def install_hub_download_fallback() -> None:
    """Download known public detector files when the Hub client rejects Xet CDN.

    Some Windows Python/HTTP stacks reject Hugging Face's signed Xet redirect
    even though the public URL is reachable.  imgutils imports
    ``hf_hub_download`` at module load, so patch it before importing imgutils
    and use a private model cache, never the user media directory.  The
    original client remains the first choice and the fallback only accepts
    DeepGHS public model repository paths.
    """
    global _DOWNLOAD_PATCHED
    if _DOWNLOAD_PATCHED:
        return
    import huggingface_hub

    original = huggingface_hub.hf_hub_download
    cache_root = Path(os.environ.get("CROP_MODEL_CACHE", Path.home() / ".cache" / "danbooru-anime-crop"))

    def fallback(repo_id: str, filename: str, repo_type: str | None = None, revision: str = "main", **kwargs: Any) -> str:
        try:
            return original(repo_id=repo_id, filename=filename, repo_type=repo_type, revision=revision, **kwargs)
        except Exception as original_error:
            relative = Path(filename)
            allowed_deepghs = repo_type == "model" and repo_id.startswith("deepghs/")
            allowed_isnet = repo_type in (None, "model") and repo_id == "skytnt/anime-seg"
            if not (allowed_deepghs or allowed_isnet) or relative.is_absolute() or ".." in relative.parts:
                raise original_error
            destination = cache_root / repo_id.replace("/", "--") / revision / relative
            if destination.is_file() and destination.stat().st_size > 0:
                return str(destination)
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_name(destination.name + ".partial")
            try:
                import requests

                url = f"https://huggingface.co/{repo_id}/resolve/{revision}/{filename}"
                with requests.get(url, stream=True, timeout=(15, 600)) as response:
                    response.raise_for_status()
                    with open(temporary, "wb") as output:
                        for chunk in response.iter_content(chunk_size=1024 * 1024):
                            if chunk:
                                output.write(chunk)
                os.replace(temporary, destination)
                return str(destination)
            except Exception:
                try:
                    os.unlink(temporary)
                except FileNotFoundError:
                    pass
                raise original_error

    huggingface_hub.hf_hub_download = fallback
    _DOWNLOAD_PATCHED = True


def _call_detector(fn: Callable[..., Any], image: Any, **preferred: Any) -> Any:
    """Call imgutils across minor API signature variants without CPU fallback."""
    try:
        signature = inspect.signature(fn)
        accepted = {key: value for key, value in preferred.items() if key in signature.parameters}
    except (TypeError, ValueError):
        accepted = preferred
    try:
        return fn(image, **accepted)
    except TypeError:
        # Old imgutils versions sometimes expose only `level` and `version`.
        accepted.pop("model_name", None)
        return fn(image, **accepted)


def _box(value: Any) -> dict[str, float] | None:
    # imgutils returns either `(x0, y0, x1, y1)`, `(box, label, score)`, or
    # a dict depending on detector generation.  Normalize all forms here.
    score = 1.0
    box = value
    if isinstance(value, (tuple, list)) and len(value) >= 3 and isinstance(value[-1], (int, float)):
        box, score = value[0], float(value[-1])
    if isinstance(value, dict):
        score = float(value.get("score", value.get("confidence", 1.0)))
        box = value.get("bbox", value.get("box", value))
    if isinstance(box, dict):
        x0 = box.get("x0", box.get("left", box.get("x")))
        y0 = box.get("y0", box.get("top", box.get("y")))
        x1 = box.get("x1", box.get("right"))
        y1 = box.get("y1", box.get("bottom"))
        if x1 is None and x0 is not None and box.get("width") is not None:
            x1 = float(x0) + float(box["width"])
        if y1 is None and y0 is not None and box.get("height") is not None:
            y1 = float(y0) + float(box["height"])
        values = (x0, y0, x1, y1)
    elif isinstance(box, (tuple, list)) and len(box) >= 4:
        values = box[:4]
    else:
        return None
    try:
        x0, y0, x1, y1 = (float(part) for part in values)
    except (TypeError, ValueError):
        return None
    if not x1 > x0 or not y1 > y0:
        return None
    return {"x0": x0, "y0": y0, "x1": x1, "y1": y1, "score": score}


def _boxes(result: Any) -> list[dict[str, float]]:
    if result is None:
        return []
    if isinstance(result, dict):
        result = result.get("detections", result.get("results", []))
    if not isinstance(result, (list, tuple)):
        return []
    return [box for item in result if (box := _box(item)) is not None]


def _detector(detectors: Any, names: tuple[str, ...]) -> Callable[..., Any] | None:
    for name in names:
        function = getattr(detectors, name, None)
        if callable(function):
            return function
    return None


def detect_poses(image: Any, device: str) -> list[dict[str, Any]]:
    """Return per-person HumanArt/RTMPose evidence for full-body crops.

    rtmlib has changed constructor arguments more than once, so this is kept
    deliberately version tolerant.  Each record carries its own visible body
    extent and ankle/torso confidence so Rust can associate it with the same
    detected person selected for a crop.
    """
    global _POSE_LAST_ERROR
    _POSE_LAST_ERROR = None
    try:
        import rtmlib

        body_type = getattr(rtmlib, "Body", None)
        if body_type is None:
            return []
        backend = "onnxruntime"
        pose = _POSE_MODELS.get(device)
        if pose is None:
            # `balanced` uses the HumanArt YOLOX detector paired with
            # RTMPose, and rtmlib maps cuda:N to the matching ORT provider.
            pose = body_type(mode="balanced", backend=backend, device=device)
            _POSE_MODELS[device] = pose
        import numpy as np

        frame = np.asarray(image.convert("RGB"))[:, :, ::-1].copy()
        keypoints, scores = pose(frame)
        if keypoints is None or scores is None:
            return []
        if len(scores) == 0:
            return []
        poses: list[dict[str, Any]] = []
        for person_keypoints, person_scores in zip(keypoints, scores):
            if len(person_scores) < 17:
                continue
            reliable = [
                point for point, score in zip(person_keypoints, person_scores)
                if float(score) >= 0.35
            ]
            if len(reliable) < 5:
                continue
            x_values = [float(point[0]) for point in reliable]
            y_values = [float(point[1]) for point in reliable]
            torso_indices = [5, 6, 11, 12]
            torso_scores = [float(person_scores[index]) for index in torso_indices if index < len(person_scores)]
            poses.append({
                "bbox": {
                    "x0": min(x_values), "y0": min(y_values),
                    "x1": max(x_values), "y1": max(y_values), "score": 1.0,
                },
                "torso_score": sum(torso_scores) / len(torso_scores) if torso_scores else 0.0,
                "left_ankle_score": float(person_scores[15]),
                "right_ankle_score": float(person_scores[16]),
            })
        return poses
    except Exception as error:
        _POSE_LAST_ERROR = str(error)
        return []


def analyze_item(item: dict[str, Any], device: str) -> dict[str, Any]:
    media_id = str(item.get("media_id", ""))
    path = Path(str(item.get("path", "")))
    result: dict[str, Any] = {
        "media_id": media_id,
        "width": 0,
        "height": 0,
        "persons": [],
        "heads": [],
        "faces": [],
        "half_bodies": [],
        "hands": [],
        "foreground": None,
        "poses": [],
        "pose_complete": False,
        "pose_error": None,
        "segmentation_error": None,
    }
    try:
        from PIL import Image

        install_hub_download_fallback()
        from imgutils import detect as detectors
        from imgutils import segment as segmenters

        with Image.open(path) as opened:
            image = opened.convert("RGB")
        result["width"], result["height"] = image.size
        jobs = {
            "heads": (_detector(detectors, ("detect_heads",)), {"model_name": HEAD_MODEL, "level": "s", "version": "v2.0"}),
            "faces": (_detector(detectors, ("detect_faces", "detect_face")), {"level": "s", "version": "v1.4"}),
            "persons": (_detector(detectors, ("detect_persons", "detect_person")), {"level": "m", "version": "v1.1"}),
            "half_bodies": (_detector(detectors, ("detect_halfbodies", "detect_halfbody")), {"level": "s", "version": "v1.0"}),
            "hands": (_detector(detectors, ("detect_hands", "detect_hand")), {"level": "s", "version": "v1.0"}),
        }
        for name, (function, kwargs) in jobs.items():
            if function is None:
                raise RuntimeError(f"imgutils 缺少 {name} 检测入口")
            result[name] = _boxes(_call_detector(function, image, **kwargs))
        # ISNet segmentation is optional protection only.  A missing model or
        # uncertain mask must not make a crop wider than detection evidence.
        segment = _detector(segmenters, ("segment_rgba_with_isnetis",))
        if segment and len(result["persons"]) == 1:
            try:
                rgba = segment(image)
                # imgutils 0.19 returns ``(mask_ndarray, rgba_image)``;
                # older releases returned the RGBA image directly.  Support
                # both contracts so the optional ISNet boundary is actually
                # used rather than silently skipped.
                if isinstance(rgba, tuple):
                    rgba = rgba[-1]
                alpha = rgba.getchannel("A") if hasattr(rgba, "getchannel") else None
                if alpha:
                    bbox = alpha.getbbox()
                    if bbox:
                        result["foreground"] = {"x0": float(bbox[0]), "y0": float(bbox[1]), "x1": float(bbox[2]), "y1": float(bbox[3]), "score": 1.0}
            except Exception as error:
                result["segmentation_error"] = str(error)
        result["poses"] = detect_poses(image, device)
        result["pose_error"] = _POSE_LAST_ERROR
        result["pose_complete"] = any(
            pose["torso_score"] >= 0.45
            and pose["left_ankle_score"] >= 0.45
            and pose["right_ankle_score"] >= 0.45
            for pose in result["poses"]
        )
    except Exception as error:
        result["error"] = str(error)
    return result


def warmup(device: str) -> dict[str, Any]:
    # A tiny synthetic picture exercises provider/model initialization and
    # triggers model downloads without touching user media.
    from PIL import Image

    health = dependency_health(device)
    if not health["dependencies_ready"]:
        return {"ready": False, "health": health, "error": "依赖或 CUDA provider 未就绪"}
    temporary = tempfile.NamedTemporaryFile(prefix="anime-crop-warmup-", suffix=".png", delete=False)
    temporary_path = temporary.name
    temporary.close()  # Windows cannot reopen a NamedTemporaryFile while it is open.
    try:
        Image.new("RGB", (768, 1024), "white").save(temporary_path, format="PNG")
        with contextlib.redirect_stdout(sys.stderr):
            result = analyze_item({"media_id": "warmup", "path": temporary_path}, device)
            # Segmentation is only applied to trustworthy single-person input
            # at task time, but preheat it explicitly so installation verifies
            # its model download and ONNX session as well.
            install_hub_download_fallback()
            from imgutils.segment import segment_rgba_with_isnetis

            segment_rgba_with_isnetis(Image.open(temporary_path).convert("RGB"))
    finally:
        try:
            os.unlink(temporary_path)
        except FileNotFoundError:
            pass
    if result.get("error"):
        return {"ready": False, "health": health, "error": result["error"]}
    if result.get("pose_error"):
        return {"ready": False, "health": health, "error": f"HumanArt/RTMPose 未就绪: {result['pose_error']}"}
    mark_models_warmed(device)
    health["models_ready"] = True
    health["ready"] = True
    return {
        "ready": True,
        "health": health,
        "models": [HEAD_MODEL, "anime_face", "anime_person", "anime_halfbody", "anime_hand", "isnet_anime", "humanart_rtmpose"],
    }


def main() -> int:
    try:
        payload = json.loads(sys.stdin.read())
        action = str(payload.get("action", "health"))
        gpu_id = str(payload.get("gpu_id", "0"))
        # Force CUDA-only inference semantics.  The Rust health check rejects a
        # provider failure; no implicit CPU execution is allowed.  Once a
        # physical GPU is isolated by CUDA_VISIBLE_DEVICES it is visible to
        # ONNX Runtime as cuda:0, including when the user selected GPU 2+.
        os.environ.setdefault("CUDA_VISIBLE_DEVICES", gpu_id)
        os.environ["ONNX_MODE"] = "CUDAExecutionProvider"
        device = "cuda:0"
        if action == "health":
            return emit(dependency_health(device))
        if action == "warmup":
            return emit(warmup(device))
        if action != "detect":
            return emit({"error": "unknown_action", "ready": False})
        health = dependency_health(device)
        if not health["dependencies_ready"]:
            return emit({"ready": False, "health": health, "items": []})
        items = payload.get("items", [])
        if not isinstance(items, list):
            return emit({"ready": False, "error": "items 必须为数组", "items": []})
        completed = 0
        detected_any = False
        for item in items:
            # Third-party detector packages occasionally log to stdout. Keep
            # their output out of the JSONL stream, then flush this item's
            # contract record immediately so a long batch is observable.
            with contextlib.redirect_stdout(sys.stderr):
                result = analyze_item(item, device)
            detected_any = detected_any or not bool(result.get("error"))
            emit({"type": "detection", "item": result})
            completed += 1
        if detected_any:
            mark_models_warmed(device)
            health["models_ready"] = True
            health["ready"] = True
        # The task protocol is JSONL rather than a single unbounded response:
        # the Rust parent can validate every returned media ID independently
        # and never needs to grant the worker write access to the workspace.
        return emit({"type": "complete", "ready": True, "health": health, "count": completed})
    except Exception as error:
        return emit({"ready": False, "error": str(error), "traceback": traceback.format_exc(limit=3)})


if __name__ == "__main__":
    raise SystemExit(main())
