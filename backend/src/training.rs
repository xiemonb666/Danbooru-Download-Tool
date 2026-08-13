use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// A model-specific trainer description.  The HTTP/UI layer deliberately uses
/// this data instead of special-casing an SDXL checkpoint or command line.
#[derive(Debug, Clone, Serialize)]
pub struct TrainingAdapter {
    pub id: &'static str,
    pub version: &'static str,
    pub label: Cow<'static, str>,
    pub family: &'static str,
    pub family_label: &'static str,
    pub training_type: &'static str,
    pub training_type_label: &'static str,
    pub trainer: &'static str,
    pub fields: Vec<TrainingField>,
    pub groups: Vec<TrainingGroup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub subgroups: Vec<TrainingSubgroup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingSubgroup {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingField {
    pub key: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub group: Cow<'static, str>,
    pub subgroup: Cow<'static, str>,
    pub kind: Cow<'static, str>,
    pub default: Value,
    pub choices: Vec<Cow<'static, str>>,
    pub required: bool,
    pub advanced: bool,
    pub help: Cow<'static, str>,
    pub description: Cow<'static, str>,
    pub when_to_adjust: Cow<'static, str>,
}

fn field(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    subgroup: &'static str,
    kind: &'static str,
    default: Value,
    choices: &[&'static str],
    required: bool,
    advanced: bool,
    help: &'static str,
    description: &'static str,
    when_to_adjust: &'static str,
) -> TrainingField {
    TrainingField {
        key: Cow::Borrowed(key),
        label: Cow::Borrowed(label),
        group: Cow::Borrowed(group),
        subgroup: Cow::Borrowed(subgroup),
        kind: Cow::Borrowed(kind),
        default,
        choices: choices.iter().copied().map(Cow::Borrowed).collect(),
        required,
        advanced,
        help: Cow::Borrowed(help),
        description: Cow::Borrowed(description),
        when_to_adjust: Cow::Borrowed(when_to_adjust),
    }
}

#[derive(Clone, Copy)]
struct KohyaAdapterDefinition {
    id: &'static str,
    family: &'static str,
    family_label: &'static str,
    training_type: &'static str,
    training_type_label: &'static str,
    trainer: &'static str,
}

const KOHYA_V26_ADAPTERS: &[KohyaAdapterDefinition] = &[
    KohyaAdapterDefinition {
        id: "sd15-lora",
        family: "sd15",
        family_label: "Stable Diffusion 1.5 / 2.x",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/train_network.py",
    },
    KohyaAdapterDefinition {
        id: "sd15-dreambooth",
        family: "sd15",
        family_label: "Stable Diffusion 1.5 / 2.x",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/train_db.py",
    },
    KohyaAdapterDefinition {
        id: "sd15-fine-tune",
        family: "sd15",
        family_label: "Stable Diffusion 1.5 / 2.x",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/fine_tune.py",
    },
    KohyaAdapterDefinition {
        id: "sd15-textual-inversion",
        family: "sd15",
        family_label: "Stable Diffusion 1.5 / 2.x",
        training_type: "textual_inversion",
        training_type_label: "Textual Inversion",
        trainer: "sd-scripts/train_textual_inversion.py",
    },
    KohyaAdapterDefinition {
        id: "sd15-leco",
        family: "sd15",
        family_label: "Stable Diffusion 1.5 / 2.x",
        training_type: "leco",
        training_type_label: "LECO",
        trainer: "sd-scripts/train_leco.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-lora",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/sdxl_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-loha",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "loha",
        training_type_label: "LoHa",
        trainer: "sd-scripts/sdxl_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-lokr",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "lokr",
        training_type_label: "LoKr",
        trainer: "sd-scripts/sdxl_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-dreambooth",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/sdxl_train.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-fine-tune",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/sdxl_train.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-textual-inversion",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "textual_inversion",
        training_type_label: "Textual Inversion",
        trainer: "sd-scripts/sdxl_train_textual_inversion.py",
    },
    KohyaAdapterDefinition {
        id: "sdxl-leco",
        family: "sdxl",
        family_label: "SDXL",
        training_type: "leco",
        training_type_label: "LECO",
        trainer: "sd-scripts/sdxl_train_leco.py",
    },
    KohyaAdapterDefinition {
        id: "sd3-lora",
        family: "sd3",
        family_label: "SD3 / SD3.5",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/sd3_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "sd3-dreambooth",
        family: "sd3",
        family_label: "SD3 / SD3.5",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/sd3_train.py",
    },
    KohyaAdapterDefinition {
        id: "sd3-fine-tune",
        family: "sd3",
        family_label: "SD3 / SD3.5",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/sd3_train.py",
    },
    KohyaAdapterDefinition {
        id: "flux-lora",
        family: "flux",
        family_label: "FLUX.1",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/flux_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "flux-dreambooth",
        family: "flux",
        family_label: "FLUX.1",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/flux_train.py",
    },
    KohyaAdapterDefinition {
        id: "flux-fine-tune",
        family: "flux",
        family_label: "FLUX.1",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/flux_train.py",
    },
    KohyaAdapterDefinition {
        id: "lumina-lora",
        family: "lumina",
        family_label: "Lumina Image 2.0",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/lumina_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "lumina-dreambooth",
        family: "lumina",
        family_label: "Lumina Image 2.0",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/lumina_train.py",
    },
    KohyaAdapterDefinition {
        id: "lumina-fine-tune",
        family: "lumina",
        family_label: "Lumina Image 2.0",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/lumina_train.py",
    },
    KohyaAdapterDefinition {
        id: "anima-lora",
        family: "anima",
        family_label: "Anima",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/anima_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "anima-loha",
        family: "anima",
        family_label: "Anima",
        training_type: "loha",
        training_type_label: "LoHa",
        trainer: "sd-scripts/anima_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "anima-lokr",
        family: "anima",
        family_label: "Anima",
        training_type: "lokr",
        training_type_label: "LoKr",
        trainer: "sd-scripts/anima_train_network.py",
    },
    KohyaAdapterDefinition {
        id: "anima-dreambooth",
        family: "anima",
        family_label: "Anima",
        training_type: "dreambooth",
        training_type_label: "DreamBooth",
        trainer: "sd-scripts/anima_train.py",
    },
    KohyaAdapterDefinition {
        id: "anima-fine-tune",
        family: "anima",
        family_label: "Anima",
        training_type: "fine_tune",
        training_type_label: "Fine-tuning",
        trainer: "sd-scripts/anima_train.py",
    },
    KohyaAdapterDefinition {
        id: "hunyuan-image-lora",
        family: "hunyuan_image",
        family_label: "HunyuanImage-2.1",
        training_type: "lora",
        training_type_label: "LoRA",
        trainer: "sd-scripts/hunyuan_image_train_network.py",
    },
];

fn fields_for_kohya_adapter(
    baseline: &[TrainingField],
    definition: KohyaAdapterDefinition,
) -> Vec<TrainingField> {
    let network_training = matches!(definition.training_type, "lora" | "loha" | "lokr");
    let textual_inversion = definition.training_type == "textual_inversion";
    let leco = definition.training_type == "leco";
    let allowed = [
        "pretrained_model_name_or_path",
        "train_data_dir",
        "dataset_config",
        "output_dir",
        "output_name",
        "resolution",
        "enable_bucket",
        "min_bucket_reso",
        "max_bucket_reso",
        "bucket_reso_steps",
        "train_batch_size",
        "max_train_epochs",
        "max_train_steps",
        "gradient_accumulation_steps",
        "gradient_checkpointing",
        "mixed_precision",
        "full_fp16",
        "save_every_n_epochs",
        "save_model_as",
        "save_precision",
        "learning_rate",
        "unet_lr",
        "text_encoder_lr",
        "optimizer_type",
        "optimizer_args",
        "lr_scheduler",
        "lr_scheduler_num_cycles",
        "lr_scheduler_power",
        "max_grad_norm",
        "seed",
        "log_with",
        "wandb_api_key",
        "logging_dir",
        "gpu_ids",
        "deepspeed",
        "advanced_parameters",
        "network_module",
        "network_dim",
        "network_alpha",
        "network_dropout",
        "rank_dropout",
        "module_dropout",
        "network_args",
        "network_weights",
        "dim_from_weights",
        "network_train_unet_only",
        "network_train_text_encoder_only",
        "cache_latents",
        "cache_latents_to_disk",
        "cache_text_encoder_outputs",
        "caption_dropout_rate",
        "caption_extension",
        "caption_tag_dropout_rate",
        "keep_tokens",
        "loss_type",
        "lr_warmup_steps",
        "reg_data_dir",
        "resume",
        "save_state",
        "sdpa",
        "shuffle_caption",
        "vae",
        "xformers",
    ];
    let mut fields = baseline
        .iter()
        .filter(|field| field.advanced || allowed.contains(&field.key.as_ref()))
        .filter(|field| {
            field.advanced
                || network_training
                || !matches!(
                    field.key.as_ref(),
                    "network_module"
                        | "network_dim"
                        | "network_alpha"
                        | "network_dropout"
                        | "rank_dropout"
                        | "module_dropout"
                )
        })
        .cloned()
        .collect::<Vec<_>>();

    if textual_inversion {
        fields.push(field(
            "token_string",
            "概念 Token",
            "network",
            "",
            "text",
            Value::String("<concept>".into()),
            &[],
            true,
            false,
            "Textual Inversion 要学习的唯一 Token。",
            "在训练集中出现的图片内容提示词里写下这个 Token，推理时用它召唤学到的概念。",
            "计划让模型学习一个新的具体概念（如特定角色、物品）时填写。",
        ));
        fields.push(field(
            "init_word",
            "初始化词",
            "network",
            "",
            "text",
            Value::String(String::new()),
            &[],
            false,
            false,
            "用已有词向量初始化 Token；留空时由上游默认处理。",
            "从某个已有词的向量开始学习，可让训练更快收敛且语义起点更可控。",
            "希望新概念从接近某个已有词的语义出发时填写；通常可以留空。",
        ));
        fields.push(field(
            "num_vectors_per_token",
            "每个 Token 的向量数",
            "network",
            "",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "增大可提升概念容量，但会提高推理提示词成本。",
            "每个概念 Token 对应的可学习向量数量，1 表示单向量嵌入。",
            "概念较复杂、单向量难以拟合时逐步增大；一般保持 1 即可。",
        ));
    }
    if leco {
        fields.retain(|field| field.key != "train_data_dir" && field.key != "dataset_config");
        fields.push(field(
            "prompts_file",
            "概念编辑 Prompt 文件",
            "dataset",
            "",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "LECO 所需的概念擦除/编辑 Prompt TOML 文件。",
            "描述如何编辑概念的 Prompt 定义文件，由 LECO 工作流提供。",
            "选择 LECO（概念编辑）训练方式时必填。",
        ));
        fields.push(field(
            "network_weights",
            "待编辑网络权重",
            "model",
            "",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "LECO 继续训练或编辑的网络权重。",
            "LECO 在此权重基础上编辑概念；通常为已完成训练的 LoRA/模型检查点。",
            "选择 LECO 训练方式时必填。",
        ));
    }
    if matches!(definition.training_type, "loha" | "lokr") {
        let module = if definition.training_type == "loha" {
            "networks.loha"
        } else {
            "networks.lokr"
        };
        if let Some(field) = fields
            .iter_mut()
            .find(|field| field.key == "network_module")
        {
            field.default = Value::String(module.into());
            field.choices = vec![module.into()];
            field.help = "由当前训练方式锁定的 Kohya 原生网络实现。".into();
        }
        if definition.training_type == "lokr" {
            fields.push(field(
                "lokr_factor",
                "LoKr Factor",
                "network",
                "",
                "number",
                Value::from(-1),
                &[],
                false,
                false,
                "控制 Kronecker 因子划分；-1 使用上游自动策略。",
                "Kronecker 乘积分解的因子大小，决定低秩近似的结构；-1 表示让上游自动选择。",
                "LoKr 适配器效果不理想时尝试调整；一般保持 -1 自动即可。",
            ));
        }
    }
    fields
}

/// This baseline mirrors the stable SDXL entry point.  At runtime the bundled
/// Python bridge augments it from `sdxl_train_network.setup_parser()` so newer
/// upstream flags remain available in the Advanced group without a Rust/UI
/// release.
pub fn builtin_adapters() -> Vec<TrainingAdapter> {
    let groups = vec![
        TrainingGroup {
            id: "model",
            label: "模型与恢复",
            description: "底模、VAE 与断点恢复",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "dataset",
            label: "数据集与 Caption",
            description: "图片目录、桶与标签",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "training",
            label: "训练",
            description: "批量、步数与损失",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "network",
            label: "LoRA 网络",
            description: "LoRA、LyCORIS 与分层权重",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "optimizer",
            label: "优化器与学习率",
            description: "优化器、调度器与参数组",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "sampling",
            label: "采样与样图",
            description: "训练预览样图与采样调度",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "performance",
            label: "精度与性能",
            description: "缓存、注意力与显存优化",
            subgroups: vec![
                TrainingSubgroup {
                    id: "memory",
                    label: "显存优化",
                },
            ],
        },
        TrainingGroup {
            id: "saving",
            label: "保存",
            description: "检查点与状态保存",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "logging",
            label: "日志与分布式",
            description: "遥测、W&B 与多卡",
            subgroups: vec![],
        },
        TrainingGroup {
            id: "advanced",
            label: "高级参数",
            description: "所有上游 CLI 参数和 TOML 覆盖",
            subgroups: vec![
                TrainingSubgroup {
                    id: "noise",
                    label: "噪声与时间步",
                },
                TrainingSubgroup {
                    id: "text",
                    label: "文本编码器与 Token",
                },
                TrainingSubgroup {
                    id: "misc",
                    label: "其他",
                },
            ],
        },
    ];
    let fields = vec![
        field(
            "pretrained_model_name_or_path",
            "底模路径",
            "model",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "用于训练的 SDXL 底模 checkpoint 或 Diffusers 模型目录",
            "训练的基础模型，LoRA/Embedding 的权重将从它开始微调。支持 .safetensors 权重文件，也支持包含配置的 Diffusers 模型目录（目录中需存在 model_index.json）。",
            "推荐使用与目标画风接近的 SDXL 底模；路径不要放在训练输出目录内，避免被自动清理。",
        ),
        field(
            "vae",
            "外置 VAE",
            "model",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "可选，覆盖底模自带的 VAE，可改善色彩与细节还原",
            "独立的 VAE 权重，用于替代底模内置 VAE。当底模自带的 VAE 解码效果偏灰、偏肉或出现伪影时，外挂更好的 VAE 可以明显改善出图色彩与细节。",
            "仅在底模内置 VAE 质量不理想时填写，推荐使用与底模同版本训练的 VAE（如 SDXL 配套的 sdxl_vae）。",
        ),
        field(
            "network_weights",
            "继续训练 LoRA",
            "model",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "加载已有的 LoRA 权重继续训练",
            "填入已有 LoRA 权重文件路径后，训练会在该权重基础上继续而不是从零开始，常用于增量补练、修复过拟合或追加新概念。加载的权重维度需与 network_dim 一致。",
            "继续精修已训练过的 LoRA 时填写；更换 network_dim 后不能直接续训。",
        ),
        field(
            "resume",
            "恢复状态目录",
            "model",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "从 Accelerate 保存的训练状态目录恢复断点",
            "恢复之前 `保存训练状态` 生成的 checkpoints 目录，可从上次中断的 epoch/step 继续训练，保留优化器状态与学习率进度，与从头训练结果更一致。",
            "训练意外中断或有计划分段训练时使用；状态目录由训练时自动生成，一般不需要手工指定新的。",
        ),
        field(
            "train_data_dir",
            "训练集目录",
            "dataset",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "存放训练图片与同名 .txt Caption 的目录",
            "训练数据主目录，目录内每张图片配一个同名的 .txt 标签文件即为一条样本。工具会按你的数据集配置自动生成 dataset.toml，无需手动编写。",
            "子目录结构建议：<主目录>/<概念目录>/图片+同名 caption；同一概念图片数量建议 50-300 张。",
        ),
        field(
            "reg_data_dir",
            "正则化数据集",
            "dataset",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "可选的正则化（正则）图片目录，用于抑制过拟合与概念漂移",
            "正则化图片与 Caption 用于对抗过拟合：训练时与目标图片按比例混用，帮助模型保留原始能力不被新概念完全覆盖。图片不需要与训练集同一组 caption。",
            "从基础模型直接生成 100-1000 张与目标概念相近但不同的图；目标概念构图复杂或数据集小于 200 张时强烈建议使用。",
        ),
        field(
            "dataset_config",
            "数据集配置",
            "dataset",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            true,
            "手动编写的高级数据集 TOML/JSON 配置（优先级高于上方的简单设置）",
            "直接指定 kohya 风格的高阶数据集配置文件（dataset.toml/json），可精确控制每个子集的图片目录、重复次数、标签扩展名等。工具会提交时校验其存在。",
            "需要多子集混合、条件图、蒙版等高级数据集能力时使用；填写后工具不再自动生成数据集配置。",
        ),
        field(
            "resolution",
            "训练分辨率",
            "dataset",
            "",
                        "text",
            Value::String("1024,1024".into()),
            &[],
            true,
            false,
            "训练分辨率，格式: 宽,高；开启桶后为参考分辨率",
            "所有图片会被缩放到该分辨率附近供训练。SDXL 推荐按 1024 的倍数组织；开启桶（Bucket）后此值作为基准尺寸，图片按最近桶缩放以避免强制拉伸。",
            "SDXL 推荐 1024,1024（或 极少数手机竖构图任务用 832,1216 等）；小于 1024 会明显损失细节。",
        ),
        field(
            "enable_bucket",
            "启用 Bucket",
            "dataset",
            "",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "按图片原始宽高比自动分桶，避免被拉伸变形",
            "把训练集按宽高比分组到不同尺寸的桶中（分辨率需为 bucket_reso_steps 的倍数），模型在每个桶内训练，减少长宽比被强行拉方的副作用。",
            "训练集宽高比差异大（混合横竖构图）时强烈建议开启；全部为 1:1 方图时可以关闭。",
        ),
        field(
            "min_bucket_reso",
            "最小 Bucket",
            "dataset",
            "",
                        "number",
            Value::from(256),
            &[],
            false,
            false,
            "最小桶分辨率（短边），低于此值的图会被放大到该尺寸",
            "限制分桶的下限，防止过小的图被分到极低分辨率的桶导致细节损失。底模原生分辨率附近的值通常为最佳。",
            "SDXL 推荐 512-640；不超过底模原生分辨率（1024）。",
        ),
        field(
            "max_bucket_reso",
            "最大 Bucket",
            "dataset",
            "",
                        "number",
            Value::from(2048),
            &[],
            false,
            false,
            "最大桶分辨率（长边）",
            "限制分桶的上限，防止超出显存或偏离底模能力的分辨率出现。超过该值的图会被缩小。",
            "SDXL 推荐 1280-1536；过大（如 2048+）收益低且训练缓慢、易过拟合。",
        ),
        field(
            "bucket_reso_steps",
            "Bucket 步长",
            "dataset",
            "",
                        "number",
            Value::from(32),
            &[],
            false,
            false,
            "分桶分辨率步长，默认 64（SDXL 支持 32 的倍数）",
            "桶分辨率按该步长取整。步长越小桶越精细、内存占用越高。",
            "SDXL 默认 32 或 64；显存紧张时选 64。",
        ),
        field(
            "caption_extension",
            "Caption 扩展名",
            "dataset",
            "",
                        "text",
            Value::String(".txt".into()),
            &[],
            false,
            false,
            "Caption 标签文件扩展名（.txt / .caption）",
            "指定与图片同名的标签文件的扩展名。kohya 系默认 .txt，WD14/BLIP 等打标工具也常用 .txt。",
            "保持 .txt 即可；换用其他工具生成的 .caption 时再修改。",
        ),
        field(
            "shuffle_caption",
            "随机打乱标签",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            false,
            "训练时随机打乱标签顺序，降低位置依赖",
            "每个样本训练时把标签随机重排（保留前 keep_tokens 个），模型不会把标签与顺序绑定，泛化更好。",
            "标签较多（>5 个）时推荐开启；标签顺序含语义层级：暂停。",
        ),
        field(
            "keep_tokens",
            "保留前置标签",
            "dataset",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            false,
            "打乱标签时固定在开头的 token 数",
            "配合 shuffle_caption：前 N 个标签保持顺序不动，常用于保留触发词/质量词处于开头强位置。",
            "触发词单独在最前时建议 keep_tokens=1 或 2；无固定词时为 0。",
        ),
        field(
            "caption_dropout_rate",
            "Caption 丢弃率",
            "dataset",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            false,
            "整条 Caption 随机丢弃比例（0-1）",
            "训练时按概率丢弃完整标签串（不含前 keep_tokens 个），让模型学会无条件生成，避免完全依赖标签。",
            "推荐 0.05-0.15；风格化/居间任务可放宽到 0-0.3。",
        ),
        field(
            "caption_tag_dropout_rate",
            "Tag 丢弃率",
            "dataset",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            false,
            "单个标签随机丢弃比例（0-1）",
            "训练时按概率丢弃每条 caption 中的部分标签，提升模型对标签的容错与泛化。",
            "推荐 0-0.1；打标噪声较大时适当提高。",
        ),
        field(
            "max_train_epochs",
            "最大 Epoch",
            "training",
            "",
                        "number",
            Value::from(10),
            &[],
            false,
            false,
            "最大训练轮数（epoch）",
            "训练数据完整过一遍的次数。与 max_train_steps 二选一堆优先：两者都填时以步数为准。epoch 数适应不同规模的同类任务。",
            "LoRA 常用 5-15；数据集小（<200 张）可到 15-25；看验证损失与样图停止。",
        ),
        field(
            "max_train_steps",
            "最大 Step",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "最大训练步数（step），填写后优先于 epoch 控制",
            "以精确步数控制训练总时长，适用于定长实验或与其它任务对齐迭代量。步数 = epoch × 每轮步数，每轮步数 = 数据集大小÷batch。",
            "边际收益一般在 1500-4000 步后递减；配合学习率调度在该处收敛。",
        ),
        field(
            "train_batch_size",
            "批量大小",
            "training",
            "",
                        "number",
            Value::from(1),
            &[],
            true,
            false,
            "单卡单次训练的图片张数",
            "每个优化步骤喂入模型的图片数。batch 越大梯度越稳、每步越贵。有效 batch = batch × 梯度累积 × GPU 数。",
            "SDXL 8GB/12GB 显存通常 1-2，24GB 可 4-8；先保证不 OOM 再增大。",
        ),
        field(
            "gradient_accumulation_steps",
            "梯度累积",
            "training",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            false,
            "梯度累积步数，等效放大 batch",
            "小显存下提高有效 batch：每 N 步平均一次梯度再更新参数。数值越大越接近大 batch 训练的稳定性。",
            "有效 batch 目标 16-32：batch=2 时建议 8-16；无需刻意追求 64+。",
        ),
        field(
            "gradient_checkpointing",
            "梯度检查点",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "用重算前向激活换显存（训练变慢）",
            "不保存全部中间激活、反向时重算，大幅降低显存占用（对 SDXL 的 UNet 可省数 GB），代价是每步时间增加约 20-30%。",
            "显存不足时开启；显存充裕时关闭以提速。",
        ),
        field(
            "loss_type",
            "损失函数",
            "training",
            "",
                        "select",
            Value::String("l2".into()),
            &["l1", "l2", "huber", "smooth_l1"],
            false,
            false,
            "训练损失函数：l2 / lpips / smooth_l1",
            "计算重建误差的方式。l2（MSE）最常用且稳定；lpips 感知损失更接近人眼但对实现版本敏感；smooth_l1 对离群值更鲁棒。",
            "常规 LoRA 用 l2；细节/风格感知要求高、已装好 lpips 时用 lpips。",
        ),
        field(
            "network_train_unet_only",
            "仅训练 U-Net",
            "training",
            "",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "冻结文本编码器，只训练 UNet 相关层",
            "训练中保持文本编码器权重完全不更新，加快训练、减少显存，适合只学画面风格/构图而标签语义不需进化的任务。",
            "风格 LoRA 常用；涉及新概念名词与画面绑定时建议放开文本编码器。",
        ),
        field(
            "network_train_text_encoder_only",
            "仅训练文本编码器",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "冻结 UNet，只训练文本编码器相关层",
            "反向策略：只更新文本编码器分支，一般配合 UNet-only 分阶段进行，用于把标签与概念绑定。",
            "概念绑定阶段使用；单独全程只训文本编码器效果一般。",
        ),
        field(
            "network_module",
            "网络模块",
            "network",
            "",
                        "select",
            Value::String("networks.lora".into()),
            &[
            "networks.lora",
            "networks.dylora",
            "networks.oft",
            "lycoris.kohya",
            ],
            true,
            false,
            "LoRA 实现模块：networks.lora / networks.lokr / networks.lycoris.kohya",
            "选择 LoRA 权重结构与实现。标准 lora 兼容性最好；LoCon/LoHa/LoKr 等为变体结构，文件不通用。",
            "常规用 networks.lora；追求层次感可试 LoCon（networks.lokr 属 LoKr）。",
        ),
        field(
            "network_dim",
            "Network Dim",
            "network",
            "",
                        "number",
            Value::from(32),
            &[],
            true,
            false,
            "LoRA 秩（rank）：决定表达容量",
            "矩阵分解的秩，决定可学习参数多少。秩越大容量越高、越易过拟合与增大文件体积。",
            "SDXL 常用 8-32；风格 LoRA 8-16，多概念/复杂细节 32-64。",
        ),
        field(
            "network_alpha",
            "Network Alpha",
            "network",
            "",
                        "number",
            Value::from(32),
            &[],
            false,
            false,
            "LoRA alpha：缩放系数，控制注入强度",
            "LoRA 注入强度 = alpha/dim。alpha 与 dim 相等时默认强度，alpha 越小注入越弱、越安全。",
            "dim=32 时 alpha 常用 16-32（学习率 1e-4 时）；dim 换数值按比例参考。",
        ),
        field(
            "network_dropout",
            "Network Dropout",
            "network",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            false,
            "LoRA 权重随机丢弃比例（0-1）",
            "训练时随机把部分 LoRA 参数置零，等价于对网络做正则，降低过拟合；过大容量会严重不足。",
            "推荐 0-0.2；过拟合明显时从 0.05 起步逐步加大。",
        ),
        field(
            "network_args",
            "Network Args",
            "network",
            "",
                        "list",
            Value::Array(vec![]),
            &[],
            false,
            true,
            "传给网络模块的额外 key=value 参数",
            "LoCon/LoHa/LoKr 或自定义网络的专属参数（如 conv_dim=64, conv_alpha=32），标准 LoRA 一般不需要。",
            "使用变体结构时按该结构的文档填写；标准 LoRA 留空。",
        ),
        field(
            "learning_rate",
            "总学习率",
            "optimizer",
            "",
                        "number",
            Value::from(0.0001),
            &[],
            false,
            false,
            "全局学习率；未单独指定 UNet/文本编码器时对全部参数生效",
            "参数更新的步长核心超参。过高发散、过低收敛慢。单独填 unet_lr/text_encoder_lr 时此值为兜底。",
            "标准 LoRA：1e-4 附近（1e-5 ~ 5e-4 区间）；AdamW8bit 同区间。",
        ),
        field(
            "unet_lr",
            "U-Net 学习率",
            "optimizer",
            "",
                        "number",
            Value::from(0.0001),
            &[],
            false,
            false,
            "仅对 U-Net 分支生效的学习率",
            "U-Net 承担画面生成主体，给稍高的学习率可加快风格学习；给低学习率可更平稳。",
            "建议 = 全局学习率 × 1.0；与文本编码器不同速时常用 ×2（1e-4 对 5e-5）。",
        ),
        field(
            "text_encoder_lr",
            "Text Encoder 学习率",
            "optimizer",
            "",
                        "number",
            Value::from(0.00001),
            &[],
            false,
            false,
            "仅对文本编码器分支生效的学习率",
            "文本编码器与语义绑定密切相关，学习率过高易导致标签语义漂移，通常低于 U-Net。",
            "建议 = 全局 × 0.2-0.5，如 2e-5 ~ 5e-5。",
        ),
        field(
            "optimizer_type",
            "优化器",
            "optimizer",
            "",
                        "select",
            Value::String("AdamW8bit".into()),
            &[
            "AdamW",
            "AdamW8bit",
            "PagedAdamW8bit",
            "Lion",
            "Lion8bit",
            "AdaFactor",
            "Prodigy",
            "DAdaptation",
            "DAdaptAdam",
            "DAdaptLion",
            "SGDNesterov",
            "RAdamScheduleFree",
            "pytorch_optimizer.CAME",
            ],
            true,
            false,
            "优化器：AdamW8bit / AdamW / Adafactor / Lion 等",
            "选择参数更新算法。AdamW8bit 内存友好且稳定，最常用；Adafactor 更省内存；Prodigy/D-Adapt 自动调学习率。",
            "常规训练选 AdamW8bit；显存紧张选 Adafactor；不想调学习率试 Prodigy。",
        ),
        field(
            "optimizer_args",
            "优化器参数",
            "optimizer",
            "",
                        "list",
            Value::Array(vec![]),
            &[],
            false,
            true,
            "优化器专属参数，key=value 列表",
            "把额外参数传给优化器（如 betas=(0.9,0.999)、weight_decay=0.01、d=0.06 等），不同优化器有不同的可用键。",
            "按所选优化器文档填写；需要调 beta/weight_decay 时使用。",
        ),
        field(
            "lr_scheduler",
            "学习率调度器",
            "optimizer",
            "",
                        "select",
            Value::String("cosine_with_restarts".into()),
            &[
            "linear",
            "cosine",
            "cosine_with_restarts",
            "polynomial",
            "constant",
            "constant_with_warmup",
            ],
            true,
            false,
            "学习率调度器：cosine / constant / constant_with_warmup / polynomial 等",
            "训练过程中学习率的变化曲线。cosine 自然衰减适合大多数任务；constant 配 warmup 适配稳定微调。",
            "推荐 cosine（或 cosine_with_restarts）；smooth 微调时用 constant_with_warmup。",
        ),
        field(
            "lr_warmup_steps",
            "预热 Step",
            "optimizer",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            false,
            "学习率预热步数",
            "从很小值线性爬升到目标学习率的步数，避免初期大更新破坏预训练权重。",
            "推荐总步数的 5-10%；如 2000 步训练配 100-200。",
        ),
        field(
            "lr_scheduler_num_cycles",
            "调度重启周期",
            "optimizer",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            false,
            "cosine_with_restarts 的重启次数",
            "cosine 曲线在一个训练周期内重复的波数，多次重启可多次细调。",
            "cosine_with_restarts 时常用 1；追求多段重尝试可 2-3。",
        ),
        field(
            "mixed_precision",
            "混合精度",
            "performance",
            "",
                        "select",
            Value::String("bf16".into()),
            &["no", "fp16", "bf16"],
            true,
            false,
            "训练混合精度：fp16 / bf16 / no",
            "主训练精度模式：fp16 内存占用低但需防溢出；bf16 对 30 系+ 新卡更稳；no 为全 fp32 最稳最慢。",
            "消费级显卡推荐 bf16（40 系）或 fp16（30 系）；追求最高稳定性用 no。",
        ),
        field(
            "full_fp16",
            "全 FP16",
            "performance",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "所有模型分量都使用 fp16（需 mixed_precision=fp16）",
            "把 UNet、文本编码器、VAE 全部以 fp16 加载与计算，极致省显存但数值风险最高。",
            "显存极紧张时，与 mixed_precision=fp16 一起开启。",
        ),
        field(
            "full_bf16",
            "全 BF16",
            "performance",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "所有模型分量都使用 bf16（需 mixed_precision=bf16）",
            "全部组件 bf16，与 full_fp16 同理但更稳，适合 Ampere+ 架构。",
            "显存紧张且显卡支持 bf16 时开启。",
        ),
        field(
            "cache_latents",
            "缓存 Latent",
            "performance",
            "",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "预先把图片编码成 VAE latent 缓存进内存",
            "训练开始前一次性跑完 VAE 编码并缓存，启动后每步省去 VAE 前向；显存换速度。",
            "数据量不大（可一次性载入内存）时推荐开启。",
        ),
        field(
            "cache_latents_to_disk",
            "Latent 写入磁盘",
            "performance",
            "",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "把 latent 缓存写入磁盘，省内存但重启更慢",
            "相对内存缓存，存放于磁盘，反复重启训练时复用，节省内存占用。",
            "数据集大、内存不足时，配合 cache_latents 开启。",
        ),
        field(
            "cache_text_encoder_outputs",
            "缓存文本编码器",
            "performance",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            false,
            "缓存文本编码器输出，训练时不重算文本分支",
            "文本编码器输出缓存到内存，每步减少一次文本编码前向（文本分支训练被关闭时常开）。",
            "network_train_unet_only 训练时强烈建议开启，显著提速。",
        ),
        field(
            "cache_text_encoder_outputs_to_disk",
            "文本编码器缓存到磁盘",
            "performance",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "文本编码器输出缓存写入磁盘",
            "把文本编码器输出持久化到磁盘，重启后直接加载。",
            "配合 disk 缓存场景使用，适合反复实验同一数据集。",
        ),
        field(
            "xformers",
            "xFormers",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "使用 xFormers 加速注意力",
            "老一代注意力优化库，在部分旧环境中比默认快；新环境优先 SDPA。",
            "环境内已装 xformers 且 SDPA 不可用时使用。",
        ),
        field(
            "sdpa",
            "PyTorch SDPA",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "使用 PyTorch 内置 SDPA 快速注意力",
            "PyTorch 2.x 的快速注意力路径，无需额外库、显存友好，推荐优先。",
            "推荐开启（PyTorch 2.0+）。",
        ),
        field(
            "save_model_as",
            "模型格式",
            "saving",
            "",
                        "select",
            Value::String("safetensors".into()),
            &["safetensors", "ckpt", "pt"],
            true,
            false,
            "保存格式：safetensors / ckpt / both",
            "输出 LoRA 权重的文件格式。safetensors 体积小且更安全；both 同时输出两份。",
            "推荐 safetensors；需要兼容旧工具（WebUI 老版本 / A1111）时用 both。",
        ),
        field(
            "save_precision",
            "保存精度",
            "saving",
            "",
                        "select",
            Value::String("fp16".into()),
            &["fp16", "bf16", "float"],
            true,
            false,
            "保存权重的精度：float / fp16 / bf16",
            "落盘精度。fp16 体积减半且几乎无感损失；float 与训练一致更精确。",
            "推荐 fp16（体积小、兼容性好）；追求无损保存用 float。",
        ),
        field(
            "output_dir",
            "输出目录",
            "saving",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "训练产物输出目录（LoRA 权重与样图）",
            "保存最终权重、中间检查点与样图的文件夹。",
            "建议专用目录；容量预留（权重一般 <500MB/page）。",
        ),
        field(
            "output_name",
            "输出名称",
            "saving",
            "",
                        "text",
            Value::String("sdxl-lora".into()),
            &[],
            true,
            false,
            "输出权重文件的前缀名称",
            "生成的权重文件名（会加上训练步数/epoch 后缀）。",
            "用易识别的语义名，如 my_style_sdxl。",
        ),
        field(
            "save_every_n_epochs",
            "每 N Epoch 保存",
            "saving",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            false,
            "每 N 个 epoch 保存一次中间权重",
            "周期性落盘中间结果，作为后续挑选/回退的候选点。",
            "推荐 1-2；磁盘大可选择更频，便于观察训练过程。",
        ),
        field(
            "save_state",
            "保存训练状态",
            "saving",
            "",
                        "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "保存完整训练状态（可断点续训）",
            "额外保存优化器状态、调度器进度与随机数状态，支持无缝续训与暂停。",
            "长训练推荐开启；短实验可关省磁盘。",
        ),
        field(
            "sample_prompts",
            "样图 Prompt 文件",
            "sampling",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "样图 Prompt 文件路径（每行一个 prompt）",
            "训练过程中周期性按这些提示词生成样图，直观观察风格/概念随训练演化的过程。",
            "写 3-10 个与目标概念匹配的 prompt，一行一个保存为 txt。",
        ),
        field(
            "sample_every_n_epochs",
            "样图间隔",
            "sampling",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            false,
            "每 N 个 epoch 生成一次样图",
            "样图采样频率（按 epoch）。太频浪费时间，太疏难以观察变化。",
            "推荐 1-2；可结合总 epoch 数保证看到 3-6 组中间结果。",
        ),
        field(
            "sample_sampler",
            "样图采样器",
            "sampling",
            "",
                        "select",
            Value::String("euler_a".into()),
            &[
            "ddim",
            "pndm",
            "lms",
            "euler",
            "euler_a",
            "heun",
            "dpm_2",
            "dpm_2_a",
            "dpmsolver",
            "dpmsolver++",
            "dpmsingle",
            "k_lms",
            "k_euler",
            "k_euler_a",
            "k_dpm_2",
            "k_dpm_2_a",
            ],
            false,
            true,
            "样图生成使用的采样器",
            "生成样图时使用的扩散采样器，尽量与最终出图工具一致，观察结果才更有参考意义。",
            "推荐与平时出图相同（如 Euler a / DPM++ 2M Karras）。",
        ),
        field(
            "logging_dir",
            "日志目录",
            "logging",
            "",
                        "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "日志目录（tensorboard / jsonl 等落盘位置）",
            "保存训练曲线日志与配置快照的目录，用 tensorboard 可查看损失曲线。",
            "用默认即可；后续用 tensorboard --logdir 打开。",
        ),
        field(
            "log_with",
            "上游日志器",
            "logging",
            "",
                        "select",
            Value::String("tensorboard".into()),
            &["tensorboard", "wandb"],
            false,
            false,
            "日志后端：tensorboard / wandb / all / report",
            "选择训练指标记录目标：tensorboard 本地免费；wandb 云端可视化；all 同时。",
            "本地回看用 tensorboard；团队/远程用 wandb。",
        ),
        field(
            "wandb_api_key",
            "W&B API Key",
            "logging",
            "",
                        "secret",
            Value::String(String::new()),
            &[],
            false,
            true,
            "W&B API Key（仅上传时写入）",
            "连接 Weights & Biases 的认证令牌，仅在提交训练并调用其写权限时使用，不会写入训练 TOML。",
            "log_with=wandb 时填写；在 wandb 后台生成。",
        ),
        field(
            "seed",
            "随机种子",
            "training",
            "",
                        "number",
            Value::from(1337),
            &[],
            false,
            false,
            "随机种子：相同种子 + 相同数据 = 可复现训练",
            "固定数据打乱与参数初始化等随机过程，便于对比实验或复现训练结果。",
            "常规保持默认（随机）；做消融对比研究时固定同一数值。",
        ),
        field(
            "gpu_ids",
            "GPU",
            "logging",
            "",
                        "list",
            Value::Array(vec![]),
            &[],
            false,
            false,
            "使用的 GPU 编号列表，空为自动分配",
            "指定哪些物理 GPU 参与训练（多卡逗号分隔，如 0,1）；留空自动选择空闲卡。",
            "单卡训练留空或填显存最大的卡；多卡填 0,1,...",
        ),
        field(
            "deepspeed",
            "DeepSpeed",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "启用 DeepSpeed 训练",
            "使用 DeepSpeed 引擎（ZeRO 分片/卸载），适合超大模型与极低显存；配置复杂、变化大，若无法运行应由工具覆盖。",
            "单个进程或低显存卡跑不动 27B 以上模型 / 微调大参数时使用；常规 LoRA 训练可关闭。",
        ),
        field(
            "adaptive_noise_scale",
            "自适应噪声缩放",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "噪声自适应缩放系数",
            "把潜在空间激活的均值绝对值按该系数叠加到噪声偏移上，让噪声强度随内容自适应。",
            "0 关闭；希望在噪声偏移基础上自适应增益时试 0.5-2。",
        ),
        field(
            "alpha_mask",
            "Alpha 掩码",
            "network",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "用图片的 alpha 通道作为训练的掩码权重",
            "把 RGBA 图片的 alpha 通道当作 loss 蒙版（与 masked_loss 配合），实现更精细的局部训练控制。",
            "使用带透明通道的蒙版图时开启，配合 masked_loss。",
        ),
        field(
            "async_upload",
            "异步上传",
            "saving",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "异步上传到 HuggingFace，不阻塞训练",
            "权重/状态上传在后台线程进行，避免上传期间训练停顿。",
            "网络慢且上传频繁时推荐开启。",
        ),
        field(
            "base_weights",
            "基座权重",
            "network",
            "",
                        "list",
            Value::Null,
            &[],
            false,
            true,
            "训练前合并进模型的一组 LoRA 权重（列表）",
            "开始训练前先按倍率把底模与若干 LoRA 融合，实现“在 LoRA 基础上再训练”的叠加工作流。",
            "多 LoRA 叠加工作流使用；单个路径与 network_weights 二选一。",
        ),
        field(
            "base_weights_multiplier",
            "基座权重乘数",
            "network",
            "",
                        "list",
            Value::Null,
            &[],
            false,
            true,
            "对应 base_weights 的合并倍率列表",
            "逐个控制上面每个 base LoRA 的融合强度，顺序与 base_weights 一一对应。",
            "通常 1.0；需要弱化某基础 LoRA 时设 0.3-0.8。",
        ),
        field(
            "bucket_no_upscale",
            "Bucket 禁止放大",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "小图不放大：宁可分到更小的桶也不用放大",
            "开启后窄小的图片不会被放大到更大的桶（避免插值带来的模糊），而是进入更小的桶训练。",
            "训练集包含较多低分辨率老图时推荐开启。",
        ),
        field(
            "cache_info",
            "打印缓存信息",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "缓存数据集元信息（图片尺寸、标签），加快多次启动的解析",
            "首次启动时把图片尺寸与标签读入内存缓存，后续启动/验证更快；对超大数据集尤其明显。",
            "数据集很大（>2000 张）时推荐开启。",
        ),
        field(
            "caption_dropout_every_n_epochs",
            "每 N Epoch 丢弃 Caption",
            "dataset",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            true,
            "每隔 N 个 epoch 清空全部 Caption 训练一轮",
            "周期性整轮清空标签训练，让模型在无标签条件下也保持稳定出图，常用于加强无条件分支。",
            "推荐 500-1000 epoch 左右触发一次；训练节奏快可调小。",
        ),
        field(
            "caption_extention",
            "Caption 扩展名（旧拼写）",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "Caption 扩展名别名字段（兼容旧命名）",
            "历史遗留的拼写别名，与 caption_extension 等价（kohya 同时接受两种拼写）。通常无需单独设置。",
            "保持默认；工具会优先使用 caption_extension。",
        ),
        field(
            "caption_prefix",
            "Caption 前缀",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "加到每条 Caption 前的前缀字符串",
            "在每个训练标签串前面统一追加一段文本，常用于固定触发词、质量词或居间提示词。",
            "例如 `masterpiece, best quality,`；触发词建议保持在每条 caption 内不重复前缀。",
        ),
        field(
            "caption_separator",
            "Caption 分隔符",
            "dataset",
            "",
                        "text",
            Value::String(",".into()),
            &[],
            false,
            true,
            "打乱标签时用于分隔标签串的连接符",
            "shuffle_caption 打乱标签顺序时，标签之间插入的分隔符。默认逗号，可换成句号等加强对标签的语义拆分。",
            "默认 `,` 即可；想要更强区分时可用 `, ` 或数字分隔。",
        ),
        field(
            "caption_suffix",
            "Caption 后缀",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "加到每条 Caption 末尾的后缀字符串",
            "用于在标签串尾部统一追加固定词（如画师名、成片风格词），不必手动改每个 txt。",
            "例如 `, solo`；若与现有标签重复则先清理原标签中的对应词。",
        ),
        field(
            "clip_skip",
            "CLIP 跳过层",
            "advanced",
            "text",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "跳过 CLIP 最后的 N 层（取早期特征）",
            "从倒数第 N 层输出作为文本特征，跳过靠近最终层的深层（常与过度拟合的纹理相关），改善构图与语义遵循。",
            "默认留空（1）；画面构图失衡时试 2。",
        ),
        field(
            "color_aug",
            "颜色增强",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "随机改变色调/饱和度/亮度等颜色增强",
            "对输入图片做随机颜色扰动，让模型对色彩更鲁棒，抑制色彩过拟合；代价是颜色细节学习变弱。",
            "训练集色彩单一（同滤镜）时推荐开启；追求颜色准确性时关闭。",
        ),
        field(
            "conditioning_data_dir",
            "条件数据目录",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "条件训练（ControlNet 风格）的条件图目录",
            "为每张训练图提供对应的条件图（如线稿、深度图、姿态图），文件名与训练图一致。需要配合 mask/条件数据集使用。",
            "仅运行条件（ControlNet/IP-Ada 类）训练时填写；普通 LoRA 训练留空。",
        ),
        field(
            "console_log_file",
            "控制台日志文件",
            "logging",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "控制台日志输出到的文件路径",
            "把训练日志同时写入文件，方便长训练后回看。",
            "需要留档时填写；不填只输出控制台。",
        ),
        field(
            "console_log_level",
            "控制台日志级别",
            "logging",
            "",
                        "select",
            Value::Null,
            &["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"],
            false,
            true,
            "控制台日志级别：DEBUG / INFO / WARNING / ERROR",
            "控制终端打印的日志详细程度。",
            "默认 INFO；排障时切 DEBUG。",
        ),
        field(
            "console_log_simple",
            "简化控制台日志",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "控制台日志简化输出",
            "跳过轨迹回溯等冗长信息，输出更紧凑。",
            "日志太长盖屏时开启。",
        ),
        field(
            "cpu_offload_checkpointing",
            "CPU 卸载检查点",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "把检查点（激活缓存）卸载到 CPU",
            "激活 checkpoint 存 CPU 内存，进一步省显存；明显拖慢训练。",
            "高分辨率/大 batch 极低显存场景的折中选择。",
        ),
        field(
            "dataset_class",
            "数据集类",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "数据集实现类名（高级）",
            "前端生成的 dataset.toml 中使用的数据集类名，一般由工具自动选择，无需修改。",
            "保持默认；仅排查数据集加载问题时由工具诊断后调整。",
        ),
        field(
            "dataset_repeats",
            "数据集重复次数",
            "dataset",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            true,
            "整个数据集整体重复次数（epoch 内多轮）",
            "每个 epoch 内数据集整体重复的次数，等效放大 epoch 训练量。与每个子集的 repeats 是两层概念，此处的值作用于整批数据。",
            "默认 1；显存小、batch 小时可通过放大次数保证迭代量充足。",
        ),
        field(
            "ddp_gradient_as_bucket_view",
            "DDP 梯度桶视图",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "DDP 梯度以 bucket view 方式同步（省显存）",
            "把梯度原地打包为视图减少一次复制，省显存，配合静态图示意使用。",
            "显存紧张的多卡训练开启。",
        ),
        field(
            "ddp_static_graph",
            "DDP 静态图",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "DDP 静态图模式（加速多卡）",
            "静态图假设网络结构不变，DDP 通信更少；若动态网络会出错。",
            "标准 LoRA 多卡训练可开启。",
        ),
        field(
            "ddp_timeout",
            "DDP 超时",
            "logging",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "分布式训练等待超时（秒）",
            "多卡进程组同步的等待上限，卡数多时默认值可能不够。",
            "多卡 4 张+ 且第一轮慢时从 1800 调大。",
        ),
        field(
            "debiased_estimation_loss",
            "无偏估计损失",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "使用去偏损失估计，消除时间步噪声幅值偏差",
            "对噪声幅度按时间步去偏（SDXL 训练实用技巧），让各信噪比区间的损失权重更均衡。",
            "SDXL 训练推荐开启；开启后通常能小幅提升收敛稳定性。",
        ),
        field(
            "debug_dataset",
            "调试数据集",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "资料集调试模式：输出每轮数据和网络结构信息",
            "以调试模式运行数据集处理，打印分辨率桶、每轮输入、网络结构等信息，用于排查数据加载问题。",
            "仅在排查数据问题时临时开启；不要用于正式训练。",
        ),
        field(
            "dim_from_weights",
            "从权重推断维度",
            "network",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "从加载的权重自动推断 dim 与 alpha",
            "提供 network_weights 续训或权重合并时，直接从现有权重自动确定秩与 alpha，避免手工对齐出错。",
            "续训/合并已有 LoRA 且不确定其 dim 时推荐开启。",
        ),
        field(
            "disable_mmap_load_safetensors",
            "禁用 mmap 加载",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "禁用 mmap 加载 safetensors（解决部分文件锁/崩溃）",
            "不用内存映射方式读取 safetensors，规避 Windows 文件占用或损坏报告问题，加载稍慢。",
            "模型文件在网络上、被占用报错或加载崩溃时开启。",
        ),
        field(
            "dynamo_backend",
            "Dynamo 后端",
            "performance",
            "",
                        "select",
            Value::String("inductor".into()),
            &["eager", "aot_eager", "inductor", "aot_ts_nvfuser", "nvprims_nvfuser", "cudagraphs", "ofi", "fx2trt", "onnxrt", "tensort", "ipex", "tvm"],
            false,
            true,
            "torch.compile 的后端选择（inductor 等）",
            "选择编译后端，通常用 inductor 或 eagermode 调试。",
            "torch_compile 开启时按环境选择；默认 inductor。",
        ),
        field(
            "enable_wildcard",
            "启用通配符",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "启用标签通配符",
            "开启后标签文本支持通配符动态替换（如 __hair__ 随机取多组词），丰富样本多样性。",
            "打标阶段已生成通配符语法时开启；普通标签留默认。",
        ),
        field(
            "face_crop_aug_range",
            "人脸裁剪增强范围",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "随机人脸裁剪增强范围，如 1.0-3.0",
            "以人脸为中心随机放大裁剪的倍数范围，让人物脸部在训练中呈现不同大小，改善脸部细节多变能力。",
            "人脸特写任务推荐 1.0-3.0；整身穿插构图为主时留空。",
        ),
        field(
            "flip_aug",
            "水平翻转增强",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "水平随机翻转增强",
            "以 50% 概率水平翻转图片（标签会相应处理左右语义），等效扩充数据集；含文字/左右语义概念时慎用。",
            "数据集较小且无左右语义依赖时推荐开启。",
        ),
        field(
            "fp16_master_weights_and_gradients",
            "FP16 主权重与梯度",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "优化器主权重与梯度也以 fp16 保存",
            "进一步把优化器内部状态降为 fp16，省显存；数值稳定性降低。",
            "DeepSpeed/大模型场景、显存吃紧且已接受 fp16 风险时使用。",
        ),
        field(
            "fp8_base",
            "FP8 基座",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "底模以 FP8 精度加载（可省显存）",
            "将底模权重量化到 FP8 加载，支持 FP8 的硬件上可显著省显存。",
            "显卡支持 FP8（H100 等数据中心卡）且模型支持时使用。",
        ),
        field(
            "fp8_base_unet",
            "FP8 U-Net 基座",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "仅 U-Net 部分使用 FP8 加载",
            "只对 UNet 应用 FP8 量化，保留文本编码器精度，折中方案。",
            "同上，仅特定硬件下的专业优化。",
        ),
        field(
            "fused_backward_pass",
            "融合反向传播",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "融合反向传播与优化器步骤，减少显存峰值",
            "SDXL 上把 backward 和 optimizer step 合并执行，降显存峰值；与 DeepSpeed 不兼容。",
            "24GB 以下显存推荐尝试开启。",
        ),
        field(
            "highvram",
            "高显存",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "把模型尽量全部放显存（VRAM 大于 RAM 的机器）",
            "适合 VRAM > RAM 的环境（如 Colab 大显卡），避免模型在 CPU 与 GPU 间搬动。",
            "显存远大于内存的机器开启（如 24GB VRAM / 8GB RAM）。",
        ),
        field(
            "huber_c",
            "Huber C",
            "training",
            "",
                        "number",
            Value::from(0.1),
            &[],
            false,
            true,
            "Huber 损失的分界常数",
            "当使用 huber 类型损失时，设定 L1/L2 切换的阈值。",
            "噪声预测任务常用 0.1-1.0；配合 huber_schedule 使用。",
        ),
        field(
            "huber_scale",
            "Huber 缩放",
            "training",
            "",
                        "number",
            Value::from(1.0),
            &[],
            false,
            true,
            "Huber 损失的缩放系数",
            "调整 huber 损失整体幅度，配合 learning_rate 平衡。",
            "保持默认 1.0；损失曲线过陡/过缓时微调。",
        ),
        field(
            "huber_schedule",
            "Huber 调度",
            "training",
            "",
                        "select",
            Value::String("snr".into()),
            &["constant", "exponential", "snr"],
            false,
            true,
            "huber 分界常数随训练进度变化的调度方式",
            "选择 huber_c 随时间变化的策略（如分段线性），训练初期大 c 后期小 c 提升鲁棒性。",
            "普通任务选择默认/constant；追求细节时试分段调度。",
        ),
        field(
            "huggingface_path_in_repo",
            "HF 仓库内路径",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "仓库内路径：往哪个子目录上传",
            "权重默认上传到仓库根目录；指定子目录可多版本并存。",
            "多版本管理时填子目录名，默认根目录。",
        ),
        field(
            "huggingface_repo_id",
            "HF 仓库 ID",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "HuggingFace 仓库 ID（user/repo 格式）",
            "上传与下载的 HF 仓库标识。",
            "按 HF 用户名/仓库名填写；可使用重复仓库轮换权重。",
        ),
        field(
            "huggingface_repo_type",
            "HF 仓库类型",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "HF 仓库类型：model / dataset / space",
            "仓库所属类型，LoRA 一般用 model。",
            "保持 model。",
        ),
        field(
            "huggingface_repo_visibility",
            "HF 仓库可见性",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "HF 仓库可见性：public / private",
            "仓库公开或私有。",
            "实验权重建议 private，发布用 public。",
        ),
        field(
            "huggingface_token",
            "HF Token",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "HuggingFace API Token（写入 token 有读权限）",
            "访问私有仓库或需要写权限时使用（比 API Key 更受限的细粒度令牌）。",
            "仅上传/下载私有仓库时填写，切勿公开分享。",
        ),
        field(
            "in_json",
            "元数据 JSON",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据 JSON：为每个文件指定独立的标签文本",
            "使用一个 JSON 文件为每张图指定标签（而非同名的 .txt），适合批量替换标签而不想改文件名。",
            "批量改标签或标签在数据库/元数据中维护时才使用。",
        ),
        field(
            "initial_epoch",
            "初始 Epoch",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "从第 N 个 epoch 开始记录进度（续训用）",
            "配合恢复状态：指定训练从哪个 epoch 计数开始，主要用于日志与调度一致性。",
            "续训场景自动带出；从头训练保持 0。",
        ),
        field(
            "initial_step",
            "初始 Step",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "从第 N 步开始记录进度（续训用）",
            "与 initial_epoch 对应的步数坐标，续训时同步调度器进度。",
            "续训自动带出；从头训练保持 0。",
        ),
        field(
            "ip_noise_gamma",
            "IP 噪声 Gamma",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "IP-Adapter 噪声 Gamma 系数",
            "调整 IP-Adapter 注入噪声的强度分布，影响参考图风格强度与噪声结构。",
            "默认 0.1 附近；风格偏弱上调、偏噪下调。",
        ),
        field(
            "ip_noise_gamma_random_strength",
            "IP 噪声随机强度",
            "advanced",
            "noise",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "IP 噪声 Gamma 强度随机化",
            "随机化 IP-Adapter 噪声 gamma，增强训练鲁棒性。",
            "配合 ip_noise_gamma 使用，效果不佳时保持默认。",
        ),
        field(
            "keep_tokens_separator",
            "保留标签分隔符",
            "dataset",
            "",
                        "text",
            Value::String("".into()),
            &[],
            false,
            true,
            "保留 token 的边界分隔符（高级）",
            "用于标记哪些 token 在 shuffle 时必须保留在开头的高级分隔符语法。",
            "默认留空；常规训练不需要。",
        ),
        field(
            "log_config",
            "记录训练配置",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "把训练配置记录进日志（含超参）",
            "训练启动时把完整参数写入日志，便于事后核对实验设置。",
            "建议开启。",
        ),
        field(
            "log_prefix",
            "日志前缀",
            "logging",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "日志文件/记录的前缀",
            "给日志项加自定义前缀，用于区分多个并行运行。",
            "多任务并行时填写。",
        ),
        field(
            "log_tracker_config",
            "追踪器配置",
            "logging",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "行日志跟踪器配置（k=v 或 JSON 形式的额外配置）",
            "高级的 tracker 定制配置。",
            "一般保持空。",
        ),
        field(
            "log_tracker_name",
            "追踪器名称",
            "logging",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "日志跟踪器名称（wandb project 等）",
            "tracker 的自定义名（对应 wandb project 名）。",
            "用 wandb 时建议用体现实验方向的名。",
        ),
        field(
            "lowram",
            "低显存",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "模型权重尽量放在显存、减轻内存压力",
            "相反策略：优先把权重驻留显存，适合大显存小内存环境。",
            "与 highvram 二选一，依内存/显存比例决定。",
        ),
        field(
            "lr_decay_steps",
            "LR 衰减步数",
            "optimizer",
            "",
            "number",
            Value::from(0),
            &[],
            false,
            true,
            "int_or_float 衰减步数：整数步数或 <1 的比例",
            "调度器开始衰减前的步数或比例宽度，用于手动控制衰减区间。",
            "未使用对应调度器时可保持 0。",
        ),
        field(
            "lr_scheduler_args",
            "调度器参数",
            "optimizer",
            "",
                        "list",
            Value::Null,
            &[],
            false,
            true,
            "调度器的额外参数 key=value 列表",
            "调度器专属参数（如 polynomial 的 power、cosine 的 T_max 等）。",
            "使用 special 调度器时按文档填写。",
        ),
        field(
            "lr_scheduler_min_lr_ratio",
            "最小 LR 比率",
            "optimizer",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "min-lr 调度器的最低学习率占初始学习率的比例",
            "学习率衰减的下限比例（0-1），用于 cosine with min lr 与 warmup decay 调度器。",
            "通常 0.05-0.2；过低则尾段训练很慢。",
        ),
        field(
            "lr_scheduler_power",
            "LR 调度幂",
            "optimizer",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            true,
            "polynomial 调度器的幂次",
            "多项式衰减的指数，1 为线性衰减，2 为平方衰减（前期快后期慢）。",
            "polynomial 调度器常用 1-2。",
        ),
        field(
            "lr_scheduler_timescale",
            "LR 时间缩放",
            "optimizer",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "inverse sqrt 调度器的时间尺度",
            "反平方根调度器的基准步数；默认取预热步数，用于无需退火的长训练。",
            "默认即可；需要更快/更慢衰减时调整。",
        ),
        field(
            "lr_scheduler_type",
            "LR 调度类型",
            "optimizer",
            "",
                        "text",
            Value::String("".into()),
            &[],
            false,
            true,
            "调度器类型（文本兜底字段，一般用上面的下拉选择即可）",
            "功能与 lr_scheduler 下拉一致，供自由输入自定义调度器名。",
            "使用标准库内调度器时用下拉；自定义时再填此字段。",
        ),
        field(
            "masked_loss",
            "掩码损失",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "仅对 RGBA 图中不透明区域计算损失",
            "alpha 通道作蒙版：只有不透明像素参与损失，实现对局部区域的专注训练（如仅训练画面人脸）。",
            "仅当你准备了带透明通道（RGBA）的训练图时启用。",
        ),
        field(
            "max_data_loader_n_workers",
            "数据加载线程数",
            "dataset",
            "",
                        "number",
            Value::from(8),
            &[],
            false,
            true,
            "数据加载进程数",
            "DataLoader 使用的并行进程数量：加载图片、打乱、缩放的流水线并发数。太大占用内存，太小拖慢 epoch 起步。",
            "推荐 CPU 核心数的一半左右（默认 8）；机器内存 <16GB 时降到 2-4。",
        ),
        field(
            "max_grad_norm",
            "梯度裁剪",
            "training",
            "",
                        "number",
            Value::from(1.0),
            &[],
            false,
            true,
            "全局梯度裁剪范数上限",
            "把所有参数梯度的总 L2 范数约束在该值内，防止梯度爆炸导致的损失发散与权重大跳变。",
            "推荐 1.0；损失震荡严重时降到 0.5-0.8。",
        ),
        field(
            "max_timestep",
            "最大时间步",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "最大时间步（接近干净图的训练边界）",
            "限制时间步上限，跳过接近干净图的低噪声步，让模型更专注中高噪声区间（强风格化）。",
            "默认不填（1000）；需要更强画风注入时设 900 以内。",
        ),
        field(
            "max_token_length",
            "最大 Token 长度",
            "advanced",
            "text",
            "number",
            Value::Null,
            &["None", "150", "225"],
            false,
            true,
            "文本编码器的最大 token 长度（75 为基数）",
            "超过 75 token 的标签会被分块处理：150/225 分别支持 2/3 段。长标签时提升语义容量，但会慢一点。",
            "SDXL 默认 75-150；长描述标签（>75 token）用 150。",
        ),
        field(
            "max_validation_steps",
            "最大验证步数",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "验证时最多处理的数据条数",
            "限制验证集处理规模，验证集很大时可以只验证前 N 条以节省时间。",
            "默认跑完整验证集；每次验证 >3 分钟时可设 100-300。",
        ),
        field(
            "mem_eff_attn",
            "内存高效注意力",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "使用旧式 mem-efficient 注意力实现",
            "历史兼容实现，仅做旧环境回退用。",
            "优先 SDPA/xFormers；此选项只在异常环境回退。",
        ),
        field(
            "metadata_author",
            "元数据作者",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据中的作者名",
            "标记权重作者，规范发布时填写。",
            "发布模型时填写你的署名。",
        ),
        field(
            "metadata_description",
            "元数据描述",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据描述：模型内容与用途概述",
            "描述这个 LoRA 训练了什么概念、适用场景，写入 metadata。",
            "发布时写清触发词与适用范围。",
        ),
        field(
            "metadata_is_negative_embedding",
            "负嵌入元数据",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据：标记为负面 embedding",
            "标注该权重属于 negative embedding，工具链可区分处理。",
            "训练 negative embedding（如 TI）时标注。",
        ),
        field(
            "metadata_license",
            "元数据许可",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据许可证（如 CC-BY-4.0）",
            "声明权重的使用授权许可。",
            "发布公开权重时按意向填写；私有可留空。",
        ),
        field(
            "metadata_merged_from",
            "元数据合并来源",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据：合并来源模型信息",
            "若权重由其它模型合并而来，记录来源，保证可追溯。",
            "合并工作流中使用。",
        ),
        field(
            "metadata_preprocessor",
            "元数据预处理器",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据：训练数据的预处理器",
            "记录所用的标签生成/预处理工具，便于复现数据管线。",
            "由工具自动记录时保持；手写可注明工具名。",
        ),
        field(
            "metadata_tags",
            "元数据标签",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据标签列表（逗号分隔）",
            "便于发现与管理的标签集合，与 metadata 规范一致。",
            "选几个关键词即可。",
        ),
        field(
            "metadata_thumbnail",
            "元数据缩略图",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据缩略图路径",
            "为权重附加一张示例图的路径（HF 预览用）。",
            "发布时放样图路径。",
        ),
        field(
            "metadata_title",
            "元数据标题",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "写入权重的元数据标题",
            "与训练相关的命名与身份信息，行业惯例填写。",
            "任意可读名称；用于管理多版本模型。",
        ),
        field(
            "metadata_trigger_phrase",
            "元数据触发词",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据触发词",
            "明确该 LoRA 的触发词，方便使用者快速上手。",
            "写下训练的触发词，如 mystyle。",
        ),
        field(
            "metadata_usage_hint",
            "元数据使用提示",
            "saving",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "元数据使用提示（如推荐 weight）",
            "对使用者的建议，如推荐权重 0.6-0.8。",
            "按实际效果写推荐权重范围。",
        ),
        field(
            "min_snr_gamma",
            "Min-SNR Gamma",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "Min-SNR 损失加权系数（0 表示关闭）",
            "对低信噪比时间步（靠近噪图）的损失按 SNR 公式降权，显著提升收敛速度与稳定性；数值为 0 时不启用。",
            "推荐 5；效果明显且几乎无副作用，是目前最常用的训练加成。",
        ),
        field(
            "min_timestep",
            "最小时间步",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "最小时间步（接近噪声图的训练边界）",
            "限制训练的扩散时间步下限，跳过接近纯噪声的高噪声步，加快训练并影响风格强度。",
            "默认不填（1）；时间不够或噪声步占用过多时设 100-200。",
        ),
        field(
            "multires_noise_discount",
            "多分辨率噪声折扣",
            "advanced",
            "noise",
                        "number",
            Value::from(0.3),
            &[],
            false,
            true,
            "多分辨率噪声的衰减系数",
            "不同频率分量叠加时的衰减，0.0-1.0。",
            "推荐 0.3-0.8；数值越小高频细节保留越多。",
        ),
        field(
            "multires_noise_iterations",
            "多分辨率噪声迭代数",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "多分辨率噪声迭代次数",
            "融合多个频率尺度的噪声（多分辨率噪声），让模型学习跨尺度一致性，细节更和谐。",
            "推荐 4-8；0 为关闭。",
        ),
        field(
            "no_half_vae",
            "VAE 不转半精度",
            "performance",
            "memory",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "VAE 不转为半精度（防 VAE 黑图/伪影）",
            "VAE 以 fp32 运行，避免半精度下 VAE 解码常见的人脸崩坏、黑块问题。",
            "出图质量优先的小 batch 显存充足时开启。",
        ),
        field(
            "no_metadata",
            "不写入元数据",
            "saving",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "不向权重文件写入训练元数据",
            "禁止把训练参数（dataset、重复数等）写入 safetensors metadata，便于调试与对比。",
            "需要可复现的元数据字段时关闭；默认写入更利于追踪。",
        ),
        field(
            "noise_offset",
            "噪声偏移",
            "advanced",
            "noise",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "噪声偏移量：让模型更‘耐造’，亮暗分布更极端的出图",
            "在目标噪声中加入固定偏移，鼓励模型忽略全局亮度细节，出图对比度更强、暗部更深。数值越大效果越强。",
            "推荐 0.05-0.2；追求低光风格可到 0.3。",
        ),
        field(
            "noise_offset_random_strength",
            "噪声偏移随机强度",
            "advanced",
            "noise",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "噪声偏移强度随机化",
            "每次迭代在 0 到设定值之间随机取噪声偏移强度，缓解固定偏移带来的色偏积累。",
            "噪声偏移数值较大时推荐开启。",
        ),
        field(
            "offload_optimizer_device",
            "优化器卸载设备",
            "optimizer",
            "",
                        "select",
            Value::Null,
            &["None", "cpu", "nvme"],
            false,
            true,
            "优化器状态卸载设备：cpu / nvme",
            "把优化器状态（如 Adam 一阶二阶矩）挪到 CPU 或 NVMe，极端低显存下的 DeepSpeed 功能。",
            "仅显存严重不足时使用；需要配套 offload 参数与 NVMe 路径。",
        ),
        field(
            "offload_optimizer_nvme_path",
            "优化器 NVMe 卸载路径",
            "optimizer",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "优化器卸载到 NVMe 的设备路径",
            "与上一条配合：优化器状态写入的 NVMe 盘符（Windows 下如 E:/）。",
            "offload_optimizer_device=nvme 时必填；pcidevice 冒号不可用作输入。",
        ),
        field(
            "offload_param_device",
            "参数卸载设备",
            "performance",
            "memory",
                        "select",
            Value::Null,
            &["None", "cpu", "nvme"],
            false,
            true,
            "参数卸载设备：cpu / nvme",
            "把模型参数整体卸载到 CPU 或 NVMe，DeepSpeed/ZeRO 的极端省显存方案。",
            "显存极度不足时配合 zero_stage 使用。",
        ),
        field(
            "offload_param_nvme_path",
            "参数 NVMe 卸载路径",
            "performance",
            "memory",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "参数卸载到 NVMe 的路径",
            "配合 offload_param_device=nvme 填写 NVMe 盘符路径。",
            "仅 NVMe 卸载场景填写。",
        ),
        field(
            "persistent_data_loader_workers",
            "持久化数据加载器",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "数据加载进程常驻，减少 epoch 间重建开销",
            "训练期间保持 DataLoader 常驻进程，避免每个 epoch 重建进程造成的延迟；代价是常驻内存占用。",
            "内存充足（32GB+）时建议开启；OOM 时关闭。",
        ),
        field(
            "prior_loss_weight",
            "先验损失权重",
            "training",
            "",
                        "number",
            Value::from(1.0),
            &[],
            false,
            true,
            "正则化（先验）样本的损失权重",
            "控制正则化图片损失在总损失中的占比。值越小正则越弱、越容易保留底模原能力；越大越强调正则集。",
            "推荐 0.5-1.0；数值越高越不容易灾变但收敛越慢。",
        ),
        field(
            "random_crop",
            "随机裁剪",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "随机裁剪而非居中裁剪",
            "缩放到桶分辨率后随机位置裁剪，让模型看到不同构图局部，减少同一构图的过拟合。",
            "构图种类少、画面主体不在中心时推荐。",
        ),
        field(
            "resize_interpolation",
            "缩放插值",
            "dataset",
            "",
                        "select",
            Value::Null,
            &["lanczos", "nearest", "bilinear", "linear", "bicubic", "cubic", "area"],
            false,
            true,
            "缩放时的插值算法",
            "图片缩放到桶分辨率时使用的插值方式：lanczos 更锐利，bicubic 适中，nearest/linear 更平滑快速。",
            "默认 lanczos；低分辨率旧图放大时可试 bicubic。",
        ),
        field(
            "resume_from_huggingface",
            "从 HF 恢复",
            "saving",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "从 HuggingFace 仓库的权重上传位置恢复训练状态",
            "训练前从 HF 下载最近的 training state 以续训（需同 config）。",
            "分布式/换机续训场景使用。",
        ),
        field(
            "sample_at_first",
            "首步采样",
            "sampling",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "训练开始前先生成一组初始样图",
            "在未训练的初始权重上先出样图，作为对比基线，方便观察训练带来的变化幅度。",
            "推荐开启；零成本获得训练前基准。",
        ),
        field(
            "sample_every_n_steps",
            "每 N 步采样",
            "sampling",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "每 N 步生成一次样图（与上面的周期二选一）",
            "按步数频率采样，适合 epoch 很长的任务。",
            "每 500-1000 步一次，避免频繁采样拖慢训练。",
        ),
        field(
            "save_every_n_steps",
            "每 N 步保存",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "每 N 步保存一次中间权重（与上面的周期二选一）",
            "按步数频率保存中间权重。",
            "每 500-2000 步一次；过频浪费磁盘与时间。",
        ),
        field(
            "save_last_n_epochs",
            "保留最近 Epoch 数",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "只保留最后 N 个 epoch 的检查点，自动清理旧的",
            "限制中间检查点保留数量，磁盘省心：保留最近 N 份。",
            "训练较长时推荐 1-3；想回溯早期状态则调大。",
        ),
        field(
            "save_last_n_epochs_state",
            "保留最近状态 Epoch 数",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "训练状态保留数量（按 epoch），单独覆盖上面保留策略",
            "与 save_last_n_epochs 等价的 state 版本，只影响保存训练状态时的保留数。",
            "与是否保存状态配套使用。",
        ),
        field(
            "save_last_n_steps",
            "保留最近步数",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "按步数保留最后 N 个检查点",
            "配合 save_every_n_steps 的保留上限。",
            "通常 1-5。",
        ),
        field(
            "save_last_n_steps_state",
            "保留最近状态步数",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "训练状态按步数保留数量",
            "state 文件夹的按步数保留上限。",
            "通常 1-3。",
        ),
        field(
            "save_n_epoch_ratio",
            "Epoch 保存比例",
            "saving",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "按 epoch 比例保存：保证全程保留 N 份",
            "不是固定间隔而是总共保留 N 份、间隔随总 epoch 自适应。",
            "希望训练全程都有均匀采样点（如 5）时使用。",
        ),
        field(
            "save_state_on_train_end",
            "结束时保存状态",
            "saving",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "训练结束时也保存一次训练状态",
            "收尾时把最终状态落盘，便于以后续训改进。",
            "计划后续接着练时开启。",
        ),
        field(
            "save_state_to_huggingface",
            "状态上传 HF",
            "saving",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "把训练状态同步上传到 HuggingFace",
            "训练中把 state 目录推送到 HF 仓库。",
            "使用 HF 仓库做异地备份时开启。",
        ),
        field(
            "scale_v_pred_loss_like_noise_pred",
            "缩放 v-pred 损失",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "将 v-prediction 损失按噪声模型同等量级缩放",
            "v-prediction 任务损失天然比噪声预测小一个量级，开启后按噪声预测的尺度归一化，学习率行为更一致。",
            "使用 v-prediction 底模（v_parameterization）时建议开启。",
        ),
        field(
            "scale_weight_norms",
            "权重范数缩放",
            "optimizer",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "按权重范数自适应缩放学习率阈值",
            "配合 Prodigy/D-Adapt 类优化器使用：参数范数超过该值后在更新时按比例缩放，稳定自动学习率。",
            "使用 Prodigy 且出现发散时，从 1.0 开始下调。",
        ),
        field(
            "secondary_separator",
            "次级分隔符",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "第二层分隔符，用于与既定分隔符一起构成层级标签结构",
            "高级打标技巧：当标签串同时包含两层语义时，用不同分隔符让模型学会区分层级（如外观与姿态）。",
            "仅在研究性/高级打标任务中使用；常规训练保持默认。",
        ),
        field(
            "skip_cache_check",
            "跳过缓存检查",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "跳过缓存有效性校验，直接信任已有缓存",
            "启动时不再逐文件核对缓存是否过期，启动更快；若数据目录被外部修改，输出可能使用陈旧缓存。",
            "仅在确认数据未变动、希望加速启动时开启。",
        ),
        field(
            "skip_image_resolution",
            "跳过图像分辨率",
            "dataset",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "跳过指定裁剪分辨率的图片（逗号分隔）",
            "训练时忽略与给定分辨率相同的图片（如跳过原始已是 SDXL 分辨率的图，避免重复计算）。",
            "专业用途；常规训练保持默认。",
        ),
        field(
            "skip_until_initial_step",
            "跳过至初始步",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "跳过 initial_step 之前的实际迭代",
            "开启后真正跳过前面若干步、而不是只改计数，用于验收时快速跑固定段。",
            "正常训练关闭；做 staged 长实验时配合 initial_step。",
        ),
        field(
            "token_warmup_min",
            "Token 预热下限",
            "training",
            "",
                        "number",
            Value::from(1),
            &[],
            false,
            true,
            "Token 预热的最小 token 数",
            "与 token_warmup_step 一起，让标签 token 数量从 min 线性增长到完整数量，渐进式引入文本信息，对多标签数据更稳。",
            "多标签（>20 tag）且文本与画面关联强时：min 设 1-5。",
        ),
        field(
            "token_warmup_step",
            "Token 预热步数",
            "training",
            "",
                        "number",
            Value::from(0),
            &[],
            false,
            true,
            "Token 预热的总步数（0 关闭）",
            "在指定步数内完成 token 数量从 min 到全量的线性爬升。0 表示不启用预热。",
            "推荐 500-2000 步；训练总量小时保持 0。",
        ),
        field(
            "tokenizer_cache_dir",
            "Tokenizer 缓存目录",
            "advanced",
            "text",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "Tokenizer 词表缓存目录",
            "把 CLIP/SDXL tokenizer 的本地缓存目录指到指定位置，避免反复下载或特定权限问题。",
            "换环境后 tokenizer 报下载/权限错误时填写。",
        ),
        field(
            "torch_compile",
            "Torch Compile",
            "performance",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "用 torch.compile 编译模型加速",
            "PyTorch 2 图编译可提升部分环境吞吐，首次编译耗时长、对显卡模型支持不一。",
            "性能敏感且你的显卡/驱动支持时开启；稳定性优先则关闭。",
        ),
        field(
            "train_inpainting",
            "修复训练",
            "training",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "按 inpainting 布局训练（输出与输入通道结构匹配）",
            "以 inpainting 模型结构进行训练，需要配套的在 inpainting 底模与合适的掩码数据。普通文生图 LoRA 不需要。",
            "仅当你以 inpainting 底模（如 sdxl-inpaint ckpt）训练时开启。",
        ),
        field(
            "training_comment",
            "训练备注",
            "advanced",
            "misc",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "训练备注（写入日志与元数据）",
            "为本次训练附加一句话说明，便于日后区分与审计。",
            "写清目标概念、数据集版本等可复现信息。",
        ),
        field(
            "use_8bit_adam",
            "8bit Adam",
            "optimizer",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "使用 8bit AdamW，节省优化器显存",
            "将 AdamW 状态量压缩为 8bit，减少约一半优化器内存。已基本被 AdamW8bit 优化器取代，兼容旧流程。",
            "一般直接用 optimizer_type=AdamW8bit 更简洁；此开关为旧式等价项。",
        ),
        field(
            "use_lion_optimizer",
            "Lion 优化器",
            "optimizer",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "使用 Lion 优化器（开发中特性）",
            "Lion 是另一类符号更新算法，官方对其稳定性还在迭代。",
            "需要稳定复现时建议优先 AdamW 系；追求尝试 Lion 时保持默认学习率 1e-4 起步。",
        ),
        field(
            "v2",
            "SD 2.x 模型",
            "model",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "SD 2.x / 2.1 系列底模的兼容开关",
            "使用 SD 2.x 系列底模（包括 SD 2.0、2.1 及配套 v-pred 变体）时启用。SDXL 训练不需要开启，开启后文本编码器结构与投影层处理会按 2.x 规范调整。",
            "仅当底模是 Stable Diffusion 2.x 时开启；训练 SDXL 一律保持关闭。",
        ),
        field(
            "v_parameterization",
            "v 参数化",
            "model",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "v-prediction 底模（如 SD 2.0-v）的兼容开关",
            "底模使用 v-prediction 目标函数（权重文件名常含 -v）时开启，训练目标由噪声预测切换为速度预测，学习率与损失表现会有所不同。",
            "仅当你明确知道底模是 v-prediction 变体（如 v2-1-v）时开启；普通 SDXL 底模保持关闭。",
        ),
        field(
            "v_pred_like_loss",
            "v-pred 损失",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "以 v-prediction 方式计算的损失加权（0-1 区间混合）",
            "在候选损失与噪声预测损失之间按权重混合出 v 型损失，是对非 v-pred 底模的近似技巧。",
            "保留默认；仅在实验 v-型损失特性时调整。",
        ),
        field(
            "vae_batch_size",
            "VAE 批大小",
            "performance",
            "memory",
                        "number",
            Value::from(1),
            &[],
            false,
            true,
            "VAE 编码/解码的批大小",
            "图片进出 VAE 时一次处理的张数。调节该值可优化 VAE 吞吐与显存峰值，通常无需修改。",
            "默认即可；显存充足时可适当调大提升 VAE 缓存速度。",
        ),
        field(
            "validate_every_n_epochs",
            "每 N Epoch 验证",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "每 N 个 epoch 运行一次验证",
            "周期性地在验证集上计算损失指标，监控泛化与过拟合趋势。",
            "推荐 1-2；数据集小时每个 epoch 都做也很快。",
        ),
        field(
            "validate_every_n_steps",
            "每 N 步验证",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "每 N 步进行一次验证（与上面的周期二选一）",
            "以步数为周期的验证频率，适合 epoch 很长或想更细粒度观察的场景。",
            "每 500-1000 步一次；过频会拖慢训练。",
        ),
        field(
            "validation_seed",
            "验证种子",
            "training",
            "",
                        "number",
            Value::Null,
            &[],
            false,
            true,
            "验证集的固定打乱种子",
            "让验证数据的顺序/子集在每次验证间保持一致，保证指标可对比。",
            "固定一个任意值（如 42）即可。",
        ),
        field(
            "validation_split",
            "验证集比例",
            "training",
            "",
                        "number",
            Value::from(0.0),
            &[],
            false,
            true,
            "从训练集划出验证集的比例（0-1）",
            "按比例从训练集中随机划出一部分作为验证集，无需单独准备验证数据。0 表示不划分。",
            "推荐 0.01-0.05；数据集很小（<100）时可设 0。",
        ),
        field(
            "wandb_run_name",
            "W&B 运行名称",
            "logging",
            "",
                        "text",
            Value::Null,
            &[],
            false,
            true,
            "W&B 本次运行的名称",
            "标记本次训练在 W&B 上的名称，便于区分实验。",
            "建议带日期/版本语义，便于排序。",
        ),
        field(
            "weighted_captions",
            "加权 Caption",
            "dataset",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "支持带权重语法的 Caption（a:b、(b)）",
            "让标签串支持 (tag:1.3)、[tag]、{tag} 等加权语法，模型按权重理解标签强弱。",
            "使用加权打标工具生成的标签时开启；否则关闭。",
        ),
        field(
            "zero3_init_flag",
            "ZeRO-3 初始化",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "ZeRO-3 参数初始化标志",
            "应对 ZeRO-3 初始化时 CPU/GPU 内存的建图问题，必要时开启。",
            "ZeRO-3 + 内存压力大时开启。",
        ),
        field(
            "zero3_save_16bit_model",
            "ZeRO-3 保存 16bit",
            "logging",
            "",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "ZeRO-3 下保存 16bit 模型",
            "分片环境中把权重合并且保存为 16bit 版本。",
            "ZeRO-3 多进程训练时通常必开。",
        ),
        field(
            "zero_stage",
            "ZeRO 级别",
            "logging",
            "",
            "number",
            Value::from(2),
            &["0", "1", "2", "3"],
            false,
            true,
            "ZeRO 优化级别：0/1/2/3",
            "DeepSpeed 的显存优化等级：0 无；1 优化器分片；2 加梯度分片；3 再分片参数（最省但最慢）。",
            "LoRA 显存尚可时 0-1；大模型训练用 2-3。",
        ),
        field(
            "zero_terminal_snr",
            "Zero 末端 SNR",
            "advanced",
            "noise",
                        "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "强制终末端 SNR 为 0 的噪声调度修正",
            "重新计算 noise schedulers 的 beta，使纯噪声步的 SNR 归零，更符合理论最优；通常让出图更亮、色彩更饱和。",
            "推荐开启（SDXL 兼容性良好）；overtrain 亮度过高可关闭。",
        ),
        field(
            "advanced_parameters",
            "原始高级参数",
            "advanced",
            "misc",
                        "json",
            Value::Object(serde_json::Map::new()),
            &[],
            false,
            true,
            "JSON 补充字段：添加未被收录的上游参数",
            "以 key-value 形式补齐工具界面未覆盖的上游 CLI/TOML 参数；不能覆盖任何已声明字段。",
            "仅在工具缺少你需要的上游参数时使用，key 需与 kohya 参数名一致。",
        ),

    ];
    KOHYA_V26_ADAPTERS
        .iter()
        .copied()
        .map(|definition| TrainingAdapter {
            id: definition.id,
            version: "kohya-ss-v26.0.0",
            label: match definition.id {
                "sdxl-lora" => "SDXL LoRA".into(),
                _ => format!(
                    "{} · {}",
                    definition.family_label, definition.training_type_label
                )
                .into(),
            },
            family: definition.family,
            family_label: definition.family_label,
            training_type: definition.training_type,
            training_type_label: definition.training_type_label,
            trainer: definition.trainer,
            fields: fields_for_kohya_adapter(&fields, definition),
            groups: groups.clone(),
        })
        .collect()
}

pub fn adapter_by_id(id: &str) -> Option<TrainingAdapter> {
    builtin_adapters()
        .into_iter()
        .find(|adapter| adapter.id == id)
}

/// A normalized argparse action exported by the bundled Python inspector.
/// The desktop keeps curated controls stable and places unrecognized upstream
/// options in Advanced, so a newer lora-scripts flag is never silently lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamParserField {
    pub key: String,
    #[serde(default = "upstream_default_value")]
    pub default: Value,
    #[serde(default)]
    pub choices: Vec<String>,
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub help: String,
}

fn upstream_default_value() -> Value {
    Value::Null
}

pub fn augment_adapter_with_upstream_fields(
    mut adapter: TrainingAdapter,
    upstream_fields: Vec<UpstreamParserField>,
) -> TrainingAdapter {
    let mut known = adapter
        .fields
        .iter()
        .map(|field| field.key.to_string())
        .collect::<HashSet<_>>();
    for upstream in upstream_fields {
        if !toml_key_is_safe(&upstream.key) || !known.insert(upstream.key.clone()) {
            continue;
        }
        let kind = match upstream.kind.as_str() {
            "boolean" | "number" | "list" | "json" => upstream.kind,
            "select" if !upstream.choices.is_empty() => "select".to_string(),
            _ => "text".to_string(),
        };
        adapter.fields.push(TrainingField {
            key: upstream.key.clone().into(),
            label: format!("--{}", upstream.key).into(),
            group: "advanced".into(),
            subgroup: "upstream".into(),
            kind: kind.into(),
            default: upstream.default,
            choices: upstream.choices.into_iter().map(Into::into).collect(),
            required: upstream.required,
            advanced: true,
            help: if upstream.help.trim().is_empty() {
                "由当前 lora-scripts parser 自动导出".into()
            } else {
                upstream.help.into()
            },
            description: "由当前 lora-scripts parser 自动导出的上游参数。含义与取值范围取决于所连接的 lora-scripts 版本，建议查阅其 `--help` 或官方文档后谨慎使用。".into(),
            when_to_adjust: "除非你清楚该参数在上游脚本中的作用，否则保持默认值不动。".into(),
        });
    }
    adapter
}

fn toml_key_is_safe(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_secret_parameter_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("api_key")
        || key.contains("access_token")
        || key.contains("auth_token")
        || key.contains("password")
}

fn toml_string(value: &str) -> String {
    // Windows `std::fs::canonicalize` returns `\\?\`-prefixed verbatim paths.
    // They are fine for Rust filesystem calls, but kohya's Python glue
    // concatenates strings with "/" (accelerator_setup.py builds the
    // TensorBoard directory as `logging_dir + "/" + timestamp`); inside a
    // verbatim path "/" is a literal character, so `os.makedirs` fails with
    // WinError 123.  Config values must therefore carry the plain form.
    let value = value.strip_prefix(r"\\?\").unwrap_or(value);
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn toml_value(value: &Value) -> Result<String, String> {
    match value {
        Value::Null => Err("null 值不能写入训练 TOML".to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(toml_string(value)),
        Value::Array(values) => values
            .iter()
            .map(toml_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|values| format!("[{}]", values.join(", "))),
        Value::Object(_) => Err("训练参数只能是标量或数组".to_string()),
    }
}

fn normalize_toml_field_value(field: &TrainingField, value: &Value) -> Result<Value, String> {
    if field.kind != "number" || value.is_number() || value.is_null() {
        return Ok(value.clone());
    }
    let text = value
        .as_str()
        .ok_or_else(|| format!("{} 必须是数值", field.label))?
        .trim();
    let number = text
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .and_then(serde_json::Number::from_f64)
        .ok_or_else(|| format!("{} 必须是有效数值", field.label))?;
    Ok(Value::Number(number))
}

/// Kohya's sd-scripts validates these top-level keys as Python floats.  The
/// browser form collapses JSON `0.0` to `0`, and `tomllib` parses `0` back as
/// an `int`, which voluptuous rejects for float-typed fields
/// (`expected float for object value @ data['validation_split']`).  Emit an
/// explicit fractional form so integer inputs survive validation.
fn is_float_only_parameter_key(key: &str) -> bool {
    // Mirrors `DATASET_ASCENDABLE_SCHEMA` in kohya's ConfigSanitizer.
    matches!(key, "validation_split" | "network_multiplier")
}

/// Kohya's argparse declares these as `type=int`.  A fractional value would be
/// silently truncated by argparse, which hides configuration mistakes.
fn is_strict_integer_parameter_key(key: &str) -> bool {
    matches!(
        key,
        "bucket_reso_steps" | "caption_dropout_every_n_epochs" | "clip_skip"
            | "dataset_repeats" | "ddp_timeout" | "gradient_accumulation_steps"
            | "initial_epoch" | "initial_step" | "keep_tokens"
            | "lr_scheduler_num_cycles" | "lr_scheduler_timescale"
            | "max_bucket_reso" | "max_data_loader_n_workers" | "max_timestep"
            | "max_token_length" | "max_train_epochs" | "max_train_steps"
            | "max_validation_steps" | "min_bucket_reso" | "min_timestep"
            | "multires_noise_iterations" | "network_dim"
            | "sample_every_n_epochs" | "sample_every_n_steps"
            | "save_every_n_epochs" | "save_every_n_steps" | "save_last_n_epochs"
            | "save_last_n_epochs_state" | "save_last_n_steps"
            | "save_last_n_steps_state" | "save_n_epoch_ratio" | "seed"
            | "token_warmup_min" | "train_batch_size" | "vae_batch_size"
            | "validate_every_n_epochs" | "validate_every_n_steps"
            | "validation_seed" | "zero_stage"
    )
}

fn validate_integer_key(key: &str, label: &str, value: &Value) -> Result<(), String> {
    if is_strict_integer_parameter_key(key)
        && value
            .as_f64()
            .is_some_and(|number| number.fract() != 0.0)
    {
        return Err(format!("{label} 必须是整数"));
    }
    Ok(())
}

fn float_fractional_form(value: &Value) -> Option<String> {
    value.as_i64().map(|integer| format!("{integer}.0"))
}

/// Serializes only declared, non-empty values. `advanced_parameters` is a
/// deliberate escape hatch for new upstream flags, but it cannot override a
/// declared user field silently.
pub fn serialize_toml(adapter: &TrainingAdapter, values: &Value) -> Result<String, String> {
    let values = values
        .as_object()
        .ok_or_else(|| "训练参数必须是对象".to_string())?;
    let field_keys = adapter
        .fields
        .iter()
        .map(|field| field.key.as_ref())
        .collect::<HashSet<_>>();
    // kohya's SDXL trainer intentionally derives `max_train_steps` from
    // `max_train_epochs` whenever both are present.  The form has a useful
    // epoch default, but an explicit step limit must remain authoritative for
    // smoke runs and exact-length experiments.
    let has_explicit_step_limit = values
        .get("max_train_steps")
        .and_then(Value::as_f64)
        .is_some_and(|steps| steps > 0.0);
    let has_sample_prompts = values
        .get("sample_prompts")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty());
    let mut encoded = BTreeMap::new();
    for field in &adapter.fields {
        // Secrets are process-only launch inputs.  A training TOML is meant to
        // be shareable and reproducible, so it must never contain credentials.
        if field.kind == "secret" || field.key == "advanced_parameters" {
            continue;
        }
        if field.key == "max_train_epochs" && has_explicit_step_limit {
            continue;
        }
        if matches!(
            field.key.as_ref(),
            "sample_every_n_epochs" | "sample_every_n_steps"
        ) && !has_sample_prompts
        {
            continue;
        }
        let value = normalize_toml_field_value(
            field,
            values.get(field.key.as_ref()).unwrap_or(&field.default),
        )?;
        if field.key == "lokr_factor" {
            if let Some(factor) = value.as_i64().filter(|factor| *factor > 0) {
                let mut network_args = values
                    .get("network_args")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                network_args.retain(|value| {
                    !value
                        .as_str()
                        .is_some_and(|argument| argument.starts_with("factor="))
                });
                network_args.push(Value::String(format!("factor={factor}")));
                encoded.insert(
                    "network_args".to_string(),
                    toml_value(&Value::Array(network_args))?,
                );
            }
            continue;
        }
        if value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty()) {
            if field.required {
                return Err(format!("{} 不能为空", field.label));
            }
            continue;
        }
        if !field.choices.is_empty()
            && !value
                .as_str()
                .is_some_and(|value| field.choices.iter().any(|choice| choice == value))
            && !(value.is_number()
                && field
                    .choices
                    .iter()
                    .any(|choice| choice.as_ref() == value.to_string()))
        {
            return Err(format!("{} 包含不支持的值", field.label));
        }
        let serialized = if is_float_only_parameter_key(field.key.as_ref()) {
            float_fractional_form(&value).unwrap_or(toml_value(&value)?)
        } else {
            validate_integer_key(field.key.as_ref(), field.label.as_ref(), &value)?;
            toml_value(&value)?
        };
        encoded.insert(field.key.to_string(), serialized);
    }
    // The form can be refreshed from an upstream parser while a queued task is
    // being submitted.  Preserve safe, typed parser fields even if that
    // request reaches a process that still has the baseline adapter cache.
    for (key, value) in values {
        if field_keys.contains(key.as_str())
            || key == "advanced_parameters"
            || value.is_null()
            || value.as_str().is_some_and(|value| value.trim().is_empty())
        {
            continue;
        }
        if matches!(
            key.as_str(),
            "sample_every_n_epochs" | "sample_every_n_steps"
        ) && !has_sample_prompts
        {
            continue;
        }
        if !toml_key_is_safe(key) {
            return Err(format!("训练参数键无效: {key}"));
        }
        if is_secret_parameter_key(key) {
            continue;
        }
        let serialized = if is_float_only_parameter_key(key) {
            float_fractional_form(value).unwrap_or(toml_value(value)?)
        } else {
            validate_integer_key(key, key, value)?;
            toml_value(value)?
        };
        encoded.insert(key.clone(), serialized);
    }
    if let Some(advanced) = values.get("advanced_parameters") {
        let advanced = advanced
            .as_object()
            .ok_or_else(|| "advanced_parameters 必须是对象".to_string())?;
        for (key, value) in advanced {
            if !toml_key_is_safe(key) {
                return Err(format!("高级参数键无效: {key}"));
            }
            if field_keys.contains(key.as_str()) {
                return Err(format!("高级参数不能覆盖表单字段: {key}"));
            }
            if encoded.contains_key(key) {
                return Err(format!("高级参数不能覆盖已导出的上游参数: {key}"));
            }
            if !value.is_null() && !value.as_str().is_some_and(|value| value.trim().is_empty()) {
                if is_secret_parameter_key(key) {
                    continue;
                }
                let serialized = if is_float_only_parameter_key(key) {
                    float_fractional_form(value).unwrap_or(toml_value(value)?)
                } else {
                    validate_integer_key(key, key, value)?;
                    toml_value(value)?
                };
                encoded.insert(key.clone(), serialized);
            }
        }
    }
    Ok(encoded
        .into_iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingGalleryDataset {
    pub root_id: String,
    #[serde(default)]
    pub relative_directory: String,
    pub repeats: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption_extension: Option<String>,
}

impl TrainingGalleryDataset {
    pub fn validate(&self) -> Result<(), String> {
        if self.root_id.trim().is_empty() || self.root_id.len() > 128 {
            return Err("图库根标识无效".to_string());
        }
        if self.repeats == 0 || self.repeats > 10_000 {
            return Err("repeat 必须介于 1 和 10000 之间".to_string());
        }
        let directory = self.relative_directory.replace('\\', "/");
        if directory.starts_with('/')
            || directory.contains(':')
            || directory.split('/').any(|part| matches!(part, "." | ".."))
        {
            return Err("图库目录必须是媒体根内的相对路径".to_string());
        }
        if let Some(extension) = self.caption_extension.as_deref() {
            if extension.is_empty()
                || extension.len() > 32
                || !extension.starts_with('.')
                || extension.contains(['/', '\\'])
            {
                return Err("caption 扩展名无效".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrainingSamplePromptSource {
    Manual,
    DatasetCaptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSampleSettings {
    pub enabled: bool,
    pub prompt_source: TrainingSamplePromptSource,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: String,
    #[serde(default = "default_dataset_caption_count")]
    pub dataset_caption_count: u32,
    #[serde(default = "default_sample_steps")]
    pub steps: u32,
    #[serde(default = "default_sample_width")]
    pub width: u32,
    #[serde(default = "default_sample_height")]
    pub height: u32,
    #[serde(default = "default_sample_every_n_epochs")]
    pub every_n_epochs: u32,
}

fn default_dataset_caption_count() -> u32 {
    4
}

fn default_sample_steps() -> u32 {
    30
}

fn default_sample_width() -> u32 {
    1024
}

fn default_sample_height() -> u32 {
    1024
}

fn default_sample_every_n_epochs() -> u32 {
    1
}

impl Default for TrainingSampleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            prompt_source: TrainingSamplePromptSource::Manual,
            prompt: String::new(),
            negative_prompt: String::new(),
            dataset_caption_count: default_dataset_caption_count(),
            steps: default_sample_steps(),
            width: default_sample_width(),
            height: default_sample_height(),
            every_n_epochs: default_sample_every_n_epochs(),
        }
    }
}

impl TrainingSampleSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.steps == 0 || self.steps > 1_000 {
            return Err("样图采样步数必须在 1 到 1000 之间".to_string());
        }
        for (label, size) in [("样图宽度", self.width), ("样图高度", self.height)] {
            if !(64..=4096).contains(&size) || size % 8 != 0 {
                return Err(format!("{label} 必须在 64 到 4096 之间且为 8 的倍数"));
            }
        }
        if self.every_n_epochs == 0 || self.every_n_epochs > 100_000 {
            return Err("样图 Epoch 间隔必须在 1 到 100000 之间".to_string());
        }
        if self.negative_prompt.len() > 16_000 {
            return Err("样图负面 Prompt 不能超过 16000 个字符".to_string());
        }
        match self.prompt_source {
            TrainingSamplePromptSource::Manual => {
                if self.prompt.trim().is_empty() {
                    return Err(
                        "启用样图后请填写正面 Prompt，或选择从数据集抽取 Caption".to_string()
                    );
                }
                if self.prompt.len() > 32_000 {
                    return Err("样图 Prompt 不能超过 32000 个字符".to_string());
                }
            }
            TrainingSamplePromptSource::DatasetCaptions => {
                if !(1..=32).contains(&self.dataset_caption_count) {
                    return Err("样图 Caption 抽取数量必须在 1 到 32 之间".to_string());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingRequest {
    pub adapter_id: String,
    pub runtime_profile_id: String,
    pub parameters: Value,
    #[serde(default)]
    pub gpu_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gallery_dataset: Option<TrainingGalleryDataset>,
    /// Multiple gallery folders are written as independent kohya subsets, so
    /// originals and each retagged augmentation family can have their own
    /// repeat count without copying either directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gallery_datasets: Vec<TrainingGalleryDataset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<TrainingSampleSettings>,
}

impl TrainingRequest {
    pub fn gallery_datasets(&self) -> Vec<&TrainingGalleryDataset> {
        if self.gallery_datasets.is_empty() {
            self.gallery_dataset.iter().collect()
        } else {
            self.gallery_datasets.iter().collect()
        }
    }

    pub fn validate(&self) -> Result<TrainingAdapter, String> {
        let adapter =
            adapter_by_id(&self.adapter_id).ok_or_else(|| "不支持的训练模型适配器".to_string())?;
        if !matches!(self.runtime_profile_id.as_str(), "windows" | "wsl")
            && !self.runtime_profile_id.starts_with("conda:")
            && !self.runtime_profile_id.starts_with("venv:")
        {
            return Err(
                "必须选择受支持的 Windows、WSL、Conda 或 Python venv 训练配置档".to_string(),
            );
        }
        if self.gallery_dataset.is_some() && !self.gallery_datasets.is_empty() {
            return Err("请使用 gallery_datasets；不能同时传入旧版 gallery_dataset".to_string());
        }
        for dataset in self.gallery_datasets() {
            dataset.validate()?;
        }
        if let Some(sample) = self.sample.as_ref() {
            sample.validate()?;
        }
        let mut validation_parameters = self.parameters.clone();
        if !self.gallery_datasets().is_empty()
            && validation_parameters
                .get("train_data_dir")
                .and_then(Value::as_str)
                .is_none_or(|path| path.trim().is_empty())
        {
            if let Some(parameters) = validation_parameters.as_object_mut() {
                parameters.insert(
                    "train_data_dir".to_string(),
                    Value::String("__gallery_dataset__".to_string()),
                );
            }
        }
        if adapter.family == "sdxl" {
            let bucket_reso_steps = match validation_parameters.get("bucket_reso_steps") {
                None | Some(Value::Null) => 32,
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| "SDXL 的 Bucket 步长必须是整数".to_string())?,
            };
            if bucket_reso_steps < 32 || bucket_reso_steps % 32 != 0 {
                return Err("SDXL 的 Bucket 步长必须为不小于 32 的 32 的倍数".to_string());
            }
        }
        let _ = serialize_toml(&adapter, &validation_parameters)?;
        let gpu_ids = self.gpu_ids.iter().collect::<BTreeSet<_>>();
        if gpu_ids.len() != self.gpu_ids.len() || self.gpu_ids.iter().any(|id| id.trim().is_empty())
        {
            return Err("GPU 列表包含重复或空标识".to_string());
        }
        Ok(adapter)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingMetric {
    pub step: u64,
    pub timestamp: u64,
    pub series: String,
    pub value: f64,
}

/// Parses the deliberately simple JSONL emitted by the bundled reporter.
pub fn parse_metric_line(line: &str) -> Result<Vec<TrainingMetric>, String> {
    #[derive(Deserialize)]
    struct Line {
        step: u64,
        timestamp: u64,
        metrics: BTreeMap<String, f64>,
    }
    let line: Line =
        serde_json::from_str(line).map_err(|error| format!("指标 JSON 无效: {error}"))?;
    Ok(line
        .metrics
        .into_iter()
        .map(|(series, value)| TrainingMetric {
            step: line.step,
            timestamp: line.timestamp,
            series,
            value,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct GpuLeaseManager {
    state: Arc<Mutex<GpuLeaseState>>,
    changes: watch::Sender<u64>,
}

impl Default for GpuLeaseManager {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            state: Arc::new(Mutex::new(GpuLeaseState::default())),
            changes,
        }
    }
}

#[derive(Debug, Default)]
struct GpuLeaseState {
    held: HashMap<String, String>,
    waiting: HashMap<String, GpuLeaseWait>,
    next_ticket: u64,
}

#[derive(Debug, Clone)]
struct GpuLeaseWait {
    ticket: u64,
    gpu_keys: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrainingGpuQueueEntry {
    pub task_id: String,
    pub gpu_ids: Vec<String>,
    pub queue_position: u64,
    pub blocker_task_ids: Vec<String>,
}

impl GpuLeaseManager {
    /// Register a training job before it starts polling for GPUs.  A ticket is
    /// deliberately assigned once so an older multi-GPU request cannot be
    /// starved by later single-card requests.
    pub fn register_waiting(&self, task_id: &str, _profile: &str, gpu_ids: &[String]) {
        let mut state = self.state.lock().expect("GPU lease lock poisoned");
        if state.waiting.contains_key(task_id) {
            return;
        }
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.saturating_add(1);
        state.waiting.insert(
            task_id.to_string(),
            GpuLeaseWait {
                ticket,
                gpu_keys: gpu_ids.iter().map(|gpu| format!("gpu:{gpu}")).collect(),
            },
        );
    }

    pub fn try_acquire(&self, task_id: &str, _profile: &str, gpu_ids: &[String]) -> bool {
        let keys = gpu_ids
            .iter()
            .map(|gpu| format!("gpu:{gpu}"))
            .collect::<Vec<_>>();
        let mut state = self.state.lock().expect("GPU lease lock poisoned");
        let was_registered = state.waiting.contains_key(task_id);
        if !was_registered {
            let ticket = state.next_ticket;
            state.next_ticket = state.next_ticket.saturating_add(1);
            state.waiting.insert(
                task_id.to_string(),
                GpuLeaseWait {
                    ticket,
                    gpu_keys: keys.iter().cloned().collect(),
                },
            );
        }
        let Some(wait) = state.waiting.get(task_id).cloned() else {
            return false;
        };
        if keys
            .iter()
            .any(|key| state.held.get(key).is_some_and(|owner| owner != task_id))
        {
            if !was_registered {
                state.waiting.remove(task_id);
            }
            return false;
        }
        if state.waiting.iter().any(|(waiting_task, earlier)| {
            waiting_task != task_id
                && earlier.ticket < wait.ticket
                && earlier
                    .gpu_keys
                    .iter()
                    .any(|key| wait.gpu_keys.contains(key))
        }) {
            if !was_registered {
                state.waiting.remove(task_id);
            }
            return false;
        }
        for key in keys {
            state.held.insert(key, task_id.to_string());
        }
        state.waiting.remove(task_id);
        true
    }

    pub fn release(&self, task_id: &str) {
        let mut state = self.state.lock().expect("GPU lease lock poisoned");
        let held_before = state.held.len();
        let waiting_before = state.waiting.len();
        state.held.retain(|_, owner| owner != task_id);
        state.waiting.remove(task_id);
        let changed = held_before != state.held.len() || waiting_before != state.waiting.len();
        drop(state);
        if changed {
            self.changes.send_modify(|generation| {
                *generation = generation.saturating_add(1);
            });
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn notify_waiters(&self) {
        self.changes.send_modify(|generation| {
            *generation = generation.saturating_add(1);
        });
    }

    pub fn blockers(&self, _profile: &str, gpu_ids: &[String]) -> Vec<String> {
        let state = self.state.lock().expect("GPU lease lock poisoned");
        let requested = gpu_ids
            .iter()
            .map(|gpu| format!("gpu:{gpu}"))
            .collect::<BTreeSet<_>>();
        let mut blockers = requested
            .iter()
            .filter_map(|gpu| state.held.get(gpu).cloned())
            .collect::<BTreeSet<_>>();
        if let Some(wait) = state
            .waiting
            .values()
            .find(|wait| wait.gpu_keys == requested)
        {
            blockers.extend(
                state
                    .waiting
                    .iter()
                    .filter(|(_, earlier)| {
                        earlier.ticket < wait.ticket
                            && earlier.gpu_keys.iter().any(|key| requested.contains(key))
                    })
                    .map(|(task_id, _)| task_id.clone()),
            );
        }
        blockers.into_iter().collect()
    }

    pub fn waiting_snapshot(&self) -> Vec<TrainingGpuQueueEntry> {
        let state = self.state.lock().expect("GPU lease lock poisoned");
        let mut waits = state
            .waiting
            .iter()
            .map(|(task_id, wait)| (task_id.clone(), wait.clone()))
            .collect::<Vec<_>>();
        waits.sort_by_key(|(_, wait)| wait.ticket);
        waits
            .iter()
            .enumerate()
            .map(|(index, (task_id, wait))| {
                let mut blockers = wait
                    .gpu_keys
                    .iter()
                    .filter_map(|key| state.held.get(key).cloned())
                    .collect::<BTreeSet<_>>();
                blockers.extend(
                    waits
                        .iter()
                        .filter(|(other_id, earlier)| {
                            other_id != task_id
                                && earlier.ticket < wait.ticket
                                && earlier
                                    .gpu_keys
                                    .iter()
                                    .any(|key| wait.gpu_keys.contains(key))
                        })
                        .map(|(other_id, _)| other_id.clone()),
                );
                TrainingGpuQueueEntry {
                    task_id: task_id.clone(),
                    gpu_ids: wait
                        .gpu_keys
                        .iter()
                        .filter_map(|key| key.strip_prefix("gpu:").map(str::to_string))
                        .collect(),
                    queue_position: (index + 1) as u64,
                    blocker_task_ids: blockers.into_iter().collect(),
                }
            })
            .collect()
    }

    pub fn assigned_gpus(&self, task_id: &str, _profile: &str) -> Vec<String> {
        let prefix = "gpu:";
        let state = self.state.lock().expect("GPU lease lock poisoned");
        let mut ids = state
            .held
            .iter()
            .filter_map(|(key, owner)| {
                (owner == task_id)
                    .then(|| key.strip_prefix(&prefix).map(str::to_string))
                    .flatten()
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::{
        augment_adapter_with_upstream_fields, builtin_adapters, serialize_toml, GpuLeaseManager,
        TrainingField, TrainingRequest, TrainingSamplePromptSource, TrainingSampleSettings,
        UpstreamParserField,
    };
    use serde_json::Value;

    #[test]
    fn gpu_leases_keep_overlapping_training_requests_in_submission_order() {
        let leases = GpuLeaseManager::default();
        let gpu_zero = vec!["0".to_string()];

        leases.register_waiting("older", "windows", &gpu_zero);
        leases.register_waiting("newer", "windows", &gpu_zero);

        assert!(leases.try_acquire("older", "windows", &gpu_zero));
        assert!(!leases.try_acquire("newer", "windows", &gpu_zero));

        leases.release("older");
        assert!(leases.try_acquire("newer", "windows", &gpu_zero));
    }

    #[test]
    fn gpu_lease_waiting_snapshot_reports_position_and_blocker() {
        let leases = GpuLeaseManager::default();
        let gpu_zero = vec!["0".to_string()];

        leases.register_waiting("first", "windows", &gpu_zero);
        leases.register_waiting("second", "windows", &gpu_zero);

        let snapshot = leases.waiting_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].task_id, "first");
        assert_eq!(snapshot[0].queue_position, 1);
        assert_eq!(snapshot[1].task_id, "second");
        assert_eq!(snapshot[1].queue_position, 2);
        assert_eq!(snapshot[1].blocker_task_ids, vec!["first"]);
    }

    #[tokio::test]
    async fn releasing_a_gpu_wakes_waiters_without_a_polling_interval() {
        let leases = GpuLeaseManager::default();
        let gpu_zero = vec!["0".to_string()];
        assert!(leases.try_acquire("holder", "windows", &gpu_zero));
        leases.register_waiting("waiter", "windows", &gpu_zero);
        assert!(!leases.try_acquire("waiter", "windows", &gpu_zero));
        let mut changes = leases.subscribe();

        leases.release("holder");

        tokio::time::timeout(std::time::Duration::from_millis(50), changes.changed())
            .await
            .expect("GPU release should wake the queue")
            .expect("GPU lease notification channel should stay open");
        assert!(leases.try_acquire("waiter", "windows", &gpu_zero));
    }

    #[test]
    fn gallery_dataset_requires_a_safe_directory_and_positive_repeat() {
        let request = super::TrainingGalleryDataset {
            root_id: "library-root".to_string(),
            relative_directory: "characters/odette".to_string(),
            repeats: 0,
            caption_extension: Some(".txt".to_string()),
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn gallery_dataset_can_supply_train_data_dir_at_task_start() {
        let request = super::TrainingRequest {
            adapter_id: "sdxl-lora".to_string(),
            runtime_profile_id: "windows".to_string(),
            gpu_ids: vec![],
            gallery_dataset: Some(super::TrainingGalleryDataset {
                root_id: "library-root".to_string(),
                relative_directory: "odette".to_string(),
                repeats: 2,
                caption_extension: None,
            }),
            gallery_datasets: vec![],
            sample: None,
            parameters: serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/base.safetensors",
                "output_dir": "D:/outputs"
            }),
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn kohya_v26_catalog_keeps_sdxl_legacy_id_and_only_exposes_upstream_compatible_methods() {
        let adapters = builtin_adapters();
        assert_eq!(adapters.len(), 27);
        assert!(adapters.iter().any(|adapter| adapter.id == "sdxl-lora"));
        assert!(adapters.iter().any(|adapter| adapter.id == "anima-lokr"));
        assert!(adapters
            .iter()
            .any(|adapter| adapter.id == "hunyuan-image-lora"));
        assert!(!adapters
            .iter()
            .any(|adapter| adapter.id == "hunyuan-image-fine-tune"));
        assert!(!adapters.iter().any(|adapter| adapter.id == "flux-loha"));
    }

    #[test]
    fn enabled_sample_settings_require_a_manual_prompt_but_allow_caption_extraction() {
        let mut manual = TrainingSampleSettings {
            enabled: true,
            prompt_source: TrainingSamplePromptSource::Manual,
            prompt: String::new(),
            negative_prompt: "low quality".to_string(),
            dataset_caption_count: 4,
            steps: 30,
            width: 1024,
            height: 1024,
            every_n_epochs: 1,
        };
        assert!(manual.validate().is_err());

        manual.prompt = "portrait of odette".to_string();
        assert!(manual.validate().is_ok());

        let captions = TrainingSampleSettings {
            enabled: true,
            prompt_source: TrainingSamplePromptSource::DatasetCaptions,
            prompt: String::new(),
            negative_prompt: String::new(),
            dataset_caption_count: 4,
            steps: 30,
            width: 1024,
            height: 1024,
            every_n_epochs: 1,
        };
        assert!(captions.validate().is_ok());
    }

    #[test]
    fn sdxl_keeps_a_32_bucket_step_and_rejects_values_not_divisible_by_32() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .unwrap();
        assert_eq!(
            adapter
                .fields
                .iter()
                .find(|field| field.key == "bucket_reso_steps")
                .unwrap()
                .default,
            Value::from(32)
        );

        let request = TrainingRequest {
            adapter_id: "sdxl-lora".to_string(),
            runtime_profile_id: "windows".to_string(),
            gpu_ids: vec![],
            gallery_dataset: None,
            gallery_datasets: vec![],
            sample: None,
            parameters: serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/base.safetensors",
                "train_data_dir": "D:/datasets/subject",
                "output_dir": "D:/outputs",
                "bucket_reso_steps": 32
            }),
        };

        assert!(request.validate().is_ok());
        let invalid = TrainingRequest {
            parameters: serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/base.safetensors",
                "train_data_dir": "D:/datasets/subject",
                "output_dir": "D:/outputs",
                "bucket_reso_steps": 24
            }),
            ..request
        };
        assert!(invalid.validate().unwrap_err().contains("32 的倍数"));
    }

    #[test]
    fn sdxl_adapter_serializes_a_user_model_without_hardcoding_it() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .expect("the built-in SDXL adapter must be available");
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/custom.safetensors",
                "train_data_dir": "D:/data/subject",
                "output_dir": "D:/output",
                "output_name": "subject-xl"
            }),
        )
        .expect("valid SDXL settings must be serializable");

        assert!(toml.contains("pretrained_model_name_or_path = \"D:/models/custom.safetensors\""));
        assert!(toml.contains("output_name = \"subject-xl\""));
    }

    #[test]
    fn scientific_notation_learning_rate_is_serialized_as_a_toml_number() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .expect("the built-in SDXL adapter must be available");
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/custom.safetensors",
                "train_data_dir": "D:/data/subject",
                "output_dir": "D:/output",
                "learning_rate": "1e-4"
            }),
        )
        .expect("a valid scientific-notation learning rate must be serializable");

        assert!(toml.contains("learning_rate = 0.0001"));
        assert!(!toml.contains("learning_rate = \"1e-4\""));
    }

    #[test]
    fn secret_fields_are_never_written_to_the_reproducible_toml_snapshot() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "wandb_api_key": "must-not-persist"
            }),
        )
        .unwrap();

        assert!(!toml.contains("must-not-persist"));
        assert!(!toml.contains("wandb_api_key"));
    }

    #[test]
    fn adapter_can_represent_and_serialize_a_field_exported_by_the_upstream_parser() {
        let mut adapter = builtin_adapters().into_iter().next().unwrap();
        let key = String::from("new_upstream_switch");
        adapter.fields.push(TrainingField {
            key: key.into(),
            label: String::from("New upstream switch").into(),
            group: String::from("advanced").into(),
            subgroup: String::from("upstream").into(),
            kind: String::from("boolean").into(),
            default: Value::Bool(false),
            choices: vec![],
            required: false,
            advanced: true,
            help: String::from("Automatically exported from the trainer parser").into(),
            description: String::from("Automatically exported from the trainer parser").into(),
            when_to_adjust: String::from("Leave at default unless known").into(),
        });

        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "new_upstream_switch": true
            }),
        )
        .unwrap();

        assert!(toml.contains("new_upstream_switch = true"));
    }

    #[test]
    fn upstream_parser_fields_are_added_to_advanced_without_duplicate_static_fields() {
        let adapter = augment_adapter_with_upstream_fields(
            builtin_adapters().into_iter().next().unwrap(),
            vec![
                UpstreamParserField {
                    key: "new_upstream_switch".into(),
                    default: Value::Bool(false),
                    choices: vec![],
                    kind: "boolean".into(),
                    required: false,
                    help: "A newly added upstream flag".into(),
                },
                UpstreamParserField {
                    key: "network_dim".into(),
                    default: Value::from(128),
                    choices: vec![],
                    kind: "number".into(),
                    required: false,
                    help: "Must not replace the curated field".into(),
                },
            ],
        );

        let exported = adapter
            .fields
            .iter()
            .find(|field| field.key == "new_upstream_switch")
            .unwrap();
        assert_eq!(exported.group, "advanced");
        assert_eq!(exported.kind, "boolean");
        assert_eq!(exported.help, "A newly added upstream flag");
        assert_eq!(
            adapter
                .fields
                .iter()
                .filter(|field| field.key == "network_dim")
                .count(),
            1
        );
    }

    #[test]
    fn parser_exported_safe_fields_survive_submission_before_the_adapter_cache_refreshes() {
        let adapter = builtin_adapters().into_iter().next().unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "new_upstream_switch": true
            }),
        )
        .unwrap();

        assert!(toml.contains("new_upstream_switch = true"));
    }

    #[test]
    fn an_explicit_step_limit_is_not_overridden_by_the_default_epoch_limit() {
        let adapter = builtin_adapters().into_iter().next().unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "max_train_steps": 2,
                "max_train_epochs": 10
            }),
        )
        .unwrap();

        assert!(toml.contains("max_train_steps = 2"));
        assert!(!toml.contains("max_train_epochs"));
    }

    #[test]
    fn sampling_is_not_enabled_without_a_prompt_file() {
        let adapter = builtin_adapters().into_iter().next().unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out"
            }),
        )
        .unwrap();

        assert!(!toml.contains("sample_every_n_epochs"));
    }

    #[test]
    fn sdxl_adapter_exposes_an_editable_advanced_override_field() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .unwrap();

        assert!(adapter.fields.iter().any(|field| {
            field.key == "advanced_parameters" && field.group == "advanced" && field.kind == "json"
        }));
    }

    #[test]
    fn advanced_override_values_are_written_as_upstream_toml_keys() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "advanced_parameters": { "new_upstream_option": 7 }
            }),
        )
        .expect("advanced overrides should be serializable");

        assert!(toml.contains("new_upstream_option = 7"));
        assert!(!toml.contains("advanced_parameters"));
    }

    #[test]
    fn verbatim_windows_prefix_is_stripped_from_path_values() {
        let adapter = builtin_adapters().into_iter().next().unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": r"\\?\D:/models/model.safetensors",
                "train_data_dir": r"\\?\D:/data",
                "output_dir": r"\\?\D:/out"
            }),
        )
        .expect("verbatim paths should be serializable");

        assert!(toml.contains("D:/models/model.safetensors"));
        assert!(toml.contains("D:/data"));
        assert!(toml.contains("D:/out"));
        assert!(!toml.contains(r"\\?"));
    }

    #[test]
    fn float_only_parameters_keep_a_fractional_form() {
        let adapter = builtin_adapters()
            .into_iter()
            .find(|adapter| adapter.id == "sdxl-lora")
            .unwrap();
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "validation_split": 0,
                "network_multiplier": 1
            }),
        )
        .expect("integer float-only parameters should be serializable");

        assert!(toml.contains("validation_split = 0.0"));
        assert!(toml.contains("network_multiplier = 1.0"));
    }

    #[test]
    fn empty_string_optional_fields_are_omitted_like_nulls() {
        let adapter = builtin_adapters()
            .into_iter()
            .next()
            .expect("the built-in adapters must be available");
        let toml = serialize_toml(
            &adapter,
            &serde_json::json!({
                "pretrained_model_name_or_path": "D:/models/model.safetensors",
                "train_data_dir": "D:/data",
                "output_dir": "D:/out",
                "vae": "",
                "some_empty_parser_field": "",
                "advanced_parameters": {"another_empty_field": ""},
            }),
        )
        .expect("empty optional values must be serializable");

        assert!(!toml.lines().any(|line| line.starts_with("vae =")));
        assert!(!toml.contains("some_empty_parser_field"));
        assert!(!toml.contains("another_empty_field"));
    }

    #[test]
    fn gpu_leases_are_exclusive_and_allow_uncontended_physical_gpus() {
        let leases = super::GpuLeaseManager::default();
        assert!(leases.try_acquire("first", "windows", &["0".into()]));
        assert!(!leases.try_acquire("second", "windows", &["0".into()]));
        assert!(leases.try_acquire("second", "windows", &["1".into()]));
        assert!(!leases.try_acquire("third", "wsl:Ubuntu", &["0".into()]));
        assert_eq!(leases.blockers("windows", &["0".into()]), vec!["first"]);
        leases.release("first");
        assert!(leases.try_acquire("second", "windows", &["0".into()]));
    }

    #[test]
    fn gpu_leases_are_global_to_the_physical_gpu_across_windows_and_wsl() {
        let leases = super::GpuLeaseManager::default();

        assert!(leases.try_acquire("windows-task", "windows", &["0".into()]));
        assert!(!leases.try_acquire("wsl-task", "wsl", &["0".into()]));
        assert!(leases.try_acquire("wsl-task", "wsl", &["1".into()]));
    }
}
