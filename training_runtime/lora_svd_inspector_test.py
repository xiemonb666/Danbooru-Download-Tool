"""Run with an installed training runtime:
python lora_svd_inspector_test.py
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import torch
from safetensors.torch import save_file

import lora_svd_inspector as inspector


class LoraSvdInspectorTests(unittest.TestCase):
    def test_json_safe_replaces_unpaired_unicode_surrogates_before_stdout(self) -> None:
        payload = {"metadata": {"broken": "value\udcff"}, "items": ["正常文本"]}

        sanitized = inspector.json_safe(payload)

        self.assertEqual(sanitized["metadata"]["broken"], "value?")
        self.assertEqual(
            json.loads(json.dumps(sanitized, ensure_ascii=False, allow_nan=False))["items"],
            ["正常文本"],
        )

    def test_stdout_payload_is_ascii_so_windows_code_pages_cannot_corrupt_ipc(self) -> None:
        encoded = inspector.json_for_stdout({"message": "中文 LoRA 报告", "metadata": {"name": "测试"}})

        self.assertTrue(encoded.isascii())
        self.assertEqual(json.loads(encoded)["message"], "中文 LoRA 报告")

    def test_ipc_request_decodes_utf8_bytes_independent_of_windows_console_code_page(self) -> None:
        decoded = inspector.json_from_utf8_bytes('{"files":[{"label":"中文 checkpoint"}]}'.encode("utf-8"))

        self.assertEqual(decoded["files"][0]["label"], "中文 checkpoint")

    def test_explicit_sdxl_architecture_is_not_overridden_by_animal_tag_metadata(self) -> None:
        architecture = inspector.architecture_for(
            {
                "modelspec.architecture": "stable-diffusion-xl-v1-base/lora",
                "ss_tag_frequency": '{"animal ears": 12}',
            },
            ["lora_unet_input_blocks_4_1_proj_in.lora_down.weight"],
        )

        self.assertEqual(architecture, "SDXL")

    def test_conflicting_sdxl_metadata_and_anima_weight_keys_are_not_guessed_as_anima(self) -> None:
        architecture = inspector.architecture_for(
            {"modelspec.architecture": "stable-diffusion-xl-v1-base/lora"},
            ["lora_transformer_anima_block.lora_down.weight"],
        )

        self.assertEqual(architecture, "Unknown / conflict")

    def test_kohya_pair_uses_alpha_scaled_exact_singular_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.safetensors"
            save_file({
                "lora_unet_block.lora_down.weight": torch.tensor([[3.0, 0.0], [0.0, 1.0]]),
                "lora_unet_block.lora_up.weight": torch.eye(2),
                "lora_unet_block.alpha": torch.tensor(2.0),
            }, str(path), metadata={"ss_network_dim": "2", "ss_steps": "42"})
            report = inspector.run({"files": [{"path": str(path), "label": "fixture"}]}, "cpu")["reports"][0]

        module = report["modules"][0]
        self.assertEqual(report["architecture"], "Stable Diffusion family")
        self.assertEqual(report["step"], 42)
        self.assertEqual(module["rank"], 2)
        self.assertAlmostEqual(module["singular_values"][0], 3.0)
        self.assertAlmostEqual(module["singular_values"][1], 1.0)
        self.assertEqual(module["effective_rank"]["energy_95"], 2)

    def test_missing_factor_is_rejected_without_a_rank_conclusion(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "broken.safetensors"
            save_file({"lora_unet_block.lora_down.weight": torch.ones((2, 2))}, str(path))
            with self.assertRaisesRegex(ValueError, "没有可分析"):
                inspector.run({"files": [{"path": str(path)}]}, "cpu")

    def test_noncanonical_spatial_up_factor_is_reported_as_uncovered(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "spatial-up.safetensors"
            save_file({
                "layer.lora_down.weight": torch.ones((2, 2, 3, 3)),
                "layer.lora_up.weight": torch.ones((2, 2, 3, 3)),
            }, str(path))
            with self.assertRaisesRegex(ValueError, "没有可分析"):
                inspector.run({"files": [{"path": str(path)}]}, "cpu")

    def test_loha_is_explicitly_uncovered_without_a_standard_rank_conclusion(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "loha.safetensors"
            save_file({
                "lora_unet_block.hada_w1_a": torch.ones((2, 2)),
                "lora_unet_block.hada_w1_b": torch.ones((2, 2)),
            }, str(path), metadata={"modelspec.architecture": "stable-diffusion-xl-v1-base/lora"})
            report = inspector.run({"files": [{"path": str(path)}]}, "cpu")["reports"][0]

        self.assertFalse(report["svd_applicable"])
        self.assertEqual(report["architecture"], "SDXL")
        self.assertIn("LoHa", report["format"])
        self.assertEqual(report["effective_rank"]["energy_99"], 0)
        self.assertFalse(report["modules"])


if __name__ == "__main__":
    unittest.main()
