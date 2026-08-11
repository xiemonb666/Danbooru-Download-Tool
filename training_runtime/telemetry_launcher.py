"""Small, dependency-free bridge around an upstream lora-scripts entry point.

The bridge deliberately does not own a model or a training loop.  It only
observes Accelerate's per-step ``log`` calls, writes an append-only JSONL stream
for the native monitor, and cooperatively stops at the next log boundary when
the desktop application places a pause/cancel control file in the run folder.
"""

from __future__ import annotations

import json
import os
import runpy
import shutil
import sys
import threading
import time
from pathlib import Path
from typing import Any


RUN_DIR = Path(os.environ.get("DANBOORU_TRAINING_RUN_DIR", ".")).resolve()
METRICS_FILE = Path(os.environ.get("DANBOORU_TRAINING_METRICS_FILE", RUN_DIR / "metrics.jsonl"))
CONTROL_FILE = Path(os.environ.get("DANBOORU_TRAINING_CONTROL_FILE", RUN_DIR / "control.json"))
RESUME_DIR = RUN_DIR / "resume_state"
_LOCK = threading.Lock()
_LAST_RESOURCE_SAMPLE = 0.0
_LOSS_TOTAL = 0.0
_LOSS_COUNT = 0
_LAST_STEP: int | None = None
_LAST_STEP_AT: float | None = None


def _number(value: Any) -> float | None:
    try:
        if hasattr(value, "detach"):
            value = value.detach()
        if hasattr(value, "item"):
            value = value.item()
        value = float(value)
        return value if value == value and value not in (float("inf"), float("-inf")) else None
    except (TypeError, ValueError, OverflowError):
        return None


def _resource_metrics() -> dict[str, float]:
    """Best-effort resources. Missing optional packages never stop training."""
    values: dict[str, float] = {}
    try:
        import psutil  # type: ignore

        values["resource.cpu_percent"] = float(psutil.cpu_percent(interval=None))
        values["resource.ram_percent"] = float(psutil.virtual_memory().percent)
    except Exception:
        pass
    try:
        disk = shutil.disk_usage(RUN_DIR)
        values["resource.disk_free_gib"] = disk.free / (1024 ** 3)
    except Exception:
        pass
    try:
        import pynvml  # type: ignore

        pynvml.nvmlInit()
        visible = os.environ.get("CUDA_VISIBLE_DEVICES", "0").split(",")[0].strip() or "0"
        handle = pynvml.nvmlDeviceGetHandleByIndex(int(visible))
        memory = pynvml.nvmlDeviceGetMemoryInfo(handle)
        values["resource.gpu_memory_used_gib"] = memory.used / (1024 ** 3)
        values["resource.gpu_memory_total_gib"] = memory.total / (1024 ** 3)
        values["resource.gpu_utilization_percent"] = float(pynvml.nvmlDeviceGetUtilizationRates(handle).gpu)
        values["resource.gpu_temperature_c"] = float(pynvml.nvmlDeviceGetTemperature(handle, pynvml.NVML_TEMPERATURE_GPU))
        values["resource.gpu_power_w"] = pynvml.nvmlDeviceGetPowerUsage(handle) / 1000.0
    except Exception:
        pass
    return values


def _write_metrics(step: int, metrics: dict[str, float]) -> None:
    if not metrics:
        return
    RUN_DIR.mkdir(parents=True, exist_ok=True)
    record = {"step": max(0, step), "timestamp": int(time.time() * 1000), "metrics": metrics}
    with _LOCK:
        with METRICS_FILE.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n")


def _training_progress_metrics(step: int, values: dict[str, float]) -> None:
    """Derive study-friendly series without changing the upstream trainer."""
    global _LOSS_TOTAL, _LOSS_COUNT, _LAST_STEP, _LAST_STEP_AT
    loss = next((values[key] for key in ("loss", "train_loss", "train/loss") if key in values), None)
    if loss is not None:
        _LOSS_TOTAL += loss
        _LOSS_COUNT += 1
        values["loss.current"] = loss
        values["loss.average"] = _LOSS_TOTAL / _LOSS_COUNT

    now = time.monotonic()
    if _LAST_STEP is not None and _LAST_STEP_AT is not None and step > _LAST_STEP:
        rate = (step - _LAST_STEP) / max(now - _LAST_STEP_AT, 1e-6)
        values["train.steps_per_second"] = rate
        try:
            maximum = int(os.environ.get("DANBOORU_TRAINING_MAX_STEPS", "0"))
        except ValueError:
            maximum = 0
        if maximum > 0:
            values["train.progress_percent"] = min(100.0, step / maximum * 100.0)
            values["train.eta_seconds"] = max(0.0, (maximum - step) / rate)
    _LAST_STEP = step
    _LAST_STEP_AT = now


def _read_action() -> str | None:
    try:
        value = json.loads(CONTROL_FILE.read_text(encoding="utf-8"))
        action = value.get("action") if isinstance(value, dict) else None
        return action if action in {"pause", "cancel"} else None
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None


def _install_accelerate_hook() -> None:
    try:
        from accelerate import Accelerator  # type: ignore
    except Exception:
        return

    original_log = Accelerator.log

    def observed_log(self: Any, values: Any, step: int | None = None, *args: Any, **kwargs: Any) -> Any:
        global _LAST_RESOURCE_SAMPLE
        result = original_log(self, values, step=step, *args, **kwargs)
        numeric = {
            str(key): number
            for key, value in (values.items() if isinstance(values, dict) else [])
            if (number := _number(value)) is not None
        }
        now = time.monotonic()
        if now - _LAST_RESOURCE_SAMPLE >= 2.0:
            numeric.update(_resource_metrics())
            _LAST_RESOURCE_SAMPLE = now
        actual_step = step if step is not None else 0
        _training_progress_metrics(actual_step, numeric)
        _write_metrics(actual_step, numeric)
        action = _read_action()
        if action == "pause":
            try:
                RESUME_DIR.mkdir(parents=True, exist_ok=True)
                self.save_state(str(RESUME_DIR))
                CONTROL_FILE.write_text(json.dumps({"action": "paused", "resume_state": str(RESUME_DIR)}), encoding="utf-8")
            except Exception as error:
                CONTROL_FILE.write_text(json.dumps({"action": "pause_failed", "error": str(error)}), encoding="utf-8")
            raise KeyboardInterrupt("Danbooru training pause requested")
        if action == "cancel":
            CONTROL_FILE.write_text(json.dumps({"action": "cancelled"}), encoding="utf-8")
            raise KeyboardInterrupt("Danbooru training cancellation requested")
        return result

    Accelerator.log = observed_log


def main() -> int:
    if len(sys.argv) < 2:
        raise SystemExit("usage: telemetry_launcher.py TRAINER [args...]")
    trainer = Path(sys.argv[1]).resolve()
    if not trainer.is_file():
        raise SystemExit(f"trainer does not exist: {trainer}")
    # Upstream scripts import sibling packages such as ``library`` and
    # ``networks``.  runpy does not automatically emulate Python's normal
    # script-directory import behaviour, so make the trainer's own directory
    # explicit without depending on a globally modified PYTHONPATH.
    sys.path.insert(0, str(trainer.parent))
    _install_accelerate_hook()
    sys.argv = [str(trainer), *sys.argv[2:]]
    try:
        runpy.run_path(str(trainer), run_name="__main__")
    except KeyboardInterrupt:
        action = _read_action()
        raise SystemExit(75 if action in {"pause", "paused", "pause_failed"} else 76)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
