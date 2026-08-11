#![allow(dead_code)]

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinSet;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "gif"];
pub const DEFAULT_MODEL: &str = "unsloth/Qwen3.6-27B-NVFP4";
pub const DEFAULT_SYSTEM_PROMPT: &str = "Analyze the image and return concise Danbooru-style tags inside exactly one <tag>...</tag> block. Use lowercase tags separated by commas; do not put explanations inside the tag block.";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum VllmLanguage {
    #[serde(rename = "zh", alias = "chinese")]
    Chinese,
    #[serde(rename = "en", alias = "english")]
    English,
    #[default]
    #[serde(rename = "danbooru")]
    Danbooru,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VllmErrorKind {
    InvalidEndpoint,
    UnsupportedMedia,
    InvalidImage,
    Io,
    InvalidRequest,
    Authentication,
    Forbidden,
    ContextLength,
    RateLimited,
    Upstream,
    Timeout,
    InvalidResponse,
    SidecarWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmError {
    pub kind: VllmErrorKind,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for VllmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for VllmError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageConstraints {
    pub max_side: u32,
    pub jpeg_quality: u8,
    pub max_source_bytes: u64,
    pub max_pixels: u64,
    pub max_encoded_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmServiceConfig {
    pub endpoint: String,
    pub allowed_hosts: Vec<String>,
    pub model: String,
    pub system_prompt: String,
    pub concurrency: usize,
    pub batch_limit: usize,
    pub max_attempts: usize,
    pub max_output_tokens: usize,
    pub timeout_seconds: u64,
    pub image: ImageConstraints,
    pub tag_mode: TagWriteMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmOutputOptions {
    pub language: VllmLanguage,
    pub max_tags: usize,
    pub max_length: usize,
    pub verify_danbooru: bool,
    pub reference_existing: bool,
}

impl Default for VllmOutputOptions {
    fn default() -> Self {
        Self {
            language: VllmLanguage::Danbooru,
            max_tags: 60,
            max_length: 400,
            verify_danbooru: false,
            reference_existing: false,
        }
    }
}

impl VllmOutputOptions {
    fn validate(&self) -> Result<(), VllmError> {
        if !(1..=200).contains(&self.max_tags) || !(1..=4_000).contains(&self.max_length) {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidRequest,
                message: "vLLM 输出数量或长度限制无效".to_string(),
                retryable: false,
            });
        }
        Ok(())
    }
}

impl Default for VllmServiceConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8000/v1".to_string(),
            allowed_hosts: Vec::new(),
            model: DEFAULT_MODEL.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            concurrency: 16,
            batch_limit: 1_000,
            max_attempts: 3,
            max_output_tokens: 2_048,
            timeout_seconds: 120,
            image: ImageConstraints::default(),
            tag_mode: TagWriteMode::Overwrite,
        }
    }
}

impl VllmServiceConfig {
    pub fn validate(&self) -> Result<(), VllmError> {
        validate_endpoint(&self.endpoint, &self.allowed_hosts).map_err(|message| VllmError {
            kind: VllmErrorKind::InvalidEndpoint,
            message,
            retryable: false,
        })?;
        let valid = (1..=32).contains(&self.concurrency)
            && (1..=10_000).contains(&self.batch_limit)
            && (1..=3).contains(&self.max_attempts)
            && (1..=32_768).contains(&self.max_output_tokens)
            && (5..=600).contains(&self.timeout_seconds)
            && !self.model.trim().is_empty()
            && self.model.len() <= 1_024
            && !self.system_prompt.trim().is_empty()
            && self.system_prompt.len() <= 64 * 1024
            && (64..=4_096).contains(&self.image.max_side)
            && (1..=100).contains(&self.image.jpeg_quality)
            && (1..=512 * 1024 * 1024).contains(&self.image.max_source_bytes)
            && (1..=200_000_000).contains(&self.image.max_pixels)
            && (1..=32 * 1024 * 1024).contains(&self.image.max_encoded_bytes);
        if !valid {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidRequest,
                message: "vLLM 设置超出安全边界".to_string(),
                retryable: false,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct VllmBatchItem {
    pub media_id: String,
    pub image_path: PathBuf,
    pub existing_tags: Option<String>,
    pub sidecar_quarantine_path: Option<PathBuf>,
    /// Stable identity tokens (for example original artist/character tags)
    /// that must precede newly inferred tags in one comma-separated caption.
    pub tag_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmTagSuccess {
    pub media_id: String,
    pub tags: Vec<String>,
    pub sidecar_written: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmRetryItem {
    pub media_id: String,
    pub code: VllmErrorKind,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmRetryManifest {
    pub created_at: u64,
    pub items: Vec<VllmRetryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmBatchResult {
    pub successes: Vec<VllmTagSuccess>,
    pub retry_manifest: VllmRetryManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VllmHealth {
    pub available: bool,
    pub models: Vec<String>,
    pub message: String,
}

#[derive(Clone)]
pub struct VllmService {
    client: Client,
    endpoint: url::Url,
    config: Arc<VllmServiceConfig>,
    api_key: Option<Arc<str>>,
    semaphore: Arc<Semaphore>,
    output: Arc<VllmOutputOptions>,
    danbooru_client: Option<crate::services::danbooru::DanbooruClient>,
    danbooru_tag_cache: Arc<RwLock<HashMap<String, bool>>>,
}

impl VllmService {
    pub fn new(config: VllmServiceConfig, api_key: Option<String>) -> Result<Self, VllmError> {
        config.validate()?;
        let mut endpoint =
            validate_endpoint(&config.endpoint, &config.allowed_hosts).map_err(|message| {
                VllmError {
                    kind: VllmErrorKind::InvalidEndpoint,
                    message,
                    retryable: false,
                }
            })?;
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|error| VllmError {
                kind: VllmErrorKind::InvalidRequest,
                message: format!("无法创建 vLLM 客户端: {error}"),
                retryable: false,
            })?;
        let concurrency = config.concurrency;
        Ok(Self {
            client,
            endpoint,
            config: Arc::new(config),
            api_key: api_key
                .filter(|value| !value.trim().is_empty())
                .map(|value| Arc::<str>::from(value.trim())),
            semaphore: Arc::new(Semaphore::new(concurrency)),
            output: Arc::new(VllmOutputOptions::default()),
            danbooru_client: None,
            danbooru_tag_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn with_output_options(mut self, output: VllmOutputOptions) -> Result<Self, VllmError> {
        output.validate()?;
        self.output = Arc::new(output);
        Ok(self)
    }

    pub fn with_danbooru_client(
        mut self,
        client: crate::services::danbooru::DanbooruClient,
    ) -> Self {
        self.danbooru_client = Some(client);
        self
    }

    pub async fn tag_batch(&self, items: Vec<VllmBatchItem>) -> Result<VllmBatchResult, VllmError> {
        if items.len() > self.config.batch_limit {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidRequest,
                message: format!("批处理最多接受 {} 个媒体项", self.config.batch_limit),
                retryable: false,
            });
        }
        let mut pending = items.into_iter();
        let mut running = JoinSet::new();
        for _ in 0..self.config.concurrency {
            let Some(item) = pending.next() else {
                break;
            };
            let service = self.clone();
            running.spawn(async move { service.process_item(item).await });
        }

        let mut successes = Vec::new();
        let mut failures = Vec::new();
        while let Some(result) = running.join_next().await {
            match result {
                Ok(Ok(success)) => successes.push(success),
                Ok(Err(failure)) => failures.push(failure),
                Err(error) => failures.push(VllmRetryItem {
                    media_id: "unknown".to_string(),
                    code: VllmErrorKind::Upstream,
                    message: format!("vLLM 批处理工作线程异常: {error}"),
                    retryable: true,
                }),
            }
            if let Some(item) = pending.next() {
                let service = self.clone();
                running.spawn(async move { service.process_item(item).await });
            }
        }
        successes.sort_by(|left, right| left.media_id.cmp(&right.media_id));
        failures.sort_by(|left, right| left.media_id.cmp(&right.media_id));
        Ok(VllmBatchResult {
            successes,
            retry_manifest: VllmRetryManifest {
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                items: failures,
            },
        })
    }

    pub async fn health(&self) -> VllmHealth {
        match self.list_models().await {
            Ok(models) => VllmHealth {
                available: true,
                message: format!("vLLM 可用，发现 {} 个模型", models.len()),
                models,
            },
            Err(error) => VllmHealth {
                available: false,
                models: Vec::new(),
                message: error.message,
            },
        }
    }

    pub async fn list_models(&self) -> Result<Vec<String>, VllmError> {
        let url = self.endpoint.join("models").map_err(|_| VllmError {
            kind: VllmErrorKind::InvalidEndpoint,
            message: "无法构造 vLLM models 地址".to_string(),
            retryable: false,
        })?;
        let mut request = self.client.get(url);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key.as_ref());
        }
        let response = request.send().await.map_err(request_transport_error)?;
        let status = response.status();
        let bytes = read_limited_response(response, 1024 * 1024).await?;
        if !status.is_success() {
            return Err(classify_http_error(
                status,
                &String::from_utf8_lossy(&bytes),
            ));
        }
        let response: VllmModelsResponse =
            serde_json::from_slice(&bytes).map_err(|error| VllmError {
                kind: VllmErrorKind::InvalidResponse,
                message: format!("无法解析 vLLM 模型列表: {error}"),
                retryable: false,
            })?;
        Ok(response.data.into_iter().map(|model| model.id).collect())
    }

    async fn process_item(&self, item: VllmBatchItem) -> Result<VllmTagSuccess, VllmRetryItem> {
        let media_id = item.media_id.clone();
        let permit = self.semaphore.clone().acquire_owned().await.map_err(|_| {
            retry_item(
                &media_id,
                VllmError {
                    kind: VllmErrorKind::Upstream,
                    message: "vLLM 并发队列已关闭".to_string(),
                    retryable: true,
                },
            )
        })?;
        let path = item.image_path.clone();
        let constraints = self.config.image.clone();
        let prepared =
            tokio::task::spawn_blocking(move || prepare_image_data_url(&path, &constraints))
                .await
                .map_err(|error| {
                    retry_item(
                        &media_id,
                        VllmError {
                            kind: VllmErrorKind::InvalidImage,
                            message: format!("图片预处理线程异常: {error}"),
                            retryable: false,
                        },
                    )
                })?
                .map_err(|error| retry_item(&media_id, error))?;

        let mut tags = retry_operation(self.config.max_attempts, || {
            self.request_tags(&prepared, item.existing_tags.as_deref())
        })
        .await
        .map_err(|error| retry_item(&media_id, error))?;
        if self.output.language == VllmLanguage::Danbooru && self.output.verify_danbooru {
            tags = self
                .verify_danbooru_tags(tags)
                .await
                .map_err(|error| retry_item(&media_id, error))?;
        }
        drop(permit);

        let final_tags = merge_caption_tags(item.tag_prefixes, tags);
        let sidecar_content = final_tags.join(",");
        let image_path = item.image_path.clone();
        let sidecar_quarantine_path = item.sidecar_quarantine_path.clone();
        let mode = self.config.tag_mode;
        tokio::task::spawn_blocking(move || {
            write_sidecar_atomic(
                &image_path,
                &sidecar_content,
                mode,
                sidecar_quarantine_path.as_deref(),
            )
        })
        .await
        .map_err(|error| {
            retry_item(
                &media_id,
                VllmError {
                    kind: VllmErrorKind::SidecarWrite,
                    message: format!("标签写入线程异常: {error}"),
                    retryable: false,
                },
            )
        })?
        .map_err(|error| retry_item(&media_id, error))?;

        Ok(VllmTagSuccess {
            media_id,
            tags: final_tags,
            sidecar_written: true,
        })
    }

    async fn request_tags(
        &self,
        prepared: &PreparedImage,
        existing_tags: Option<&str>,
    ) -> Result<Vec<String>, VllmError> {
        let existing = existing_tags
            .filter(|_| self.output.reference_existing)
            .unwrap_or_default()
            .chars()
            .take(32_768)
            .collect::<String>();
        let task = match self.output.language {
            VllmLanguage::Chinese => "请用简洁、客观的中文描述图片内容",
            VllmLanguage::English => {
                "Describe the visible image content in concise, objective English"
            }
            VllmLanguage::Danbooru => "为这张图片生成规范的 Danbooru 风格标签",
        };
        let user_text = if existing.trim().is_empty() {
            format!("{task}，优先放入 <tag>...</tag>；不要输出无关内容。")
        } else {
            format!("{task}，参考并修正已有标签，优先放入 <tag>...</tag>。已有标签：{existing}")
        };
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": self.config.system_prompt},
                {"role": "user", "content": [
                    {"type": "image_url", "image_url": {"url": prepared.data_url}},
                    {"type": "text", "text": user_text}
                ]}
            ],
            "stream": false,
            "max_tokens": self.config.max_output_tokens,
            "temperature": 0.2
        });
        let url = self
            .endpoint
            .join("chat/completions")
            .map_err(|_| VllmError {
                kind: VllmErrorKind::InvalidEndpoint,
                message: "无法构造 vLLM chat/completions 地址".to_string(),
                retryable: false,
            })?;
        let mut request = self.client.post(url).json(&body);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key.as_ref());
        }
        let response = request.send().await.map_err(request_transport_error)?;
        let status = response.status();
        let bytes = read_limited_response(response, 4 * 1024 * 1024).await?;
        if !status.is_success() {
            return Err(classify_http_error(
                status,
                &String::from_utf8_lossy(&bytes),
            ));
        }
        let response: ChatCompletionResponse =
            serde_json::from_slice(&bytes).map_err(|error| VllmError {
                kind: VllmErrorKind::InvalidResponse,
                message: format!("无法解析 vLLM 响应: {error}"),
                retryable: false,
            })?;
        let content = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
            .ok_or_else(|| VllmError {
                kind: VllmErrorKind::InvalidResponse,
                message: "vLLM 响应没有文本内容".to_string(),
                retryable: false,
            })?;
        parse_model_output(
            content,
            self.output.language,
            self.output.max_tags,
            self.output.max_length,
        )
    }

    async fn verify_danbooru_tags(&self, tags: Vec<String>) -> Result<Vec<String>, VllmError> {
        let client = self.danbooru_client.as_ref().ok_or_else(|| VllmError {
            kind: VllmErrorKind::InvalidRequest,
            message: "已启用 Danbooru 标签校验，但共享客户端不可用".to_string(),
            retryable: false,
        })?;
        let mut verified = Vec::new();
        for tag in tags {
            let cached = self.danbooru_tag_cache.read().await.get(&tag).copied();
            let exists = match cached {
                Some(exists) => exists,
                None => {
                    let exists = client
                        .tag_category(&tag)
                        .await
                        .map_err(|error| VllmError {
                            kind: VllmErrorKind::Upstream,
                            message: format!("Danbooru 标签校验失败: {}", error.message),
                            retryable: error.retryable,
                        })?
                        .is_some();
                    self.danbooru_tag_cache
                        .write()
                        .await
                        .insert(tag.clone(), exists);
                    exists
                }
            };
            if exists {
                verified.push(tag);
            }
        }
        if verified.is_empty() {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidResponse,
                message: "模型返回的标签均未通过 Danbooru 在线校验".to_string(),
                retryable: false,
            });
        }
        Ok(verified)
    }
}

fn merge_caption_tags(prefixes: Vec<String>, inferred: Vec<String>) -> Vec<String> {
    let mut final_tags = Vec::with_capacity(prefixes.len() + inferred.len());
    let mut seen = HashSet::new();
    for tag in prefixes.into_iter().chain(inferred.into_iter()) {
        let normalized = tag.trim().trim_matches(',').trim().to_string();
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            final_tags.push(normalized);
        }
    }
    final_tags
}

fn retry_item(media_id: &str, error: VllmError) -> VllmRetryItem {
    VllmRetryItem {
        media_id: media_id.to_string(),
        code: error.kind,
        message: error.message,
        retryable: error.retryable,
    }
}

fn request_transport_error(error: reqwest::Error) -> VllmError {
    let timeout = error.is_timeout();
    VllmError {
        kind: if timeout {
            VllmErrorKind::Timeout
        } else {
            VllmErrorKind::Upstream
        },
        message: if timeout {
            "vLLM 请求超时".to_string()
        } else {
            format!("vLLM 连接失败: {error}")
        },
        retryable: true,
    }
}

async fn read_limited_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, VllmError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidResponse,
            message: "vLLM 响应超过大小限制".to_string(),
            retryable: false,
        });
    }
    let mut output = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_transport_error)? {
        if output.len().saturating_add(chunk.len()) > limit {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidResponse,
                message: "vLLM 响应超过大小限制".to_string(),
                retryable: false,
            });
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn parse_tag_output(raw: &str) -> Result<Vec<String>, VllmError> {
    let without_thinking =
        if let (Some(start), Some(end)) = (raw.find("<think>"), raw.find("</think>")) {
            format!("{}{}", &raw[..start], &raw[end + "</think>".len()..])
        } else {
            raw.to_string()
        };
    let candidate = if let Some(start) = without_thinking.rfind("<tag>") {
        let after_start = &without_thinking[start + "<tag>".len()..];
        after_start
            .split_once("</tag>")
            .map_or(after_start, |(inside, _)| inside)
    } else {
        without_thinking
            .trim()
            .trim_matches('`')
            .trim_start_matches("tags:")
            .trim_start_matches("Tags:")
            .trim_start_matches("标签：")
            .trim()
    };
    let mut seen = HashSet::new();
    let tags = candidate
        .split([',', '，', '\n', '\r'])
        .map(|tag| {
            tag.trim()
                .trim_matches(['"', '\'', '*', '-', '.', ':', '：', '#'])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("_")
                .to_lowercase()
        })
        .filter(|tag| {
            !tag.is_empty()
                && tag.len() <= 80
                && !tag.starts_with("http")
                && seen.insert(tag.clone())
        })
        .collect::<Vec<_>>();
    if tags.is_empty() {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidResponse,
            message: "模型未返回有效标签".to_string(),
            retryable: false,
        });
    }
    Ok(tags)
}

fn parse_model_output(
    raw: &str,
    language: VllmLanguage,
    max_tags: usize,
    max_length: usize,
) -> Result<Vec<String>, VllmError> {
    if !(1..=200).contains(&max_tags) || !(1..=4_000).contains(&max_length) {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidRequest,
            message: "vLLM 输出数量或长度限制无效".to_string(),
            retryable: false,
        });
    }
    if language == VllmLanguage::Danbooru {
        let mut tags = parse_tag_output(raw)?;
        tags.truncate(max_tags);
        return Ok(tags);
    }
    let without_thinking =
        if let (Some(start), Some(end)) = (raw.find("<think>"), raw.find("</think>")) {
            format!("{}{}", &raw[..start], &raw[end + "</think>".len()..])
        } else {
            raw.to_string()
        };
    let candidate = if let Some(start) = without_thinking.rfind("<tag>") {
        let after_start = &without_thinking[start + "<tag>".len()..];
        after_start
            .split_once("</tag>")
            .map_or(after_start, |(inside, _)| inside)
    } else {
        without_thinking.trim().trim_matches('`')
    };
    let description = candidate
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_length)
        .collect::<String>();
    if description.is_empty() {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidResponse,
            message: "模型未返回有效描述".to_string(),
            retryable: false,
        });
    }
    Ok(vec![description])
}

impl Default for ImageConstraints {
    fn default() -> Self {
        Self {
            max_side: 1_536,
            jpeg_quality: 85,
            max_source_bytes: 128 * 1024 * 1024,
            max_pixels: 100_000_000,
            max_encoded_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreparedImage {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagWriteMode {
    #[default]
    Overwrite,
    Append,
}

pub fn write_sidecar_atomic(
    image_path: &Path,
    content: &str,
    mode: TagWriteMode,
    quarantine_path: Option<&Path>,
) -> Result<PathBuf, VllmError> {
    if content.trim().is_empty() {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidRequest,
            message: "模型标签内容不能为空".to_string(),
            retryable: false,
        });
    }
    let metadata = fs::metadata(image_path).map_err(sidecar_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(VllmError {
            kind: VllmErrorKind::SidecarWrite,
            message: "标签目标不是普通媒体文件".to_string(),
            retryable: false,
        });
    }
    let sidecar = image_path.with_extension("txt");
    let existing_sidecar = match fs::symlink_metadata(&sidecar) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(VllmError {
                    kind: VllmErrorKind::SidecarWrite,
                    message: "拒绝替换非普通标签文件".to_string(),
                    retryable: false,
                });
            }
            if metadata.len() > 8 * 1024 * 1024 {
                return Err(VllmError {
                    kind: VllmErrorKind::SidecarWrite,
                    message: "现有标签文件超过 8 MiB 限制".to_string(),
                    retryable: false,
                });
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(sidecar_io_error(error)),
    };
    if existing_sidecar && quarantine_path.is_none() {
        return Err(VllmError {
            kind: VllmErrorKind::SidecarWrite,
            message: "替换现有标签文件必须提供隔离目标".to_string(),
            retryable: false,
        });
    }
    if let Some(quarantine_path) = quarantine_path {
        if quarantine_path == sidecar {
            return Err(VllmError {
                kind: VllmErrorKind::SidecarWrite,
                message: "标签文件与隔离目标不能相同".to_string(),
                retryable: false,
            });
        }
        match fs::symlink_metadata(quarantine_path) {
            Ok(_) => {
                return Err(VllmError {
                    kind: VllmErrorKind::SidecarWrite,
                    message: "标签隔离目标已存在".to_string(),
                    retryable: false,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(sidecar_io_error(error)),
        }
    }
    let output = match mode {
        TagWriteMode::Overwrite => content.trim().to_string(),
        TagWriteMode::Append => {
            let existing = if sidecar.exists() {
                fs::read_to_string(&sidecar).map_err(sidecar_io_error)?
            } else {
                String::new()
            };
            let existing = existing.trim().trim_end_matches(',').trim_end();
            if existing.is_empty() {
                content.trim().to_string()
            } else {
                format!("{existing},\n{}", content.trim())
            }
        }
    };
    let parent = sidecar.parent().ok_or_else(|| VllmError {
        kind: VllmErrorKind::SidecarWrite,
        message: "标签文件缺少父目录".to_string(),
        retryable: false,
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(sidecar_io_error)?;
    temporary
        .write_all(output.as_bytes())
        .map_err(sidecar_io_error)?;
    temporary.flush().map_err(sidecar_io_error)?;
    temporary.as_file().sync_all().map_err(sidecar_io_error)?;
    let moved_sidecar = if existing_sidecar {
        let quarantine_path = quarantine_path.expect("existing sidecar requires quarantine path");
        let parent = quarantine_path.parent().ok_or_else(|| VllmError {
            kind: VllmErrorKind::SidecarWrite,
            message: "标签隔离目标缺少父目录".to_string(),
            retryable: false,
        })?;
        fs::create_dir_all(parent).map_err(sidecar_io_error)?;
        if fs::symlink_metadata(quarantine_path).is_ok() {
            return Err(VllmError {
                kind: VllmErrorKind::SidecarWrite,
                message: "标签隔离目标已存在".to_string(),
                retryable: false,
            });
        }
        fs::rename(&sidecar, quarantine_path).map_err(sidecar_io_error)?;
        Some(quarantine_path)
    } else {
        None
    };
    if let Err(error) = temporary.persist(&sidecar) {
        let rollback_error =
            moved_sidecar.and_then(|quarantine_path| fs::rename(quarantine_path, &sidecar).err());
        let rollback = rollback_error
            .map(|error| format!("；旧标签回滚失败: {error}"))
            .unwrap_or_default();
        return Err(VllmError {
            kind: VllmErrorKind::SidecarWrite,
            message: format!("无法原子替换标签文件: {}{rollback}", error.error),
            retryable: false,
        });
    }
    Ok(sidecar)
}

fn sidecar_io_error(error: std::io::Error) -> VllmError {
    VllmError {
        kind: VllmErrorKind::SidecarWrite,
        message: format!("标签文件写入失败: {error}"),
        retryable: false,
    }
}

pub fn classify_http_error(status: reqwest::StatusCode, body: &str) -> VllmError {
    let normalized = body.to_ascii_lowercase();
    let context_error = normalized.contains("context length")
        || normalized.contains("maximum context")
        || normalized.contains("token limit")
        || normalized.contains("too many tokens");
    let (kind, retryable) = if context_error {
        (VllmErrorKind::ContextLength, false)
    } else {
        match status.as_u16() {
            300..=399 => (VllmErrorKind::InvalidEndpoint, false),
            400 | 404 | 405 | 409 | 413 | 415 | 422 => (VllmErrorKind::InvalidRequest, false),
            401 => (VllmErrorKind::Authentication, false),
            403 => (VllmErrorKind::Forbidden, false),
            408 => (VllmErrorKind::Timeout, true),
            429 => (VllmErrorKind::RateLimited, true),
            400..=499 => (VllmErrorKind::InvalidRequest, false),
            _ => (VllmErrorKind::Upstream, true),
        }
    };
    VllmError {
        kind,
        message: format!(
            "vLLM 返回 HTTP {}: {}",
            status.as_u16(),
            body.chars().take(500).collect::<String>()
        ),
        retryable,
    }
}

async fn retry_operation<T, F, Future>(
    max_attempts: usize,
    mut operation: F,
) -> Result<T, VllmError>
where
    F: FnMut() -> Future,
    Future: std::future::Future<Output = Result<T, VllmError>>,
{
    let attempts = max_attempts.max(1);
    for attempt in 0..attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if !error.retryable || attempt + 1 == attempts => return Err(error),
            Err(_) => {
                let delay = 100_u64.saturating_mul(1_u64 << attempt.min(4));
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
    unreachable!("attempt count is always at least one")
}

pub fn prepare_image_data_url(
    path: &Path,
    constraints: &ImageConstraints,
) -> Result<PreparedImage, VllmError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !IMAGE_EXTS.contains(&extension.as_str()) {
        return Err(VllmError {
            kind: VllmErrorKind::UnsupportedMedia,
            message: "vLLM 视觉打标仅接受静态图片或可解码图片；视频、Ugoira 与 SWF 被拒绝"
                .to_string(),
            retryable: false,
        });
    }
    if constraints.max_side == 0
        || !(1..=100).contains(&constraints.jpeg_quality)
        || constraints.max_source_bytes == 0
        || constraints.max_pixels == 0
        || constraints.max_encoded_bytes == 0
    {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidRequest,
            message: "图片约束参数无效".to_string(),
            retryable: false,
        });
    }
    let metadata = fs::metadata(path).map_err(|error| VllmError {
        kind: VllmErrorKind::Io,
        message: format!("无法读取图片元数据: {error}"),
        retryable: false,
    })?;
    if !metadata.file_type().is_file() || metadata.len() > constraints.max_source_bytes {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidImage,
            message: "图片不是普通文件或超过源文件大小限制".to_string(),
            retryable: false,
        });
    }

    let reader = image::ImageReader::open(path)
        .map_err(image_io_error)?
        .with_guessed_format()
        .map_err(image_io_error)?;
    let (source_width, source_height) = reader.into_dimensions().map_err(image_decode_error)?;
    if u64::from(source_width).saturating_mul(u64::from(source_height)) > constraints.max_pixels {
        return Err(VllmError {
            kind: VllmErrorKind::InvalidImage,
            message: "图片像素数量超过模型输入限制".to_string(),
            retryable: false,
        });
    }

    let image = image::ImageReader::open(path)
        .map_err(image_io_error)?
        .with_guessed_format()
        .map_err(image_io_error)?
        .decode()
        .map_err(image_decode_error)?;
    let (width, height) = bounded_dimensions(source_width, source_height, constraints.max_side);
    let image = if (width, height) == (source_width, source_height) {
        image
    } else {
        image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    };
    let mut rgb = composite_on_white(&image);
    let mut quality = constraints.jpeg_quality;

    loop {
        let encoded = encode_jpeg(&rgb, quality)?;
        if encoded.len() <= constraints.max_encoded_bytes {
            return Ok(PreparedImage {
                data_url: format!("data:image/jpeg;base64,{}", B64.encode(&encoded)),
                width: rgb.width(),
                height: rgb.height(),
                encoded_bytes: encoded.len(),
            });
        }
        if quality > 50 {
            quality = quality.saturating_sub(10).max(50);
            continue;
        }
        if rgb.width().max(rgb.height()) <= 128 {
            return Err(VllmError {
                kind: VllmErrorKind::InvalidImage,
                message: "压缩后图片仍超过模型输入字节限制".to_string(),
                retryable: false,
            });
        }
        let next_width = ((rgb.width() as f64 * 0.8).round() as u32).max(1);
        let next_height = ((rgb.height() as f64 * 0.8).round() as u32).max(1);
        rgb = image::imageops::resize(
            &rgb,
            next_width,
            next_height,
            image::imageops::FilterType::Lanczos3,
        );
    }
}

fn image_io_error(error: std::io::Error) -> VllmError {
    VllmError {
        kind: VllmErrorKind::Io,
        message: format!("无法读取图片: {error}"),
        retryable: false,
    }
}

fn image_decode_error(error: image::ImageError) -> VllmError {
    VllmError {
        kind: VllmErrorKind::InvalidImage,
        message: format!("无法解码图片: {error}"),
        retryable: false,
    }
}

fn bounded_dimensions(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    if width <= max_side && height <= max_side {
        return (width, height);
    }
    let scale = max_side as f64 / width.max(height) as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn composite_on_white(image: &image::DynamicImage) -> image::RgbImage {
    let rgba = image.to_rgba8();
    let mut output = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u16::from(pixel[3]);
        let inverse = 255_u16.saturating_sub(alpha);
        output.put_pixel(
            x,
            y,
            image::Rgb([
                ((u16::from(pixel[0]) * alpha + 255 * inverse) / 255) as u8,
                ((u16::from(pixel[1]) * alpha + 255 * inverse) / 255) as u8,
                ((u16::from(pixel[2]) * alpha + 255 * inverse) / 255) as u8,
            ]),
        );
    }
    output
}

fn encode_jpeg(image: &image::RgbImage, quality: u8) -> Result<Vec<u8>, VllmError> {
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
        .encode(
            image,
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(image_decode_error)?;
    Ok(encoded)
}

pub fn validate_endpoint(candidate: &str, allowed_hosts: &[String]) -> Result<url::Url, String> {
    let url = url::Url::parse(candidate).map_err(|_| "vLLM 地址无效".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "vLLM 地址缺少主机名".to_string())?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let effective_port = url.port_or_known_default();
    let explicitly_allowed = allowed_hosts.iter().any(|allowed| {
        if allowed.contains('@') {
            return false;
        }
        let Ok(authority) = allowed.trim().parse::<axum::http::uri::Authority>() else {
            return false;
        };
        let Some(allowed_port) = authority.port_u16() else {
            return false;
        };
        let allowed_host = authority
            .host()
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or_else(|| authority.host());
        allowed_host.eq_ignore_ascii_case(host) && effective_port == Some(allowed_port)
    });
    let scheme_allowed = if loopback {
        matches!(url.scheme(), "http" | "https")
    } else {
        url.scheme() == "https" && explicitly_allowed
    };
    if !scheme_allowed
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "vLLM 仅允许 loopback，或与 allowlist 中 host:port 精确匹配的 HTTPS 地址".to_string(),
        );
    }
    Ok(url)
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VllmModelsResponse {
    data: Vec<VllmModel>,
}

#[derive(Debug, Deserialize)]
struct VllmModel {
    id: String,
}

#[cfg(test)]
mod secure_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn caption_prefixes_stay_first_and_are_comma_deduplicated() {
        let tags = merge_caption_tags(
            vec!["artist:origin".to_string(), "character_a".to_string()],
            vec![
                "1girl".to_string(),
                "character_a".to_string(),
                " blue_hair, ".to_string(),
            ],
        );

        assert_eq!(
            tags,
            vec!["artist:origin", "character_a", "1girl", "blue_hair"]
        );
    }

    #[test]
    fn endpoint_policy_rejects_non_loopback_http() {
        let result = validate_endpoint("http://192.168.1.50:8000/v1", &[]);

        assert!(result.is_err());
    }

    #[test]
    fn endpoint_allowlist_requires_the_exact_effective_port() {
        let endpoint = "https://vision.example.test:8443/v1";

        assert!(validate_endpoint(endpoint, &["vision.example.test".to_string()]).is_err());
        assert!(validate_endpoint(endpoint, &["vision.example.test:443".to_string()]).is_err());
        assert!(validate_endpoint(endpoint, &["vision.example.test:8443".to_string()]).is_ok());
    }

    #[test]
    fn media_preparation_rejects_video_and_ugoira() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let video = directory.path().join("clip.webm");
        fs::write(&video, b"video").expect("video fixture");

        let result = prepare_image_data_url(&video, &ImageConstraints::default());

        assert!(matches!(
            result,
            Err(VllmError {
                kind: VllmErrorKind::UnsupportedMedia,
                ..
            })
        ));
    }

    #[test]
    fn plain_danbooru_tag_list_is_accepted_when_model_omits_tag_wrapper() {
        assert_eq!(
            parse_tag_output("cat, blue hair, solo").expect("plain tag list"),
            vec!["cat", "blue_hair", "solo"]
        );
    }

    #[test]
    fn english_and_chinese_modes_preserve_natural_language_descriptions() {
        assert_eq!(
            parse_model_output(
                "<tag>A woman with blue hair standing beside a red car.</tag>",
                VllmLanguage::English,
                60,
                400,
            )
            .unwrap(),
            vec!["A woman with blue hair standing beside a red car."]
        );
        assert_eq!(
            parse_model_output(
                "画面中是一位蓝发少女，站在红色汽车旁。",
                VllmLanguage::Chinese,
                60,
                400,
            )
            .unwrap(),
            vec!["画面中是一位蓝发少女，站在红色汽车旁。"]
        );
    }

    #[test]
    fn media_preparation_resizes_and_reencodes_within_bounds() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("large.png");
        image::RgbImage::from_pixel(200, 100, image::Rgb([20, 40, 60]))
            .save(&source)
            .expect("image fixture");
        let constraints = ImageConstraints {
            max_side: 64,
            max_encoded_bytes: 128 * 1024,
            ..ImageConstraints::default()
        };

        let prepared = prepare_image_data_url(&source, &constraints).expect("prepared image");

        assert_eq!((prepared.width, prepared.height), (64, 32));
        assert!(prepared.data_url.starts_with("data:image/jpeg;base64,"));
        assert!(prepared.encoded_bytes <= constraints.max_encoded_bytes);
    }

    #[test]
    fn sidecar_append_uses_atomic_same_directory_replacement() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let image = directory.path().join("image.jpg");
        fs::write(&image, b"image").expect("image fixture");
        fs::write(image.with_extension("txt"), "old_tag").expect("existing tags");

        let quarantine = directory.path().join("quarantine/image.txt");
        let sidecar =
            write_sidecar_atomic(&image, "new_tag", TagWriteMode::Append, Some(&quarantine))
                .expect("atomic sidecar write");

        assert_eq!(
            fs::read_to_string(&sidecar).expect("updated tags"),
            "old_tag,\nnew_tag"
        );
        assert_eq!(fs::read_to_string(quarantine).unwrap(), "old_tag");
        assert!(fs::read_dir(directory.path())
            .expect("directory listing")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp")));
    }

    #[test]
    fn context_and_other_deterministic_4xx_errors_are_not_retryable() {
        let error = classify_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            "maximum context length exceeded",
        );

        assert_eq!(error.kind, VllmErrorKind::ContextLength);
        assert!(!error.retryable);
    }

    #[test]
    fn redirects_are_not_followed_or_retried() {
        let error = classify_http_error(reqwest::StatusCode::FOUND, "redirect");

        assert_eq!(error.kind, VllmErrorKind::InvalidEndpoint);
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn deterministic_failures_are_attempted_only_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();

        let result = retry_operation(3, move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<(), _>(VllmError {
                    kind: VllmErrorKind::ContextLength,
                    message: "context length".to_string(),
                    retryable: false,
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rate_limited_http_responses_are_retried() {
        let rate_limited =
            classify_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "retry later");
        assert_eq!(rate_limited.kind, VllmErrorKind::RateLimited);
        assert!(rate_limited.retryable);
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();

        let result = retry_operation(3, move || {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            let rate_limited = rate_limited.clone();
            async move {
                if attempt == 0 {
                    Err(rate_limited)
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn service_config_enforces_bounded_concurrency() {
        let config = VllmServiceConfig {
            endpoint: "http://127.0.0.1:8000/v1".to_string(),
            allowed_hosts: Vec::new(),
            model: "vision-model".to_string(),
            system_prompt: "tag the image".to_string(),
            concurrency: 0,
            batch_limit: 100,
            max_attempts: 3,
            max_output_tokens: 1024,
            timeout_seconds: 120,
            image: ImageConstraints::default(),
            tag_mode: TagWriteMode::Overwrite,
        };

        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn failed_batch_item_stays_in_place_and_enters_retry_manifest() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let image_path = directory.path().join("image.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]))
            .save(&image_path)
            .expect("image fixture");
        let config = VllmServiceConfig {
            endpoint: "http://127.0.0.1:9/v1".to_string(),
            allowed_hosts: Vec::new(),
            model: "vision-model".to_string(),
            system_prompt: "return <tag> tags".to_string(),
            concurrency: 1,
            batch_limit: 10,
            max_attempts: 1,
            max_output_tokens: 128,
            timeout_seconds: 5,
            image: ImageConstraints::default(),
            tag_mode: TagWriteMode::Overwrite,
        };
        let service = VllmService::new(config, None).expect("service");

        let result = service
            .tag_batch(vec![VllmBatchItem {
                media_id: "media-1".to_string(),
                image_path: image_path.clone(),
                existing_tags: None,
                sidecar_quarantine_path: None,
                tag_prefixes: Vec::new(),
            }])
            .await
            .expect("batch result");

        assert_eq!(result.retry_manifest.items.len(), 1);
        assert!(image_path.exists());
        assert!(!directory.path().join("error").exists());
    }

    #[tokio::test]
    async fn health_uses_the_validated_endpoint_models_route() {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({"data": [{"id": "vision-model"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let config = VllmServiceConfig {
            endpoint: format!("http://{address}/v1"),
            allowed_hosts: Vec::new(),
            model: "vision-model".to_string(),
            system_prompt: "return <tag> tags".to_string(),
            concurrency: 1,
            batch_limit: 10,
            max_attempts: 1,
            max_output_tokens: 128,
            timeout_seconds: 5,
            image: ImageConstraints::default(),
            tag_mode: TagWriteMode::Overwrite,
        };
        let service = VllmService::new(config, None).expect("service");

        let health = service.health().await;

        server.abort();
        assert!(health.available);
        assert_eq!(health.models, vec!["vision-model"]);
    }

    #[tokio::test]
    async fn online_danbooru_validation_filters_unknown_generated_tags() {
        async fn tags(
            axum::extract::Query(query): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >,
        ) -> axum::Json<serde_json::Value> {
            if query.get("search[name]").is_some_and(|tag| tag == "cat") {
                axum::Json(serde_json::json!([{ "category": 0 }]))
            } else {
                axum::Json(serde_json::json!([]))
            }
        }
        let app = axum::Router::new().route("/tags.json", axum::routing::get(tags));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let danbooru = crate::services::danbooru::DanbooruClient::new(
            crate::services::danbooru::DanbooruClientConfig {
                base_url: format!("http://{address}"),
                requests_per_second: 1_000,
                ..crate::services::danbooru::DanbooruClientConfig::default()
            },
        )
        .unwrap();
        let service = VllmService::new(VllmServiceConfig::default(), None)
            .unwrap()
            .with_output_options(VllmOutputOptions {
                language: VllmLanguage::Danbooru,
                max_tags: 60,
                max_length: 400,
                verify_danbooru: true,
                reference_existing: false,
            })
            .unwrap()
            .with_danbooru_client(danbooru);

        let verified = service
            .verify_danbooru_tags(vec!["cat".to_string(), "invented_tag".to_string()])
            .await
            .unwrap();

        server.abort();
        assert_eq!(verified, vec!["cat"]);
    }
}
