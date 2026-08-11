# Native training telemetry bridge

`telemetry_launcher.py` is a local bridge written for DanbooruDownload Tool
Pro. It observes `accelerate.Accelerator.log` and writes JSONL for the native
training monitor. It does not include a model checkpoint, model weights, or
the upstream `lora-scripts` source.

The training runtime is designed for the AGPL-3.0-or-later `lora-scripts`
project. When the bundled runtime source is distributed, keep its complete
corresponding source, license text, upstream notices, and any local
modifications alongside the runtime. Upstream license:
https://www.gnu.org/licenses/agpl-3.0.en.html
