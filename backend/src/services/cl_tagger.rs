//! Local ONNX tagger (WD EVA02-large fine-tune style) used as an alternative
//! captioner for dataset augmentation. The model and its tag mapping live in
//! a user-configured directory; nothing is hardcoded to a machine path.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use ort::inputs;

const INPUT_SIDE: u32 = 448;
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.4578275, 0.40821073];
const CLIP_STD: [f32; 3] = [0.26862954, 0.26130258, 0.27577711];

/// HuggingFace hub 缓存中的仓库目录名与模型子目录。
pub const CL_TAGGER_REPO_DIR: &str = "models--cella110n--cl_tagger";
pub const CL_TAGGER_MODEL_SUBDIR: &str = "cl_tagger_1_01";
pub const CL_TAGGER_MODEL_FILE: &str = "model.onnx";
pub const CL_TAGGER_MAPPING_FILE: &str = "tag_mapping.json";
pub const CL_TAGGER_HF_REPO: &str = "cella110n/cl_tagger";
pub const CL_TAGGER_HF_SUBPATH: &str = "cl_tagger_1_01";

/// 候选的 HuggingFace hub 缓存根目录（`.../hub`），按优先级排序。
fn hf_hub_cache_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("HF_HUB_CACHE") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("HUGGINGFACE_HUB_CACHE") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(home) = std::env::var_os("HF_HOME") {
        candidates.push(PathBuf::from(home).join("hub"));
    }
    if let Some(home) = std::env::var_os("HUGGINGFACE_HOME") {
        candidates.push(PathBuf::from(home).join("hub"));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        candidates.push(
            PathBuf::from(profile)
                .join(".cache")
                .join("huggingface")
                .join("hub"),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".cache")
                .join("huggingface")
                .join("hub"),
        );
    }
    candidates
}

/// 主 HuggingFace hub 缓存根目录（下载目标，优先显式环境变量）。
pub fn primary_hf_hub_cache() -> Option<PathBuf> {
    hf_hub_cache_candidates().into_iter().next()
}

/// 在 HuggingFace 缓存中检测 cl_tagger 模型目录（含 `model.onnx` 与
/// `tag_mapping.json`）。返回 `None` 表示缓存中不存在可用模型。
pub fn detect_model_in_hf_cache() -> Option<PathBuf> {
    for cache in hf_hub_cache_candidates() {
        let snapshots = cache.join(CL_TAGGER_REPO_DIR).join("snapshots");
        let Ok(entries) = fs::read_dir(&snapshots) else {
            continue;
        };
        let mut snapshot_dirs = entries
            .flatten()
            .filter_map(|entry| {
                let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                is_dir.then(|| entry.path())
            })
            .collect::<Vec<_>>();
        snapshot_dirs.sort();
        for snapshot in snapshot_dirs.into_iter().rev() {
            let model_dir = snapshot.join(CL_TAGGER_MODEL_SUBDIR);
            if model_dir.join(CL_TAGGER_MODEL_FILE).is_file()
                && model_dir.join(CL_TAGGER_MAPPING_FILE).is_file()
            {
                return Some(model_dir);
            }
        }
    }
    None
}

/// 解析模型目录：显式路径优先，其次 HF 缓存检测，最后返回 None。
pub fn resolve_model_dir(requested: &str) -> Option<PathBuf> {
    let requested = requested.trim();
    if requested.is_empty() {
        return detect_model_in_hf_cache();
    }
    Some(PathBuf::from(requested))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClTagCategory {
    General,
    Character,
    Copyright,
    Meta,
    Model,
    Rating,
    Quality,
}

impl ClTagCategory {}

#[derive(Debug, Clone)]
pub struct ClTaggerConfig {
    /// Directory containing `model.onnx` and `tag_mapping.json`.
    pub model_dir: PathBuf,
    pub general_threshold: f32,
    pub character_threshold: f32,
    pub copyright_threshold: f32,
    pub quality_threshold: f32,
    pub max_tags: usize,
}

impl Default for ClTaggerConfig {
    fn default() -> Self {
        Self {
            model_dir: PathBuf::new(),
            general_threshold: 0.35,
            character_threshold: 0.6,
            copyright_threshold: 0.6,
            quality_threshold: 0.35,
            max_tags: 60,
        }
    }
}

impl ClTaggerConfig {
    pub fn model_path(&self) -> PathBuf {
        self.model_dir.join("model.onnx")
    }

    pub fn mapping_path(&self) -> PathBuf {
        self.model_dir.join("tag_mapping.json")
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.model_dir.as_os_str().is_empty() {
            return Err("未配置 CL Tagger 模型路径".to_string());
        }
        if !self.model_path().is_file() {
            return Err(format!(
                "模型文件不存在: {}",
                self.model_path().display()
            ));
        }
        if !self.mapping_path().is_file() {
            return Err(format!(
                "标签映射文件不存在: {}",
                self.mapping_path().display()
            ));
        }
        if !(0.0..1.0).contains(&self.general_threshold)
            || !(0.0..1.0).contains(&self.character_threshold)
            || !(0.0..1.0).contains(&self.copyright_threshold)
            || !(0.0..1.0).contains(&self.quality_threshold)
        {
            return Err("CL Tagger 阈值必须在 0..1 之间".to_string());
        }
        if !(1..=500).contains(&self.max_tags) {
            return Err("CL Tagger 标签数限制必须在 1..=500 之间".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ClTagEntry {
    tag: String,
    category: ClTagCategory,
}

/// A loaded ONNX session plus its index -> tag mapping. Not `Sync`: guard it
/// with a mutex so a single model instance serves all workers serially.
pub struct ClTaggerModel {
    session: Session,
    tags: Arc<Vec<ClTagEntry>>,
    config: ClTaggerConfig,
}

impl fmt::Debug for ClTaggerModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClTaggerModel")
            .field("model_dir", &self.config.model_dir)
            .field("tag_count", &self.tags.len())
            .finish_non_exhaustive()
    }
}

impl ClTaggerModel {
    pub fn load(config: ClTaggerConfig) -> Result<Self, String> {
        config.validate()?;
        let session = Session::builder()
            .map_err(|error| format!("无法创建 ONNX 运行环境: {error}"))?
            .commit_from_file(config.model_path())
            .map_err(|error| format!("无法加载 ONNX 模型: {error}"))?;
        let tags = load_tag_mapping(&config.mapping_path())?;
        Ok(Self {
            session,
            tags: Arc::new(tags),
            config,
        })
    }

    pub fn tag_image(&mut self, image_path: &Path) -> Result<Vec<String>, String> {
        let image = image::open(image_path)
            .map_err(|error| format!("无法读取图片 {}: {error}", image_path.display()))?;
        let rgb = image.to_rgb8();
        let resized = imageops::resize(
            &rgb,
            INPUT_SIDE,
            INPUT_SIDE,
            imageops::FilterType::Lanczos3,
        );
        let mut tensor = Array4::<f32>::zeros((1, 3, INPUT_SIDE as usize, INPUT_SIDE as usize));
        for (x, y, pixel) in resized.enumerate_pixels() {
            for channel in 0..3 {
                let value = pixel[channel] as f32 / 255.0;
                tensor[[0, channel, y as usize, x as usize]] =
                    (value - CLIP_MEAN[channel]) / CLIP_STD[channel];
            }
        }
        let input_name = self
            .session
            .inputs()
            .iter()
            .next()
            .ok_or_else(|| "ONNX 模型没有输入".to_string())?
            .name()
            .to_string();
        let value = Tensor::from_array(tensor.into_dyn())
            .map_err(|error| format!("无法构造推理张量: {error}"))?;
        let logits = {
            let outputs = self
                .session
                .run(inputs![input_name => value])
                .map_err(|error| format!("ONNX 推理失败: {error}"))?;
            let (_, view) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|error| format!("无法读取推理输出: {error}"))?;
            view.to_vec()
        };
        if logits.len() != self.tags.len() {
            return Err(format!(
                "模型输出维度 {} 与标签映射数量 {} 不一致",
                logits.len(),
                self.tags.len()
            ));
        }
        let mut picked: Vec<(f32, usize)> = Vec::new();
        for (index, entry) in self.tags.iter().enumerate() {
            let probability = sigmoid(logits[index]);
            let threshold = self.threshold_for(entry.category);
            if probability >= threshold && self.include_category(entry.category) {
                picked.push((probability, index));
            }
        }
        picked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        picked.truncate(self.config.max_tags);
        Ok(picked
            .into_iter()
            .map(|(_, index)| self.tags[index].tag.clone())
            .collect())
    }

    fn threshold_for(&self, category: ClTagCategory) -> f32 {
        match category {
            ClTagCategory::General => self.config.general_threshold,
            ClTagCategory::Character => self.config.character_threshold,
            ClTagCategory::Copyright => self.config.copyright_threshold,
            ClTagCategory::Quality => self.config.quality_threshold,
            _ => 1.0,
        }
    }

    fn include_category(&self, category: ClTagCategory) -> bool {
        matches!(
            category,
            ClTagCategory::General
                | ClTagCategory::Character
                | ClTagCategory::Copyright
                | ClTagCategory::Quality
        )
    }
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}

fn load_tag_mapping(path: &Path) -> Result<Vec<ClTagEntry>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("无法读取标签映射 {}: {error}", path.display()))?;
    let root: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("标签映射 JSON 无效: {error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| "标签映射必须是 JSON 对象".to_string())?;
    let mut keys = object.keys().collect::<Vec<_>>();
    keys.sort_by_key(|key| key.parse::<u64>().unwrap_or(u64::MAX));
    let mut entries = Vec::with_capacity(keys.len());
    for key in keys {
        let value = object.get(key).ok_or_else(|| "标签映射条目缺失".to_string())?;
        let tag = value
            .get("tag")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("标签映射条目 {key} 缺少 tag"))?
            .to_string();
        let category = match value
            .get("category")
            .and_then(serde_json::Value::as_str)
        {
            Some("General") => ClTagCategory::General,
            Some("Character") => ClTagCategory::Character,
            Some("Copyright") => ClTagCategory::Copyright,
            Some("Meta") => ClTagCategory::Meta,
            Some("Model") => ClTagCategory::Model,
            Some("Rating") => ClTagCategory::Rating,
            Some("Quality") => ClTagCategory::Quality,
            _ => ClTagCategory::Meta,
        };
        entries.push(ClTagEntry { tag, category });
    }
    if entries.is_empty() {
        return Err("标签映射为空".to_string());
    }
    Ok(entries)
}

/// Returns the tag -> category lookup used by tests and diagnostics.
#[allow(dead_code)]
pub fn tag_categories(mapping: &str) -> Result<HashMap<String, ClTagCategory>, String> {
    let path = tempfile::NamedTempFile::new()
        .map_err(|error| error.to_string())?;
    fs::write(path.path(), mapping).map_err(|error| error.to_string())?;
    Ok(load_tag_mapping(path.path())?
        .into_iter()
        .map(|entry| (entry.tag, entry.category))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_maps_extreme_logits_to_probabilities() {
        assert!(sigmoid(0.0) - 0.5 < 1e-6);
        assert!(sigmoid(10.0) > 0.999);
        assert!(sigmoid(-10.0) < 0.001);
    }

    #[test]
    fn tag_mapping_parses_numeric_index_order_and_categories() {
        let mapping = r#"{
            "0": {"tag": "general", "category": "Rating"},
            "1": {"tag": "explicit", "category": "Rating"},
            "10": {"tag": "1girl", "category": "General"},
            "2": {"tag": "solo", "category": "General"},
            "100": {"tag": "hatsune_miku", "category": "Character"},
            "11": {"tag": "vocaloid", "category": "Copyright"}
        }"#;
        let categories = tag_categories(mapping).expect("mapping parses");
        assert_eq!(
            categories.get("1girl"),
            Some(&ClTagCategory::General)
        );
        assert_eq!(
            categories.get("hatsune_miku"),
            Some(&ClTagCategory::Character)
        );
        assert_eq!(
            categories.get("vocaloid"),
            Some(&ClTagCategory::Copyright)
        );
        assert_eq!(
            categories.get("explicit"),
            Some(&ClTagCategory::Rating)
        );
    }

    #[test]
    fn config_validation_rejects_missing_model_directory() {
        let config = ClTaggerConfig::default();
        assert!(config.validate().is_err());
    }
}