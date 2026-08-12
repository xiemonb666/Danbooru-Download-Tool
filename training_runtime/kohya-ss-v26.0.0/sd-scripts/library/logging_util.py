"""Logging / experiment-tracker helpers extracted from ``library.train_util``.

This module hosts:

- :func:`init_trackers` — initialise accelerate experiment trackers
  (W&B / TensorBoard / etc.) with our own conventions.
- :class:`LossRecorder` — running moving-average loss accumulator used by
  the training loops for progress display.

Both used to live in ``library.train_util`` and are still re-exported from
there for backward compatibility. New code should import from this module.
"""

import argparse
from typing import List

import toml
from accelerate import Accelerator

from library.args import get_sanitized_config_or_none
from library.utils import setup_logging

setup_logging()
import logging

logger = logging.getLogger(__name__)


def init_trackers(accelerator: Accelerator, args: argparse.Namespace, default_tracker_name: str):
    """
    Initialize experiment trackers with tracker specific behaviors
    """
    if accelerator.is_main_process:
        init_kwargs = {}
        if args.wandb_run_name:
            init_kwargs["wandb"] = {"name": args.wandb_run_name}
        if args.log_tracker_config is not None:
            init_kwargs = toml.load(args.log_tracker_config)

        # ----- 新增：为 TensorBoard 规范日志目录 -----
        # 检查是否已有配置，若没有则创建一个空的 tensorboard 配置
        if "tensorboard" not in init_kwargs:
            init_kwargs["tensorboard"] = {}
        # 如果 args 中有 logging_dir 且未在配置中指定，则使用它
        if hasattr(args, "logging_dir") and args.logging_dir:
            log_dir = os.path.normpath(args.logging_dir)
        # 否则使用 output_dir 加上 logs 子目录（模仿 Accelerate 默认行为）
        elif hasattr(args, "output_dir") and args.output_dir:
            log_dir = os.path.normpath(os.path.join(args.output_dir, "logs"))
        else:
            # 保底：使用当前目录下的 logs
            log_dir = os.path.normpath("logs")
        # 将规范后的路径写入 tensorboard 配置
        init_kwargs["tensorboard"]["log_dir"] = log_dir
        # ------------------------------------------

        accelerator.init_trackers(
            default_tracker_name if args.log_tracker_name is None else args.log_tracker_name,
            config=get_sanitized_config_or_none(args),
            init_kwargs=init_kwargs,
        )

        if "wandb" in [tracker.name for tracker in accelerator.trackers]:
            import wandb
            wandb_tracker = accelerator.get_tracker("wandb", unwrap=True)
            wandb_tracker.define_metric("epoch", hidden=True)
            wandb_tracker.define_metric("val_step", hidden=True)
            wandb_tracker.define_metric("global_step", hidden=True)


class LossRecorder:
    def __init__(self):
        self.loss_list: List[float] = []
        self.loss_total: float = 0.0

    def add(self, *, epoch: int, step: int, loss: float) -> None:
        if epoch == 0:
            self.loss_list.append(loss)
        else:
            while len(self.loss_list) <= step:
                self.loss_list.append(0.0)
            self.loss_total -= self.loss_list[step]
            self.loss_list[step] = loss
        self.loss_total += loss

    @property
    def moving_average(self) -> float:
        losses = len(self.loss_list)
        if losses == 0:
            return 0
        return self.loss_total / losses
