"""Export Kohya/sd-scripts argparse schemas without starting a training run.

The Rust service invokes this with the profile's isolated Python only after the
runtime health check succeeds.  Keeping introspection in Python means SDXL
flags follow the vendored upstream parser rather than a manually copied list.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any


def json_value(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, (list, tuple)):
        return [item for item in (json_value(item) for item in value) if item is not None]
    return None


def clean_json(value: Any) -> Any:
    """Remove malformed surrogate code points before emitting JSON to Rust."""
    if isinstance(value, str):
        return value.encode("utf-8", "replace").decode("utf-8")
    if isinstance(value, list):
        return [clean_json(item) for item in value]
    if isinstance(value, dict):
        return {clean_json(key): clean_json(item) for key, item in value.items()}
    return value


def field_kind(action: argparse.Action) -> str:
    if isinstance(action, (argparse._StoreTrueAction, argparse._StoreFalseAction, argparse.BooleanOptionalAction)):
        return "boolean"
    if action.choices:
        return "select"
    if action.nargs not in (None, "?"):
        return "list"
    if action.type in (int, float):
        return "number"
    return "text"


def inspect(entrypoint: Path, adapter_id: str) -> list[dict[str, Any]]:
    script_root = entrypoint.parent
    if str(script_root) not in sys.path:
        sys.path.insert(0, str(script_root))
    specification = importlib.util.spec_from_file_location(f"danbooru_{adapter_id.replace('-', '_')}", entrypoint)
    if specification is None or specification.loader is None:
        raise RuntimeError(f"cannot import trainer: {entrypoint}")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    parser = module.setup_parser()
    fields: list[dict[str, Any]] = []
    for action in parser._actions:
        if action.dest in {argparse.SUPPRESS, "help"} or not action.option_strings:
            continue
        fields.append(
            {
                "key": action.dest,
                "default": json_value(action.default),
                "choices": [str(choice) for choice in action.choices] if action.choices else [],
                "kind": field_kind(action),
                "required": bool(action.required),
                "help": action.help or "",
            }
        )
    return clean_json(sorted(fields, key=lambda field: field["key"]))


def main() -> int:
    if len(sys.argv) < 2:
        raise SystemExit("usage: adapter_inspector.py ADAPTER_ID=TRAINER.py [...]")
    adapters: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []
    for argument in sys.argv[1:]:
        adapter_id, separator, raw_entrypoint = argument.partition("=")
        if not separator or not adapter_id or not raw_entrypoint:
            raise SystemExit(f"invalid adapter entrypoint: {argument}")
        try:
            adapters.append({"id": adapter_id, "fields": inspect(Path(raw_entrypoint).resolve(), adapter_id)})
        except Exception as error:  # keep other compatible entrypoints discoverable
            errors.append({"id": adapter_id, "error": str(error)})
    # ASCII IPC is stable even when a Windows Conda process inherited a GBK
    # console code page. JSON parsers restore all escaped Unicode on receipt.
    print(json.dumps(clean_json({"adapters": adapters, "errors": errors}), ensure_ascii=True, allow_nan=False))
    return 0 if adapters else 1


if __name__ == "__main__":
    raise SystemExit(main())
