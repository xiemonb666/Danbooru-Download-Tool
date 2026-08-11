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
/// A derived crop must visibly change the composition.  Requiring at least an
/// thirty-percent canvas reduction avoids visually near-original JPEG re-encodes
/// under portrait/upper-body/full-body filenames.
const MAX_NEAR_ORIGINAL_AREA_RATIO: f32 = 0.70;
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
    pub full_body_tight: bool,
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
            full_body_tight: true,
            max_derived_per_family: 3,
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
        if !self.portrait && !self.upper_body && !self.full_body_tight {
            return Err("智能裁剪至少需要启用一种构图".to_string());
        }
        if !(1..=3).contains(&self.max_derived_per_family) {
            return Err("每个 family 的智能裁剪数量必须在 1..=3 之间".to_string());
        }
        Ok(())
    }
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
            output_directory: PathBuf::from("dataset-expanded"),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatasetAugmentationSummary {
    pub output_relative_directory: PathBuf,
    pub derived_relative_directory: PathBuf,
    pub training_relative_directory: PathBuf,
    pub generated: usize,
    pub rejected: usize,
    pub retagging_pending: usize,
    pub retagged: usize,
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
    generated: usize,
    rejected: usize,
    retagging_pending: usize,
    retagged: usize,
    ready_variant_counts: BTreeMap<String, usize>,
}

impl DatasetAugmentationWorkspace {
    pub fn create(
        root: VerifiedMediaRoot,
        task_id: &str,
        config: DatasetAugmentationConfig,
    ) -> Result<Self, ToolError> {
        config.validate().map_err(ToolError::InvalidManifest)?;
        let output_relative = next_output_directory(&root, &config.output_directory, task_id)?;
        let output = root.resolve(&output_relative)?;
        for directory in [
            "raw/images",
            "raw/labels",
            "derived/horizontal_flip/images",
            "derived/horizontal_flip/labels",
            "derived/portrait/images",
            "derived/portrait/labels",
            "derived/upper_body/images",
            "derived/upper_body/labels",
            "derived/full_body_tight/images",
            "derived/full_body_tight/labels",
            "metadata",
            "retagging",
            "splits",
            "rejected",
        ] {
            fs::create_dir_all(output.join(directory)).map_err(ToolError::Io)?;
        }
        write_json_atomic(&output.join("metadata/config.json"), &config)?;
        write_json_atomic(
            &output.join("INCOMPLETE.json"),
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
            generated: 0,
            rejected: 0,
            retagging_pending: 0,
            retagged: 0,
            ready_variant_counts: BTreeMap::new(),
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
            let rejection = DatasetAugmentationRejection {
                source_media_id: source.media_id.clone(),
                relative_path: source.relative_path.clone(),
                reason,
            };
            self.append_json_line("rejected/rejections.jsonl", &rejection)?;
            self.rejected += 1;
            return Ok(DatasetAugmentationItemResult::Rejected(rejection));
        }

        let output = self.root.resolve(&self.output_relative)?;
        let caption = source_caption(&self.root, &source.relative_path, &source.fallback_caption)?;
        let token = source_token(source);
        let family_id = family_id(source);
        let split = split_for_family(&family_id, &self.config);
        let extension = source_extension(&source.relative_path)?;
        let raw_image = output
            .join("raw/images")
            .join(format!("{token}.{extension}"));
        let raw_label = output.join("raw/labels").join(format!("{token}.txt"));
        copy_new_file(&source_path, &raw_image)?;
        write_text_atomic(&raw_label, &caption)?;
        write_text_atomic(&raw_image.with_extension("txt"), &caption)?;

        let bucket = choose_bucket(width, height);
        let mut samples = Vec::new();
        let original = self.write_original_sample(
            source, &token, &family_id, &split, extension, width, height, bucket,
        )?;
        samples.push(original);
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
                Some(analysis) => self.record_crop_rejection(
                    source,
                    analysis
                        .error
                        .as_deref()
                        .unwrap_or("检测模型未返回可用结果"),
                )?,
                None => self.record_crop_rejection(source, "未获得智能裁剪检测结果")?,
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
            return Ok(Vec::new());
        }
        let candidates = smart_crop_candidates(analysis, width, height, &self.config);
        if candidates.is_empty() {
            self.record_crop_rejection(
                source,
                "没有满足关键部位保护和原生分辨率约束的智能裁剪候选",
            )?;
            return Ok(Vec::new());
        }
        let mut samples = Vec::new();
        for candidate in candidates
            .into_iter()
            .take(usize::from(self.config.smart_crop.max_derived_per_family))
        {
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
        Ok(samples)
    }

    fn record_crop_rejection(
        &mut self,
        source: &DatasetAugmentationSource,
        reason: &str,
    ) -> Result<(), ToolError> {
        self.append_json_line(
            "rejected/rejections.jsonl",
            &DatasetAugmentationRejection {
                source_media_id: source.media_id.clone(),
                relative_path: source.relative_path.clone(),
                reason: format!("智能裁剪拒绝：{reason}"),
            },
        )?;
        self.rejected += 1;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn write_original_sample(
        &mut self,
        source: &DatasetAugmentationSource,
        token: &str,
        family_id: &str,
        split: &str,
        extension: &str,
        width: u32,
        height: u32,
        bucket: Option<ResolutionBucket>,
    ) -> Result<DatasetAugmentationSample, ToolError> {
        let sample_id = format!("{token}_original");
        let image_relative = self
            .output_relative
            .join("raw/images")
            .join(format!("{token}.{extension}"));
        self.record_sample(
            source,
            &sample_id,
            family_id,
            "original",
            &image_relative,
            width,
            height,
            bucket,
            split,
            false,
        )
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
        let training_relative_directory = self.output_relative.join("ready/train/images");
        let ready = serde_json::json!({
            "format_version": 1,
            "training_relative_directory": training_relative_directory,
            "split_directories": {
                "train": self.output_relative.join("ready/train/images"),
                "validation": self.output_relative.join("ready/validation/images"),
                "test": self.output_relative.join("ready/test/images"),
            },
            "generated": self.generated,
            "rejected": self.rejected,
            "retagging_pending": self.retagging_pending,
            "retagged": self.retagged,
            "training_subsets": self.training_subsets_manifest(),
        });
        let output = self.root.resolve(&self.output_relative)?;
        write_json_atomic(
            &output.join("metadata/training-subsets.json"),
            &self.training_subsets_manifest(),
        )?;
        write_json_atomic(&output.join("READY.json"), &ready)?;
        fs::remove_file(output.join("INCOMPLETE.json")).map_err(ToolError::Io)?;
        Ok(DatasetAugmentationSummary {
            output_relative_directory: self.output_relative.clone(),
            derived_relative_directory: self.output_relative.join("derived"),
            training_relative_directory,
            generated: self.generated,
            rejected: self.rejected,
            retagging_pending: self.retagging_pending,
            retagged: self.retagged,
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
            .join(if sample.variant == "original" {
                PathBuf::from("images")
            } else {
                PathBuf::from(&sample.variant).join("images")
            })
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
            ("original", "原图", false),
            ("horizontal_flip", "水平翻转", true),
            ("portrait", "肖像裁剪", true),
            ("upper_body", "上半身裁剪", true),
            ("full_body_tight", "紧凑全身裁剪", true),
        ];
        serde_json::json!({
            "format_version": 1,
            "family_binding": "metadata/dataset.jsonl",
            "splits": ["train", "validation", "test"],
            "subsets": variants.into_iter().map(|(id, label, requires_retagging)| {
                let directory = if id == "original" {
                    self.output_relative.join("ready/train/images")
                } else {
                    self.output_relative.join("ready/train").join(id).join("images")
                };
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
        let path = self.root.resolve(&self.output_relative.join(relative))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(ToolError::Io)?;
        serde_json::to_writer(&mut file, value)
            .map_err(|error| ToolError::InvalidManifest(error.to_string()))?;
        file.write_all(b"\n").map_err(ToolError::Io)
    }
}

fn next_output_directory(
    root: &VerifiedMediaRoot,
    requested: &Path,
    task_id: &str,
) -> Result<PathBuf, ToolError> {
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
        let relative = requested.join(name);
        if !root.resolve(&relative)?.exists() {
            return Ok(relative);
        }
    }
    Err(ToolError::InvalidManifest(
        "无法创建新的数据集输出目录，请选择其他目录".to_string(),
    ))
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

fn source_caption(
    root: &VerifiedMediaRoot,
    source_relative: &Path,
    fallback: &str,
) -> Result<String, ToolError> {
    let label_relative = source_relative.with_extension("txt");
    let label_path = root.resolve(&label_relative)?;
    match fs::symlink_metadata(&label_path) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() <= 8 * 1024 * 1024 => {
            fs::read_to_string(root.resolve_existing_file(&label_relative)?).map_err(ToolError::Io)
        }
        Ok(metadata) if metadata.file_type().is_file() => Err(ToolError::InvalidManifest(
            "标签文件超过 8 MiB 安全上限".to_string(),
        )),
        Ok(_) => Err(ToolError::NotRegularFile),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback.to_string()),
        Err(error) => Err(ToolError::Io(error)),
    }
}

fn source_extension(relative_path: &Path) -> Result<&str, ToolError> {
    match relative_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg") => Ok("jpg"),
        Some("jpeg") => Ok("jpeg"),
        Some("png") => Ok("png"),
        Some("webp") => Ok("webp"),
        Some("bmp") => Ok("bmp"),
        _ => Err(ToolError::InvalidManifest(
            "数据集增广仅支持 PNG、JPEG、WebP 和 BMP 静态图片".to_string(),
        )),
    }
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
) -> Option<CropRect> {
    analysis
        .poses
        .iter()
        .filter(|pose| {
            pose.torso_score >= 0.45
                && pose.left_ankle_score >= 0.45
                && pose.right_ankle_score >= 0.45
        })
        .filter_map(|pose| detection_rect(&pose.bbox, width, height))
        .filter(|pose_box| {
            subject.iou(*pose_box) >= 0.20
                || subject.contains(*pose_box)
                || pose_box.contains(subject)
        })
        .max_by(|left, right| subject.iou(*left).total_cmp(&subject.iou(*right)))
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
        "full_body_tight" => aspect <= 1.0,
        _ => true,
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
        && composition_aspect_is_valid(candidate)
        && crop_keeps_critical_parts(candidate.rect, critical)
        && rect_area_ratio(candidate.rect, source_width, source_height)
            < MAX_NEAR_ORIGINAL_AREA_RATIO
}

fn deduplicate_crop_candidates(mut candidates: Vec<CropCandidate>) -> Vec<CropCandidate> {
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut distinct = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if distinct.iter().any(|existing: &CropCandidate| {
            candidate.rect.iou(existing.rect) >= MAX_DUPLICATE_CROP_IOU
        }) {
            continue;
        }
        distinct.push(candidate);
    }
    distinct
}

/// Builds and scores a maximum of three composition candidates.  The output is
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

    // Faces, heads and hands are all protected before any crop is emitted.
    let mut critical = Vec::new();
    critical.extend(heads.iter().copied());
    critical.extend(faces.iter().copied());
    critical.extend(hands.iter().copied());
    if critical.is_empty() {
        return Vec::new();
    }

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
    if config.smart_crop.portrait {
        if let Some(head) = head {
            let base = CropRect {
                x0: head.x0,
                y0: head.y0,
                x1: head.x1,
                y1: (head.y1 + subject.height() / 3).min(subject.y1),
            };
            let rect = expand_crop(base, 0.65, 0.45, width, height);
            let candidate = CropCandidate {
                kind: "portrait",
                rect,
                score: subject_score + 0.35,
            };
            if candidate_is_native_and_safe(candidate, &critical, config, width, height)
                && crop_excludes_other_people(candidate.rect, &other_people)
            {
                candidates.push(candidate);
            }
        }
    }
    if config.smart_crop.upper_body {
        let base = half_body.unwrap_or(CropRect {
            x0: subject.x0,
            y0: subject.y0,
            x1: subject.x1,
            y1: subject.y0 + (subject.height() * 2 / 3),
        });
        let base = foreground.map(|mask| base.union(mask)).unwrap_or(base);
        let rect = expand_crop(base, 0.22, 0.18, width, height);
        let candidate = CropCandidate {
            kind: "upper_body",
            rect,
            score: subject_score + 0.25,
        };
        if candidate_is_native_and_safe(candidate, &critical, config, width, height)
            && crop_excludes_other_people(candidate.rect, &other_people)
        {
            candidates.push(candidate);
        }
    }
    if config.smart_crop.full_body_tight {
        // Require a non-edge body and pose confirmation before creating a
        // full-body composition.  This avoids inventing feet that are already
        // cropped by the source artwork.
        let edge = width.min(height) / 100;
        if subject.x0 > edge
            && subject.y0 > edge
            && subject.x1 + edge < width
            && subject.y1 + edge < height
        {
            let Some(pose) = complete_pose_for_subject(analysis, subject, width, height) else {
                return deduplicate_crop_candidates(candidates);
            };
            let base = foreground
                .map(|mask| subject.union(mask))
                .unwrap_or(subject);
            let rect = expand_crop(base, 0.08, 0.06, width, height);
            let mut full_critical = critical.clone();
            full_critical.push(subject);
            full_critical.push(pose);
            let candidate = CropCandidate {
                kind: "full_body_tight",
                rect,
                score: subject_score + 0.15,
            };
            if candidate_is_native_and_safe(candidate, &full_critical, config, width, height)
                && crop_excludes_other_people(candidate.rect, &other_people)
            {
                candidates.push(candidate);
            }
        }
    }
    deduplicate_crop_candidates(candidates)
}

fn copy_new_file(source: &Path, destination: &Path) -> Result<(), ToolError> {
    if destination.exists() {
        return Err(ToolError::Conflict(destination.to_path_buf()));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| ToolError::InvalidManifest("输出文件缺少父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(ToolError::Io)?;
    fs::copy(source, destination).map_err(ToolError::Io)?;
    Ok(())
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

fn write_text_atomic(destination: &Path, content: &str) -> Result<(), ToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| ToolError::InvalidManifest("输出文件缺少父目录".to_string()))?;
    fs::create_dir_all(parent).map_err(ToolError::Io)?;
    let temporary = destination.with_extension(format!(
        "{}.tmp-{}",
        destination
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("txt"),
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, content).map_err(ToolError::Io)?;
    fs::rename(temporary, destination).map_err(ToolError::Io)
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
        candidate_is_native_and_safe, choose_bucket, deduplicate_crop_candidates,
        smart_crop_candidates, split_for_family, AnimeCropAnalysis, AnimeCropBox, AnimeCropPose,
        CropCandidate, CropRect, DatasetAugmentationConfig, DatasetAugmentationItemResult,
        DatasetAugmentationSource, DatasetAugmentationWorkspace,
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
        assert_eq!(samples.len(), 2);
        assert!(!samples[0].requires_retagging);
        assert!(samples[1].requires_retagging);
        assert!(temporary.path().join("source.png").is_file());
        let output = temporary.path().join("dataset-expanded/task-1");
        assert!(output.join("raw/images/media1.png").is_file());
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
            .join(&samples[1].split)
            .join("images")
            .join("media1_horizontal_flip.txt")
            .exists());
        let summary = workspace.finish().unwrap();
        let ready = output.join("ready").join(&samples[0].split).join("images");
        assert!(ready.join("media1_original.png").is_file());
        assert!(ready.join("media1_original.txt").is_file());
        assert!(!ready.join("media1_horizontal_flip.png").exists());
        assert_eq!(summary.retagging_pending, 1);
        let metadata = std::fs::read_to_string(output.join("metadata/dataset.jsonl")).unwrap();
        assert!(metadata.contains("\"allow_non_uniform_scaling\":false"));
        assert!(metadata.contains("\"requires_retagging\":true"));
        let retagging = std::fs::read_to_string(output.join("metadata/retagging.jsonl")).unwrap();
        assert!(retagging.contains("media1_horizontal_flip"));
    }

    #[test]
    fn original_sample_reuses_raw_image_instead_of_creating_a_derived_original_copy() {
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
            panic!("source should be represented by its raw copy");
        };

        assert_eq!(samples.len(), 1);
        assert!(samples[0]
            .output_relative_path
            .to_string_lossy()
            .contains("raw/images"));
        assert!(!temporary
            .path()
            .join("dataset-expanded/task-original/derived/original")
            .exists());
    }

    #[test]
    fn workspace_materializes_a_train_ready_split_only_when_finished() {
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

        let output = temporary.path().join("dataset-expanded/task-ready");
        assert!(!output.join("READY.json").exists());
        assert!(output.join("INCOMPLETE.json").is_file());
        let summary = workspace.finish().unwrap();
        let ready = output.join("ready").join(&samples[0].split).join("images");
        assert!(ready
            .join(format!("{}.png", samples[0].sample_id))
            .is_file());
        assert_eq!(
            std::fs::read_to_string(ready.join(format!("{}.txt", samples[0].sample_id))).unwrap(),
            "1girl"
        );
        assert!(output.join("READY.json").is_file());
        assert!(!output.join("INCOMPLETE.json").exists());
        assert_eq!(
            summary.training_relative_directory,
            PathBuf::from("dataset-expanded/task-ready/ready/train/images")
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
            .join("dataset-expanded/task-promote/ready")
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
                .join("dataset-expanded/task-promote/metadata/training-subsets.json"),
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
        AnimeCropPose {
            bbox: crop_box(x0, y0, x1, y1),
            torso_score: 0.95,
            left_ankle_score: 0.95,
            right_ankle_score: 0.95,
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
        assert!(config.smart_crop.full_body_tight);
        assert_eq!(config.smart_crop.max_derived_per_family, 3);
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
        assert_eq!(candidates.len(), 3);
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
            .any(|candidate| candidate.kind == "full_body_tight"));
    }

    #[test]
    fn smart_crop_rejects_overlapping_people_and_unprotected_hand() {
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
        assert!(smart_crop_candidates(&unsafe_hand, 1700, 2100, &crop_config()).is_empty());
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
        config.smart_crop.full_body_tight = false;

        let candidates = smart_crop_candidates(&analysis, 2000, 2000, &config);

        assert!(candidates.is_empty());
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
}
