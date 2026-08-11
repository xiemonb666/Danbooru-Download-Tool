use crate::media_root::{metadata_is_link_or_reparse_point, validate_root_path};
use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const SAFE_MEDIA_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "avif", "heic", "heif", "mp4", "webm", "zip",
];
const QUARANTINE_DIR: &str = ".danbooru-quarantine";
const CATEGORY_ARTIST: i64 = 1;
const CATEGORY_COPYRIGHT: i64 = 3;
const CATEGORY_CHARACTER: i64 = 4;
const CATEGORY_CIRCLE: i64 = 5;
const CATEGORY_META: i64 = 6;

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtistPrefix {
    #[default]
    Artist,
    At,
}

impl ArtistPrefix {
    fn format(self, tag: &str) -> String {
        match self {
            Self::Artist => format!("artist:{tag}"),
            Self::At => format!("@{tag}"),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TagPipelineConfig {
    pub artist_prefix: ArtistPrefix,
    pub categories: BTreeMap<String, Option<i64>>,
}

pub(crate) fn is_quarantine_dir_name(name: &OsStr) -> bool {
    #[cfg(windows)]
    {
        name.to_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(QUARANTINE_DIR))
    }
    #[cfg(not(windows))]
    {
        name == OsStr::new(QUARANTINE_DIR)
    }
}

fn validate_relative_path(path: &Path) -> Result<PathBuf, String> {
    use std::path::Component;

    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err("媒体路径必须是非空相对路径".to_string());
    }
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(path.to_path_buf())
    } else {
        Err("媒体路径不能包含根目录、盘符、点段或父目录".to_string())
    }
}

#[derive(Debug)]
pub enum ToolError {
    InvalidRelativePath,
    OutsideRoot,
    NotDirectory,
    NotRegularFile,
    Conflict(PathBuf),
    Io(std::io::Error),
    Image(image::ImageError),
    InvalidManifest(String),
    ConverterUnavailable,
    ConversionFailed,
    ConversionTimedOut,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRelativePath => write!(formatter, "媒体路径必须是根目录内的规范相对路径"),
            Self::OutsideRoot => write!(formatter, "媒体路径越过了已验证的根目录"),
            Self::NotDirectory => write!(formatter, "媒体根目录不存在或不是目录"),
            Self::NotRegularFile => write!(formatter, "媒体路径不存在或不是普通文件"),
            Self::Conflict(path) => write!(formatter, "目标已存在，拒绝覆盖: {}", path.display()),
            Self::Io(error) => write!(formatter, "文件系统错误: {error}"),
            Self::Image(error) => write!(formatter, "图片解码错误: {error}"),
            Self::InvalidManifest(message) => write!(formatter, "无效的操作清单: {message}"),
            Self::ConverterUnavailable => formatter.write_str("未安装 heif-convert，无法转换 HEIC"),
            Self::ConversionFailed => formatter.write_str("heif-convert 未能生成有效 JPEG"),
            Self::ConversionTimedOut => formatter.write_str("heif-convert 转换超时"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<image::ImageError> for ToolError {
    fn from(error: image::ImageError) -> Self {
        Self::Image(error)
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedMediaRoot {
    canonical: PathBuf,
}

impl VerifiedMediaRoot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ToolError> {
        let path = path.as_ref();
        validate_root_path(path).map_err(|_| ToolError::NotDirectory)?;
        let canonical = fs::canonicalize(path).map_err(ToolError::Io)?;
        if !canonical.is_dir() {
            return Err(ToolError::NotDirectory);
        }
        Ok(Self { canonical })
    }

    pub fn path(&self) -> &Path {
        &self.canonical
    }

    pub fn resolve(&self, relative: &Path) -> Result<PathBuf, ToolError> {
        let relative =
            validate_relative_path(relative).map_err(|_| ToolError::InvalidRelativePath)?;
        let mut lexical = self.canonical.clone();
        for component in relative.components() {
            lexical.push(component.as_os_str());
            match fs::symlink_metadata(&lexical) {
                Ok(metadata) if metadata_is_link_or_reparse_point(&metadata) => {
                    return Err(ToolError::OutsideRoot);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(ToolError::Io(error)),
            }
        }
        let candidate = self.canonical.join(relative);

        if candidate.exists() {
            let canonical = fs::canonicalize(&candidate).map_err(ToolError::Io)?;
            if !canonical.starts_with(&self.canonical) {
                return Err(ToolError::OutsideRoot);
            }
            return Ok(canonical);
        }

        let mut ancestor = candidate.parent();
        while let Some(path) = ancestor {
            if path.exists() {
                let canonical_parent = fs::canonicalize(path).map_err(ToolError::Io)?;
                if !canonical_parent.starts_with(&self.canonical) {
                    return Err(ToolError::OutsideRoot);
                }
                break;
            }
            ancestor = path.parent();
        }
        Ok(candidate)
    }

    pub fn resolve_existing_file(&self, relative: &Path) -> Result<PathBuf, ToolError> {
        let path = self.resolve(relative)?;
        let metadata = fs::metadata(&path).map_err(ToolError::Io)?;
        if !metadata.file_type().is_file() {
            return Err(ToolError::NotRegularFile);
        }
        Ok(path)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolOperation {
    ExactDedup,
    NearDedup,
    IntegrityCheck,
    DeleteByTag,
    DeleteSelected,
    TagPipeline,
    HeicConvert,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolCandidate {
    pub relative_path: PathBuf,
    pub companion_paths: Vec<PathBuf>,
    pub reason: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolManifest {
    pub batch_id: String,
    pub operation: ToolOperation,
    pub created_at: u64,
    #[serde(default)]
    pub root_fingerprint: String,
    #[serde(default)]
    pub file_fingerprints: Vec<ToolFileFingerprint>,
    #[serde(default)]
    pub pairs: Vec<SimilarPair>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_pipeline_config: Option<TagPipelineConfig>,
    pub candidates: Vec<ToolCandidate>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ToolFileFingerprint {
    pub relative_path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct QuarantineResult {
    pub batch_id: String,
    pub moved: usize,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RestoreResult {
    pub batch_id: String,
    pub restored: usize,
    pub conflicts: Vec<PathBuf>,
    pub remaining: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SimilarPair {
    pub left: PathBuf,
    pub right: PathBuf,
    pub distance: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ResizeResult {
    pub output_relative: PathBuf,
    pub width: u32,
    pub height: u32,
    pub quarantine_batch: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TagPipelineResult {
    pub batch_id: String,
    pub changed: usize,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HeicConversionItem {
    pub original_relative: PathBuf,
    pub output_relative: PathBuf,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HeicConversionResult {
    pub batch_id: String,
    pub items: Vec<HeicConversionItem>,
}

#[cfg(test)]
pub fn transform_tag_content(content: &str) -> String {
    transform_tag_content_with_categories(content, &BTreeMap::new(), ArtistPrefix::Artist)
}

pub fn transform_tag_content_with_categories(
    content: &str,
    categories: &BTreeMap<String, Option<i64>>,
    artist_prefix: ArtistPrefix,
) -> String {
    let mut tokens = content
        .split([',', '\n', '\r', ';'])
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            let category = categories.get(token).copied().flatten();
            if matches!(
                category,
                Some(CATEGORY_COPYRIGHT | CATEGORY_CIRCLE | CATEGORY_META)
            ) {
                return None;
            }
            let priority = match category {
                Some(CATEGORY_ARTIST) => 0,
                Some(CATEGORY_CHARACTER) => 1,
                _ => 2,
            };
            let token = if category == Some(CATEGORY_ARTIST) {
                artist_prefix.format(token)
            } else {
                token.to_string()
            };
            Some((priority, token))
        })
        .collect::<Vec<_>>();
    tokens.sort_by_key(|(priority, _)| *priority);
    let mut seen = HashSet::new();
    tokens
        .into_iter()
        .filter_map(|(_, token)| {
            let token = token.replace('_', " ");
            let mut escaped = String::with_capacity(token.len());
            let mut previous = None;
            for character in token.chars() {
                if matches!(character, '(' | ')') && previous != Some('\\') {
                    escaped.push('\\');
                }
                escaped.push(character);
                previous = Some(character);
            }
            seen.insert(escaped.clone()).then_some(escaped)
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
pub fn plan_tag_pipeline(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    plan_tag_pipeline_classified(root, media_files, TagPipelineConfig::default())
}

pub fn collect_tag_pipeline_tokens(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<BTreeSet<String>, ToolError> {
    if media_files.is_empty() || media_files.len() > 10_000 {
        return Err(ToolError::InvalidManifest(
            "标签任务的媒体数量必须在 1..=10000".to_string(),
        ));
    }
    let mut tags = BTreeSet::new();
    for media_relative in media_files {
        let media_relative =
            validate_relative_path(media_relative).map_err(|_| ToolError::InvalidRelativePath)?;
        root.resolve_existing_file(&media_relative)?;
        let sidecar_relative = media_relative.with_extension("txt");
        let sidecar = match fs::symlink_metadata(root.resolve(&sidecar_relative)?) {
            Ok(metadata) if metadata.is_file() && !metadata_is_link_or_reparse_point(&metadata) => {
                if metadata.len() > 8 * 1024 * 1024 {
                    return Err(ToolError::InvalidManifest(format!(
                        "标签文件超过 8 MiB 安全上限: {}",
                        sidecar_relative.display()
                    )));
                }
                root.resolve_existing_file(&sidecar_relative)?
            }
            Ok(_) => return Err(ToolError::NotRegularFile),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ToolError::Io(error)),
        };
        let content = fs::read_to_string(sidecar).map_err(ToolError::Io)?;
        for tag in content
            .split([',', '\n', '\r', ';'])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
        {
            tags.insert(tag.to_string());
            if tags.len() > 50_000 {
                return Err(ToolError::InvalidManifest(
                    "标签分类查询单批最多 50000 个唯一标签".to_string(),
                ));
            }
        }
    }
    Ok(tags)
}

pub fn plan_tag_pipeline_classified(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
    config: TagPipelineConfig,
) -> Result<ToolManifest, ToolError> {
    if media_files.is_empty() || media_files.len() > 10_000 {
        return Err(ToolError::InvalidManifest(
            "标签任务的媒体数量必须在 1..=10000".to_string(),
        ));
    }
    if config.categories.len() > 50_000 {
        return Err(ToolError::InvalidManifest(
            "标签分类缓存单批最多 50000 项".to_string(),
        ));
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for media_relative in media_files {
        let media_relative =
            validate_relative_path(media_relative).map_err(|_| ToolError::InvalidRelativePath)?;
        root.resolve_existing_file(&media_relative)?;
        let sidecar_relative = media_relative.with_extension("txt");
        if !seen.insert(sidecar_relative.clone()) {
            continue;
        }
        let sidecar = match fs::symlink_metadata(root.resolve(&sidecar_relative)?) {
            Ok(metadata) if metadata.is_file() && !metadata_is_link_or_reparse_point(&metadata) => {
                root.resolve_existing_file(&sidecar_relative)?
            }
            Ok(_) => return Err(ToolError::NotRegularFile),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(ToolError::Io(error)),
        };
        let size = fs::metadata(&sidecar).map_err(ToolError::Io)?.len();
        if size > 8 * 1024 * 1024 {
            return Err(ToolError::InvalidManifest(format!(
                "标签文件超过 8 MiB 安全上限: {}",
                sidecar_relative.display()
            )));
        }
        let content = fs::read_to_string(sidecar).map_err(ToolError::Io)?;
        if transform_tag_content_with_categories(&content, &config.categories, config.artist_prefix)
            != content
        {
            candidates.push(ToolCandidate {
                relative_path: sidecar_relative,
                companion_paths: Vec::new(),
                reason: "tag_pipeline_original".to_string(),
                size,
                sha256: None,
            });
        }
    }
    let mut manifest = new_manifest(root, ToolOperation::TagPipeline, candidates)?;
    manifest.tag_pipeline_config = Some(config);
    Ok(manifest)
}

pub fn plan_heic_conversion(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    if media_files.is_empty() || media_files.len() > 1_000 {
        return Err(ToolError::InvalidManifest(
            "HEIC 转换的媒体数量必须在 1..=1000".to_string(),
        ));
    }
    let mut candidates = Vec::new();
    let mut sources = HashSet::new();
    let mut outputs = HashSet::new();
    for media_relative in media_files {
        let media_relative =
            validate_relative_path(media_relative).map_err(|_| ToolError::InvalidRelativePath)?;
        if !sources.insert(media_relative.clone()) {
            continue;
        }
        let extension = media_relative
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| {
                ToolError::InvalidManifest(format!(
                    "媒体不是 HEIC/HEIF 文件: {}",
                    media_relative.display()
                ))
            })?;
        if !matches!(extension.as_str(), "heic" | "heif") {
            return Err(ToolError::InvalidManifest(format!(
                "媒体不是 HEIC/HEIF 文件: {}",
                media_relative.display()
            )));
        }
        let source = root.resolve_existing_file(&media_relative)?;
        let size = fs::metadata(source).map_err(ToolError::Io)?.len();
        if size == 0 || size > 512 * 1024 * 1024 {
            return Err(ToolError::InvalidManifest(format!(
                "HEIC 文件大小必须在 1 B..=512 MiB: {}",
                media_relative.display()
            )));
        }
        let output_relative = media_relative.with_extension("jpg");
        if !outputs.insert(output_relative.clone()) {
            return Err(ToolError::Conflict(output_relative));
        }
        let output = root.resolve(&output_relative)?;
        if output.exists() {
            return Err(ToolError::Conflict(output_relative));
        }
        candidates.push(ToolCandidate {
            relative_path: media_relative,
            companion_paths: Vec::new(),
            reason: "heic_original".to_string(),
            size,
            sha256: None,
        });
    }
    new_manifest(root, ToolOperation::HeicConvert, candidates)
}

pub fn apply_heic_conversion_with<F>(
    root: &VerifiedMediaRoot,
    manifest: &ToolManifest,
    mut converter: F,
) -> Result<HeicConversionResult, ToolError>
where
    F: FnMut(&Path, &Path) -> Result<(), ToolError>,
{
    if manifest.operation != ToolOperation::HeicConvert {
        return Err(ToolError::InvalidManifest(
            "预检清单不是 HEIC 转换操作".to_string(),
        ));
    }
    for candidate in &manifest.candidates {
        if !candidate.companion_paths.is_empty()
            || !candidate
                .relative_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(extension.to_ascii_lowercase().as_str(), "heic" | "heif")
                })
        {
            return Err(ToolError::InvalidManifest(
                "HEIC 转换清单包含无效文件".to_string(),
            ));
        }
        let output_relative = candidate.relative_path.with_extension("jpg");
        if root.resolve(&output_relative)?.exists() {
            return Err(ToolError::Conflict(output_relative));
        }
    }

    apply_quarantine(root, manifest)?;
    let mut prepared: Vec<(PathBuf, HeicConversionItem)> = Vec::new();
    let mut published = Vec::new();
    for candidate in &manifest.candidates {
        let input_relative = PathBuf::from(QUARANTINE_DIR)
            .join(&manifest.batch_id)
            .join(&candidate.relative_path);
        let input = root.resolve_existing_file(&input_relative)?;
        let output_relative = candidate.relative_path.with_extension("jpg");
        let output_name = output_relative
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ToolError::InvalidRelativePath)?;
        let temporary_relative = output_relative
            .with_file_name(format!(".{output_name}.tmp-{}.jpg", uuid::Uuid::new_v4()));
        let temporary = root.resolve(&temporary_relative)?;
        let conversion_result = converter(&input, &temporary).and_then(|()| {
            let metadata = fs::symlink_metadata(&temporary).map_err(ToolError::Io)?;
            if !metadata.is_file()
                || metadata_is_link_or_reparse_point(&metadata)
                || metadata.len() == 0
                || metadata.len() > 512 * 1024 * 1024
            {
                return Err(ToolError::InvalidManifest(
                    "HEIC 转换器输出不是安全的普通 JPEG 文件".to_string(),
                ));
            }
            let (width, height) = validate_jpeg_output(&temporary)?;
            Ok((width, height, metadata.len()))
        });
        let (width, height, byte_size) = match conversion_result {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                cleanup_heic_prepared(&prepared);
                rollback_published_files(root, &published);
                restore_after_conversion_failure(root, manifest)?;
                return Err(error);
            }
        };
        prepared.push((
            temporary,
            HeicConversionItem {
                original_relative: candidate.relative_path.clone(),
                output_relative,
                width,
                height,
                byte_size,
            },
        ));
    }

    for (temporary, item) in &prepared {
        let destination = root.resolve(&item.output_relative)?;
        if let Err(error) = publish_noreplace(temporary, &destination) {
            cleanup_heic_prepared(&prepared);
            rollback_published_files(root, &published);
            restore_after_conversion_failure(root, manifest)?;
            return Err(error);
        }
        published.push(item.output_relative.clone());
    }
    Ok(HeicConversionResult {
        batch_id: manifest.batch_id.clone(),
        items: prepared.into_iter().map(|(_, item)| item).collect(),
    })
}

pub fn apply_heic_conversion(
    root: &VerifiedMediaRoot,
    manifest: &ToolManifest,
) -> Result<HeicConversionResult, ToolError> {
    apply_heic_conversion_with(root, manifest, |input, output| {
        run_converter_process(
            OsStr::new("heif-convert"),
            input,
            output,
            std::time::Duration::from_secs(120),
        )
    })
}

pub fn rollback_heic_conversion(
    root: &VerifiedMediaRoot,
    result: &HeicConversionResult,
) -> Result<RestoreResult, ToolError> {
    let outputs = result
        .items
        .iter()
        .map(|item| item.output_relative.clone())
        .collect::<Vec<_>>();
    rollback_published_files(root, &outputs);
    let restored = restore_quarantine(root, &result.batch_id)?;
    if restored.restored != result.items.len()
        || restored.remaining != 0
        || !restored.conflicts.is_empty()
    {
        return Err(ToolError::InvalidManifest(
            "HEIC 数据库写入失败，且原文件回滚不完整".to_string(),
        ));
    }
    let quarantine_root = root.resolve(Path::new(QUARANTINE_DIR))?;
    match fs::remove_dir(quarantine_root) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => return Err(ToolError::Io(error)),
    }
    Ok(restored)
}

fn validate_jpeg_output(path: &Path) -> Result<(u32, u32), ToolError> {
    let mut reader = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(ToolError::Io)?;
    if reader.format() != Some(image::ImageFormat::Jpeg) {
        return Err(ToolError::InvalidManifest(
            "HEIC 转换器输出格式不是 JPEG".to_string(),
        ));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(ToolError::Image)?;
    Ok(image.dimensions())
}

fn run_converter_process(
    binary: &OsStr,
    input: &Path,
    output: &Path,
    timeout: std::time::Duration,
) -> Result<(), ToolError> {
    let mut child = std::process::Command::new(binary)
        .arg(input)
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ToolError::ConverterUnavailable
            } else {
                ToolError::ConversionFailed
            }
        })?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(ToolError::ConversionFailed),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ToolError::ConversionTimedOut);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ToolError::ConversionFailed);
            }
        }
    }
}

fn cleanup_heic_prepared(prepared: &[(PathBuf, HeicConversionItem)]) {
    for (temporary, _) in prepared {
        let _ = fs::remove_file(temporary);
    }
}

fn restore_after_conversion_failure(
    root: &VerifiedMediaRoot,
    manifest: &ToolManifest,
) -> Result<(), ToolError> {
    let restored = restore_quarantine(root, &manifest.batch_id)?;
    if restored.restored != manifest.candidates.len()
        || restored.remaining != 0
        || !restored.conflicts.is_empty()
    {
        return Err(ToolError::InvalidManifest(
            "HEIC 转换失败，且原文件回滚不完整".to_string(),
        ));
    }
    let quarantine_root = root.resolve(Path::new(QUARANTINE_DIR))?;
    match fs::remove_dir(quarantine_root) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => return Err(ToolError::Io(error)),
    }
    Ok(())
}

pub fn apply_tag_pipeline(
    root: &VerifiedMediaRoot,
    manifest: &ToolManifest,
) -> Result<TagPipelineResult, ToolError> {
    if manifest.operation != ToolOperation::TagPipeline {
        return Err(ToolError::InvalidManifest(
            "预检清单不是标签处理操作".to_string(),
        ));
    }
    let config = manifest.tag_pipeline_config.clone().unwrap_or_default();
    let mut prepared = Vec::with_capacity(manifest.candidates.len());
    for candidate in &manifest.candidates {
        if !candidate.companion_paths.is_empty() {
            cleanup_prepared_files(&prepared);
            return Err(ToolError::InvalidManifest(
                "标签处理清单不能包含伴随文件".to_string(),
            ));
        }
        let source = root.resolve_existing_file(&candidate.relative_path)?;
        let metadata = fs::symlink_metadata(&source).map_err(ToolError::Io)?;
        if metadata_is_link_or_reparse_point(&metadata) {
            cleanup_prepared_files(&prepared);
            return Err(ToolError::NotRegularFile);
        }
        if metadata.len() > 8 * 1024 * 1024 {
            cleanup_prepared_files(&prepared);
            return Err(ToolError::InvalidManifest(format!(
                "标签文件超过 8 MiB 安全上限: {}",
                candidate.relative_path.display()
            )));
        }
        let content = fs::read_to_string(&source).map_err(ToolError::Io)?;
        let transformed = transform_tag_content_with_categories(
            &content,
            &config.categories,
            config.artist_prefix,
        );
        if transformed == content {
            cleanup_prepared_files(&prepared);
            return Err(ToolError::InvalidManifest(format!(
                "预检标签文件已无需处理: {}",
                candidate.relative_path.display()
            )));
        }
        let file_name = candidate
            .relative_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ToolError::InvalidRelativePath)?;
        let temporary_relative = candidate
            .relative_path
            .with_file_name(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        let temporary = root.resolve(&temporary_relative)?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(ToolError::Io)?;
            file.write_all(transformed.as_bytes())
                .map_err(ToolError::Io)?;
            file.flush().map_err(ToolError::Io)?;
            file.sync_all().map_err(ToolError::Io)
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            cleanup_prepared_files(&prepared);
            return Err(error);
        }
        prepared.push((temporary, candidate.relative_path.clone()));
    }

    if let Err(error) = apply_quarantine(root, manifest) {
        cleanup_prepared_files(&prepared);
        return Err(error);
    }

    let mut published = Vec::with_capacity(prepared.len());
    for (temporary, relative) in &prepared {
        let destination = root.resolve(relative)?;
        if let Err(error) = publish_noreplace(temporary, &destination) {
            cleanup_prepared_files(&prepared);
            rollback_published_files(root, &published);
            let restored = restore_quarantine(root, &manifest.batch_id)?;
            if restored.remaining != 0 || !restored.conflicts.is_empty() {
                return Err(ToolError::InvalidManifest(
                    "标签替换失败，且原文件回滚不完整".to_string(),
                ));
            }
            return Err(error);
        }
        published.push(relative.clone());
    }

    Ok(TagPipelineResult {
        batch_id: manifest.batch_id.clone(),
        changed: published.len(),
        paths: published,
    })
}

pub fn rollback_tag_pipeline(
    root: &VerifiedMediaRoot,
    result: &TagPipelineResult,
) -> Result<RestoreResult, ToolError> {
    for relative in result.paths.iter().rev() {
        let relative =
            validate_relative_path(relative).map_err(|_| ToolError::InvalidRelativePath)?;
        let lexical = root.path().join(&relative);
        match fs::symlink_metadata(&lexical) {
            Ok(metadata) if metadata.is_file() && !metadata_is_link_or_reparse_point(&metadata) => {
                fs::remove_file(lexical).map_err(ToolError::Io)?;
            }
            Ok(_) => return Err(ToolError::NotRegularFile),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ToolError::Io(error)),
        }
    }
    let restored = restore_quarantine(root, &result.batch_id)?;
    let quarantine_root = root.resolve(Path::new(QUARANTINE_DIR))?;
    match fs::remove_dir(quarantine_root) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => return Err(ToolError::Io(error)),
    }
    Ok(restored)
}

fn publish_noreplace(temporary: &Path, destination: &Path) -> Result<(), ToolError> {
    fs::hard_link(temporary, destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            ToolError::Conflict(
                destination
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_default(),
            )
        } else {
            ToolError::Io(error)
        }
    })?;
    if let Err(error) = fs::remove_file(temporary) {
        let _ = fs::remove_file(destination);
        return Err(ToolError::Io(error));
    }
    Ok(())
}

fn cleanup_prepared_files(prepared: &[(PathBuf, PathBuf)]) {
    for (temporary, _) in prepared {
        let _ = fs::remove_file(temporary);
    }
}

fn rollback_published_files(root: &VerifiedMediaRoot, published: &[PathBuf]) {
    for relative in published.iter().rev() {
        if let Ok(path) = root.resolve_existing_file(relative) {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn plan_exact_duplicates(root: &VerifiedMediaRoot) -> Result<ToolManifest, ToolError> {
    let media_files = collect_media_files(root)?;
    plan_exact_duplicates_selected(root, &media_files)
}

pub fn plan_exact_duplicates_selected(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    let media_files = validate_selected_media_files(root, media_files)?;
    let mut by_size: BTreeMap<u64, Vec<PathBuf>> = BTreeMap::new();
    for relative in &media_files {
        let path = root.resolve_existing_file(relative)?;
        let size = fs::metadata(path).map_err(ToolError::Io)?.len();
        by_size.entry(size).or_default().push(relative.clone());
    }

    let mut candidates = Vec::new();
    for (size, mut same_size) in by_size {
        if same_size.len() < 2 {
            continue;
        }
        same_size.sort();
        let mut by_hash: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for relative in same_size {
            let hash = sha256_file(&root.resolve_existing_file(&relative)?)?;
            by_hash.entry(hash).or_default().push(relative);
        }
        for (sha256, mut same_hash) in by_hash {
            if same_hash.len() < 2 {
                continue;
            }
            same_hash.sort();
            let keeper = same_hash.remove(0);
            for relative_path in same_hash {
                candidates.push(ToolCandidate {
                    companion_paths: Vec::new(),
                    reason: format!("exact_duplicate_of:{}", keeper.display()),
                    relative_path,
                    size,
                    sha256: Some(sha256.clone()),
                });
            }
        }
    }
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    attach_orphaned_sidecars(root, &media_files, &mut candidates)?;
    new_manifest(root, ToolOperation::ExactDedup, candidates)
}

pub fn plan_integrity_check(root: &VerifiedMediaRoot) -> Result<ToolManifest, ToolError> {
    let media_files = collect_media_files(root)?;
    plan_integrity_check_selected(root, &media_files)
}

pub fn plan_integrity_check_selected(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    let media_files = validate_selected_media_files(root, media_files)?;
    let mut candidates = Vec::new();
    for relative_path in &media_files {
        let path = root.resolve_existing_file(relative_path)?;
        let size = fs::metadata(&path).map_err(ToolError::Io)?.len();
        let reason = if size == 0 {
            Some("empty_file".to_string())
        } else if is_decodable_image(relative_path) {
            open_limited_image(&path)
                .err()
                .map(|error| format!("decode_failed:{error}"))
        } else {
            None
        };
        if let Some(reason) = reason {
            candidates.push(ToolCandidate {
                companion_paths: Vec::new(),
                relative_path: relative_path.clone(),
                reason,
                size,
                sha256: None,
            });
        }
    }
    attach_orphaned_sidecars(root, &media_files, &mut candidates)?;
    new_manifest(root, ToolOperation::IntegrityCheck, candidates)
}

pub fn plan_delete_by_tag(root: &VerifiedMediaRoot, tag: &str) -> Result<ToolManifest, ToolError> {
    let media_files = collect_media_files(root)?;
    plan_delete_by_tag_selected(root, tag, &media_files)
}

pub fn plan_delete_selected(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    let media_files = validate_selected_media_files(root, media_files)?;
    let mut candidates = Vec::with_capacity(media_files.len());
    for relative_path in &media_files {
        let media_path = root.resolve_existing_file(relative_path)?;
        let size = fs::metadata(media_path).map_err(ToolError::Io)?.len();
        candidates.push(ToolCandidate {
            relative_path: relative_path.clone(),
            companion_paths: Vec::new(),
            reason: "selected_by_user".to_string(),
            size,
            sha256: None,
        });
    }
    attach_orphaned_sidecars(root, &media_files, &mut candidates)?;
    new_manifest(root, ToolOperation::DeleteSelected, candidates)
}

pub fn plan_delete_by_tag_selected(
    root: &VerifiedMediaRoot,
    tag: &str,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    let wanted = normalize_tag_token(tag);
    if wanted.is_empty() {
        return Err(ToolError::InvalidManifest("标签不能为空".to_string()));
    }

    let media_files = validate_selected_media_files(root, media_files)?;
    let mut candidates = Vec::new();
    for relative_path in &media_files {
        let sidecars = existing_sidecars(root, relative_path)?;
        let Some(sidecar_relative) = sidecars.first() else {
            continue;
        };
        let sidecar_path = root.resolve_existing_file(sidecar_relative)?;
        let sidecar_size = fs::metadata(&sidecar_path).map_err(ToolError::Io)?.len();
        if sidecar_size > 8 * 1024 * 1024 {
            return Err(ToolError::InvalidManifest(format!(
                "标签文件过大: {}",
                sidecar_relative.display()
            )));
        }
        let content = fs::read_to_string(sidecar_path).map_err(ToolError::Io)?;
        let matches = sidecar_contains_tag(&content, &wanted);
        if !matches {
            continue;
        }
        let media_path = root.resolve_existing_file(relative_path)?;
        let size = fs::metadata(media_path).map_err(ToolError::Io)?.len();
        candidates.push(ToolCandidate {
            relative_path: relative_path.clone(),
            companion_paths: Vec::new(),
            reason: format!("tag:{wanted}"),
            size,
            sha256: None,
        });
    }
    attach_orphaned_sidecars(root, &media_files, &mut candidates)?;
    new_manifest(root, ToolOperation::DeleteByTag, candidates)
}

#[cfg(test)]
pub fn plan_similar_images(
    root: &VerifiedMediaRoot,
    max_distance: u32,
) -> Result<Vec<SimilarPair>, ToolError> {
    let media_files = collect_media_files(root)?;
    plan_similar_images_selected(root, max_distance, &media_files)
}

pub fn plan_similar_images_selected(
    root: &VerifiedMediaRoot,
    max_distance: u32,
    media_files: &[PathBuf],
) -> Result<Vec<SimilarPair>, ToolError> {
    const MAX_SIMILAR_PAIRS: usize = 10_000;
    if max_distance > 64 {
        return Err(ToolError::InvalidManifest(
            "dHash 距离必须在 0..=64".to_string(),
        ));
    }
    let image_paths = validate_selected_media_files(root, media_files)?
        .into_iter()
        .filter(|path| is_decodable_image(path))
        .collect::<Vec<_>>();
    if image_paths.len() > 5_000 {
        return Err(ToolError::InvalidManifest(
            "近似图片扫描单批最多 5000 项，请缩小范围".to_string(),
        ));
    }

    let mut hashes = Vec::with_capacity(image_paths.len());
    for relative in image_paths {
        let image = open_limited_image(&root.resolve_existing_file(&relative)?)?;
        hashes.push((relative, difference_hash(&image)));
    }

    let mut pairs = Vec::new();
    for left_index in 0..hashes.len() {
        for right_index in (left_index + 1)..hashes.len() {
            let distance = (hashes[left_index].1 ^ hashes[right_index].1).count_ones();
            if distance <= max_distance {
                if pairs.len() == MAX_SIMILAR_PAIRS {
                    return Err(ToolError::InvalidManifest(format!(
                        "近似候选超过 {MAX_SIMILAR_PAIRS} 对，请缩小图库范围或降低距离阈值"
                    )));
                }
                pairs.push(SimilarPair {
                    left: hashes[left_index].0.clone(),
                    right: hashes[right_index].0.clone(),
                    distance,
                });
            }
        }
    }
    Ok(pairs)
}

pub fn plan_near_duplicates(
    root: &VerifiedMediaRoot,
    max_distance: u32,
) -> Result<ToolManifest, ToolError> {
    let media_files = collect_media_files(root)?;
    plan_near_duplicates_selected(root, max_distance, &media_files)
}

pub fn plan_near_duplicates_selected(
    root: &VerifiedMediaRoot,
    max_distance: u32,
    media_files: &[PathBuf],
) -> Result<ToolManifest, ToolError> {
    let media_files = validate_selected_media_files(root, media_files)?;
    let pairs = plan_similar_images_selected(root, max_distance, &media_files)?;
    let mut adjacency: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for pair in &pairs {
        adjacency
            .entry(pair.left.clone())
            .or_default()
            .insert(pair.right.clone());
        adjacency
            .entry(pair.right.clone())
            .or_default()
            .insert(pair.left.clone());
    }

    let mut unvisited = adjacency.keys().cloned().collect::<BTreeSet<_>>();
    let mut candidates = Vec::new();
    while let Some(keeper) = unvisited.iter().next().cloned() {
        let mut stack = vec![keeper.clone()];
        let mut component = BTreeSet::new();
        while let Some(relative) = stack.pop() {
            if !unvisited.remove(&relative) {
                continue;
            }
            component.insert(relative.clone());
            if let Some(neighbours) = adjacency.get(&relative) {
                stack.extend(neighbours.iter().rev().cloned());
            }
        }

        for relative_path in component.into_iter().skip(1) {
            let path = root.resolve_existing_file(&relative_path)?;
            candidates.push(ToolCandidate {
                relative_path,
                companion_paths: Vec::new(),
                reason: format!(
                    "near_duplicate_cluster_keeper:{};dhash_threshold:{max_distance}",
                    keeper.display()
                ),
                size: fs::metadata(path).map_err(ToolError::Io)?.len(),
                sha256: None,
            });
        }
    }

    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    attach_orphaned_sidecars(root, &media_files, &mut candidates)?;
    let mut manifest = new_manifest(root, ToolOperation::NearDedup, candidates)?;
    let mut fingerprinted = manifest
        .file_fingerprints
        .iter()
        .map(|fingerprint| fingerprint.relative_path.clone())
        .collect::<BTreeSet<_>>();
    for relative_path in pairs.iter().flat_map(|pair| [&pair.left, &pair.right]) {
        if !fingerprinted.insert(relative_path.clone()) {
            continue;
        }
        let path = root.resolve_existing_file(relative_path)?;
        manifest.file_fingerprints.push(ToolFileFingerprint {
            relative_path: relative_path.clone(),
            size: fs::metadata(&path).map_err(ToolError::Io)?.len(),
            sha256: sha256_file(&path)?,
        });
    }
    manifest
        .file_fingerprints
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    manifest.pairs = pairs;
    Ok(manifest)
}

pub fn resize_to_jpeg_with_quarantine(
    root: &VerifiedMediaRoot,
    source_relative: &Path,
    max_size: u32,
    quality: u8,
    batch_id: &str,
) -> Result<ResizeResult, ToolError> {
    if max_size == 0 || !(1..=100).contains(&quality) {
        return Err(ToolError::InvalidManifest(
            "缩放尺寸必须大于 0，JPEG 质量必须在 1..=100".to_string(),
        ));
    }
    validate_batch_id(batch_id)?;
    let source_relative =
        validate_relative_path(source_relative).map_err(|_| ToolError::InvalidRelativePath)?;
    if !is_decodable_image(&source_relative) {
        return Err(ToolError::InvalidManifest(
            "该格式不能安全解码为 JPEG".to_string(),
        ));
    }
    let source = root.resolve_existing_file(&source_relative)?;
    let output_relative = source_relative.with_extension("jpg");
    let output = root.resolve(&output_relative)?;
    if output != source && output.exists() {
        return Err(ToolError::Conflict(output_relative));
    }

    let image = open_limited_image(&source)?;
    let (source_width, source_height) = image.dimensions();
    let (width, height) = scaled_dimensions(source_width, source_height, max_size);
    let resized = if (width, height) == (source_width, source_height) {
        image
    } else {
        image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    };
    let rgb = flatten_alpha(&resized);

    let output_name = output_relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ToolError::InvalidRelativePath)?;
    let temporary_relative =
        output_relative.with_file_name(format!(".{output_name}.tmp-{}", uuid::Uuid::new_v4()));
    let temporary = root.resolve(&temporary_relative)?;
    if let Some(parent) = temporary.parent() {
        fs::create_dir_all(parent).map_err(ToolError::Io)?;
    }
    write_jpeg_file(&temporary, &rgb, quality)?;

    let size = fs::metadata(&source).map_err(ToolError::Io)?.len();
    let mut manifest = new_manifest(
        root,
        ToolOperation::IntegrityCheck,
        vec![ToolCandidate {
            relative_path: source_relative.clone(),
            companion_paths: Vec::new(),
            reason: "resize_original".to_string(),
            size,
            sha256: None,
        }],
    )?;
    manifest.batch_id = batch_id.to_string();
    if let Err(error) = apply_quarantine(root, &manifest) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    if let Err(error) = fs::rename(&temporary, &output) {
        let _ = fs::remove_file(&temporary);
        let quarantined = root
            .path()
            .join(QUARANTINE_DIR)
            .join(batch_id)
            .join(&source_relative);
        if !source.exists() {
            if let Some(parent) = source.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::rename(quarantined, &source);
        }
        return Err(ToolError::Io(error));
    }

    Ok(ResizeResult {
        output_relative,
        width,
        height,
        quarantine_batch: batch_id.to_string(),
    })
}

fn scaled_dimensions(width: u32, height: u32, max_size: u32) -> (u32, u32) {
    if width <= max_size && height <= max_size {
        return (width, height);
    }
    let scale = max_size as f64 / width.max(height) as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn flatten_alpha(image: &DynamicImage) -> image::RgbImage {
    let rgba = image.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y).0;
        let alpha = u16::from(pixel[3]);
        let composite =
            |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        image::Rgb([
            composite(pixel[0]),
            composite(pixel[1]),
            composite(pixel[2]),
        ])
    })
}

fn write_jpeg_file(path: &Path, image: &image::RgbImage, quality: u8) -> Result<(), ToolError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(ToolError::Io)?;
    let mut writer = std::io::BufWriter::new(file);
    let mut encoder = JpegEncoder::new_with_quality(&mut writer, quality);
    encoder.encode(
        image,
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    drop(encoder);
    writer.flush().map_err(ToolError::Io)?;
    writer.get_ref().sync_all().map_err(ToolError::Io)?;
    Ok(())
}

fn difference_hash(image: &DynamicImage) -> u64 {
    let gray = image
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if gray.get_pixel(x, y)[0] > gray.get_pixel(x + 1, y)[0] {
                hash |= 1;
            }
        }
    }
    hash
}

fn open_limited_image(path: &Path) -> Result<DynamicImage, ToolError> {
    const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
    const MAX_DIMENSION: u32 = 16_384;
    const MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
    let metadata = fs::metadata(path).map_err(ToolError::Io)?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(ToolError::InvalidManifest(
            "图片源文件超过 512 MiB 安全上限".to_string(),
        ));
    }
    let mut reader = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(ToolError::Io)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    reader.limits(limits);
    reader.decode().map_err(ToolError::Image)
}

fn normalize_tag_token(tag: &str) -> String {
    tag.split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .to_lowercase()
}

fn sidecar_contains_tag(content: &str, wanted: &str) -> bool {
    if content.contains([',', '\n', '\r']) {
        content
            .split([',', '\n', '\r'])
            .map(normalize_tag_token)
            .any(|token| token == wanted)
    } else {
        content
            .split_whitespace()
            .map(normalize_tag_token)
            .any(|token| token == wanted)
    }
}

pub fn apply_quarantine(
    root: &VerifiedMediaRoot,
    manifest: &ToolManifest,
) -> Result<QuarantineResult, ToolError> {
    validate_batch_id(&manifest.batch_id)?;
    if manifest.root_fingerprint.is_empty() || manifest.root_fingerprint != root_fingerprint(root) {
        return Err(ToolError::InvalidManifest(
            "预检清单不属于当前媒体根".to_string(),
        ));
    }
    let mut listed_paths = manifest
        .candidates
        .iter()
        .flat_map(|candidate| {
            std::iter::once(candidate.relative_path.clone())
                .chain(candidate.companion_paths.iter().cloned())
        })
        .collect::<BTreeSet<_>>();
    if manifest.operation == ToolOperation::NearDedup {
        for pair in &manifest.pairs {
            listed_paths.insert(pair.left.clone());
            listed_paths.insert(pair.right.clone());
        }
    }
    let fingerprint_paths = manifest
        .file_fingerprints
        .iter()
        .map(|fingerprint| fingerprint.relative_path.clone())
        .collect::<BTreeSet<_>>();
    if listed_paths != fingerprint_paths {
        return Err(ToolError::InvalidManifest(
            "预检文件指纹集合与操作清单不一致".to_string(),
        ));
    }
    for fingerprint in &manifest.file_fingerprints {
        let path = root.resolve_existing_file(&fingerprint.relative_path)?;
        let size = fs::metadata(&path).map_err(ToolError::Io)?.len();
        let sha256 = sha256_file(&path)?;
        if size != fingerprint.size || sha256 != fingerprint.sha256 {
            return Err(ToolError::InvalidManifest(format!(
                "预检后文件已变化: {}",
                fingerprint.relative_path.display()
            )));
        }
    }
    if manifest.candidates.is_empty() {
        return Ok(QuarantineResult {
            batch_id: manifest.batch_id.clone(),
            moved: 0,
            paths: Vec::new(),
        });
    }

    let mut relative_paths = Vec::new();
    let mut unique_paths = HashSet::new();
    for candidate in &manifest.candidates {
        for relative in
            std::iter::once(&candidate.relative_path).chain(candidate.companion_paths.iter())
        {
            validate_relative_path(relative).map_err(|_| ToolError::InvalidRelativePath)?;
            if relative
                .components()
                .next()
                .is_some_and(|component| is_quarantine_dir_name(component.as_os_str()))
            {
                return Err(ToolError::InvalidManifest(
                    "操作清单不能引用隔离区自身".to_string(),
                ));
            }
            if unique_paths.insert(relative.clone()) {
                relative_paths.push(relative.clone());
            }
        }
    }
    relative_paths.sort();

    let quarantine_relative = PathBuf::from(QUARANTINE_DIR).join(&manifest.batch_id);
    let mut moves = Vec::with_capacity(relative_paths.len());
    for relative in &relative_paths {
        let source = root.resolve_existing_file(relative)?;
        let destination_relative = quarantine_relative.join(relative);
        let destination = root.resolve(&destination_relative)?;
        if destination.exists() {
            return Err(ToolError::Conflict(destination_relative));
        }
        moves.push((source, destination, relative.clone()));
    }

    let mut completed: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, destination, _) in &moves {
        let parent = destination.parent().ok_or(ToolError::InvalidRelativePath)?;
        fs::create_dir_all(parent).map_err(ToolError::Io)?;
        let checked_destination = root.resolve(
            destination
                .strip_prefix(root.path())
                .map_err(|_| ToolError::OutsideRoot)?,
        )?;
        if checked_destination.exists() {
            rollback_moves(&completed);
            return Err(ToolError::Conflict(
                checked_destination
                    .strip_prefix(root.path())
                    .unwrap_or(&checked_destination)
                    .to_path_buf(),
            ));
        }
        if let Err(error) = fs::rename(source, &checked_destination) {
            rollback_moves(&completed);
            return Err(ToolError::Io(error));
        }
        completed.push((checked_destination, source.clone()));
    }

    Ok(QuarantineResult {
        batch_id: manifest.batch_id.clone(),
        moved: relative_paths.len(),
        paths: relative_paths,
    })
}

pub fn restore_quarantine(
    root: &VerifiedMediaRoot,
    batch_id: &str,
) -> Result<RestoreResult, ToolError> {
    validate_batch_id(batch_id)?;
    let batch_relative = PathBuf::from(QUARANTINE_DIR).join(batch_id);
    let batch_path = root.resolve(&batch_relative)?;
    if !batch_path.is_dir() {
        return Err(ToolError::InvalidManifest(
            "隔离批次不存在或不是目录".to_string(),
        ));
    }
    let canonical_batch = fs::canonicalize(&batch_path).map_err(ToolError::Io)?;
    if !canonical_batch.starts_with(root.path()) {
        return Err(ToolError::OutsideRoot);
    }

    let mut quarantined_files = Vec::new();
    for entry in WalkDir::new(&canonical_batch).follow_links(false) {
        let entry = entry.map_err(|error| {
            error
                .into_io_error()
                .map(ToolError::Io)
                .unwrap_or_else(|| ToolError::InvalidManifest("无法遍历隔离批次".to_string()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&canonical_batch)
            .map_err(|_| ToolError::OutsideRoot)?
            .to_path_buf();
        validate_relative_path(&relative).map_err(|_| ToolError::InvalidRelativePath)?;
        quarantined_files.push((entry.path().to_path_buf(), relative));
    }
    quarantined_files.sort_by(|left, right| left.1.cmp(&right.1));

    let mut restored = 0;
    let mut conflicts = Vec::new();
    for (source, relative) in &quarantined_files {
        let destination = root.resolve(relative)?;
        if destination.exists() {
            conflicts.push(relative.clone());
            continue;
        }
        let parent = destination.parent().ok_or(ToolError::InvalidRelativePath)?;
        fs::create_dir_all(parent).map_err(ToolError::Io)?;
        let destination = root.resolve(relative)?;
        if destination.exists() {
            conflicts.push(relative.clone());
            continue;
        }
        fs::rename(source, destination).map_err(ToolError::Io)?;
        restored += 1;
    }
    prune_empty_directories(&canonical_batch)?;

    Ok(RestoreResult {
        batch_id: batch_id.to_string(),
        restored,
        conflicts,
        remaining: quarantined_files.len().saturating_sub(restored),
    })
}

#[cfg(test)]
pub fn purge_quarantine(root: &VerifiedMediaRoot, batch_id: &str) -> Result<usize, ToolError> {
    validate_batch_id(batch_id)?;
    let batch_relative = PathBuf::from(QUARANTINE_DIR).join(batch_id);
    let batch_path = root.resolve(&batch_relative)?;
    if !batch_path.is_dir() {
        return Err(ToolError::InvalidManifest(
            "隔离批次不存在或不是目录".to_string(),
        ));
    }
    let canonical_batch = fs::canonicalize(&batch_path).map_err(ToolError::Io)?;
    if !canonical_batch.starts_with(root.path()) {
        return Err(ToolError::OutsideRoot);
    }

    let mut entries = WalkDir::new(&canonical_batch)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            error
                .into_io_error()
                .map(ToolError::Io)
                .unwrap_or_else(|| ToolError::InvalidManifest("无法遍历隔离批次".to_string()))
        })?;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.depth()));
    let removed = entries
        .iter()
        .filter(|entry| entry.file_type().is_file())
        .count();
    for entry in entries {
        if entry.file_type().is_dir() {
            fs::remove_dir(entry.path()).map_err(ToolError::Io)?;
        } else {
            fs::remove_file(entry.path()).map_err(ToolError::Io)?;
        }
    }
    fs::remove_dir(&canonical_batch).map_err(ToolError::Io)?;
    Ok(removed)
}

fn rollback_moves(completed: &[(PathBuf, PathBuf)]) {
    for (quarantined, original) in completed.iter().rev() {
        if let Some(parent) = original.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::rename(quarantined, original);
    }
}

fn prune_empty_directories(root: &Path) -> Result<(), ToolError> {
    let mut directories = WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        if fs::read_dir(&directory)
            .map_err(ToolError::Io)?
            .next()
            .is_none()
        {
            fs::remove_dir(&directory).map_err(ToolError::Io)?;
        }
    }
    if root.is_dir() && fs::read_dir(root).map_err(ToolError::Io)?.next().is_none() {
        fs::remove_dir(root).map_err(ToolError::Io)?;
    }
    Ok(())
}

fn validate_batch_id(batch_id: &str) -> Result<(), ToolError> {
    if batch_id.is_empty()
        || batch_id.len() > 128
        || !batch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ToolError::InvalidManifest(
            "隔离批次 ID 只能包含 ASCII 字母、数字、连字符和下划线".to_string(),
        ));
    }
    Ok(())
}

fn new_manifest(
    root: &VerifiedMediaRoot,
    operation: ToolOperation,
    candidates: Vec<ToolCandidate>,
) -> Result<ToolManifest, ToolError> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut paths = BTreeSet::new();
    for candidate in &candidates {
        paths.insert(candidate.relative_path.clone());
        paths.extend(candidate.companion_paths.iter().cloned());
    }
    let mut file_fingerprints = Vec::with_capacity(paths.len());
    for relative_path in paths {
        let path = root.resolve_existing_file(&relative_path)?;
        file_fingerprints.push(ToolFileFingerprint {
            relative_path,
            size: fs::metadata(&path).map_err(ToolError::Io)?.len(),
            sha256: sha256_file(&path)?,
        });
    }
    Ok(ToolManifest {
        batch_id: uuid::Uuid::new_v4().to_string(),
        operation,
        created_at,
        root_fingerprint: root_fingerprint(root),
        file_fingerprints,
        pairs: Vec::new(),
        tag_pipeline_config: None,
        candidates,
    })
}

fn root_fingerprint(root: &VerifiedMediaRoot) -> String {
    let path = root.path().to_string_lossy();
    #[cfg(windows)]
    let path = path.to_ascii_lowercase();
    let mut digest = Sha256::new();
    digest.update(path.as_bytes());
    hex::encode(digest.finalize())
}

fn collect_media_files(root: &VerifiedMediaRoot) -> Result<Vec<PathBuf>, ToolError> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root.path())
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_quarantine_dir_name(entry.file_name()));
    for entry in walker {
        let entry = entry.map_err(|error| {
            error
                .into_io_error()
                .map(ToolError::Io)
                .unwrap_or_else(|| ToolError::InvalidManifest("无法遍历媒体根目录".to_string()))
        })?;
        if !entry.file_type().is_file() || !has_supported_media_extension(entry.path()) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root.path())
            .map_err(|_| ToolError::OutsideRoot)?
            .to_path_buf();
        files.push(relative);
    }
    files.sort();
    Ok(files)
}

fn validate_selected_media_files(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
) -> Result<Vec<PathBuf>, ToolError> {
    let mut selected = BTreeSet::new();
    for relative in media_files {
        let relative =
            validate_relative_path(relative).map_err(|_| ToolError::InvalidRelativePath)?;
        if !has_supported_media_extension(&relative) {
            return Err(ToolError::InvalidManifest(format!(
                "不支持的媒体格式: {}",
                relative.display()
            )));
        }
        root.resolve_existing_file(&relative)?;
        selected.insert(relative);
    }
    Ok(selected.into_iter().collect())
}

fn has_supported_media_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| SAFE_MEDIA_EXTS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_decodable_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp"
            )
        })
        .unwrap_or(false)
}

fn existing_sidecars(
    root: &VerifiedMediaRoot,
    media_relative: &Path,
) -> Result<Vec<PathBuf>, ToolError> {
    let sidecar = media_relative.with_extension("txt");
    let sidecar_path = root.resolve(&sidecar)?;
    if sidecar_path.is_file() {
        Ok(vec![sidecar])
    } else {
        Ok(Vec::new())
    }
}

fn attach_orphaned_sidecars(
    root: &VerifiedMediaRoot,
    media_files: &[PathBuf],
    candidates: &mut [ToolCandidate],
) -> Result<(), ToolError> {
    let candidate_paths = candidates
        .iter()
        .map(|candidate| candidate.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let surviving_sidecars = media_files
        .iter()
        .filter(|media| !candidate_paths.contains(*media))
        .map(|media| media.with_extension("txt"))
        .collect::<BTreeSet<_>>();
    let mut attached = BTreeSet::new();

    for candidate in candidates {
        for sidecar in existing_sidecars(root, &candidate.relative_path)? {
            if !surviving_sidecars.contains(&sidecar) && attached.insert(sidecar.clone()) {
                candidate.companion_paths.push(sidecar);
            }
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, ToolError> {
    let mut file = File::open(path).map_err(ToolError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ToolError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_pipeline_normalizes_deduplicates_and_escapes_tokens() {
        assert_eq!(
            transform_tag_content("blue_hair, smile\nblue_hair; character_(series)\r\n"),
            "blue hair,smile,character \\(series\\)"
        );
    }

    #[test]
    fn tag_pipeline_preserves_legacy_category_order_filtering_and_artist_prefix() {
        let categories = BTreeMap::from([
            ("landscape".to_string(), Some(0)),
            ("alice".to_string(), Some(4)),
            ("john_doe".to_string(), Some(1)),
            ("series_name".to_string(), Some(3)),
            ("studio_name".to_string(), Some(5)),
            ("highres".to_string(), Some(6)),
        ]);

        assert_eq!(
            transform_tag_content_with_categories(
                "landscape,alice,john_doe,series_name,studio_name,highres",
                &categories,
                ArtistPrefix::Artist,
            ),
            "artist:john doe,alice,landscape"
        );
    }

    #[test]
    fn tag_pipeline_preflight_only_lists_changed_selected_sidecars() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").expect("selected media");
        fs::write(directory.path().join("selected.txt"), "blue_hair,blue_hair")
            .expect("selected tags");
        fs::write(directory.path().join("without-tags.jpg"), b"media")
            .expect("media without sidecar");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_tag_pipeline(
            &root,
            &[
                PathBuf::from("selected.jpg"),
                PathBuf::from("without-tags.jpg"),
            ],
        )
        .expect("tag preflight");

        assert_eq!(manifest.operation, ToolOperation::TagPipeline);
        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            Path::new("selected.txt")
        );
        assert_eq!(manifest.file_fingerprints.len(), 1);
    }

    #[test]
    fn integrity_preflight_can_be_scoped_to_selected_directory_media() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"").expect("selected corrupt image");
        fs::write(directory.path().join("outside.jpg"), b"").expect("outside corrupt image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_integrity_check_selected(&root, &[PathBuf::from("selected.jpg")])
            .expect("scoped integrity preflight");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("selected.jpg")
        );
    }

    #[test]
    fn exact_dedup_preflight_does_not_compare_files_outside_the_selected_directory() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected-a.jpg"), b"duplicate").unwrap();
        fs::write(directory.path().join("selected-b.jpg"), b"duplicate").unwrap();
        fs::write(directory.path().join("outside.jpg"), b"duplicate").unwrap();
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_exact_duplicates_selected(
            &root,
            &[
                PathBuf::from("selected-a.jpg"),
                PathBuf::from("selected-b.jpg"),
            ],
        )
        .expect("scoped dedup preflight");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("selected-b.jpg")
        );
    }

    #[test]
    fn delete_by_tag_preflight_only_matches_selected_directory_media() {
        let directory = tempfile::tempdir().expect("temp root");
        for stem in ["selected", "outside"] {
            fs::write(directory.path().join(format!("{stem}.jpg")), b"media").unwrap();
            fs::write(directory.path().join(format!("{stem}.txt")), "remove_me").unwrap();
        }
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest =
            plan_delete_by_tag_selected(&root, "remove_me", &[PathBuf::from("selected.jpg")])
                .expect("scoped delete preflight");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("selected.jpg")
        );
    }

    #[test]
    fn delete_selected_moves_the_selected_media_and_its_sidecar_to_quarantine() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").unwrap();
        fs::write(directory.path().join("selected.txt"), b"tag_a, tag_b").unwrap();
        fs::write(directory.path().join("outside.jpg"), b"outside").unwrap();
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_delete_selected(&root, &[PathBuf::from("selected.jpg")])
            .expect("selected delete preflight");
        let result = apply_quarantine(&root, &manifest).expect("selected delete apply");

        assert_eq!(manifest.operation, ToolOperation::DeleteSelected);
        assert_eq!(result.moved, 2);
        assert!(!directory.path().join("selected.jpg").exists());
        assert!(!directory.path().join("selected.txt").exists());
        assert!(directory.path().join("outside.jpg").exists());
        assert!(directory
            .path()
            .join(QUARANTINE_DIR)
            .join(&manifest.batch_id)
            .join("selected.jpg")
            .exists());
        assert!(directory
            .path()
            .join(QUARANTINE_DIR)
            .join(&manifest.batch_id)
            .join("selected.txt")
            .exists());
    }

    #[test]
    fn near_dedup_preflight_only_compares_selected_directory_media() {
        let directory = tempfile::tempdir().expect("temp root");
        for name in ["selected-a.png", "selected-b.png", "outside.png"] {
            image::RgbImage::from_pixel(16, 16, image::Rgb([10, 20, 30]))
                .save(directory.path().join(name))
                .unwrap();
        }
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_near_duplicates_selected(
            &root,
            1,
            &[
                PathBuf::from("selected-a.png"),
                PathBuf::from("selected-b.png"),
            ],
        )
        .expect("scoped near dedup preflight");

        assert!(manifest.pairs.iter().all(
            |pair| !pair.left.ends_with("outside.png") && !pair.right.ends_with("outside.png")
        ));
        assert_eq!(manifest.pairs.len(), 1);
    }

    #[test]
    fn tag_pipeline_quarantines_original_before_atomic_replacement() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").expect("selected media");
        fs::write(directory.path().join("selected.txt"), "blue_hair,blue_hair")
            .expect("selected tags");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let mut manifest =
            plan_tag_pipeline(&root, &[PathBuf::from("selected.jpg")]).expect("tag preflight");
        manifest.batch_id = "tag-task-1".to_string();

        let result = apply_tag_pipeline(&root, &manifest).expect("apply tag pipeline");

        assert_eq!(result.changed, 1);
        assert_eq!(
            fs::read_to_string(directory.path().join("selected.txt")).unwrap(),
            "blue hair"
        );
        assert_eq!(
            fs::read_to_string(
                directory
                    .path()
                    .join(".danbooru-quarantine/tag-task-1/selected.txt")
            )
            .unwrap(),
            "blue_hair,blue_hair"
        );
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")));
    }

    #[test]
    fn classified_tag_pipeline_manifest_preserves_prefix_rules_through_confirmation() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").unwrap();
        fs::write(
            directory.path().join("selected.txt"),
            "landscape,john_doe,alice,series_name",
        )
        .unwrap();
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let config = TagPipelineConfig {
            artist_prefix: ArtistPrefix::At,
            categories: BTreeMap::from([
                ("landscape".to_string(), Some(0)),
                ("john_doe".to_string(), Some(1)),
                ("alice".to_string(), Some(4)),
                ("series_name".to_string(), Some(3)),
            ]),
        };
        let manifest =
            plan_tag_pipeline_classified(&root, &[PathBuf::from("selected.jpg")], config)
                .expect("classified preflight");

        apply_tag_pipeline(&root, &manifest).expect("classified apply");

        assert_eq!(
            fs::read_to_string(directory.path().join("selected.txt")).unwrap(),
            "@john doe,alice,landscape"
        );
    }

    #[test]
    fn tag_pipeline_collects_unique_original_tokens_for_category_lookup() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").unwrap();
        fs::write(
            directory.path().join("selected.txt"),
            "john_doe, alice\njohn_doe;series_name",
        )
        .unwrap();
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let tags = collect_tag_pipeline_tokens(&root, &[PathBuf::from("selected.jpg")])
            .expect("tag tokens");

        assert_eq!(
            tags.into_iter().collect::<Vec<_>>(),
            ["alice", "john_doe", "series_name"]
        );
    }

    #[test]
    fn tag_pipeline_rejects_an_in_root_linked_media_directory() {
        let directory = tempfile::tempdir().expect("temp root");
        let real = directory.path().join("real");
        let linked = directory.path().join("linked");
        fs::create_dir_all(&real).expect("real directory");
        fs::write(real.join("selected.jpg"), b"media").expect("selected media");
        fs::write(real.join("selected.txt"), "blue_hair").expect("selected tags");
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&real)
            .status()
            .unwrap()
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &linked).expect("linked directory");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let result = plan_tag_pipeline(&root, &[PathBuf::from("linked/selected.jpg")]);

        assert!(matches!(result, Err(ToolError::OutsideRoot)));
    }

    #[test]
    fn tag_pipeline_rejects_sidecars_larger_than_eight_mib_before_reading() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.jpg"), b"media").expect("selected media");
        fs::write(
            directory.path().join("selected.txt"),
            vec![b'a'; 8 * 1024 * 1024 + 1],
        )
        .expect("oversized sidecar");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let error = plan_tag_pipeline(&root, &[PathBuf::from("selected.jpg")]).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("8 MiB"));
        assert!(message.contains("selected.txt"));
        assert!(!message.contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn heic_preflight_lists_only_selected_registered_relative_files() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.heic"), b"heic").expect("selected HEIC");
        fs::write(directory.path().join("unselected.heic"), b"heic").expect("unselected HEIC");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest =
            plan_heic_conversion(&root, &[PathBuf::from("selected.heic")]).expect("HEIC preflight");

        assert_eq!(manifest.operation, ToolOperation::HeicConvert);
        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            Path::new("selected.heic")
        );
        assert!(!serde_json::to_string(&manifest)
            .unwrap()
            .contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn heic_conversion_quarantines_original_and_atomically_publishes_jpeg() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.heic"), b"fake-heic").expect("selected HEIC");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let mut manifest =
            plan_heic_conversion(&root, &[PathBuf::from("selected.heic")]).expect("HEIC preflight");
        manifest.batch_id = "heic-task-1".to_string();

        let result = apply_heic_conversion_with(&root, &manifest, |_input, output| {
            image::RgbImage::from_pixel(3, 2, image::Rgb([20, 40, 60]))
                .save_with_format(output, image::ImageFormat::Jpeg)
                .map_err(ToolError::Image)
        })
        .expect("HEIC conversion");

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].output_relative, Path::new("selected.jpg"));
        assert_eq!((result.items[0].width, result.items[0].height), (3, 2));
        assert!(!directory.path().join("selected.heic").exists());
        assert!(directory.path().join("selected.jpg").is_file());
        assert_eq!(
            fs::read(
                directory
                    .path()
                    .join(".danbooru-quarantine/heic-task-1/selected.heic")
            )
            .unwrap(),
            b"fake-heic"
        );
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")));
    }

    #[test]
    fn missing_heif_converter_has_a_stable_non_path_error() {
        let directory = tempfile::tempdir().expect("temp root");
        let error = run_converter_process(
            OsStr::new("definitely-missing-heif-convert-binary"),
            &directory.path().join("input.heic"),
            &directory.path().join("output.jpg"),
            std::time::Duration::from_millis(10),
        )
        .unwrap_err();

        assert!(matches!(error, ToolError::ConverterUnavailable));
        assert!(!error
            .to_string()
            .contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn unavailable_heif_converter_restores_original_before_returning() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("selected.heic"), b"fake-heic").expect("selected HEIC");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let mut manifest =
            plan_heic_conversion(&root, &[PathBuf::from("selected.heic")]).expect("HEIC preflight");
        manifest.batch_id = "heic-unavailable".to_string();

        let error = apply_heic_conversion_with(&root, &manifest, |_input, _output| {
            Err(ToolError::ConverterUnavailable)
        })
        .unwrap_err();

        assert!(matches!(error, ToolError::ConverterUnavailable));
        assert_eq!(
            fs::read(directory.path().join("selected.heic")).unwrap(),
            b"fake-heic"
        );
        assert!(!directory.path().join("selected.jpg").exists());
        assert!(!directory.path().join(".danbooru-quarantine").exists());
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")));
    }

    #[test]
    fn relative_paths_reject_parent_components() {
        assert!(validate_relative_path(Path::new("../outside.jpg")).is_err());
    }

    #[test]
    fn verified_root_resolves_only_children() {
        let directory = tempfile::tempdir().expect("temp root");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        assert!(root.resolve(Path::new("nested/image.jpg")).is_ok());
        assert!(root.resolve(Path::new("../outside.jpg")).is_err());
    }

    #[test]
    fn verified_media_root_rejects_a_root_path_that_is_itself_a_link() {
        let directory = tempfile::tempdir().expect("temp base");
        let real_root = directory.path().join("real");
        let linked_root = directory.path().join("linked");
        fs::create_dir(&real_root).expect("real root");
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&linked_root)
            .arg(&real_root)
            .status()
            .expect("create junction")
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_root, &linked_root).expect("create symlink");

        let result = VerifiedMediaRoot::open(&linked_root);

        #[cfg(windows)]
        fs::remove_dir(&linked_root).expect("remove junction");
        #[cfg(unix)]
        fs::remove_file(&linked_root).expect("remove symlink");
        assert!(result.is_err());
    }

    #[test]
    fn exact_dedup_produces_a_non_destructive_manifest() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("a.jpg"), b"same bytes").expect("first image");
        fs::write(directory.path().join("b.jpg"), b"same bytes").expect("second image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_exact_duplicates(&root).expect("dedup plan");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(manifest.candidates[0].relative_path, PathBuf::from("b.jpg"));
        assert!(directory.path().join("a.jpg").exists());
        assert!(directory.path().join("b.jpg").exists());
    }

    #[cfg(windows)]
    #[test]
    fn exact_dedup_does_not_scan_case_variant_of_reserved_quarantine_directory() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("a.jpg"), b"same bytes").expect("first image");
        fs::write(directory.path().join("b.jpg"), b"same bytes").expect("second image");
        let hidden = directory.path().join(".DANBOORU-QUARANTINE/batch");
        fs::create_dir_all(&hidden).expect("case-variant quarantine directory");
        fs::write(hidden.join("hidden.jpg"), b"same bytes").expect("hidden image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_exact_duplicates(&root).expect("dedup plan");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(manifest.candidates[0].relative_path, PathBuf::from("b.jpg"));
    }

    #[test]
    fn exact_dedup_keeps_a_sidecar_still_used_by_the_surviving_media() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("a.jpg"), b"same bytes").expect("keeper image");
        fs::write(directory.path().join("a.png"), b"same bytes").expect("duplicate image");
        fs::write(directory.path().join("a.txt"), b"shared tags").expect("shared sidecar");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let mut manifest = plan_exact_duplicates(&root).expect("dedup plan");
        manifest.batch_id = "shared-sidecar".to_string();

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(manifest.candidates[0].relative_path, PathBuf::from("a.png"));
        assert!(manifest.candidates[0].companion_paths.is_empty());

        apply_quarantine(&root, &manifest).expect("quarantine duplicate");

        assert!(directory.path().join("a.jpg").is_file());
        assert!(directory.path().join("a.txt").is_file());
        assert!(!directory.path().join("a.png").exists());
    }

    #[test]
    fn confirmed_manifest_rejects_a_primary_file_changed_after_preflight() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("a.jpg"), b"same bytes").expect("first image");
        fs::write(directory.path().join("b.jpg"), b"same bytes").expect("second image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let manifest = plan_exact_duplicates(&root).expect("dedup plan");
        let candidate = manifest.candidates[0].relative_path.clone();
        fs::write(
            directory.path().join(&candidate),
            b"changed after preflight",
        )
        .expect("mutate candidate");

        let result = apply_quarantine(&root, &manifest);

        assert!(matches!(result, Err(ToolError::InvalidManifest(_))));
        assert!(directory.path().join(candidate).is_file());
    }

    #[test]
    fn confirmed_manifest_is_bound_to_the_preflight_root() {
        let first = tempfile::tempdir().expect("first root");
        fs::write(first.path().join("a.jpg"), b"same bytes").expect("first image");
        fs::write(first.path().join("b.jpg"), b"same bytes").expect("second image");
        let first_root = VerifiedMediaRoot::open(first.path()).expect("valid first root");
        let manifest = plan_exact_duplicates(&first_root).expect("dedup plan");

        let second = tempfile::tempdir().expect("second root");
        fs::write(second.path().join("b.jpg"), b"same bytes").expect("matching decoy");
        let second_root = VerifiedMediaRoot::open(second.path()).expect("valid second root");

        let result = apply_quarantine(&second_root, &manifest);

        assert!(matches!(result, Err(ToolError::InvalidManifest(_))));
        assert!(second.path().join("b.jpg").is_file());
    }

    #[test]
    fn confirmed_tag_manifest_rejects_a_changed_sidecar() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("image.jpg"), b"image").expect("image");
        fs::write(directory.path().join("image.txt"), b"cat").expect("sidecar");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let manifest = plan_delete_by_tag(&root, "cat").expect("delete plan");
        fs::write(directory.path().join("image.txt"), b"dog").expect("changed sidecar");

        let result = apply_quarantine(&root, &manifest);

        assert!(matches!(result, Err(ToolError::InvalidManifest(_))));
        assert!(directory.path().join("image.jpg").is_file());
        assert!(directory.path().join("image.txt").is_file());
    }

    #[test]
    fn confirmed_manifest_moves_media_and_sidecar_to_quarantine() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::create_dir(directory.path().join("nested")).expect("nested directory");
        fs::write(directory.path().join("nested/b.jpg"), b"image").expect("image");
        fs::write(directory.path().join("nested/b.txt"), b"tag").expect("sidecar");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let mut manifest = new_manifest(
            &root,
            ToolOperation::DeleteByTag,
            vec![ToolCandidate {
                relative_path: PathBuf::from("nested/b.jpg"),
                companion_paths: vec![PathBuf::from("nested/b.txt")],
                reason: "tag:remove_me".to_string(),
                size: 5,
                sha256: None,
            }],
        )
        .expect("manifest fingerprints");
        manifest.batch_id = "batch-1".to_string();

        let result = apply_quarantine(&root, &manifest).expect("quarantine");

        assert_eq!(result.moved, 2);
        assert!(!directory.path().join("nested/b.jpg").exists());
        assert!(directory
            .path()
            .join(".danbooru-quarantine/batch-1/nested/b.jpg")
            .exists());
        assert!(directory
            .path()
            .join(".danbooru-quarantine/batch-1/nested/b.txt")
            .exists());
    }

    #[test]
    fn restore_never_overwrites_an_existing_file() {
        let directory = tempfile::tempdir().expect("temp root");
        let quarantined = directory
            .path()
            .join(".danbooru-quarantine/batch-2/nested/image.jpg");
        fs::create_dir_all(quarantined.parent().expect("quarantine parent"))
            .expect("quarantine directory");
        fs::write(&quarantined, b"old bytes").expect("quarantined image");
        fs::create_dir_all(directory.path().join("nested")).expect("media directory");
        fs::write(directory.path().join("nested/image.jpg"), b"new bytes")
            .expect("replacement image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let result = restore_quarantine(&root, "batch-2").expect("restore report");

        assert_eq!(result.restored, 0);
        assert_eq!(result.conflicts, vec![PathBuf::from("nested/image.jpg")]);
        assert_eq!(
            fs::read(&quarantined).expect("quarantined bytes"),
            b"old bytes"
        );
        assert_eq!(
            fs::read(directory.path().join("nested/image.jpg")).expect("live bytes"),
            b"new bytes"
        );
    }

    #[test]
    fn purge_removes_only_the_explicit_batch() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::create_dir_all(directory.path().join(".danbooru-quarantine/batch-a"))
            .expect("first batch");
        fs::create_dir_all(directory.path().join(".danbooru-quarantine/batch-b"))
            .expect("second batch");
        fs::write(
            directory.path().join(".danbooru-quarantine/batch-a/a.jpg"),
            b"a",
        )
        .expect("first image");
        fs::write(
            directory.path().join(".danbooru-quarantine/batch-b/b.jpg"),
            b"b",
        )
        .expect("second image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let removed = purge_quarantine(&root, "batch-a").expect("manual purge");

        assert_eq!(removed, 1);
        assert!(!directory
            .path()
            .join(".danbooru-quarantine/batch-a")
            .exists());
        assert!(directory
            .path()
            .join(".danbooru-quarantine/batch-b/b.jpg")
            .exists());
    }

    #[test]
    fn integrity_check_reports_corrupt_images_without_deleting_them() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("broken.jpg"), b"not a jpeg").expect("broken image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_integrity_check(&root).expect("integrity report");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("broken.jpg")
        );
        assert!(directory.path().join("broken.jpg").exists());
    }

    #[test]
    fn tag_delete_plan_matches_normalized_tokens_not_substrings() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("catgirl.jpg"), b"image").expect("first media");
        fs::write(directory.path().join("catgirl.txt"), "catgirl, blue_eyes").expect("first tags");
        fs::write(directory.path().join("cat.jpg"), b"image").expect("second media");
        fs::write(directory.path().join("cat.txt"), "portrait,\n Cat ").expect("second tags");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_delete_by_tag(&root, "cat").expect("tag delete plan");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("cat.jpg")
        );
        assert!(directory.path().join("cat.jpg").exists());
    }

    #[test]
    fn tag_delete_plan_accepts_legacy_space_delimited_tokens() {
        let directory = tempfile::tempdir().expect("temp root");
        fs::write(directory.path().join("legacy.jpg"), b"image").expect("media fixture");
        fs::write(directory.path().join("legacy.txt"), "dog cat fox").expect("tag fixture");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_delete_by_tag(&root, "cat").expect("tag delete plan");

        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(
            manifest.candidates[0].relative_path,
            PathBuf::from("legacy.jpg")
        );
    }

    #[test]
    fn similar_image_scan_returns_candidates_without_mutating_files() {
        let directory = tempfile::tempdir().expect("temp root");
        let image = image::GrayImage::from_pixel(32, 32, image::Luma([128]));
        image
            .save(directory.path().join("a.png"))
            .expect("first image");
        image
            .save(directory.path().join("b.png"))
            .expect("second image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let pairs = plan_similar_images(&root, 0).expect("similar image plan");

        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].distance, 0);
        assert!(directory.path().join("a.png").exists());
        assert!(directory.path().join("b.png").exists());
    }

    #[test]
    fn near_duplicate_plan_keeps_the_lexicographically_first_image() {
        let directory = tempfile::tempdir().expect("temp root");
        let image = image::GrayImage::from_pixel(32, 32, image::Luma([128]));
        for name in ["b.png", "a.png", "c.png"] {
            image
                .save(directory.path().join(name))
                .expect("fixture image");
        }
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let manifest = plan_near_duplicates(&root, 0).expect("near duplicate manifest");

        assert_eq!(manifest.operation, ToolOperation::NearDedup);
        assert_eq!(manifest.pairs.len(), 3);
        assert_eq!(
            manifest
                .candidates
                .iter()
                .map(|candidate| candidate.relative_path.clone())
                .collect::<Vec<_>>(),
            [PathBuf::from("b.png"), PathBuf::from("c.png")]
        );
        assert!(manifest.candidates.iter().all(|candidate| candidate
            .reason
            .contains("near_duplicate_cluster_keeper:a.png")));
        for name in ["a.png", "b.png", "c.png"] {
            assert!(directory.path().join(name).is_file());
        }
    }

    #[test]
    fn near_duplicate_confirmation_rejects_a_changed_keeper() {
        let directory = tempfile::tempdir().expect("temp root");
        let image = image::GrayImage::from_pixel(32, 32, image::Luma([128]));
        for name in ["a.png", "b.png"] {
            image
                .save(directory.path().join(name))
                .expect("fixture image");
        }
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");
        let manifest = plan_near_duplicates(&root, 0).expect("near duplicate manifest");
        fs::write(directory.path().join("a.png"), b"changed keeper")
            .expect("replace keeper after preflight");

        let result = apply_quarantine(&root, &manifest);

        assert!(matches!(result, Err(ToolError::InvalidManifest(_))));
        assert!(directory.path().join("b.png").is_file());
        assert!(!directory.path().join(QUARANTINE_DIR).exists());
    }

    #[test]
    fn similar_image_scan_stops_before_an_unbounded_pair_manifest() {
        let directory = tempfile::tempdir().expect("temp root");
        let image = image::GrayImage::from_pixel(2, 2, image::Luma([128]));
        for index in 0..142 {
            image
                .save(directory.path().join(format!("{index}.png")))
                .expect("fixture image");
        }
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let error = plan_similar_images(&root, 0).expect_err("pair manifest must be capped");

        assert!(error.to_string().contains("10000"));
    }

    #[test]
    fn image_decoder_rejects_huge_declared_dimensions_before_allocation() {
        let directory = tempfile::tempdir().expect("temp root");
        let path = directory.path().join("bomb.bmp");
        let mut header = vec![0_u8; 54];
        header[0..2].copy_from_slice(b"BM");
        header[2..6].copy_from_slice(&54_u32.to_le_bytes());
        header[10..14].copy_from_slice(&54_u32.to_le_bytes());
        header[14..18].copy_from_slice(&40_u32.to_le_bytes());
        header[18..22].copy_from_slice(&20_000_i32.to_le_bytes());
        header[22..26].copy_from_slice(&20_000_i32.to_le_bytes());
        header[26..28].copy_from_slice(&1_u16.to_le_bytes());
        header[28..30].copy_from_slice(&24_u16.to_le_bytes());
        fs::write(&path, header).expect("bomb header");

        let error = open_limited_image(&path).expect_err("huge image must be rejected");

        assert!(matches!(
            error,
            ToolError::Image(image::ImageError::Limits(_))
        ));
    }

    #[test]
    fn resize_writes_atomically_after_quarantining_the_original() {
        let directory = tempfile::tempdir().expect("temp root");
        let image = image::RgbImage::from_pixel(16, 8, image::Rgb([20, 40, 60]));
        image
            .save(directory.path().join("image.png"))
            .expect("source image");
        let root = VerifiedMediaRoot::open(directory.path()).expect("valid root");

        let result =
            resize_to_jpeg_with_quarantine(&root, Path::new("image.png"), 8, 90, "batch-resize")
                .expect("resize");

        assert_eq!(result.output_relative, PathBuf::from("image.jpg"));
        assert_eq!((result.width, result.height), (8, 4));
        assert!(!directory.path().join("image.png").exists());
        assert!(directory.path().join("image.jpg").exists());
        assert!(directory
            .path()
            .join(".danbooru-quarantine/batch-resize/image.png")
            .exists());
        assert!(fs::read_dir(directory.path())
            .expect("root listing")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")));
    }
}
