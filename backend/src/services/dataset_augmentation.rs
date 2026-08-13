use crate::services::image_processor::{ToolError, VerifiedMediaRoot};
use image::{codecs::png::PngEncoder, ColorType, GenericImageView, ImageEncoder, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const RESOLUTION_BUCKETS: &[(u32, u32)] = &[
    (768, 1024),
    (832, 1216),
    (896, 1344),
    (1024, 1280),
    (1024, 1536),
    (1024, 1024),
];
const MAX_SOURCE_PIXELS: u64 = 100_000_000;
const MAX_UPSCALE_RATIO: f64 = 1.20;
const SMART_CROP_MIN_SCORE: f32 = 0.55;
/// A derived crop must visibly change the composition. Portraits are held to
/// the strictest retained-area limit so that a portrait label never masks an
/// image that still reads like the original full composition.
const MAX_PORTRAIT_AREA_RATIO: f32 = 0.55;
const MAX_UPPER_BODY_AREA_RATIO: f32 = 0.80;
const MAX_COWBOY_SHOT_AREA_RATIO: f32 = 0.78;
const MAX_FULL_BODY_TIGHT_AREA_RATIO: f32 = 0.82;
const MAX_LOWER_BODY_AREA_RATIO: f32 = 0.76;
const MAX_FEET_AREA_RATIO: f32 = 0.52;
/// A near-canvas ISNet mask is not useful crop evidence.  It can describe an
/// illustration plus its decorative foreground, so using it would turn every
/// safe composition back into the source canvas.
const MAX_FOREGROUND_PROTECTION_AREA_RATIO: f32 = 0.88;
/// Different composition labels must not create visually identical samples.
const MAX_DUPLICATE_CROP_IOU: f32 = 0.96;

/// Configuration for GPU-backed, anime-aware crops.  The cropper is deliberately
/// conservative: the source image is always retained and a rejected crop never
/// causes the family to be dropped from the dataset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartCropConfig {
    pub enabled: bool,
    pub runtime_profile_id: String,
    pub gpu_id: String,
    pub quality_profile: String,
    pub portrait: bool,
    pub upper_body: bool,
    #[serde(default)]
    pub cowboy_shot: bool,
    pub full_body_tight: bool,
    #[serde(default)]
    pub lower_body: bool,
    #[serde(default)]
    pub feet: bool,
    /// When disabled, one fully visible foot is sufficient for the feet view.
    /// Enabling it requires complete left and right foot keypoint evidence.
    #[serde(default)]
    pub require_both_feet: bool,
    pub max_derived_per_family: u8,
}

impl Default for SmartCropConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            runtime_profile_id: "conda:lora".to_string(),
            gpu_id: "0".to_string(),
            quality_profile: "anime-quality".to_string(),
            portrait: true,
            upper_body: true,
            cowboy_shot: true,
            full_body_tight: true,
            lower_body: true,
            feet: true,
            require_both_feet: false,
            max_derived_per_family: 6,
        }
    }
}

impl SmartCropConfig {
    fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.runtime_profile_id.trim().is_empty() {
            return Err("智能裁剪必须选择 Python 运行时".to_string());
        }
        if self.gpu_id.parse::<u32>().is_err() {
            return Err("智能裁剪 GPU 必须是非负整数编号".to_string());
        }
        if self.quality_profile != "anime-quality" {
            return Err("智能裁剪仅支持 anime-quality 质量档".to_string());
        }
        if !self.portrait
            && !self.upper_body
            && !self.cowboy_shot
            && !self.full_body_tight
            && !self.lower_body
            && !self.feet
        {
            return Err("智能裁剪至少需要启用一种构图".to_string());
        }
        if !(1..=6).contains(&self.max_derived_per_family) {
            return Err("每个 family 的智能裁剪数量必须在 1..=6 之间".to_string());
        }
        Ok(())
    }
}

fn enabled_smart_crop_variants(config: &SmartCropConfig) -> Vec<&'static str> {
    [
        ("portrait", config.portrait),
        ("upper_body", config.upper_body),
        ("cowboy_shot", config.cowboy_shot),
        ("full_body_tight", config.full_body_tight),
        ("lower_body", config.lower_body),
        ("feet", config.feet),
    ]
    .into_iter()
    .filter_map(|(variant, enabled)| enabled.then_some(variant))
    .collect()
}

/// Controls the optional second-pass captioning of transformed images. This
/// is opt-in because a derived image is never safe to train from until a new
/// caption has actually been written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DerivedRetaggingConfig {
    pub send_to_vllm: bool,
    pub preserve_artist_character_tags: bool,
}

impl Default for DerivedRetaggingConfig {
    fn default() -> Self {
        Self {
            send_to_vllm: false,
            preserve_artist_character_tags: true,
        }
    }
}

/// JSON contract emitted by `anime_crop_worker.py`. Coordinates are expressed
/// in the source image's native pixels and are only consumed after the Rust
/// side has already validated the source path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeCropBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    #[serde(default)]
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnimeCropPoint {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnimeCropPoseKeypoints {
    #[serde(default)]
    pub left_hip: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_hip: Option<AnimeCropPoint>,
    #[serde(default)]
    pub left_knee: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_knee: Option<AnimeCropPoint>,
    #[serde(default)]
    pub left_ankle: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_ankle: Option<AnimeCropPoint>,
    #[serde(default)]
    pub left_big_toe: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_big_toe: Option<AnimeCropPoint>,
    #[serde(default)]
    pub left_small_toe: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_small_toe: Option<AnimeCropPoint>,
    #[serde(default)]
    pub left_heel: Option<AnimeCropPoint>,
    #[serde(default)]
    pub right_heel: Option<AnimeCropPoint>,
}

/// A pose record tied to one detected person.  Unlike the former global pose
/// boolean, this lets the cropper prove that the selected subject—not another
/// person in the scene—has visible ankles and torso keypoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeCropPose {
    pub bbox: AnimeCropBox,
    #[serde(default)]
    pub torso_score: f32,
    #[serde(default)]
    pub left_ankle_score: f32,
    #[serde(default)]
    pub right_ankle_score: f32,
    #[serde(default)]
    pub keypoints: AnimeCropPoseKeypoints,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnimeCropAnalysis {
    pub media_id: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub persons: Vec<AnimeCropBox>,
    #[serde(default)]
    pub heads: Vec<AnimeCropBox>,
    #[serde(default)]
    pub faces: Vec<AnimeCropBox>,
    #[serde(default)]
    pub half_bodies: Vec<AnimeCropBox>,
    #[serde(default)]
    pub hands: Vec<AnimeCropBox>,
    #[serde(default)]
    pub foreground: Option<AnimeCropBox>,
    #[serde(default)]
    pub poses: Vec<AnimeCropPose>,
    #[serde(default)]
    pub pose_complete: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct CropRect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl CropRect {
    fn width(self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }
    fn height(self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }
    fn contains(self, other: Self) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }
    fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
    fn iou(self, other: Self) -> f32 {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        let intersection = x1.saturating_sub(x0) as f32 * y1.saturating_sub(y0) as f32;
        let union = (self.width() as f32 * self.height() as f32)
            + (other.width() as f32 * other.height() as f32)
            - intersection;
        if union > 0.0 {
            intersection / union
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CropCandidate {
    kind: &'static str,
    rect: CropRect,
    score: f32,
}

type SmartCropRejection = (&'static str, &'static str);

#[derive(Debug, Default)]
struct SmartCropCandidateEvaluation {
    candidates: Vec<CropCandidate>,
    rejections: BTreeMap<&'static str, SmartCropRejection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetAugmentationConfig {
    pub output_directory: PathBuf,
    pub min_megapixels: f64,
    pub min_long_side: u32,
    pub min_short_side: u32,
    pub horizontal_flip: bool,
    pub train_percent: u8,
    pub validation_percent: u8,
    pub test_percent: u8,
    pub jpeg_quality: u8,
    pub smart_crop: SmartCropConfig,
    pub retagging: DerivedRetaggingConfig,
}

impl Default for DatasetAugmentationConfig {
    fn default() -> Self {
        Self {
            output_directory: PathBuf::from(".augmentation"),
            min_megapixels: 1.8,
            min_long_side: 1536,
            min_short_side: 768,
            horizontal_flip: false,
            train_percent: 90,
            validation_percent: 5,
            test_percent: 5,
            jpeg_quality: 95,
            smart_crop: SmartCropConfig::default(),
            retagging: DerivedRetaggingConfig::default(),
        }
    }
}

impl DatasetAugmentationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_directory.as_os_str().is_empty()
            || self.output_directory.is_absolute()
            || !self
                .output_directory
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err("输出目录必须是媒体根内的规范相对路径".to_string());
        }
        if !self.min_megapixels.is_finite() || !(0.1..=1000.0).contains(&self.min_megapixels) {
            return Err("最小像素数必须在 0.1..=1000 MP 之间".to_string());
        }
        if !(1..=100_000).contains(&self.min_long_side)
            || !(1..=100_000).contains(&self.min_short_side)
        {
            return Err("原生分辨率门槛必须在 1..=100000 之间".to_string());
        }
        if usize::from(self.train_percent)
            + usize::from(self.validation_percent)
            + usize::from(self.test_percent)
            != 100
        {
            return Err("训练、验证和测试集比例之和必须为 100".to_string());
        }
        if !(1..=100).contains(&self.jpeg_quality) {
            return Err("JPEG 质量必须在 1..=100 之间".to_string());
        }
        self.smart_crop.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DatasetAugmentationSource {
    pub media_id: String,
    pub relative_path: PathBuf,
    pub sha256: Option<String>,
    pub fallback_caption: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetAugmentationSample {
    pub source_media_id: String,
    pub sample_id: String,
    pub family_id: String,
    pub variant: String,
    pub output_relative_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub split: String,
    pub requires_retagging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetAugmentationRejection {
    pub source_media_id: String,
    pub relative_path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DatasetAugmentationItemResult {
    Generated(Vec<DatasetAugmentationSample>),
    Rejected(DatasetAugmentationRejection),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetAugmentationSummary {
    pub output_relative_directory: PathBuf,
    pub metadata_relative_directory: PathBuf,
    pub derived_relative_directory: PathBuf,
    pub training_relative_directory: PathBuf,
    pub generated: usize,
    pub rejected: usize,
    pub retagging_pending: usize,
    pub retagged: usize,
    pub rejection_reasons: BTreeMap<String, usize>,
    pub smart_crop_by_variant: BTreeMap<String, SmartCropVariantSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SmartCropCoverageSummary {
    pub count: usize,
    pub average: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SmartCropVariantSummary {
    pub requested: usize,
    pub generated: usize,
    pub rejected: usize,
    pub coverage_percent: SmartCropCoverageSummary,
    pub rejection_reasons: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
struct SmartCropVariantAccumulator {
    requested: usize,
    generated: usize,
    rejected: usize,
    coverage_total: f64,
    coverage_min: Option<f64>,
    coverage_max: Option<f64>,
    rejection_reasons: BTreeMap<String, usize>,
}

impl SmartCropVariantAccumulator {
    fn summary(&self) -> SmartCropVariantSummary {
        SmartCropVariantSummary {
            requested: self.requested,
            generated: self.generated,
            rejected: self.rejected,
            coverage_percent: SmartCropCoverageSummary {
                count: self.generated,
                average: if self.generated == 0 {
                    0.0
                } else {
                    (self.coverage_total / self.generated as f64 * 10.0).round() / 10.0
                },
                min: self.coverage_min.unwrap_or(0.0),
                max: self.coverage_max.unwrap_or(0.0),
            },
            rejection_reasons: self.rejection_reasons.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SmartCropDecision<'a> {
    source_media_id: &'a str,
    source_relative_path: String,
    crop_type: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crop: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retained_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct DatasetMetadataRecord<'a> {
    sample_id: &'a str,
    family_id: &'a str,
    parent_media_id: &'a str,
    crop_type: &'a str,
    native_width: u32,
    native_height: u32,
    bucket_width: Option<u32>,
    bucket_height: Option<u32>,
    orientation: &'a str,
    upscale_ratio: f64,
    preserve_aspect_ratio: bool,
    allow_non_uniform_scaling: bool,
    split: &'a str,
    requires_retagging: bool,
    source_relative_path: String,
    output_relative_path: String,
}

#[derive(Debug, Clone, Serialize)]
struct FamilyMetadataRecord<'a> {
    family_id: &'a str,
    source_media_id: &'a str,
    split: &'a str,
    source_sha256: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct ResolutionBucket {
    width: u32,
    height: u32,
    upscale_ratio: f64,
}

pub struct DatasetAugmentationWorkspace {
    root: VerifiedMediaRoot,
    config: DatasetAugmentationConfig,
    output_relative: PathBuf,
    metadata_relative: PathBuf,
    generated: usize,
    rejected: usize,
    retagging_pending: usize,
    retagged: usize,
    ready_variant_counts: BTreeMap<String, usize>,
    rejection_reasons: BTreeMap<String, usize>,
    smart_crop_stats: BTreeMap<String, SmartCropVariantAccumulator>,
}

impl DatasetAugmentationWorkspace {
    pub fn create(
        root: VerifiedMediaRoot,
        task_id: &str,
        config: DatasetAugmentationConfig,
    ) -> Result<Self, ToolError> {
        config.validate().map_err(ToolError::InvalidManifest)?;
        let (output_relative, metadata_relative) =
            next_augmentation_directories(&root, &config.output_directory, task_id)?;
        let output = root.resolve(&output_relative)?;
        let metadata = root.resolve(&metadata_relative)?;
        for directory in [
            "derived/horizontal_flip/images",
            "derived/portrait/images",
            "derived/upper_body/images",
            "derived/cowboy_shot/images",
            "derived/full_body_tight/images",
            "derived/lower_body/images",
            "derived/feet/images",
            "metadata",
            "retagging",
            "splits",
            "rejected",
        ] {
            if directory.starts_with("derived/") {
                fs::create_dir_all(output.join(directory)).map_err(ToolError::Io)?;
            } else {
                fs::create_dir_all(metadata.join(directory)).map_err(ToolError::Io)?;
            }
        }
        write_json_atomic(&metadata.join("metadata/config.json"), &config)?;
        write_json_atomic(
            &metadata.join("INCOMPLETE.json"),
            &serde_json::json!({
                "format_version": 1,
                "state": "incomplete",
                "task_id": task_id,
                "message": "This augmentation output must not be used for training until READY.json exists.",
            }),
        )?;
        Ok(Self {
            root,
            config,
            output_relative,
            metadata_relative,
            generated: 0,
            rejected: 0,
            retagging_pending: 0,
            retagged: 0,
            ready_variant_counts: BTreeMap::new(),
            rejection_reasons: BTreeMap::new(),
            smart_crop_stats: BTreeMap::new(),
        })
    }

    #[allow(dead_code)]
    pub fn process(
        &mut self,
        source: &DatasetAugmentationSource,
    ) -> Result<DatasetAugmentationItemResult, ToolError> {
        self.process_with_analysis(source, None)
    }

    pub fn process_with_analysis(
        &mut self,
        source: &DatasetAugmentationSource,
        analysis: Option<&AnimeCropAnalysis>,
    ) -> Result<DatasetAugmentationItemResult, ToolError> {
        let source_path = self.root.resolve_existing_file(&source.relative_path)?;
        let image = ImageReader::open(&source_path)
            .map_err(ToolError::Io)?
            .with_guessed_format()?
            .decode()?;
        let (width, height) = image.dimensions();
        let rejection_reason = source_rejection_reason(width, height, &self.config);
        if let Some(reason) = rejection_reason {
            if self.config.smart_crop.enabled {
                self.record_all_crop_rejections(
                    source,
                    width,
                    height,
                    "native_resolution_too_low",
                    "源图不满足智能裁剪的原生分辨率门槛",
                )?;
            }
            let rejection = DatasetAugmentationRejection {
                source_media_id: source.media_id.clone(),
                relative_path: source.relative_path.clone(),
                reason,
            };
            self.append_json_line("rejected/rejections.jsonl", &rejection)?;
            self.record_rejection_reason(&rejection.reason);
            self.rejected += 1;
            return Ok(DatasetAugmentationItemResult::Rejected(rejection));
        }

        let token = source_token(source);
        let family_id = family_id(source);
        let split = split_for_family(&family_id, &self.config);

        let bucket = choose_bucket(width, height);
        let mut samples = Vec::new();
        if self.config.horizontal_flip {
            let flipped = image.fliph();
            samples.push(self.write_flipped_sample(
                source, &flipped, &token, &family_id, &split, width, height, bucket,
            )?);
        }
        if self.config.smart_crop.enabled {
            match analysis {
                Some(analysis) if analysis.error.is_none() => {
                    samples.extend(self.write_smart_crop_samples(
                        source, &image, &token, &family_id, &split, analysis,
                    )?);
                }
                Some(analysis) => {
                    self.record_crop_rejection(
                        source,
                        analysis
                            .error
                            .as_deref()
                            .unwrap_or("检测模型未返回可用结果"),
                    )?;
                    self.record_all_crop_rejections(
                        source,
                        width,
                        height,
                        "detection_failed",
                        "检测模型未返回可用结果",
                    )?;
                }
                None => {
                    self.record_crop_rejection(source, "未获得智能裁剪检测结果")?;
                    self.record_all_crop_rejections(
                        source,
                        width,
                        height,
                        "detection_failed",
                        "未获得智能裁剪检测结果",
                    )?;
                }
            }
        }
        self.append_json_line(
            "metadata/families.jsonl",
            &FamilyMetadataRecord {
                family_id: &family_id,
                source_media_id: &source.media_id,
                split: &split,
                source_sha256: source.sha256.as_deref(),
            },
        )?;
        self.generated += samples.len();
        Ok(DatasetAugmentationItemResult::Generated(samples))
    }

    #[allow(clippy::too_many_arguments)]
    fn write_smart_crop_samples(
        &mut self,
        source: &DatasetAugmentationSource,
        image: &image::DynamicImage,
        token: &str,
        family_id: &str,
        split: &str,
        analysis: &AnimeCropAnalysis,
    ) -> Result<Vec<DatasetAugmentationSample>, ToolError> {
        let (width, height) = image.dimensions();
        if analysis.width != 0
            && analysis.height != 0
            && (analysis.width != width || analysis.height != height)
        {
            self.record_crop_rejection(source, "检测结果尺寸与源图片不一致")?;
            self.record_all_crop_rejections(
                source,
                width,
                height,
                "analysis_dimension_mismatch",
                "检测结果尺寸与源图片不一致",
            )?;
            return Ok(Vec::new());
        }
        let evaluation = smart_crop_candidate_evaluation(analysis, width, height, &self.config);
        let candidates = evaluation.candidates;
        if candidates.is_empty() {
            self.record_crop_rejection(
                source,
                "没有满足关键部位保护和原生分辨率约束的智能裁剪候选",
            )?;
        }
        let selected = candidates
            .iter()
            .take(usize::from(self.config.smart_crop.max_derived_per_family))
            .copied()
            .collect::<Vec<_>>();
        let mut samples = Vec::new();
        for candidate in &selected {
            let cropped = image.crop_imm(
                candidate.rect.x0,
                candidate.rect.y0,
                candidate.rect.width(),
                candidate.rect.height(),
            );
            let sample_id = format!("{token}_{}", candidate.kind);
            let image_relative = self
                .output_relative
                .join(format!("derived/{}/images", candidate.kind))
                .join(format!("{sample_id}.png"));
            write_png_atomic(&self.root.resolve(&image_relative)?, &cropped)?;
            samples.push(self.record_sample(
                source,
                &sample_id,
                family_id,
                candidate.kind,
                &image_relative,
                candidate.rect.width(),
                candidate.rect.height(),
                choose_bucket(candidate.rect.width(), candidate.rect.height()),
                split,
                true,
            )?);
        }
        for variant in enabled_smart_crop_variants(&self.config.smart_crop) {
            if let Some(candidate) = selected
                .iter()
                .copied()
                .find(|candidate| candidate.kind == variant)
            {
                self.record_crop_decision(source, variant, Some(candidate), width, height, None)?;
                continue;
            }
            let reason = if candidates.iter().any(|candidate| candidate.kind == variant) {
                ("family_limit", "达到每个 family 的派生图数量上限")
            } else {
                evaluation.rejections.get(variant).copied().unwrap_or_else(|| {
                    smart_crop_rejection_reason(variant, analysis, width, height, &self.config)
                })
            };
            self.record_crop_decision(
                source,
                variant,
                None,
                width,
                height,
                Some(reason),
            )?;
        }
        Ok(samples)
    }

    fn record_crop_rejection(
        &mut self,
        source: &DatasetAugmentationSource,
        reason: &str,
    ) -> Result<(), ToolError> {
        let reason = format!("智能裁剪拒绝：{reason}");
        self.append_json_line(
            "rejected/rejections.jsonl",
            &DatasetAugmentationRejection {
                source_media_id: source.media_id.clone(),
                relative_path: source.relative_path.clone(),
                reason: reason.clone(),
            },
        )?;
        self.record_rejection_reason(&reason);
        self.rejected += 1;
        Ok(())
    }

    fn record_crop_decision(
        &mut self,
        source: &DatasetAugmentationSource,
        crop_type: &'static str,
        candidate: Option<CropCandidate>,
        source_width: u32,
        source_height: u32,
        reason: Option<(&'static str, &'static str)>,
    ) -> Result<(), ToolError> {
        let retained_percent = candidate.map(|candidate| {
            let source_area = f64::from(source_width) * f64::from(source_height);
            if source_area <= 0.0 {
                0.0
            } else {
                (f64::from(candidate.rect.width()) * f64::from(candidate.rect.height())
                    / source_area
                    * 1000.0)
                    .round()
                    / 10.0
            }
        });
        let stats = self
            .smart_crop_stats
            .entry(crop_type.to_string())
            .or_default();
        stats.requested += 1;
        if let Some(percent) = retained_percent {
            stats.generated += 1;
            stats.coverage_total += percent;
            stats.coverage_min = Some(stats.coverage_min.map_or(percent, |value| value.min(percent)));
            stats.coverage_max = Some(stats.coverage_max.map_or(percent, |value| value.max(percent)));
        } else if let Some((code, _)) = reason {
            stats.rejected += 1;
            *stats.rejection_reasons.entry(code.to_string()).or_default() += 1;
        }
        let (reason_code, reason_text) = reason
            .map(|(code, text)| (Some(code), Some(text)))
            .unwrap_or((None, None));
        self.append_json_line(
            "metadata/smart-crop-evaluations.jsonl",
            &SmartCropDecision {
                source_media_id: &source.media_id,
                source_relative_path: source.relative_path.to_string_lossy().replace('\\', "/"),
                crop_type,
                status: if candidate.is_some() { "generated" } else { "rejected" },
                reason_code,
                reason: reason_text,
                crop: candidate.map(|candidate| {
                    serde_json::json!({
                        "x": candidate.rect.x0,
                        "y": candidate.rect.y0,
                        "width": candidate.rect.width(),
                        "height": candidate.rect.height(),
                    })
                }),
                retained_percent,
            },
        )
    }

    fn record_all_crop_rejections(
        &mut self,
        source: &DatasetAugmentationSource,
        source_width: u32,
        source_height: u32,
        code: &'static str,
        reason: &'static str,
    ) -> Result<(), ToolError> {
        for variant in enabled_smart_crop_variants(&self.config.smart_crop) {
            self.record_crop_decision(
                source,
                variant,
                None,
                source_width,
                source_height,
                Some((code, reason)),
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_flipped_sample(
        &mut self,
        source: &DatasetAugmentationSource,
        image: &image::DynamicImage,
        token: &str,
        family_id: &str,
        split: &str,
        width: u32,
        height: u32,
        bucket: Option<ResolutionBucket>,
    ) -> Result<DatasetAugmentationSample, ToolError> {
        let sample_id = format!("{token}_horizontal_flip");
        let image_relative = self
            .output_relative
            .join("derived/horizontal_flip/images")
            .join(format!("{sample_id}.png"));
        write_png_atomic(&self.root.resolve(&image_relative)?, image)?;
        self.record_sample(
            source,
            &sample_id,
            family_id,
            "horizontal_flip",
            &image_relative,
            width,
            height,
            bucket,
            split,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_sample(
        &mut self,
        source: &DatasetAugmentationSource,
        sample_id: &str,
        family_id: &str,
        variant: &str,
        image_relative: &Path,
        width: u32,
        height: u32,
        bucket: Option<ResolutionBucket>,
        split: &str,
        requires_retagging: bool,
    ) -> Result<DatasetAugmentationSample, ToolError> {
        let record = DatasetMetadataRecord {
            sample_id,
            family_id,
            parent_media_id: &source.media_id,
            crop_type: variant,
            native_width: width,
            native_height: height,
            bucket_width: bucket.map(|bucket| bucket.width),
            bucket_height: bucket.map(|bucket| bucket.height),
            orientation: if height >= width {
                "portrait"
            } else {
                "landscape"
            },
            upscale_ratio: bucket.map(|bucket| bucket.upscale_ratio).unwrap_or(1.0),
            preserve_aspect_ratio: true,
            allow_non_uniform_scaling: false,
            split,
            requires_retagging,
            source_relative_path: source.relative_path.to_string_lossy().replace('\\', "/"),
            output_relative_path: image_relative.to_string_lossy().replace('\\', "/"),
        };
        self.append_json_line("metadata/dataset.jsonl", &record)?;
        self.append_json_line(
            &format!("splits/{split}.jsonl"),
            &serde_json::json!({
                "sample_id": sample_id,
                "family_id": family_id,
                "relative_path": record.output_relative_path,
            }),
        )?;
        if requires_retagging {
            self.append_json_line(
                "metadata/retagging.jsonl",
                &serde_json::json!({
                    "sample_id": sample_id,
                    "family_id": family_id,
                    "split": split,
                    "relative_path": record.output_relative_path,
                    "reason": "derived image requires a newly generated caption; source caption was intentionally not copied",
                }),
            )?;
            self.retagging_pending += 1;
        }
        let sample = DatasetAugmentationSample {
            source_media_id: source.media_id.clone(),
            sample_id: sample_id.to_string(),
            family_id: family_id.to_string(),
            variant: variant.to_string(),
            output_relative_path: image_relative.to_path_buf(),
            width,
            height,
            split: split.to_string(),
            requires_retagging,
        };
        if !sample.requires_retagging {
            self.materialize_train_ready_sample(&sample)?;
            *self
                .ready_variant_counts
                .entry(sample.variant.clone())
                .or_default() += 1;
        }
        Ok(sample)
    }

    /// Promotes only successfully recaptioned transformed samples into the
    /// trainable split tree. The source caption is never used as a fallback.
    pub fn promote_retagged_samples(
        &mut self,
        samples: &[DatasetAugmentationSample],
    ) -> Result<(), ToolError> {
        for sample in samples {
            if !sample.requires_retagging {
                continue;
            }
            let image = self.root.resolve(&sample.output_relative_path)?;
            let caption = image.with_extension("txt");
            let content = fs::read_to_string(&caption).map_err(ToolError::Io)?;
            if content.trim().is_empty() {
                return Err(ToolError::InvalidManifest(format!(
                    "重新打标结果为空，不能加入训练: {}",
                    sample.sample_id
                )));
            }
            self.materialize_train_ready_sample(sample)?;
            self.append_json_line(
                "metadata/retagging.jsonl",
                &serde_json::json!({
                    "sample_id": sample.sample_id,
                    "family_id": sample.family_id,
                    "split": sample.split,
                    "relative_path": sample.output_relative_path,
                    "status": "completed",
                    "message": "new caption written and sample promoted into the train-ready split",
                }),
            )?;
            self.retagging_pending = self.retagging_pending.saturating_sub(1);
            self.retagged += 1;
            *self
                .ready_variant_counts
                .entry(sample.variant.clone())
                .or_default() += 1;
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<DatasetAugmentationSummary, ToolError> {
        let training_relative_directory = self.output_relative.join("ready/train");
        let ready = serde_json::json!({
            "format_version": 1,
            "image_output_relative_directory": self.output_relative,
            "metadata_relative_directory": self.metadata_relative,
            "training_relative_directory": training_relative_directory,
            "split_directories": {
                "train": self.output_relative.join("ready/train"),
                "validation": self.output_relative.join("ready/validation"),
                "test": self.output_relative.join("ready/test"),
            },
            "generated": self.generated,
            "rejected": self.rejected,
            "retagging_pending": self.retagging_pending,
            "retagged": self.retagged,
            "training_subsets": self.training_subsets_manifest(),
            "smart_crop_by_variant": self.smart_crop_stats.iter().map(|(variant, stats)| (variant.clone(), stats.summary())).collect::<BTreeMap<_, _>>(),
        });
        let metadata = self.root.resolve(&self.metadata_relative)?;
        write_json_atomic(
            &metadata.join("metadata/training-subsets.json"),
            &self.training_subsets_manifest(),
        )?;
        write_json_atomic(&metadata.join("READY.json"), &ready)?;
        fs::remove_file(metadata.join("INCOMPLETE.json")).map_err(ToolError::Io)?;
        Ok(DatasetAugmentationSummary {
            output_relative_directory: self.output_relative.clone(),
            metadata_relative_directory: self.metadata_relative.clone(),
            derived_relative_directory: self.output_relative.join("derived"),
            training_relative_directory,
            generated: self.generated,
            rejected: self.rejected,
            retagging_pending: self.retagging_pending,
            retagged: self.retagged,
            rejection_reasons: self.rejection_reasons.clone(),
            smart_crop_by_variant: self
                .smart_crop_stats
                .iter()
                .map(|(variant, stats)| (variant.clone(), stats.summary()))
                .collect(),
        })
    }

    fn materialize_train_ready_sample(
        &self,
        sample: &DatasetAugmentationSample,
    ) -> Result<(), ToolError> {
        let extension = sample
            .output_relative_path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| ToolError::InvalidManifest("训练样本缺少图片扩展名".to_string()))?;
        let ready_relative = self
            .output_relative
            .join("ready")
            .join(&sample.split)
            .join(&sample.variant)
            .join("images")
            .join(format!("{}.{}", sample.sample_id, extension));
        let source = self.root.resolve(&sample.output_relative_path)?;
        let destination = self.root.resolve(&ready_relative)?;
        link_or_copy_new_file(&source, &destination)?;
        let source_caption = source.with_extension("txt");
        let destination_caption = destination.with_extension("txt");
        link_or_copy_new_file(&source_caption, &destination_caption)
    }

    fn training_subsets_manifest(&self) -> serde_json::Value {
        let variants = [
            ("horizontal_flip", "水平翻转", true),
            ("portrait", "肖像裁剪", true),
            ("upper_body", "上半身裁剪", true),
            ("cowboy_shot", "牛仔视角裁剪", true),
            ("full_body_tight", "紧凑全身裁剪", true),
            ("lower_body", "下半身裁剪", true),
            ("feet", "脚部视角裁剪", true),
        ];
        serde_json::json!({
            "format_version": 1,
            "family_binding_relative_path": self.metadata_relative.join("metadata/dataset.jsonl"),
            "source_dataset": {
                "id": "original",
                "label": "原图（源目录，不复制）",
                "relative_directory": self.source_relative_directory(),
                "requires_retagging": false,
                "default_repeats": 1,
            },
            "splits": ["train", "validation", "test"],
            "subsets": variants.into_iter().map(|(id, label, requires_retagging)| {
                let directory = self.output_relative.join("ready/train").join(id).join("images");
                serde_json::json!({
                    "id": id,
                    "label": label,
                    "relative_directory": directory,
                    "requires_retagging": requires_retagging,
                    "training_ready_count": self.ready_variant_counts.get(id).copied().unwrap_or(0),
                    "default_repeats": 1,
                })
            }).collect::<Vec<_>>(),
        })
    }

    fn append_json_line(&self, relative: &str, value: &impl Serialize) -> Result<(), ToolError> {
        let path = self.root.resolve(&self.metadata_relative.join(relative))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(ToolError::Io)?;
        serde_json::to_writer(&mut file, value)
            .map_err(|error| ToolError::InvalidManifest(error.to_string()))?;
        file.write_all(b"\n").map_err(ToolError::Io)
    }

    fn source_relative_directory(&self) -> PathBuf {
        self.output_relative
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn record_rejection_reason(&mut self, reason: &str) {
        *self
            .rejection_reasons
            .entry(reason.to_string())
            .or_default() += 1;
    }
}

fn next_augmentation_directories(
    root: &VerifiedMediaRoot,
    requested: &Path,
    task_id: &str,
) -> Result<(PathBuf, PathBuf), ToolError> {
    let safe_task_id = task_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(96)
        .collect::<String>();
    if safe_task_id.is_empty() {
        return Err(ToolError::InvalidManifest("任务 ID 无效".to_string()));
    }
    for attempt in 1..=100 {
        let name = if attempt == 1 {
            safe_task_id.clone()
        } else {
            format!("{safe_task_id}-{attempt}")
        };
        let image_relative = requested.join(&name);
        let metadata_relative = augmentation_metadata_directory(requested)?.join(name);
        if !root.resolve(&image_relative)?.exists() && !root.resolve(&metadata_relative)?.exists() {
            return Ok((image_relative, metadata_relative));
        }
    }
    Err(ToolError::InvalidManifest(
        "无法创建新的数据集输出目录，请选择其他目录".to_string(),
    ))
}

fn augmentation_metadata_directory(image_output_root: &Path) -> Result<PathBuf, ToolError> {
    let parent = image_output_root.parent().ok_or_else(|| {
        ToolError::InvalidManifest(
            "增广输出目录必须位于原数据目录的 .augmentation 子目录".to_string(),
        )
    })?;
    Ok(parent.join(".augmentation-metadata"))
}

fn source_rejection_reason(
    width: u32,
    height: u32,
    config: &DatasetAugmentationConfig,
) -> Option<String> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_SOURCE_PIXELS {
        return Some("图片像素数超过安全上限".to_string());
    }
    if pixels as f64 / 1_000_000.0 < config.min_megapixels {
        return Some("图片未达到最小像素数门槛".to_string());
    }
    if width.max(height) < config.min_long_side {
        return Some("图片未达到最小长边门槛".to_string());
    }
    if width.min(height) < config.min_short_side {
        return Some("图片未达到原生短边门槛".to_string());
    }
    None
}

fn source_token(source: &DatasetAugmentationSource) -> String {
    let token = source
        .media_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(64)
        .collect::<String>();
    if token.is_empty() {
        short_digest(source.relative_path.to_string_lossy().as_bytes())
    } else {
        token
    }
}

fn family_id(source: &DatasetAugmentationSource) -> String {
    let seed = source.sha256.as_deref().unwrap_or(&source.media_id);
    format!("family_{}", short_digest(seed.as_bytes()))
}

fn short_digest(value: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(value);
    hex::encode(hash.finalize())[..16].to_string()
}

fn split_for_family(family_id: &str, config: &DatasetAugmentationConfig) -> String {
    let mut hash = Sha256::new();
    hash.update(family_id.as_bytes());
    let percent = hash.finalize()[0] % 100;
    if percent < config.test_percent {
        "test".to_string()
    } else if percent < config.test_percent + config.validation_percent {
        "validation".to_string()
    } else {
        "train".to_string()
    }
}

fn choose_bucket(width: u32, height: u32) -> Option<ResolutionBucket> {
    let source_ratio = width as f64 / height as f64;
    RESOLUTION_BUCKETS
        .iter()
        .flat_map(|&(bucket_width, bucket_height)| {
            [(bucket_width, bucket_height), (bucket_height, bucket_width)]
        })
        .filter_map(|(bucket_width, bucket_height)| {
            let upscale_ratio =
                (bucket_width as f64 / width as f64).max(bucket_height as f64 / height as f64);
            (upscale_ratio <= MAX_UPSCALE_RATIO).then(|| {
                let aspect_distance =
                    (source_ratio.ln() - (bucket_width as f64 / bucket_height as f64).ln()).abs();
                (
                    aspect_distance,
                    ResolutionBucket {
                        width: bucket_width,
                        height: bucket_height,
                        upscale_ratio,
                    },
                )
            })
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, bucket)| bucket)
}

fn detection_rect(detection: &AnimeCropBox, width: u32, height: u32) -> Option<CropRect> {
    if !detection.score.is_finite() || detection.score < SMART_CROP_MIN_SCORE {
        return None;
    }
    let x0 = detection.x0.floor().max(0.0).min(width as f32) as u32;
    let y0 = detection.y0.floor().max(0.0).min(height as f32) as u32;
    let x1 = detection.x1.ceil().max(0.0).min(width as f32) as u32;
    let y1 = detection.y1.ceil().max(0.0).min(height as f32) as u32;
    (x1 > x0 && y1 > y0).then_some(CropRect { x0, y0, x1, y1 })
}

fn expand_crop(
    rect: CropRect,
    padding_x: f32,
    padding_y: f32,
    width: u32,
    height: u32,
) -> CropRect {
    let pad_x = (rect.width() as f32 * padding_x).round() as i64;
    let pad_y = (rect.height() as f32 * padding_y).round() as i64;
    CropRect {
        x0: (i64::from(rect.x0) - pad_x).max(0) as u32,
        y0: (i64::from(rect.y0) - pad_y).max(0) as u32,
        x1: (i64::from(rect.x1) + pad_x).min(i64::from(width)) as u32,
        y1: (i64::from(rect.y1) + pad_y).min(i64::from(height)) as u32,
    }
}

fn is_near_subject(rect: CropRect, subject: CropRect) -> bool {
    subject.contains(rect) || subject.iou(rect) > 0.01 || {
        let expanded = expand_crop(subject, 0.15, 0.15, u32::MAX, u32::MAX);
        expanded.contains(rect)
    }
}

fn crop_keeps_critical_parts(rect: CropRect, critical: &[CropRect]) -> bool {
    critical.iter().all(|part| rect.contains(*part))
}

fn expand_crop_to_minimum_size(
    rect: CropRect,
    minimum: u32,
    width: u32,
    height: u32,
) -> CropRect {
    let target_width = rect.width().max(minimum).min(width);
    let target_height = rect.height().max(minimum).min(height);
    let center_x = (u64::from(rect.x0) + u64::from(rect.x1)) / 2;
    let center_y = (u64::from(rect.y0) + u64::from(rect.y1)) / 2;
    let mut x0 = center_x.saturating_sub(u64::from(target_width / 2)) as u32;
    let mut y0 = center_y.saturating_sub(u64::from(target_height / 2)) as u32;
    x0 = x0.min(width.saturating_sub(target_width));
    y0 = y0.min(height.saturating_sub(target_height));
    CropRect {
        x0,
        y0,
        x1: x0 + target_width,
        y1: y0 + target_height,
    }
}

fn expand_crop_for_native_bucket(
    rect: CropRect,
    candidate_kind: &'static str,
    minimum_short_side: u32,
    width: u32,
    height: u32,
    keep_bottom: bool,
) -> CropRect {
    let current = CropCandidate {
        kind: candidate_kind,
        rect,
        score: 0.0,
    };
    if rect.width() >= minimum_short_side
        && rect.height() >= minimum_short_side
        && choose_bucket(rect.width(), rect.height()).is_some()
        && composition_aspect_is_valid(current)
    {
        return rect;
    }
    RESOLUTION_BUCKETS
        .iter()
        .flat_map(|&(bucket_width, bucket_height)| {
            [(bucket_width, bucket_height), (bucket_height, bucket_width)]
        })
        .filter_map(|(bucket_width, bucket_height)| {
            let required_width = ((f64::from(bucket_width) / MAX_UPSCALE_RATIO).ceil() as u32)
                .max(minimum_short_side)
                .max(rect.width());
            let required_height = ((f64::from(bucket_height) / MAX_UPSCALE_RATIO).ceil() as u32)
                .max(minimum_short_side)
                .max(rect.height());
            if required_width > width || required_height > height {
                return None;
            }
            let center_x = (u64::from(rect.x0) + u64::from(rect.x1)) / 2;
            let mut x0 = center_x.saturating_sub(u64::from(required_width / 2)) as u32;
            x0 = x0.min(width - required_width);
            let mut y0 = if keep_bottom {
                rect.y1.saturating_sub(required_height)
            } else {
                let center_y = (u64::from(rect.y0) + u64::from(rect.y1)) / 2;
                center_y.saturating_sub(u64::from(required_height / 2)) as u32
            };
            y0 = y0.min(height - required_height);
            let expanded = CropRect {
                x0,
                y0,
                x1: x0 + required_width,
                y1: y0 + required_height,
            };
            let candidate = CropCandidate {
                kind: candidate_kind,
                rect: expanded,
                score: 0.0,
            };
            (choose_bucket(expanded.width(), expanded.height()).is_some()
                && composition_aspect_is_valid(candidate))
            .then_some(expanded)
        })
        .min_by_key(|candidate| u64::from(candidate.width()) * u64::from(candidate.height()))
        .unwrap_or(rect)
}

fn crop_does_not_clip_parts(rect: CropRect, protected: &[CropRect]) -> bool {
    protected.iter().all(|part| {
        let overlaps = rect.x0 < part.x1
            && rect.x1 > part.x0
            && rect.y0 < part.y1
            && rect.y1 > part.y0;
        !overlaps || rect.contains(*part)
    })
}

fn crop_excludes_other_people(rect: CropRect, others: &[CropRect]) -> bool {
    others.iter().all(|other| {
        let x0 = rect.x0.max(other.x0);
        let y0 = rect.y0.max(other.y0);
        let x1 = rect.x1.min(other.x1);
        let y1 = rect.y1.min(other.y1);
        let intersection = x1.saturating_sub(x0) as f32 * y1.saturating_sub(y0) as f32;
        let other_area = other.width() as f32 * other.height() as f32;
        other_area <= 0.0 || intersection / other_area < 0.05
    })
}

fn complete_pose_for_subject(
    analysis: &AnimeCropAnalysis,
    subject: CropRect,
    width: u32,
    height: u32,
) -> Option<(&AnimeCropPose, CropRect)> {
    analysis
        .poses
        .iter()
        .filter(|pose| pose.torso_score >= 0.20)
        .filter_map(|pose| {
            let bbox = detection_rect(&pose.bbox, width, height)?;
            let left_ankle = reliable_point(&pose.keypoints.left_ankle, 0.25, width, height);
            let right_ankle = reliable_point(&pose.keypoints.right_ankle, 0.25, width, height);
            let left_foot = complete_foot_side(pose, true, width, height);
            let right_foot = complete_foot_side(pose, false, width, height);
            (left_ankle.is_some()
                && right_ankle.is_some()
                && (left_foot.is_some() || right_foot.is_some()))
            .then_some((pose, bbox))
        })
        .filter(|(_, pose_box)| {
            subject.iou(*pose_box) >= 0.20
                || subject.contains(*pose_box)
                || pose_box.contains(subject)
        })
        .max_by(|(_, left), (_, right)| subject.iou(*left).total_cmp(&subject.iou(*right)))
}

fn associated_pose_for_subject<'a>(
    analysis: &'a AnimeCropAnalysis,
    subject: CropRect,
    width: u32,
    height: u32,
) -> Option<&'a AnimeCropPose> {
    analysis
        .poses
        .iter()
        .filter_map(|pose| detection_rect(&pose.bbox, width, height).map(|bbox| (pose, bbox)))
        .filter(|(_, bbox)| {
            subject.iou(*bbox) >= 0.20 || subject.contains(*bbox) || bbox.contains(subject)
        })
        .max_by(|(_, left), (_, right)| subject.iou(*left).total_cmp(&subject.iou(*right)))
        .map(|(pose, _)| pose)
}

fn reliable_point(
    point: &Option<AnimeCropPoint>,
    min_score: f32,
    width: u32,
    height: u32,
) -> Option<CropRect> {
    let point = point.as_ref()?;
    if !point.x.is_finite()
        || !point.y.is_finite()
        || !point.score.is_finite()
        || point.score < min_score
        || point.x < 0.0
        || point.y < 0.0
        || point.x >= width as f32
        || point.y >= height as f32
    {
        return None;
    }
    let radius = ((width.min(height) as f32 * 0.008).round() as u32).max(2);
    let x = point.x.round() as u32;
    let y = point.y.round() as u32;
    Some(CropRect {
        x0: x.saturating_sub(radius),
        y0: y.saturating_sub(radius),
        x1: x.saturating_add(radius).min(width),
        y1: y.saturating_add(radius).min(height),
    })
}

fn complete_lower_side(
    pose: &AnimeCropPose,
    left: bool,
    width: u32,
    height: u32,
) -> Option<Vec<CropRect>> {
    let points = if left {
        [
            &pose.keypoints.left_hip,
            &pose.keypoints.left_knee,
            &pose.keypoints.left_ankle,
        ]
    } else {
        [
            &pose.keypoints.right_hip,
            &pose.keypoints.right_knee,
            &pose.keypoints.right_ankle,
        ]
    };
    points
        .into_iter()
        .enumerate()
        .map(|(index, point)| {
            reliable_point(
                point,
                if index == 0 { 0.20 } else { 0.25 },
                width,
                height,
            )
        })
        .collect::<Option<Vec<_>>>()
}

fn complete_foot_side(
    pose: &AnimeCropPose,
    left: bool,
    width: u32,
    height: u32,
) -> Option<Vec<CropRect>> {
    let (ankle, big_toe, small_toe, heel) = if left {
        (
            &pose.keypoints.left_ankle,
            &pose.keypoints.left_big_toe,
            &pose.keypoints.left_small_toe,
            &pose.keypoints.left_heel,
        )
    } else {
        (
            &pose.keypoints.right_ankle,
            &pose.keypoints.right_big_toe,
            &pose.keypoints.right_small_toe,
            &pose.keypoints.right_heel,
        )
    };
    let ankle = reliable_point(ankle, 0.25, width, height)?;
    let heel = reliable_point(heel, 0.28, width, height)?;
    let toe = reliable_point(big_toe, 0.30, width, height)
        .or_else(|| reliable_point(small_toe, 0.30, width, height))?;
    let edge_margin = ((width.min(height) as f32 * 0.01).round() as u32).max(8);
    if [heel, toe].iter().any(|point| {
        point.x0 <= edge_margin
            || point.y0 <= edge_margin
            || point.x1.saturating_add(edge_margin) >= width
            || point.y1.saturating_add(edge_margin) >= height
    }) {
        return None;
    }
    let center = |rect: CropRect| {
        (
            (f64::from(rect.x0) + f64::from(rect.x1)) / 2.0,
            (f64::from(rect.y0) + f64::from(rect.y1)) / 2.0,
        )
    };
    let (heel_x, heel_y) = center(heel);
    let (toe_x, toe_y) = center(toe);
    let foot_length = ((heel_x - toe_x).powi(2) + (heel_y - toe_y).powi(2)).sqrt();
    if foot_length < f64::from(width.min(height)).mul_add(0.015, 0.0).max(24.0) {
        return None;
    }
    let mut evidence = vec![ankle, heel, toe];
    if let Some(other_toe) = reliable_point(small_toe, 0.30, width, height) {
        evidence.push(other_toe);
    }
    Some(evidence)
}

fn crop_excludes_other_pose_lower_parts(
    rect: CropRect,
    analysis: &AnimeCropAnalysis,
    primary_pose: &AnimeCropPose,
    width: u32,
    height: u32,
) -> bool {
    analysis
        .poses
        .iter()
        .filter(|pose| !std::ptr::eq(*pose, primary_pose))
        .flat_map(|pose| {
            [
                &pose.keypoints.left_knee,
                &pose.keypoints.right_knee,
                &pose.keypoints.left_ankle,
                &pose.keypoints.right_ankle,
                &pose.keypoints.left_big_toe,
                &pose.keypoints.right_big_toe,
                &pose.keypoints.left_small_toe,
                &pose.keypoints.right_small_toe,
                &pose.keypoints.left_heel,
                &pose.keypoints.right_heel,
            ]
        })
        .filter_map(|point| reliable_point(point, 0.35, width, height))
        .all(|part| {
            rect.x1 <= part.x0 || rect.x0 >= part.x1 || rect.y1 <= part.y0 || rect.y0 >= part.y1
        })
}

fn rect_area_ratio(rect: CropRect, source_width: u32, source_height: u32) -> f32 {
    let source_area = source_width as f32 * source_height as f32;
    if source_area > 0.0 {
        rect.width() as f32 * rect.height() as f32 / source_area
    } else {
        1.0
    }
}

fn composition_aspect_is_valid(candidate: CropCandidate) -> bool {
    let aspect = candidate.rect.width() as f32 / candidate.rect.height().max(1) as f32;
    match candidate.kind {
        "portrait" => aspect <= 0.95,
        "upper_body" => aspect <= 1.10,
        "cowboy_shot" => aspect <= 1.05,
        "full_body_tight" => aspect <= 1.25,
        "lower_body" => aspect <= 1.20,
        "feet" => aspect <= 2.40,
        _ => true,
    }
}

fn composition_area_limit(kind: &str) -> f32 {
    match kind {
        "portrait" => MAX_PORTRAIT_AREA_RATIO,
        "upper_body" => MAX_UPPER_BODY_AREA_RATIO,
        "cowboy_shot" => MAX_COWBOY_SHOT_AREA_RATIO,
        "full_body_tight" => MAX_FULL_BODY_TIGHT_AREA_RATIO,
        "lower_body" => MAX_LOWER_BODY_AREA_RATIO,
        "feet" => MAX_FEET_AREA_RATIO,
        _ => MAX_FULL_BODY_TIGHT_AREA_RATIO,
    }
}

fn candidate_is_native_and_safe(
    candidate: CropCandidate,
    critical: &[CropRect],
    config: &DatasetAugmentationConfig,
    source_width: u32,
    source_height: u32,
) -> bool {
    candidate.rect.width() >= config.min_short_side
        && candidate.rect.height() >= config.min_short_side
        && choose_bucket(candidate.rect.width(), candidate.rect.height()).is_some()
        && composition_aspect_is_valid(candidate)
        && crop_keeps_critical_parts(candidate.rect, critical)
        && rect_area_ratio(candidate.rect, source_width, source_height)
            < composition_area_limit(candidate.kind)
}

fn deduplicate_crop_candidates_with_rejections(
    mut candidates: Vec<CropCandidate>,
    mut rejections: BTreeMap<&'static str, SmartCropRejection>,
) -> SmartCropCandidateEvaluation {
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut distinct = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if distinct.iter().any(|existing: &CropCandidate| {
            candidate.rect.iou(existing.rect) >= MAX_DUPLICATE_CROP_IOU
        }) {
            rejections.insert(
                candidate.kind,
                (
                    "duplicate_composition",
                    "候选框与更高分构图近乎相同，已去除重复派生图",
                ),
            );
            continue;
        }
        distinct.push(candidate);
    }
    SmartCropCandidateEvaluation {
        candidates: distinct,
        rejections,
    }
}

fn smart_crop_candidate_evaluation(
    analysis: &AnimeCropAnalysis,
    width: u32,
    height: u32,
    config: &DatasetAugmentationConfig,
) -> SmartCropCandidateEvaluation {
    let candidates = smart_crop_candidates(analysis, width, height, config);
    let mut rejections = BTreeMap::new();
    for variant in enabled_smart_crop_variants(&config.smart_crop) {
        if !candidates.iter().any(|candidate| candidate.kind == variant) {
            rejections.insert(
                variant,
                smart_crop_rejection_reason(variant, analysis, width, height, config),
            );
        }
    }
    SmartCropCandidateEvaluation {
        candidates,
        rejections,
    }
}

fn deduplicate_crop_candidates(candidates: Vec<CropCandidate>) -> Vec<CropCandidate> {
    deduplicate_crop_candidates_with_rejections(candidates, BTreeMap::new()).candidates
}

fn smart_crop_rejection_reason(
    variant: &str,
    analysis: &AnimeCropAnalysis,
    width: u32,
    height: u32,
    config: &DatasetAugmentationConfig,
) -> (&'static str, &'static str) {
    let mut persons = analysis
        .persons
        .iter()
        .filter_map(|item| detection_rect(item, width, height).map(|rect| (rect, item.score)))
        .collect::<Vec<_>>();
    persons.sort_by(|left, right| right.1.total_cmp(&left.1));
    let Some((subject, _)) = persons.first().copied() else {
        return ("no_primary_person", "未检测到高置信主人物");
    };
    if persons
        .iter()
        .skip(1)
        .any(|(other, score)| *score >= SMART_CROP_MIN_SCORE && subject.iou(*other) >= 0.12)
    {
        return (
            "ambiguous_overlapping_people",
            "多人主体严重重叠，无法可靠关联身份",
        );
    }
    if matches!(variant, "portrait" | "upper_body" | "cowboy_shot") {
        let has_head_or_face = analysis
            .heads
            .iter()
            .chain(analysis.faces.iter())
            .filter_map(|item| detection_rect(item, width, height))
            .any(|rect| is_near_subject(rect, subject));
        if !has_head_or_face {
            return ("no_head_or_face", "没有与主人物关联的高置信头部或人脸");
        }
    }
    if variant == "full_body_tight"
        && complete_pose_for_subject(analysis, subject, width, height).is_none()
    {
        return ("incomplete_pose", "主人物躯干或脚踝姿态证据不完整");
    }
    if matches!(variant, "lower_body" | "feet") {
        let Some(pose) = associated_pose_for_subject(analysis, subject, width, height) else {
            return ("incomplete_pose", "没有与主人物可靠关联的姿态");
        };
        let left_foot = complete_foot_side(pose, true, width, height);
        let right_foot = complete_foot_side(pose, false, width, height);
        if variant == "lower_body"
            && (complete_lower_side(pose, true, width, height).is_none()
                || complete_lower_side(pose, false, width, height).is_none()
                || left_foot.is_none()
                || right_foot.is_none())
        {
            return (
                "lower_body_evidence_missing",
                "双髋、双膝、双踝或完整双脚证据不足",
            );
        }
        if variant == "feet" {
            if config.smart_crop.require_both_feet
                && (left_foot.is_none() || right_foot.is_none())
            {
                return (
                    "complete_both_feet_required",
                    "已开启完整双脚规则，但左右脚没有同时完整可见",
                );
            }
            if left_foot.is_none() && right_foot.is_none() {
                return (
                    "feet_evidence_missing",
                    "没有检测到一只包含脚踝、脚跟和脚趾的完整脚",
                );
            }
        }
    }
    if width < config.min_short_side || height < config.min_short_side {
        return ("native_resolution_too_low", "候选裁剪原生分辨率不足");
    }
    if persons.len() > 1 {
        return ("secondary_person_included", "候选框无法可靠排除其他人物");
    }
    (
        "quality_rule_rejected",
        "候选未通过关键部位、构图比例、面积或去重质量规则",
    )
}

/// Builds and scores a maximum of six composition candidates.  The output is
/// intentionally one best crop per composition type; multiple detected people
/// with materially overlapping boxes are rejected to avoid silently making a
/// wrong identity choice in a LoRA family.
fn smart_crop_candidates(
    analysis: &AnimeCropAnalysis,
    width: u32,
    height: u32,
    config: &DatasetAugmentationConfig,
) -> Vec<CropCandidate> {
    let mut persons = analysis
        .persons
        .iter()
        .filter_map(|item| detection_rect(item, width, height).map(|rect| (rect, item.score)))
        .collect::<Vec<_>>();
    persons.sort_by(|left, right| right.1.total_cmp(&left.1));
    let Some((subject, subject_score)) = persons.first().copied() else {
        return Vec::new();
    };
    if persons
        .iter()
        .skip(1)
        .any(|(other, score)| *score >= SMART_CROP_MIN_SCORE && subject.iou(*other) >= 0.12)
    {
        return Vec::new();
    }
    let other_people = persons
        .iter()
        .skip(1)
        .filter(|(_, score)| *score >= SMART_CROP_MIN_SCORE)
        .map(|(rect, _)| *rect)
        .collect::<Vec<_>>();

    let heads = analysis
        .heads
        .iter()
        .filter_map(|item| detection_rect(item, width, height))
        .filter(|rect| is_near_subject(*rect, subject))
        .collect::<Vec<_>>();
    let faces = analysis
        .faces
        .iter()
        .filter_map(|item| detection_rect(item, width, height))
        .filter(|rect| is_near_subject(*rect, subject))
        .collect::<Vec<_>>();
    let hands = analysis
        .hands
        .iter()
        .filter_map(|item| detection_rect(item, width, height))
        .filter(|rect| is_near_subject(*rect, subject))
        .collect::<Vec<_>>();
    let half_body = analysis
        .half_bodies
        .iter()
        .filter_map(|item| detection_rect(item, width, height))
        .filter(|rect| is_near_subject(*rect, subject))
        .max_by_key(|rect| rect.width() * rect.height());
    let foreground = analysis
        .foreground
        .as_ref()
        .and_then(|item| detection_rect(item, width, height))
        .filter(|rect| is_near_subject(*rect, subject))
        .filter(|rect| {
            rect_area_ratio(*rect, width, height) < MAX_FOREGROUND_PROTECTION_AREA_RATIO
        });

    // Head and face evidence is required for upper-body compositions. Hands
    // may be wholly inside or outside a crop, but a boundary may not slice
    // through one. Lower-body compositions use their own pose evidence.
    let mut upper_critical = Vec::new();
    upper_critical.extend(heads.iter().copied());
    upper_critical.extend(faces.iter().copied());

    let head = heads
        .iter()
        .copied()
        .max_by_key(|rect| rect.width() * rect.height())
        .or_else(|| {
            faces
                .iter()
                .copied()
                .max_by_key(|rect| rect.width() * rect.height())
        });
    let mut candidates = Vec::new();
    if config.smart_crop.portrait && !upper_critical.is_empty() {
        if let Some(head) = head {
            let chest_limit = subject.y0 + subject.height() * 48 / 100;
            let chest_bottom = (head.y1 + head.height() / 2)
                .max(subject.y0 + subject.height() * 38 / 100)
                .min(chest_limit)
                .min(subject.y1);
            let base = CropRect {
                x0: head.x0,
                y0: head.y0,
                x1: head.x1,
                y1: chest_bottom,
            };
            let mut rect = expand_crop(base, 0.35, 0.08, width, height);
            rect.y1 = rect.y1.min(chest_limit);
            rect = expand_crop_for_native_bucket(
                rect,
                "portrait",
                config.min_short_side,
                width,
                height,
                true,
            );
            let candidate = CropCandidate {
                kind: "portrait",
                rect,
                score: subject_score + 0.35,
            };
            if candidate_is_native_and_safe(candidate, &upper_critical, config, width, height)
                && crop_does_not_clip_parts(candidate.rect, &hands)
                && crop_excludes_other_people(candidate.rect, &other_people)
            {
                candidates.push(candidate);
            }
        }
    }
    let associated_pose = associated_pose_for_subject(analysis, subject, width, height);
    if config.smart_crop.upper_body && !upper_critical.is_empty() {
        let anatomical_waist = associated_pose.and_then(|pose| {
            let left = reliable_point(&pose.keypoints.left_hip, 0.20, width, height)?;
            let right = reliable_point(&pose.keypoints.right_hip, 0.20, width, height)?;
            Some(((left.y0 + left.y1 + right.y0 + right.y1) / 4).min(subject.y1))
        });
        let waist_minimum = subject.y0 + subject.height() * 55 / 100;
        let hip_limit = subject.y0 + subject.height() * 68 / 100;
        let detected_bottom = half_body
            .map(|detected| detected.y1)
            .unwrap_or(subject.y0 + subject.height() * 60 / 100);
        let bottom = anatomical_waist.unwrap_or_else(|| detected_bottom.clamp(waist_minimum, hip_limit));
        let upper_x0 = upper_critical
            .iter()
            .map(|part| part.x0)
            .min()
            .unwrap_or(subject.x0);
        let upper_x1 = upper_critical
            .iter()
            .map(|part| part.x1)
            .max()
            .unwrap_or(subject.x1);
        let base = CropRect {
            x0: upper_x0,
            y0: subject.y0,
            x1: upper_x1,
            y1: bottom,
        };
        let mut rect = expand_crop(base, 0.14, 0.08, width, height);
        rect.y1 = anatomical_waist.unwrap_or_else(|| rect.y1.clamp(waist_minimum, hip_limit));
        rect = expand_crop_for_native_bucket(
            rect,
            "upper_body",
            config.min_short_side,
            width,
            height,
            true,
        );
        let candidate = CropCandidate {
            kind: "upper_body",
            rect,
            score: subject_score + 0.25,
        };
        if candidate_is_native_and_safe(candidate, &upper_critical, config, width, height)
            && crop_does_not_clip_parts(candidate.rect, &hands)
            && crop_excludes_other_people(candidate.rect, &other_people)
        {
            candidates.push(candidate);
        }
    }
    if config.smart_crop.cowboy_shot && !upper_critical.is_empty() {
        let mid_thigh = subject.y0 + subject.height() * 72 / 100;
        let knee_limit = subject.y0 + subject.height() * 88 / 100;
        let anatomical_thigh = associated_pose.and_then(|pose| {
            let left_hip = reliable_point(&pose.keypoints.left_hip, 0.20, width, height)?;
            let right_hip = reliable_point(&pose.keypoints.right_hip, 0.20, width, height)?;
            let left_knee = reliable_point(&pose.keypoints.left_knee, 0.25, width, height)?;
            let right_knee = reliable_point(&pose.keypoints.right_knee, 0.25, width, height)?;
            let hip_y = (left_hip.y0 + left_hip.y1 + right_hip.y0 + right_hip.y1) / 4;
            let knee_y = (left_knee.y0 + left_knee.y1 + right_knee.y0 + right_knee.y1) / 4;
            (knee_y > hip_y).then_some(hip_y + (knee_y - hip_y) * 3 / 4)
        });
        let base = CropRect {
            x0: subject.x0,
            y0: subject.y0,
            x1: subject.x1,
            y1: anatomical_thigh.unwrap_or(subject.y0 + subject.height() * 82 / 100),
        };
        let mut rect = expand_crop(base, 0.10, 0.04, width, height);
        rect.y1 = anatomical_thigh.unwrap_or_else(|| rect.y1.clamp(mid_thigh, knee_limit));
        rect = expand_crop_for_native_bucket(
            rect,
            "cowboy_shot",
            config.min_short_side,
            width,
            height,
            true,
        );
        let candidate = CropCandidate {
            kind: "cowboy_shot",
            rect,
            score: subject_score + 0.20,
        };
        if candidate_is_native_and_safe(candidate, &upper_critical, config, width, height)
            && crop_does_not_clip_parts(candidate.rect, &hands)
            && crop_excludes_other_people(candidate.rect, &other_people)
        {
            candidates.push(candidate);
        }
    }
    if config.smart_crop.full_body_tight {
        if let Some((pose_record, pose)) =
            complete_pose_for_subject(analysis, subject, width, height)
        {
            // Person detectors deliberately return loose, and often
            // edge-clamped, boxes. Use the associated complete pose plus
            // protected visual parts instead of its background padding.
            let base = upper_critical
                .iter()
                .copied()
                .chain(hands.iter().copied())
                .fold(pose, CropRect::union);
            let base = foreground
                .filter(|mask| {
                    rect_area_ratio(base.union(*mask), width, height)
                        < MAX_FULL_BODY_TIGHT_AREA_RATIO
                })
                .map(|mask| base.union(mask))
                .unwrap_or(base);
            let rect = expand_crop(base, 0.08, 0.03, width, height);
            let rect = expand_crop_for_native_bucket(
                rect,
                "full_body_tight",
                config.min_short_side,
                width,
                height,
                false,
            );
            let mut full_critical = upper_critical.clone();
            full_critical.extend(hands.iter().copied());
            full_critical.push(pose);
            let candidate = CropCandidate {
                kind: "full_body_tight",
                rect,
                score: subject_score + 0.15,
            };
            if candidate_is_native_and_safe(candidate, &full_critical, config, width, height)
                && crop_excludes_other_people(candidate.rect, &other_people)
                && crop_excludes_other_pose_lower_parts(
                    candidate.rect,
                    analysis,
                    pose_record,
                    width,
                    height,
                )
            {
                candidates.push(candidate);
            }
        }
    }
    if let Some(pose) = associated_pose {
        if config.smart_crop.lower_body {
            let left_leg = complete_lower_side(pose, true, width, height);
            let right_leg = complete_lower_side(pose, false, width, height);
            let left_foot = complete_foot_side(pose, true, width, height);
            let right_foot = complete_foot_side(pose, false, width, height);
            if let (Some(mut left_leg), Some(right_leg), Some(left_foot), Some(right_foot)) =
                (left_leg, right_leg, left_foot, right_foot)
            {
                left_leg.extend(right_leg);
                left_leg.extend(left_foot);
                left_leg.extend(right_foot);
                let lower_critical = left_leg;
                let mut base = lower_critical
                    .iter()
                    .copied()
                    .reduce(CropRect::union)
                    .expect("complete lower-body evidence is non-empty");
                base.x0 = subject.x0;
                base.x1 = subject.x1;
                let rect = expand_crop(base, 0.12, 0.08, width, height);
                let rect = expand_crop_for_native_bucket(
                    rect,
                    "lower_body",
                    config.min_short_side,
                    width,
                    height,
                    true,
                );
                let candidate = CropCandidate {
                    kind: "lower_body",
                    rect,
                    score: subject_score + 0.12,
                };
                if candidate_is_native_and_safe(
                    candidate,
                    &lower_critical,
                    config,
                    width,
                    height,
                ) && crop_does_not_clip_parts(candidate.rect, &hands)
                    && crop_excludes_other_people(candidate.rect, &other_people)
                    && crop_excludes_other_pose_lower_parts(
                        candidate.rect,
                        analysis,
                        pose,
                        width,
                        height,
                    )
                {
                    candidates.push(candidate);
                }
            }
        }
        if config.smart_crop.feet {
            let left = complete_foot_side(pose, true, width, height);
            let right = complete_foot_side(pose, false, width, height);
            let evidence = match (left, right, config.smart_crop.require_both_feet) {
                (Some(mut left), Some(right), _) => {
                    left.extend(right);
                    Some(left)
                }
                (Some(left), None, false) => Some(left),
                (None, Some(right), false) => Some(right),
                _ => None,
            };
            if let Some(feet_critical) = evidence {
                let base = feet_critical
                    .iter()
                    .copied()
                    .reduce(CropRect::union)
                    .expect("complete feet evidence is non-empty");
                let rect = expand_crop(base, 0.55, 0.35, width, height);
                let rect = expand_crop_to_minimum_size(
                    rect,
                    config.min_short_side,
                    width,
                    height,
                );
                let rect = expand_crop_for_native_bucket(
                    rect,
                    "feet",
                    config.min_short_side,
                    width,
                    height,
                    true,
                );
                let candidate = CropCandidate {
                    kind: "feet",
                    rect,
                    score: subject_score + 0.10,
                };
                if candidate_is_native_and_safe(
                    candidate,
                    &feet_critical,
                    config,
                    width,
                    height,
                ) && crop_does_not_clip_parts(candidate.rect, &hands)
                    && crop_excludes_other_people(candidate.rect, &other_people)
                    && crop_excludes_other_pose_lower_parts(
                        candidate.rect,
                        analysis,
                        pose,
                        width,
                        height,
                    )
                {
                    candidates.push(candidate);
                }
            }
        }
    }
    deduplicate_crop_candidates(candidates)
}

fn link_or_copy_new_file(source: &Path, destination: &Path) -> Result<(), ToolError> {
    if destination.exists() {
        return Err(ToolError::Conflict(destination.to_path_buf()));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| ToolError::InvalidManifest("训练样本缺少父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(ToolError::Io)?;
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination).map_err(ToolError::Io)?;
            Ok(())
        }
    }
}

fn write_png_atomic(destination: &Path, image: &image::DynamicImage) -> Result<(), ToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| ToolError::InvalidManifest("输出文件缺少父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(ToolError::Io)?;
    let temporary = destination.with_extension(format!("png.tmp-{}", uuid::Uuid::new_v4()));
    let mut file = File::create(&temporary).map_err(ToolError::Io)?;
    let rgba = image.to_rgba8();
    PngEncoder::new(&mut file)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(ToolError::Image)?;
    file.flush().map_err(ToolError::Io)?;
    fs::rename(temporary, destination).map_err(ToolError::Io)
}

fn write_json_atomic(destination: &Path, value: &impl Serialize) -> Result<(), ToolError> {
    let content = serde_json::to_vec_pretty(value)
        .map_err(|error| ToolError::InvalidManifest(error.to_string()))?;
    let temporary = destination.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    fs::write(&temporary, content).map_err(ToolError::Io)?;
    fs::rename(temporary, destination).map_err(ToolError::Io)
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_is_native_and_safe, choose_bucket, crop_does_not_clip_parts,
        deduplicate_crop_candidates, enabled_smart_crop_variants,
        smart_crop_candidates, split_for_family, AnimeCropAnalysis, AnimeCropBox, AnimeCropPoint,
        AnimeCropPose, AnimeCropPoseKeypoints, CropCandidate, CropRect,
        DatasetAugmentationConfig, DatasetAugmentationItemResult, DatasetAugmentationSource,
        DatasetAugmentationWorkspace, SmartCropConfig,
    };
    use crate::services::image_processor::VerifiedMediaRoot;
    use image::RgbImage;
    use std::path::PathBuf;

    #[test]
    fn bucket_selection_never_requires_non_uniform_scaling() {
        let bucket = choose_bucket(2048, 3072).expect("native image should match a bucket");
        assert!((bucket.width as f64 / bucket.height as f64 - 2.0 / 3.0).abs() < 0.001);
        assert!(bucket.upscale_ratio <= 1.0);
    }

    #[test]
    fn family_split_is_stable_for_every_derivative() {
        let config = DatasetAugmentationConfig::default();
        assert_eq!(
            split_for_family("family_01", &config),
            split_for_family("family_01", &config)
        );
    }

    #[test]
    fn split_percentages_must_total_one_hundred() {
        let config = DatasetAugmentationConfig {
            train_percent: 80,
            validation_percent: 10,
            test_percent: 9,
            ..DatasetAugmentationConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn workspace_keeps_sources_immutable_and_never_copies_original_tags_to_flips() {
        let temporary = tempfile::tempdir().unwrap();
        let image_path = temporary.path().join("source.png");
        RgbImage::new(400, 400).save(&image_path).unwrap();
        std::fs::write(temporary.path().join("source.txt"), "1girl, blue_hair").unwrap();
        let config = DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            horizontal_flip: true,
            ..DatasetAugmentationConfig::default()
        };
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace = DatasetAugmentationWorkspace::create(root, "task-1", config).unwrap();

        let result = workspace
            .process(&DatasetAugmentationSource {
                media_id: "media-1".to_string(),
                relative_path: PathBuf::from("source.png"),
                sha256: Some("source-hash".to_string()),
                fallback_caption: String::new(),
            })
            .unwrap();

        let DatasetAugmentationItemResult::Generated(samples) = result else {
            panic!("source should qualify for augmentation");
        };
        assert_eq!(samples.len(), 1);
        assert!(samples[0].requires_retagging);
        assert!(temporary.path().join("source.png").is_file());
        let output = temporary.path().join(".augmentation/task-1");
        let metadata_output = temporary.path().join(".augmentation-metadata/task-1");
        assert!(!output.join("raw/images/media1.png").exists());
        assert!(output
            .join("derived/horizontal_flip/images/media1_horizontal_flip.png")
            .is_file());
        assert_eq!(
            &std::fs::read(
                output.join("derived/horizontal_flip/images/media1_horizontal_flip.png"),
            )
            .unwrap()[..8],
            b"\x89PNG\r\n\x1a\n"
        );
        assert!(!output
            .join("derived/horizontal_flip/images/media1_horizontal_flip.txt")
            .exists());
        assert!(!output
            .join("derived/horizontal_flip/labels/media1_horizontal_flip.txt")
            .exists());
        assert!(!output
            .join("ready")
            .join(&samples[0].split)
            .join("horizontal_flip/images")
            .join("media1_horizontal_flip.txt")
            .exists());
        let summary = workspace.finish().unwrap();
        assert!(!output.join("ready").exists());
        assert_eq!(summary.retagging_pending, 1);
        let metadata =
            std::fs::read_to_string(metadata_output.join("metadata/dataset.jsonl")).unwrap();
        assert!(metadata.contains("\"allow_non_uniform_scaling\":false"));
        assert!(metadata.contains("\"requires_retagging\":true"));
        let retagging =
            std::fs::read_to_string(metadata_output.join("metadata/retagging.jsonl")).unwrap();
        assert!(retagging.contains("media1_horizontal_flip"));
    }

    #[test]
    fn workspace_keeps_originals_in_place_and_separates_augmentation_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let source_directory = temporary.path().join("characters/alice");
        std::fs::create_dir_all(&source_directory).unwrap();
        RgbImage::new(800, 800)
            .save(source_directory.join("source.png"))
            .unwrap();
        std::fs::write(source_directory.join("source.txt"), "1girl").unwrap();
        let config = DatasetAugmentationConfig {
            output_directory: PathBuf::from("characters/alice/.augmentation"),
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            horizontal_flip: true,
            smart_crop: super::SmartCropConfig {
                enabled: false,
                ..super::SmartCropConfig::default()
            },
            ..DatasetAugmentationConfig::default()
        };
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace =
            DatasetAugmentationWorkspace::create(root, "task-layout", config).unwrap();

        let DatasetAugmentationItemResult::Generated(samples) = workspace
            .process(&DatasetAugmentationSource {
                media_id: "media-layout".to_string(),
                relative_path: PathBuf::from("characters/alice/source.png"),
                sha256: None,
                fallback_caption: String::new(),
            })
            .unwrap()
        else {
            panic!("source should qualify for augmentation");
        };

        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].variant, "horizontal_flip");
        let images = temporary
            .path()
            .join("characters/alice/.augmentation/task-layout");
        let metadata = temporary
            .path()
            .join("characters/alice/.augmentation-metadata/task-layout");
        assert!(!images.join("raw").exists());
        assert!(!images.join("original").exists());
        assert!(metadata.join("metadata/dataset.jsonl").is_file());
        assert!(metadata.join("INCOMPLETE.json").is_file());
    }

    #[test]
    fn workspace_prepares_a_dedicated_cowboy_shot_subset_without_copying_originals() {
        let temporary = tempfile::tempdir().unwrap();
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();

        DatasetAugmentationWorkspace::create(
            root,
            "task-cowboy",
            DatasetAugmentationConfig::default(),
        )
        .unwrap();

        assert!(temporary
            .path()
            .join(".augmentation/task-cowboy/derived/cowboy_shot/images")
            .is_dir());
        assert!(temporary
            .path()
            .join(".augmentation/task-cowboy/derived/lower_body/images")
            .is_dir());
        assert!(temporary
            .path()
            .join(".augmentation/task-cowboy/derived/feet/images")
            .is_dir());
        assert!(!temporary
            .path()
            .join(".augmentation/task-cowboy/original")
            .exists());
    }

    #[test]
    fn workspace_records_one_explainable_decision_for_each_enabled_crop_type() {
        let temporary = tempfile::tempdir().unwrap();
        RgbImage::new(1200, 1600)
            .save(temporary.path().join("source.png"))
            .unwrap();
        let mut config = DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            ..DatasetAugmentationConfig::default()
        };
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.lower_body = false;
        config.smart_crop.feet = false;
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace =
            DatasetAugmentationWorkspace::create(root, "task-decisions", config).unwrap();
        let source = DatasetAugmentationSource {
            media_id: "media-decisions".to_string(),
            relative_path: PathBuf::from("source.png"),
            sha256: None,
            fallback_caption: String::new(),
        };
        let analysis = AnimeCropAnalysis {
            media_id: source.media_id.clone(),
            width: 1200,
            height: 1600,
            persons: vec![crop_box(300.0, 80.0, 900.0, 1500.0)],
            heads: vec![crop_box(450.0, 120.0, 750.0, 480.0)],
            faces: vec![crop_box(500.0, 200.0, 700.0, 400.0)],
            ..AnimeCropAnalysis::default()
        };

        workspace
            .process_with_analysis(&source, Some(&analysis))
            .unwrap();
        let summary = workspace.finish().unwrap();

        assert_eq!(summary.smart_crop_by_variant["portrait"].requested, 1);
        assert_eq!(summary.smart_crop_by_variant["portrait"].generated, 1);
        assert_eq!(summary.smart_crop_by_variant["full_body_tight"].requested, 1);
        assert_eq!(summary.smart_crop_by_variant["full_body_tight"].rejected, 1);
        assert_eq!(
            summary.smart_crop_by_variant["full_body_tight"].rejection_reasons["incomplete_pose"],
            1
        );
        let evaluations = std::fs::read_to_string(
            temporary
                .path()
                .join(".augmentation-metadata/task-decisions/metadata/smart-crop-evaluations.jsonl"),
        )
        .unwrap();
        assert_eq!(evaluations.lines().count(), 2);
        assert!(evaluations.contains("\"status\":\"generated\""));
        assert!(evaluations.contains("\"reason_code\":\"incomplete_pose\""));
    }

    #[test]
    fn training_manifest_exposes_cowboy_shot_as_an_independent_repeatable_subset() {
        let temporary = tempfile::tempdir().unwrap();
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let workspace = DatasetAugmentationWorkspace::create(
            root,
            "task-cowboy-manifest",
            DatasetAugmentationConfig::default(),
        )
        .unwrap();

        let manifest = workspace.training_subsets_manifest();
        let cowboy = manifest["subsets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|subset| subset["id"] == "cowboy_shot")
            .expect("cowboy shot should be independently configurable for training");

        assert_eq!(cowboy["default_repeats"], 1);
        assert!(cowboy["relative_directory"]
            .as_str()
            .unwrap()
            .contains("cowboy_shot"));
        assert!(manifest["subsets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|subset| subset["id"] == "lower_body"));
        assert!(manifest["subsets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|subset| subset["id"] == "feet"));
    }

    #[test]
    fn workspace_does_not_create_an_original_sample_or_raw_copy() {
        let temporary = tempfile::tempdir().unwrap();
        RgbImage::new(800, 800)
            .save(temporary.path().join("source.png"))
            .unwrap();
        let config = DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            smart_crop: super::SmartCropConfig {
                enabled: false,
                ..super::SmartCropConfig::default()
            },
            ..DatasetAugmentationConfig::default()
        };
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace =
            DatasetAugmentationWorkspace::create(root, "task-original", config).unwrap();

        let DatasetAugmentationItemResult::Generated(samples) = workspace
            .process(&DatasetAugmentationSource {
                media_id: "media-original".to_string(),
                relative_path: PathBuf::from("source.png"),
                sha256: None,
                fallback_caption: String::new(),
            })
            .unwrap()
        else {
            panic!("source should complete even when no derived operation is enabled");
        };

        assert!(samples.is_empty());
        assert!(!temporary
            .path()
            .join(".augmentation/task-original/raw")
            .exists());
        assert!(!temporary
            .path()
            .join(".augmentation/task-original/derived/original")
            .exists());
    }

    #[test]
    fn workspace_writes_completion_state_to_separate_metadata_directory() {
        let temporary = tempfile::tempdir().unwrap();
        RgbImage::new(800, 800)
            .save(temporary.path().join("source.png"))
            .unwrap();
        std::fs::write(temporary.path().join("source.txt"), "1girl").unwrap();
        let config = DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            smart_crop: super::SmartCropConfig {
                enabled: false,
                ..super::SmartCropConfig::default()
            },
            ..DatasetAugmentationConfig::default()
        };
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace =
            DatasetAugmentationWorkspace::create(root, "task-ready", config).unwrap();
        let DatasetAugmentationItemResult::Generated(samples) = workspace
            .process(&DatasetAugmentationSource {
                media_id: "media-ready".to_string(),
                relative_path: PathBuf::from("source.png"),
                sha256: None,
                fallback_caption: String::new(),
            })
            .unwrap()
        else {
            panic!("source should qualify for augmentation");
        };

        let output = temporary.path().join(".augmentation/task-ready");
        let metadata_output = temporary.path().join(".augmentation-metadata/task-ready");
        assert!(!output.join("READY.json").exists());
        assert!(metadata_output.join("INCOMPLETE.json").is_file());
        let summary = workspace.finish().unwrap();
        assert!(samples.is_empty());
        assert!(metadata_output.join("READY.json").is_file());
        assert!(!metadata_output.join("INCOMPLETE.json").exists());
        assert_eq!(
            summary.training_relative_directory,
            PathBuf::from(".augmentation/task-ready/ready/train")
        );
    }

    #[test]
    fn retagged_derivative_is_promoted_as_a_bound_training_subset() {
        let temporary = tempfile::tempdir().unwrap();
        RgbImage::new(800, 800)
            .save(temporary.path().join("source.png"))
            .unwrap();
        std::fs::write(
            temporary.path().join("source.txt"),
            "artist:origin, character_a",
        )
        .unwrap();
        let config = DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 1,
            horizontal_flip: true,
            smart_crop: super::SmartCropConfig {
                enabled: false,
                ..super::SmartCropConfig::default()
            },
            ..DatasetAugmentationConfig::default()
        };
        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        let mut workspace =
            DatasetAugmentationWorkspace::create(root, "task-promote", config).unwrap();
        let DatasetAugmentationItemResult::Generated(samples) = workspace
            .process(&DatasetAugmentationSource {
                media_id: "media-promote".to_string(),
                relative_path: PathBuf::from("source.png"),
                sha256: None,
                fallback_caption: String::new(),
            })
            .unwrap()
        else {
            panic!("source should qualify for augmentation");
        };
        let derived = samples
            .iter()
            .find(|sample| sample.requires_retagging)
            .cloned()
            .unwrap();
        std::fs::write(
            temporary
                .path()
                .join(&derived.output_relative_path)
                .with_extension("txt"),
            "artist:origin,character_a,1girl,blue_hair",
        )
        .unwrap();

        workspace
            .promote_retagged_samples(&[derived.clone()])
            .unwrap();
        let summary = workspace.finish().unwrap();
        let ready = temporary
            .path()
            .join(".augmentation/task-promote/ready")
            .join(&derived.split)
            .join("horizontal_flip/images");
        assert!(ready.join(format!("{}.png", derived.sample_id)).is_file());
        assert_eq!(
            std::fs::read_to_string(ready.join(format!("{}.txt", derived.sample_id))).unwrap(),
            "artist:origin,character_a,1girl,blue_hair"
        );
        assert_eq!(summary.retagging_pending, 0);
        assert_eq!(summary.retagged, 1);
        let manifest = std::fs::read_to_string(
            temporary
                .path()
                .join(".augmentation-metadata/task-promote/metadata/training-subsets.json"),
        )
        .unwrap();
        assert!(manifest.contains("horizontal_flip"));
        assert!(manifest.contains("\"training_ready_count\": 1"));
    }

    fn crop_box(x0: f32, y0: f32, x1: f32, y1: f32) -> AnimeCropBox {
        AnimeCropBox {
            x0,
            y0,
            x1,
            y1,
            score: 0.95,
        }
    }

    fn complete_pose(x0: f32, y0: f32, x1: f32, y1: f32) -> AnimeCropPose {
        let point = |x_ratio: f32, y_ratio: f32| {
            crop_point(x0 + (x1 - x0) * x_ratio, y0 + (y1 - y0) * y_ratio)
        };
        AnimeCropPose {
            bbox: crop_box(x0, y0, x1, y1),
            torso_score: 0.95,
            left_ankle_score: 0.95,
            right_ankle_score: 0.95,
            keypoints: AnimeCropPoseKeypoints {
                left_hip: point(0.42, 0.50),
                right_hip: point(0.58, 0.50),
                left_knee: point(0.42, 0.74),
                right_knee: point(0.58, 0.74),
                left_ankle: point(0.41, 0.92),
                right_ankle: point(0.59, 0.92),
                left_big_toe: point(0.36, 0.98),
                right_big_toe: point(0.64, 0.98),
                left_small_toe: point(0.40, 0.99),
                right_small_toe: point(0.60, 0.99),
                left_heel: point(0.42, 0.96),
                right_heel: point(0.58, 0.96),
            },
        }
    }

    fn crop_point(x: f32, y: f32) -> Option<AnimeCropPoint> {
        Some(AnimeCropPoint { x, y, score: 0.95 })
    }

    fn scored_crop_point(x: f32, y: f32, score: f32) -> Option<AnimeCropPoint> {
        Some(AnimeCropPoint { x, y, score })
    }

    fn stylized_full_body_pose() -> AnimeCropPose {
        AnimeCropPose {
            bbox: crop_box(720.0, 520.0, 1720.0, 3010.0),
            torso_score: 0.27,
            left_ankle_score: 0.29,
            right_ankle_score: 0.42,
            keypoints: AnimeCropPoseKeypoints {
                left_hip: scored_crop_point(1280.0, 1500.0, 0.24),
                right_hip: scored_crop_point(1040.0, 1510.0, 0.27),
                left_knee: scored_crop_point(1450.0, 2110.0, 0.26),
                right_knee: scored_crop_point(1050.0, 2160.0, 0.35),
                left_ankle: scored_crop_point(1530.0, 2670.0, 0.29),
                right_ankle: scored_crop_point(1010.0, 2690.0, 0.42),
                left_big_toe: scored_crop_point(1600.0, 2960.0, 0.39),
                left_small_toe: scored_crop_point(1660.0, 2910.0, 0.44),
                right_big_toe: scored_crop_point(930.0, 2990.0, 0.60),
                right_small_toe: scored_crop_point(980.0, 2985.0, 0.61),
                left_heel: scored_crop_point(1510.0, 2760.0, 0.31),
                right_heel: scored_crop_point(1030.0, 2780.0, 0.39),
            },
        }
    }

    fn pose_with_left_foot_only() -> AnimeCropPose {
        AnimeCropPose {
            bbox: crop_box(560.0, 300.0, 1240.0, 2180.0),
            torso_score: 0.95,
            left_ankle_score: 0.95,
            right_ankle_score: 0.10,
            keypoints: AnimeCropPoseKeypoints {
                left_hip: crop_point(760.0, 1120.0),
                right_hip: crop_point(1040.0, 1120.0),
                left_knee: crop_point(760.0, 1600.0),
                right_knee: None,
                left_ankle: crop_point(750.0, 2030.0),
                right_ankle: None,
                left_big_toe: crop_point(700.0, 2160.0),
                left_small_toe: crop_point(780.0, 2165.0),
                left_heel: crop_point(750.0, 2100.0),
                ..AnimeCropPoseKeypoints::default()
            },
        }
    }

    fn crop_config() -> DatasetAugmentationConfig {
        DatasetAugmentationConfig {
            min_megapixels: 0.1,
            min_long_side: 1,
            min_short_side: 512,
            ..DatasetAugmentationConfig::default()
        }
    }

    #[test]
    fn smart_crop_defaults_enable_all_anime_compositions() {
        let config = DatasetAugmentationConfig::default();
        assert!(config.smart_crop.enabled);
        assert!(config.smart_crop.portrait);
        assert!(config.smart_crop.upper_body);
        assert!(config.smart_crop.cowboy_shot);
        assert!(config.smart_crop.full_body_tight);
        assert!(config.smart_crop.lower_body);
        assert!(config.smart_crop.feet);
        assert!(!config.smart_crop.require_both_feet);
        assert_eq!(config.smart_crop.max_derived_per_family, 6);
    }

    #[test]
    fn legacy_three_variant_config_does_not_silently_enable_and_starve_new_variants() {
        let legacy = serde_json::json!({
            "enabled": true,
            "runtime_profile_id": "conda:lora",
            "gpu_id": "0",
            "quality_profile": "anime-quality",
            "portrait": true,
            "upper_body": true,
            "full_body_tight": true,
            "max_derived_per_family": 3
        });

        let config: SmartCropConfig = serde_json::from_value(legacy).unwrap();

        assert!(config.portrait && config.upper_body && config.full_body_tight);
        assert!(!config.cowboy_shot && !config.lower_body && !config.feet);
        assert_eq!(enabled_smart_crop_variants(&config).len(), 3);
    }

    #[test]
    fn smart_crop_keeps_family_safe_candidates_and_uses_native_pixels() {
        let analysis = AnimeCropAnalysis {
            media_id: "source".to_string(),
            persons: vec![crop_box(450.0, 120.0, 1250.0, 1840.0)],
            heads: vec![crop_box(620.0, 180.0, 1050.0, 650.0)],
            faces: vec![crop_box(690.0, 280.0, 970.0, 540.0)],
            half_bodies: vec![crop_box(480.0, 120.0, 1210.0, 1210.0)],
            hands: vec![],
            poses: vec![complete_pose(500.0, 160.0, 1200.0, 1820.0)],
            ..AnimeCropAnalysis::default()
        };
        let candidates = smart_crop_candidates(&analysis, 1700, 2100, &crop_config());
        assert_eq!(
            candidates.len(),
            6,
            "generated variants: {:?}",
            candidates.iter().map(|candidate| candidate.kind).collect::<Vec<_>>()
        );
        assert!(candidates
            .iter()
            .all(|candidate| candidate.rect.width() >= 512));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.rect.height() >= 512));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "portrait"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "upper_body"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "cowboy_shot"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "full_body_tight"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "lower_body"));
        assert!(candidates.iter().any(|candidate| candidate.kind == "feet"));
    }

    #[test]
    fn portrait_crop_stops_at_the_chest_instead_of_becoming_a_cowboy_shot() {
        let subject = CropRect {
            x0: 450,
            y0: 100,
            x1: 1350,
            y1: 2200,
        };
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(450.0, 100.0, 1350.0, 2200.0)],
            heads: vec![crop_box(650.0, 160.0, 1100.0, 650.0)],
            faces: vec![crop_box(720.0, 250.0, 1030.0, 560.0)],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.full_body_tight = false;

        let portrait = smart_crop_candidates(&analysis, 1800, 2400, &config)
            .into_iter()
            .find(|candidate| candidate.kind == "portrait")
            .expect("a high-confidence head should produce a portrait crop");

        let chest_limit = subject.y0 + subject.height() * 48 / 100;
        assert!(
            portrait.rect.y1 <= chest_limit,
            "portrait bottom {} extended below chest limit {chest_limit}",
            portrait.rect.y1
        );
    }

    #[test]
    fn upper_body_crop_reaches_the_waist_when_half_body_detection_is_only_head_and_shoulders() {
        let subject = CropRect {
            x0: 450,
            y0: 100,
            x1: 1350,
            y1: 2200,
        };
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(450.0, 100.0, 1350.0, 2200.0)],
            heads: vec![crop_box(650.0, 160.0, 1100.0, 650.0)],
            faces: vec![crop_box(720.0, 250.0, 1030.0, 560.0)],
            half_bodies: vec![crop_box(650.0, 160.0, 1100.0, 800.0)],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.full_body_tight = false;

        let upper_body = smart_crop_candidates(&analysis, 1800, 2400, &config)
            .into_iter()
            .find(|candidate| candidate.kind == "upper_body")
            .expect("a clear single subject should produce an upper-body crop");

        let waist_minimum = subject.y0 + subject.height() * 55 / 100;
        assert!(
            upper_body.rect.y1 >= waist_minimum,
            "upper-body bottom {} stopped above waist minimum {waist_minimum}",
            upper_body.rect.y1
        );
    }

    #[test]
    fn cowboy_shot_ends_between_mid_thigh_and_the_knees() {
        let subject = CropRect {
            x0: 450,
            y0: 100,
            x1: 1350,
            y1: 2200,
        };
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(450.0, 100.0, 1350.0, 2200.0)],
            heads: vec![crop_box(650.0, 160.0, 1100.0, 650.0)],
            faces: vec![crop_box(720.0, 250.0, 1030.0, 560.0)],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = true;
        config.smart_crop.full_body_tight = false;

        let cowboy = smart_crop_candidates(&analysis, 1800, 2400, &config)
            .into_iter()
            .find(|candidate| candidate.kind == "cowboy_shot")
            .expect("a clear single subject should produce a cowboy shot");

        let mid_thigh = subject.y0 + subject.height() * 72 / 100;
        let knee_limit = subject.y0 + subject.height() * 88 / 100;
        assert!(
            cowboy.rect.y1 >= mid_thigh && cowboy.rect.y1 <= knee_limit,
            "cowboy bottom {} was outside {mid_thigh}..={knee_limit}",
            cowboy.rect.y1
        );
    }

    #[test]
    fn feet_crop_allows_one_complete_visible_foot_unless_both_feet_are_required() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(500.0, 200.0, 1300.0, 2240.0)],
            poses: vec![pose_with_left_foot_only()],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.full_body_tight = false;
        config.smart_crop.lower_body = false;
        config.smart_crop.feet = true;
        config.smart_crop.require_both_feet = false;

        assert!(smart_crop_candidates(&analysis, 1800, 2400, &config)
            .iter()
            .any(|candidate| candidate.kind == "feet"));

        config.smart_crop.require_both_feet = true;
        assert!(!smart_crop_candidates(&analysis, 1800, 2400, &config)
            .iter()
            .any(|candidate| candidate.kind == "feet"));
    }

    #[test]
    fn feet_crop_does_not_require_a_knee_and_ignores_a_distant_partial_other_foot() {
        let mut pose = pose_with_left_foot_only();
        pose.keypoints.left_knee = None;
        pose.keypoints.right_ankle = scored_crop_point(1500.0, 2050.0, 0.60);
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(420.0, 180.0, 1640.0, 2280.0)],
            poses: vec![pose],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.full_body_tight = false;
        config.smart_crop.lower_body = false;

        assert!(smart_crop_candidates(&analysis, 1800, 2400, &config)
            .iter()
            .any(|candidate| candidate.kind == "feet"));
    }

    #[test]
    fn upper_and_cowboy_use_anatomical_hip_and_knee_anchors() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(500.0, 400.0, 1900.0, 3200.0)],
            heads: vec![crop_box(850.0, 500.0, 1510.0, 1120.0)],
            faces: vec![crop_box(970.0, 680.0, 1390.0, 1050.0)],
            poses: vec![stylized_full_body_pose()],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.full_body_tight = false;
        config.smart_crop.lower_body = false;
        config.smart_crop.feet = false;

        let candidates = smart_crop_candidates(&analysis, 2508, 3541, &config);
        let upper = candidates
            .iter()
            .find(|candidate| candidate.kind == "upper_body")
            .expect("reliable hips should anchor an upper-body crop at the waist");
        let cowboy = candidates
            .iter()
            .find(|candidate| candidate.kind == "cowboy_shot")
            .expect("reliable hips and knees should anchor a cowboy crop at the thighs");

        assert!(upper.rect.y1 >= 1450 && upper.rect.y1 <= 1650);
        assert!(cowboy.rect.y1 > upper.rect.y1 && cowboy.rect.y1 < 2110);
    }

    #[test]
    fn full_and_lower_body_accept_stylized_pose_scores_with_complete_visible_endpoints() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(500.0, 400.0, 1900.0, 3200.0)],
            heads: vec![crop_box(850.0, 500.0, 1510.0, 1120.0)],
            faces: vec![crop_box(970.0, 680.0, 1390.0, 1050.0)],
            poses: vec![stylized_full_body_pose()],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.feet = false;

        let candidates = smart_crop_candidates(&analysis, 2508, 3541, &config);
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "full_body_tight"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "lower_body"));
    }

    #[test]
    fn native_crop_requires_a_bucket_without_more_than_twenty_percent_upscale() {
        let candidate = CropCandidate {
            kind: "full_body_tight",
            rect: CropRect {
                x0: 100,
                y0: 100,
                x1: 868,
                y1: 868,
            },
            score: 0.9,
        };

        assert!(!candidate_is_native_and_safe(
            candidate,
            &[],
            &crop_config(),
            2400,
            2400,
        ));
    }

    #[test]
    fn full_body_uses_complete_pose_evidence_when_the_person_detector_box_touches_canvas_edges() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(180.0, 0.0, 1820.0, 2390.0)],
            heads: vec![crop_box(720.0, 70.0, 1250.0, 620.0)],
            faces: vec![crop_box(810.0, 190.0, 1160.0, 520.0)],
            poses: vec![complete_pose(430.0, 100.0, 1570.0, 2260.0)],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.portrait = false;
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;

        let candidates = smart_crop_candidates(&analysis, 2000, 2400, &config);

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.kind == "full_body_tight"),
            "complete subject pose should not be rejected only because the detector box touches an edge"
        );
    }

    #[test]
    fn smart_crop_rejects_overlapping_people_and_never_slices_a_hand() {
        let overlapping = AnimeCropAnalysis {
            persons: vec![
                crop_box(200.0, 100.0, 1000.0, 1700.0),
                crop_box(600.0, 120.0, 1400.0, 1700.0),
            ],
            heads: vec![crop_box(420.0, 180.0, 760.0, 560.0)],
            faces: vec![crop_box(480.0, 260.0, 690.0, 480.0)],
            ..AnimeCropAnalysis::default()
        };
        assert!(smart_crop_candidates(&overlapping, 1800, 2100, &crop_config()).is_empty());

        let unsafe_hand = AnimeCropAnalysis {
            persons: vec![crop_box(450.0, 120.0, 1250.0, 1840.0)],
            heads: vec![crop_box(620.0, 180.0, 1050.0, 650.0)],
            faces: vec![crop_box(690.0, 280.0, 970.0, 540.0)],
            hands: vec![crop_box(600.0, 1700.0, 880.0, 1950.0)],
            ..AnimeCropAnalysis::default()
        };
        let hand = CropRect {
            x0: 600,
            y0: 1700,
            x1: 880,
            y1: 1950,
        };
        let candidates = smart_crop_candidates(&unsafe_hand, 1700, 2100, &crop_config());
        assert!(!candidates.is_empty());
        assert!(candidates
            .iter()
            .all(|candidate| crop_does_not_clip_parts(candidate.rect, &[hand])));
    }

    #[test]
    fn smart_crop_does_not_use_an_unassociated_global_pose_for_full_body() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(450.0, 120.0, 1250.0, 1840.0)],
            heads: vec![crop_box(620.0, 180.0, 1050.0, 650.0)],
            faces: vec![crop_box(690.0, 280.0, 970.0, 540.0)],
            pose_complete: true,
            ..AnimeCropAnalysis::default()
        };

        let candidates = smart_crop_candidates(&analysis, 1700, 2100, &crop_config());

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.kind == "full_body_tight"));
    }

    #[test]
    fn smart_crop_rejects_a_primary_crop_that_would_include_a_second_person() {
        let analysis = AnimeCropAnalysis {
            persons: vec![
                crop_box(700.0, 100.0, 1300.0, 1900.0),
                crop_box(520.0, 100.0, 690.0, 1900.0),
            ],
            heads: vec![crop_box(820.0, 180.0, 1150.0, 620.0)],
            faces: vec![crop_box(880.0, 280.0, 1090.0, 520.0)],
            half_bodies: vec![crop_box(700.0, 100.0, 1300.0, 1300.0)],
            ..AnimeCropAnalysis::default()
        };

        let candidates = smart_crop_candidates(&analysis, 2000, 2200, &crop_config());

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.kind == "upper_body"));
    }

    #[test]
    fn smart_crop_never_labels_a_landscape_crop_as_a_portrait() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(700.0, 100.0, 1300.0, 1000.0)],
            heads: vec![crop_box(750.0, 200.0, 1250.0, 400.0)],
            faces: vec![crop_box(860.0, 240.0, 1140.0, 390.0)],
            ..AnimeCropAnalysis::default()
        };
        let mut config = crop_config();
        config.smart_crop.upper_body = false;
        config.smart_crop.cowboy_shot = false;
        config.smart_crop.full_body_tight = false;

        let candidates = smart_crop_candidates(&analysis, 2000, 2000, &config);

        assert!(candidates.iter().all(|candidate| {
            candidate.kind != "portrait"
                || candidate.rect.width() as f32 / candidate.rect.height() as f32 <= 0.95
        }));
    }

    #[test]
    fn smart_crop_rejects_near_original_crops_from_a_canvas_sized_foreground_mask() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(5.0, 1.0, 2096.0, 2099.0)],
            heads: vec![crop_box(120.0, 0.0, 1653.0, 1184.0)],
            faces: vec![crop_box(562.0, 510.0, 1282.0, 1188.0)],
            half_bodies: vec![crop_box(214.0, 7.0, 2064.0, 1592.0)],
            hands: vec![
                crop_box(1167.0, 1531.0, 1733.0, 1992.0),
                crop_box(887.0, 1575.0, 1132.0, 1868.0),
                crop_box(243.0, 1575.0, 670.0, 1979.0),
            ],
            foreground: Some(crop_box(27.0, 0.0, 2100.0, 2100.0)),
            ..AnimeCropAnalysis::default()
        };

        assert!(smart_crop_candidates(&analysis, 2100, 2100, &crop_config()).is_empty());
    }

    #[test]
    fn smart_crop_ignores_a_canvas_sized_foreground_mask_when_a_tight_upper_body_exists() {
        let analysis = AnimeCropAnalysis {
            persons: vec![crop_box(500.0, 200.0, 1500.0, 2100.0)],
            heads: vec![crop_box(760.0, 250.0, 1200.0, 720.0)],
            faces: vec![crop_box(830.0, 340.0, 1130.0, 620.0)],
            half_bodies: vec![crop_box(560.0, 200.0, 1440.0, 1400.0)],
            foreground: Some(crop_box(0.0, 0.0, 2000.0, 2400.0)),
            ..AnimeCropAnalysis::default()
        };

        let candidates = smart_crop_candidates(&analysis, 2000, 2400, &crop_config());

        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == "upper_body"));
    }

    #[test]
    fn smart_crop_keeps_only_the_higher_scored_candidate_when_compositions_are_near_duplicates() {
        let candidates = deduplicate_crop_candidates(vec![
            CropCandidate {
                kind: "portrait",
                rect: CropRect {
                    x0: 100,
                    y0: 100,
                    x1: 900,
                    y1: 900,
                },
                score: 0.95,
            },
            CropCandidate {
                kind: "upper_body",
                rect: CropRect {
                    x0: 104,
                    y0: 104,
                    x1: 896,
                    y1: 896,
                },
                score: 0.85,
            },
        ]);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind, "portrait");
    }

    #[test]
    fn smart_crop_rejects_a_crop_that_removes_less_than_thirty_percent_of_the_canvas() {
        let candidate = CropCandidate {
            kind: "portrait",
            rect: CropRect {
                x0: 0,
                y0: 0,
                x1: 1500,
                y1: 2000,
            },
            score: 0.9,
        };

        assert!(!candidate_is_native_and_safe(
            candidate,
            &[],
            &crop_config(),
            2000,
            2000,
        ));
    }

    #[test]
    fn smart_crop_requires_a_stronger_reframe_for_portrait_compositions() {
        let candidate = CropCandidate {
            kind: "portrait",
            rect: CropRect {
                x0: 400,
                y0: 0,
                x1: 1600,
                y1: 2000,
            },
            score: 0.9,
        };

        assert!(!candidate_is_native_and_safe(
            candidate,
            &[],
            &crop_config(),
            2000,
            2000,
        ));
    }
}
