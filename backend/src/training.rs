use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

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
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingField {
    pub key: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub group: Cow<'static, str>,
    pub kind: Cow<'static, str>,
    pub default: Value,
    pub choices: Vec<Cow<'static, str>>,
    pub required: bool,
    pub advanced: bool,
    pub help: Cow<'static, str>,
}

fn field(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    kind: &'static str,
    default: Value,
    choices: &[&'static str],
    required: bool,
    advanced: bool,
    help: &'static str,
) -> TrainingField {
    TrainingField {
        key: Cow::Borrowed(key),
        label: Cow::Borrowed(label),
        group: Cow::Borrowed(group),
        kind: Cow::Borrowed(kind),
        default,
        choices: choices.iter().copied().map(Cow::Borrowed).collect(),
        required,
        advanced,
        help: Cow::Borrowed(help),
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
        "conv_dim",
        "conv_alpha",
        "network_args",
        "network_weights",
        "dim_from_weights",
    ];
    let mut fields = baseline
        .iter()
        .filter(|field| allowed.contains(&field.key.as_ref()))
        .filter(|field| {
            network_training
                || !matches!(
                    field.key.as_ref(),
                    "network_module"
                        | "network_dim"
                        | "network_alpha"
                        | "network_dropout"
                        | "rank_dropout"
                        | "module_dropout"
                        | "conv_dim"
                        | "conv_alpha"
                )
        })
        .cloned()
        .collect::<Vec<_>>();

    if textual_inversion {
        fields.push(field(
            "token_string",
            "概念 Token",
            "network",
            "text",
            Value::String("<concept>".into()),
            &[],
            true,
            false,
            "Textual Inversion 要学习的唯一 Token。",
        ));
        fields.push(field(
            "init_word",
            "初始化词",
            "network",
            "text",
            Value::String(String::new()),
            &[],
            false,
            false,
            "用已有词向量初始化 Token；留空时由上游默认处理。",
        ));
        fields.push(field(
            "num_vectors_per_token",
            "每个 Token 的向量数",
            "network",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "增大可提升概念容量，但会提高推理提示词成本。",
        ));
    }
    if leco {
        fields.retain(|field| field.key != "train_data_dir" && field.key != "dataset_config");
        fields.push(field(
            "prompts_file",
            "概念编辑 Prompt 文件",
            "dataset",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "LECO 所需的概念擦除/编辑 Prompt TOML 文件。",
        ));
        fields.push(field(
            "network_weights",
            "待编辑网络权重",
            "model",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "LECO 继续训练或编辑的网络权重。",
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
                "number",
                Value::from(-1),
                &[],
                false,
                false,
                "控制 Kronecker 因子划分；-1 使用上游自动策略。",
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
        },
        TrainingGroup {
            id: "dataset",
            label: "数据集与 Caption",
            description: "图片目录、桶与标签",
        },
        TrainingGroup {
            id: "training",
            label: "训练",
            description: "批量、步数与损失",
        },
        TrainingGroup {
            id: "network",
            label: "LoRA 网络",
            description: "LoRA、LyCORIS 与分层权重",
        },
        TrainingGroup {
            id: "optimizer",
            label: "优化器与学习率",
            description: "优化器、调度器与参数组",
        },
        TrainingGroup {
            id: "performance",
            label: "精度与性能",
            description: "缓存、注意力与显存优化",
        },
        TrainingGroup {
            id: "saving",
            label: "保存与样图",
            description: "检查点、状态与训练预览",
        },
        TrainingGroup {
            id: "logging",
            label: "日志与分布式",
            description: "遥测、W&B 与多卡",
        },
        TrainingGroup {
            id: "advanced",
            label: "高级参数",
            description: "所有上游 CLI 参数和 TOML 覆盖",
        },
    ];
    let fields = vec![
        field(
            "pretrained_model_name_or_path",
            "底模路径",
            "model",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "SDXL checkpoint 或 Diffusers 模型目录",
        ),
        field(
            "vae",
            "外置 VAE",
            "model",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "可选，用于覆盖底模内 VAE",
        ),
        field(
            "network_weights",
            "继续训练 LoRA",
            "model",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "已有 LoRA 权重",
        ),
        field(
            "resume",
            "恢复状态目录",
            "model",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "Accelerate save_state 目录",
        ),
        field(
            "train_data_dir",
            "训练集目录",
            "dataset",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "DreamBooth 子目录或数据集配置的图片目录",
        ),
        field(
            "reg_data_dir",
            "正则化数据集",
            "dataset",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "可选正则化图片目录",
        ),
        field(
            "dataset_config",
            "数据集配置",
            "dataset",
            "path",
            Value::String(String::new()),
            &[],
            false,
            true,
            "高级 dataset TOML/JSON 配置",
        ),
        field(
            "resolution",
            "训练分辨率",
            "dataset",
            "text",
            Value::String("1024,1024".into()),
            &[],
            true,
            false,
            "宽,高；启用 bucket 时作为目标分辨率",
        ),
        field(
            "enable_bucket",
            "启用 Bucket",
            "dataset",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "保留不同宽高比",
        ),
        field(
            "min_bucket_reso",
            "最小 Bucket",
            "dataset",
            "number",
            Value::from(256),
            &[],
            false,
            false,
            "最小 bucket 分辨率",
        ),
        field(
            "max_bucket_reso",
            "最大 Bucket",
            "dataset",
            "number",
            Value::from(2048),
            &[],
            false,
            false,
            "最大 bucket 分辨率",
        ),
        field(
            "bucket_reso_steps",
            "Bucket 步长",
            "dataset",
            "number",
            Value::from(32),
            &[],
            false,
            false,
            "SDXL 支持 32 的倍数；默认 32",
        ),
        field(
            "caption_extension",
            "Caption 扩展名",
            "dataset",
            "text",
            Value::String(".txt".into()),
            &[],
            false,
            false,
            "同名标签文件扩展名",
        ),
        field(
            "shuffle_caption",
            "随机打乱标签",
            "dataset",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            false,
            "与文本编码器缓存互斥",
        ),
        field(
            "keep_tokens",
            "保留前置标签",
            "dataset",
            "number",
            Value::from(0),
            &[],
            false,
            false,
            "shuffle 时固定保留的 token 数",
        ),
        field(
            "caption_dropout_rate",
            "Caption 丢弃率",
            "dataset",
            "number",
            Value::from(0),
            &[],
            false,
            false,
            "随机丢弃完整 caption",
        ),
        field(
            "caption_tag_dropout_rate",
            "Tag 丢弃率",
            "dataset",
            "number",
            Value::from(0),
            &[],
            false,
            false,
            "随机丢弃单个标签",
        ),
        field(
            "max_train_epochs",
            "最大 Epoch",
            "training",
            "number",
            Value::from(10),
            &[],
            false,
            false,
            "到达轮数后结束",
        ),
        field(
            "max_train_steps",
            "最大 Step",
            "training",
            "number",
            Value::Null,
            &[],
            false,
            true,
            "显式指定时覆盖 epoch 计算",
        ),
        field(
            "train_batch_size",
            "批量大小",
            "training",
            "number",
            Value::from(1),
            &[],
            true,
            false,
            "每卡 batch size",
        ),
        field(
            "gradient_accumulation_steps",
            "梯度累积",
            "training",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "有效 batch = batch × 累积 × GPU 数",
        ),
        field(
            "gradient_checkpointing",
            "梯度检查点",
            "training",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "降低显存，增加耗时",
        ),
        field(
            "loss_type",
            "损失函数",
            "training",
            "select",
            Value::String("l2".into()),
            &["l1", "l2", "huber", "smooth_l1"],
            false,
            false,
            "训练损失类型",
        ),
        field(
            "network_train_unet_only",
            "仅训练 U-Net",
            "training",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "SDXL 常用配置",
        ),
        field(
            "network_train_text_encoder_only",
            "仅训练文本编码器",
            "training",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "与仅 U-Net 不可同时启用",
        ),
        field(
            "network_module",
            "网络模块",
            "network",
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
            "LoRA 实现模块",
        ),
        field(
            "network_dim",
            "Network Dim",
            "network",
            "number",
            Value::from(32),
            &[],
            true,
            false,
            "LoRA rank",
        ),
        field(
            "network_alpha",
            "Network Alpha",
            "network",
            "number",
            Value::from(32),
            &[],
            false,
            false,
            "LoRA alpha",
        ),
        field(
            "network_dropout",
            "Network Dropout",
            "network",
            "number",
            Value::from(0),
            &[],
            false,
            false,
            "LoRA dropout",
        ),
        field(
            "conv_dim",
            "Conv Dim",
            "network",
            "number",
            Value::Null,
            &[],
            false,
            true,
            "LoCon/LyCORIS 卷积 rank",
        ),
        field(
            "conv_alpha",
            "Conv Alpha",
            "network",
            "number",
            Value::Null,
            &[],
            false,
            true,
            "LoCon/LyCORIS 卷积 alpha",
        ),
        field(
            "network_args",
            "Network Args",
            "network",
            "list",
            Value::Array(vec![]),
            &[],
            false,
            true,
            "逐行传递给网络模块",
        ),
        field(
            "down_lr_weight",
            "Down Block 权重",
            "network",
            "text",
            Value::String(String::new()),
            &[],
            false,
            true,
            "逗号分隔的分层学习率权重",
        ),
        field(
            "mid_lr_weight",
            "Mid Block 权重",
            "network",
            "text",
            Value::String(String::new()),
            &[],
            false,
            true,
            "中间层学习率权重",
        ),
        field(
            "up_lr_weight",
            "Up Block 权重",
            "network",
            "text",
            Value::String(String::new()),
            &[],
            false,
            true,
            "逗号分隔的分层学习率权重",
        ),
        field(
            "learning_rate",
            "总学习率",
            "optimizer",
            "number",
            Value::from(0.0001),
            &[],
            false,
            false,
            "未单独设置参数组时使用",
        ),
        field(
            "unet_lr",
            "U-Net 学习率",
            "optimizer",
            "number",
            Value::from(0.0001),
            &[],
            false,
            false,
            "U-Net 参数组学习率",
        ),
        field(
            "text_encoder_lr",
            "Text Encoder 学习率",
            "optimizer",
            "number",
            Value::from(0.00001),
            &[],
            false,
            false,
            "文本编码器参数组学习率",
        ),
        field(
            "optimizer_type",
            "优化器",
            "optimizer",
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
            "上游支持的优化器",
        ),
        field(
            "optimizer_args",
            "优化器参数",
            "optimizer",
            "list",
            Value::Array(vec![]),
            &[],
            false,
            true,
            "逐行 key=value",
        ),
        field(
            "lr_scheduler",
            "学习率调度器",
            "optimizer",
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
            "学习率调度策略",
        ),
        field(
            "lr_warmup_steps",
            "预热 Step",
            "optimizer",
            "number",
            Value::from(0),
            &[],
            false,
            false,
            "学习率预热",
        ),
        field(
            "lr_scheduler_num_cycles",
            "调度重启周期",
            "optimizer",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "cosine_with_restarts 周期",
        ),
        field(
            "mixed_precision",
            "混合精度",
            "performance",
            "select",
            Value::String("bf16".into()),
            &["no", "fp16", "bf16"],
            true,
            false,
            "训练精度",
        ),
        field(
            "full_fp16",
            "全 FP16",
            "performance",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "要求 mixed_precision=fp16",
        ),
        field(
            "full_bf16",
            "全 BF16",
            "performance",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "要求 mixed_precision=bf16",
        ),
        field(
            "cache_latents",
            "缓存 Latent",
            "performance",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "降低 VAE 开销",
        ),
        field(
            "cache_latents_to_disk",
            "Latent 写入磁盘",
            "performance",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "降低内存占用",
        ),
        field(
            "cache_text_encoder_outputs",
            "缓存文本编码器",
            "performance",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            false,
            "仅 U-Net 时可用",
        ),
        field(
            "cache_text_encoder_outputs_to_disk",
            "文本编码器缓存到磁盘",
            "performance",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            true,
            "文本编码器缓存持久化",
        ),
        field(
            "xformers",
            "xFormers",
            "performance",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "内存高效注意力",
        ),
        field(
            "sdpa",
            "PyTorch SDPA",
            "performance",
            "boolean",
            Value::Bool(false),
            &[],
            false,
            false,
            "PyTorch 原生注意力",
        ),
        field(
            "save_model_as",
            "模型格式",
            "saving",
            "select",
            Value::String("safetensors".into()),
            &["safetensors", "ckpt", "pt"],
            true,
            false,
            "LoRA 输出格式",
        ),
        field(
            "save_precision",
            "保存精度",
            "saving",
            "select",
            Value::String("fp16".into()),
            &["fp16", "bf16", "float"],
            true,
            false,
            "权重保存精度",
        ),
        field(
            "output_dir",
            "输出目录",
            "saving",
            "path",
            Value::String(String::new()),
            &[],
            true,
            false,
            "LoRA、检查点和样图输出目录",
        ),
        field(
            "output_name",
            "输出名称",
            "saving",
            "text",
            Value::String("sdxl-lora".into()),
            &[],
            true,
            false,
            "生成文件前缀",
        ),
        field(
            "save_every_n_epochs",
            "每 N Epoch 保存",
            "saving",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "检查点间隔",
        ),
        field(
            "save_state",
            "保存训练状态",
            "saving",
            "boolean",
            Value::Bool(true),
            &[],
            false,
            false,
            "用于恢复和安全暂停",
        ),
        field(
            "sample_prompts",
            "样图 Prompt 文件",
            "saving",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "每行一个样图 prompt",
        ),
        field(
            "sample_every_n_epochs",
            "样图间隔",
            "saving",
            "number",
            Value::from(1),
            &[],
            false,
            false,
            "每 N epoch 生成样图",
        ),
        field(
            "sample_sampler",
            "样图采样器",
            "saving",
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
            "样图采样器",
        ),
        field(
            "logging_dir",
            "日志目录",
            "logging",
            "path",
            Value::String(String::new()),
            &[],
            false,
            false,
            "运行日志与指标位置",
        ),
        field(
            "log_with",
            "上游日志器",
            "logging",
            "select",
            Value::String("tensorboard".into()),
            &["tensorboard", "wandb"],
            false,
            false,
            "原生监控始终额外记录 JSONL",
        ),
        field(
            "wandb_api_key",
            "W&B API Key",
            "logging",
            "secret",
            Value::String(String::new()),
            &[],
            false,
            true,
            "仅随本次启动传给运行时，不写入预设",
        ),
        field(
            "seed",
            "随机种子",
            "logging",
            "number",
            Value::from(1337),
            &[],
            false,
            false,
            "可重复训练的随机种子",
        ),
        field(
            "gpu_ids",
            "GPU",
            "logging",
            "list",
            Value::Array(vec![]),
            &[],
            false,
            false,
            "空值为自动选择；多项启动 Accelerate 多卡",
        ),
        field(
            "deepspeed",
            "DeepSpeed",
            "logging",
            "path",
            Value::String(String::new()),
            &[],
            false,
            true,
            "DeepSpeed 配置文件",
        ),
        field(
            "advanced_parameters",
            "原始高级参数",
            "advanced",
            "json",
            Value::Object(serde_json::Map::new()),
            &[],
            false,
            true,
            "JSON 对象：补充尚未被适配器声明的上游 CLI/TOML 参数；不能覆盖已有字段",
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
#[derive(Debug, Clone, Deserialize)]
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
        {
            return Err(format!("{} 包含不支持的值", field.label));
        }
        encoded.insert(field.key.to_string(), toml_value(&value)?);
    }
    // The form can be refreshed from an upstream parser while a queued task is
    // being submitted.  Preserve safe, typed parser fields even if that
    // request reaches a process that still has the baseline adapter cache.
    for (key, value) in values {
        if field_keys.contains(key.as_str()) || key == "advanced_parameters" || value.is_null() {
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
        encoded.insert(key.clone(), toml_value(value)?);
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
            if !value.is_null() {
                if is_secret_parameter_key(key) {
                    continue;
                }
                encoded.insert(key.clone(), toml_value(value)?);
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

#[derive(Debug, Clone, Default)]
pub struct GpuLeaseManager {
    state: Arc<Mutex<GpuLeaseState>>,
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
        state.held.retain(|_, owner| owner != task_id);
        state.waiting.remove(task_id);
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
            kind: String::from("boolean").into(),
            default: Value::Bool(false),
            choices: vec![],
            required: false,
            advanced: true,
            help: String::from("Automatically exported from the trainer parser").into(),
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
