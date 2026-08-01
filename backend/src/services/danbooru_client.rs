use axum::body::Bytes;
use md5::{Digest, Md5};
use reqwest::header::{CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::Mutex;
use tokio::time::sleep;
use url::Url;

pub const DEFAULT_BASE_URL: &str = "https://danbooru.donmai.us";
pub const DEFAULT_REQUESTS_PER_SECOND: u32 = 8;
pub const MAX_POSTS_PER_PAGE: u16 = 200;
pub const MAX_MEDIA_DOWNLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const DEFAULT_FILENAME_TEMPLATE: &str = "{id}_score_{score}.{ext}";
pub const SUPPORTED_MEDIA_EXTENSIONS: &[&str] =
    &["jpg", "png", "webp", "gif", "avif", "mp4", "webm"];

type DownloadPathLocks = HashMap<PathBuf, Weak<Mutex<()>>>;
static DOWNLOAD_PATH_LOCKS: OnceLock<StdMutex<DownloadPathLocks>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DanbooruErrorKind {
    InvalidRequest,
    InvalidQuery,
    InvalidCredentials,
    Forbidden,
    NotFound,
    PageLimit,
    TagLimit,
    RateLimited,
    UpstreamUnavailable,
    Network,
    InvalidResponse,
    UnsafeMediaUrl,
    UnsupportedMedia,
    InvalidTemplate,
    Integrity,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanbooruError {
    pub kind: DanbooruErrorKind,
    pub message: String,
    pub retryable: bool,
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
}

impl DanbooruError {
    fn new(kind: DanbooruErrorKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            status: None,
            retry_after: None,
        }
    }

    fn with_status(mut self, status: StatusCode) -> Self {
        self.status = Some(status.as_u16());
        self
    }

    fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }
}

impl fmt::Display for DanbooruError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DanbooruError {}

#[derive(Clone)]
pub struct DanbooruClientConfig {
    pub base_url: String,
    pub username: String,
    pub api_key: String,
    pub proxy_url: Option<String>,
    pub requests_per_second: u32,
    pub trusted_media_hosts: Vec<String>,
    pub timeout: Duration,
}

impl Default for DanbooruClientConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            username: String::new(),
            api_key: String::new(),
            proxy_url: None,
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            trusted_media_hosts: vec!["danbooru.donmai.us".into(), "cdn.donmai.us".into()],
            timeout: Duration::from_secs(90),
        }
    }
}

impl fmt::Debug for DanbooruClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DanbooruClientConfig")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .field(
                "proxy_url",
                &self.proxy_url.as_ref().map(|_| "<configured>"),
            )
            .field("requests_per_second", &self.requests_per_second)
            .field("trusted_media_hosts", &self.trusted_media_hosts)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Debug)]
struct RateLimiter {
    next_request: Mutex<Instant>,
    interval: Duration,
}

impl RateLimiter {
    fn new(requests_per_second: u32) -> Result<Self, DanbooruError> {
        if requests_per_second == 0 || requests_per_second > 1000 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                "requests_per_second 必须在 1..=1000 之间",
                false,
            ));
        }
        Ok(Self {
            next_request: Mutex::new(Instant::now()),
            interval: Duration::from_secs_f64(1.0 / f64::from(requests_per_second)),
        })
    }

    async fn acquire(&self) {
        let scheduled = {
            let mut next = self.next_request.lock().await;
            let now = Instant::now();
            let scheduled = (*next).max(now);
            *next = scheduled + self.interval;
            scheduled
        };
        let now = Instant::now();
        if scheduled > now {
            sleep(scheduled - now).await;
        }
    }
}

#[derive(Clone)]
pub struct DanbooruClient {
    client: Client,
    base_url: Url,
    username: Arc<str>,
    api_key: Arc<str>,
    trusted_media_hosts: Arc<BTreeSet<String>>,
    rate_limiter: Arc<RateLimiter>,
}

pub struct MediaResponse {
    response: Response,
    max_body_bytes: u64,
    expected_body_bytes: Option<u64>,
}

impl MediaResponse {
    fn new(response: Response, max_body_bytes: u64, expected_body_bytes: Option<u64>) -> Self {
        Self {
            response,
            max_body_bytes,
            expected_body_bytes,
        }
    }

    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        self.response.headers()
    }

    pub fn bytes_stream(
        mut self,
    ) -> impl futures_core::Stream<Item = Result<Bytes, DanbooruError>> {
        async_stream::try_stream! {
            let mut received = 0_u64;
            while let Some(chunk) = self.response.chunk().await.map_err(network_error)? {
                let next = received.saturating_add(chunk.len() as u64);
                if next > self.max_body_bytes {
                    Err(DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "媒体响应超过可信大小上限",
                        false,
                    ))?;
                }
                received = next;
                yield chunk;
            }
            if self.expected_body_bytes.is_some_and(|expected| expected != received) {
                Err(DanbooruError::new(
                    DanbooruErrorKind::Integrity,
                    "媒体响应长度与可信预期大小不一致",
                    true,
                ))?;
            }
        }
    }
}

impl fmt::Debug for DanbooruClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DanbooruClient")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .field("trusted_media_hosts", &self.trusted_media_hosts)
            .finish_non_exhaustive()
    }
}

impl DanbooruClient {
    pub fn new(config: DanbooruClientConfig) -> Result<Self, DanbooruError> {
        let base_url = parse_base_url(&config.base_url)?;
        let mut trusted_media_hosts = config
            .trusted_media_hosts
            .into_iter()
            .map(|host| host.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if let Some(host) = base_url.host_str() {
            trusted_media_hosts.insert(host.to_ascii_lowercase());
        }

        let mut builder = Client::builder()
            .no_proxy()
            .timeout(config.timeout)
            .connect_timeout(Duration::from_secs(15).min(config.timeout))
            .read_timeout(Duration::from_secs(60).min(config.timeout))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .tcp_keepalive(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("DanbooruDownloadToolPro/3.0");

        if let Some(proxy_url) = config
            .proxy_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
        {
            let proxy = reqwest::Proxy::all(proxy_url).map_err(|error| {
                DanbooruError::new(
                    DanbooruErrorKind::InvalidRequest,
                    format!("代理配置无效: {error}"),
                    false,
                )
            })?;
            builder = builder.proxy(proxy);
        }

        let client = builder.build().map_err(|error| {
            DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                format!("HTTP 客户端初始化失败: {error}"),
                false,
            )
        })?;

        Ok(Self {
            client,
            base_url,
            username: Arc::from(config.username),
            api_key: Arc::from(config.api_key),
            trusted_media_hosts: Arc::new(trusted_media_hosts),
            rate_limiter: Arc::new(RateLimiter::new(config.requests_per_second)?),
        })
    }

    pub async fn posts(&self, query: &PostQuery) -> Result<PostPage, DanbooruError> {
        if !(1..=MAX_POSTS_PER_PAGE).contains(&query.limit) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                format!("limit 必须在 1..={MAX_POSTS_PER_PAGE} 之间"),
                false,
            ));
        }
        if query.page.is_empty() || query.page.len() > 64 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                "page 参数无效",
                false,
            ));
        }
        let params = vec![
            ("tags".to_owned(), query.tags.clone()),
            ("page".to_owned(), query.page.clone()),
            ("limit".to_owned(), query.limit.to_string()),
        ];
        let posts: Vec<Post> = self.get_json("posts.json", &params).await?;
        Ok(PostPage {
            posts,
            page: query.page.clone(),
            limit: query.limit,
        })
    }

    pub async fn post(&self, id: u64) -> Result<Post, DanbooruError> {
        if id == 0 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                "post id 必须大于 0",
                false,
            ));
        }
        self.get_json(&format!("posts/{id}.json"), &[]).await
    }

    pub async fn autocomplete(
        &self,
        query: &str,
        limit: u16,
    ) -> Result<Vec<AutocompleteItem>, DanbooruError> {
        if query.trim().is_empty() || query.len() > 256 || !(1..=20).contains(&limit) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                "自动补全参数无效",
                false,
            ));
        }
        let params = vec![
            ("search[query]".to_owned(), query.to_owned()),
            ("search[type]".to_owned(), "tag_query".to_owned()),
            ("limit".to_owned(), limit.to_string()),
        ];
        self.get_json("autocomplete.json", &params).await
    }

    pub async fn tag_category(&self, tag: &str) -> Result<Option<i64>, DanbooruError> {
        let tag = tag.trim();
        if tag.is_empty() || tag.len() > 256 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                "标签名称长度必须在 1..=256 字节之间",
                false,
            ));
        }
        let tags: Vec<TagCategoryResponse> = self
            .get_json(
                "tags.json",
                &[
                    ("search[name]".to_owned(), tag.to_owned()),
                    ("limit".to_owned(), "1".to_owned()),
                ],
            )
            .await?;
        Ok(tags.first().and_then(|item| item.category))
    }

    pub async fn count(&self, tags: &str) -> Result<u64, DanbooruError> {
        let value: Value = self
            .get_json("counts/posts.json", &[("tags".to_owned(), tags.to_owned())])
            .await?;
        value
            .pointer("/counts/posts")
            .and_then(Value::as_u64)
            .or_else(|| value.get("count").and_then(Value::as_u64))
            .ok_or_else(|| {
                DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    "Danbooru 计数响应缺少 counts.posts",
                    false,
                )
            })
    }

    pub fn validate_media_url(&self, candidate: &str) -> Result<Url, DanbooruError> {
        let url = Url::parse(candidate).map_err(|_| {
            DanbooruError::new(DanbooruErrorKind::UnsafeMediaUrl, "媒体地址格式无效", false)
        })?;
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(DanbooruError::new(
                DanbooruErrorKind::UnsafeMediaUrl,
                "媒体地址不能包含用户信息或片段",
                false,
            ));
        }
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
            return Err(DanbooruError::new(
                DanbooruErrorKind::UnsafeMediaUrl,
                "媒体地址缺少主机名",
                false,
            ));
        };
        if !self.trusted_media_hosts.contains(&host) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::UnsafeMediaUrl,
                format!("媒体主机不在允许列表: {host}"),
                false,
            ));
        }
        let loopback = is_loopback_host(&host);
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && loopback
                && self.base_url.host_str().is_some_and(is_loopback_host))
        {
            return Err(DanbooruError::new(
                DanbooruErrorKind::UnsafeMediaUrl,
                "媒体地址必须使用 HTTPS",
                false,
            ));
        }
        Ok(url)
    }

    pub fn resolve_media(
        &self,
        post: &Post,
        variant: MediaVariant,
    ) -> Result<MediaSource, DanbooruError> {
        let (url, extension, size, md5) = match variant {
            MediaVariant::Original | MediaVariant::UgoiraZip => {
                let url = post.file_url.as_deref().ok_or_else(|| {
                    DanbooruError::new(
                        DanbooruErrorKind::Forbidden,
                        "该帖子没有可访问的原文件地址",
                        false,
                    )
                })?;
                let extension = post.file_ext.clone().or_else(|| extension_from_url(url));
                let extension = normalized_extension(extension.as_deref())?;
                if variant == MediaVariant::UgoiraZip && extension != "zip" {
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::UnsupportedMedia,
                        "该帖子不是 Ugoira ZIP",
                        false,
                    ));
                }
                (url, extension, post.file_size, post.md5.clone())
            }
            MediaVariant::Large => {
                let url = post.large_file_url.as_deref().ok_or_else(|| {
                    DanbooruError::new(
                        DanbooruErrorKind::Forbidden,
                        "该帖子没有 large 媒体地址",
                        false,
                    )
                })?;
                (
                    url,
                    normalized_extension(extension_from_url(url).as_deref())?,
                    None,
                    None,
                )
            }
            MediaVariant::Preview => {
                let url = post.preview_file_url.as_deref().ok_or_else(|| {
                    DanbooruError::new(DanbooruErrorKind::Forbidden, "该帖子没有预览地址", false)
                })?;
                (
                    url,
                    normalized_extension(extension_from_url(url).as_deref())?,
                    None,
                    None,
                )
            }
            MediaVariant::Sample => {
                if let Ok(info) = select_asset_variant(post, |item| {
                    if item.variant_type == "original" {
                        return false;
                    }
                    item.file_ext
                        .as_deref()
                        .is_some_and(is_static_image_extension)
                        || item
                            .url
                            .as_deref()
                            .and_then(extension_from_url)
                            .as_deref()
                            .is_some_and(is_static_image_extension)
                }) {
                    let url = info.url.as_deref().ok_or_else(|| {
                        DanbooruError::new(
                            DanbooruErrorKind::InvalidResponse,
                            "媒体 variant 缺少 URL",
                            false,
                        )
                    })?;
                    let extension = info.file_ext.clone().or_else(|| extension_from_url(url));
                    (
                        url,
                        normalized_extension(extension.as_deref())?,
                        info.file_size,
                        info.md5.clone(),
                    )
                } else {
                    let url = post
                        .large_file_url
                        .as_deref()
                        .filter(|url| {
                            extension_from_url(url)
                                .as_deref()
                                .is_some_and(is_static_image_extension)
                        })
                        .or(post.preview_file_url.as_deref())
                        .or(post.large_file_url.as_deref())
                        .ok_or_else(|| {
                            DanbooruError::new(
                                DanbooruErrorKind::Forbidden,
                                "该帖子没有可访问的清晰预览地址",
                                false,
                            )
                        })?;
                    (
                        url,
                        normalized_extension(extension_from_url(url).as_deref())?,
                        None,
                        None,
                    )
                }
            }
            MediaVariant::UgoiraWebm => {
                let info = select_asset_variant(post, |item| {
                    item.file_ext.as_deref() == Some("webm")
                        || item.url.as_deref().and_then(extension_from_url).as_deref()
                            == Some("webm")
                })?;
                let url = info.url.as_deref().ok_or_else(|| {
                    DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "Ugoira WebM variant 缺少 URL",
                        false,
                    )
                })?;
                (url, "webm".into(), info.file_size, info.md5.clone())
            }
        };
        if size.is_some_and(|bytes| bytes > MAX_MEDIA_DOWNLOAD_BYTES) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidResponse,
                "媒体文件超过可信大小上限",
                false,
            ));
        }
        let url = self.validate_media_url(url)?;
        Ok(MediaSource {
            post_id: post.id,
            variant,
            url,
            extension,
            expected_size: size,
            expected_md5: md5,
        })
    }

    pub fn recommended_media(&self, post: &Post) -> Result<Vec<MediaSource>, DanbooruError> {
        if post.file_ext.as_deref() == Some("zip") {
            return Ok(vec![
                self.resolve_media(post, MediaVariant::UgoiraWebm)?,
                self.resolve_media(post, MediaVariant::UgoiraZip)?,
            ]);
        }
        Ok(vec![self.resolve_media(post, MediaVariant::Original)?])
    }

    pub async fn open_media(
        &self,
        source: &MediaSource,
        range: Option<&str>,
    ) -> Result<MediaResponse, DanbooruError> {
        let response = self.send_media(source.url.clone(), range).await?;
        let status = response.status();
        if !(status.is_success() || status == StatusCode::PARTIAL_CONTENT) {
            return Err(map_status(status, retry_after(&response)));
        }
        validate_content_type(&source.extension, response.headers().get(CONTENT_TYPE))?;
        let (max_body_bytes, expected_body_bytes) = if status == StatusCode::PARTIAL_CONTENT {
            let range = parse_content_range(response.headers().get(CONTENT_RANGE))?;
            if let Some(expected) = source.expected_size {
                if range.total != expected {
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::Integrity,
                        format!(
                            "媒体 Content-Range 总长度与帖子元数据不一致: 期望 {expected}，实际 {}",
                            range.total
                        ),
                        true,
                    ));
                }
            }
            let segment_length = range.end - range.start + 1;
            if response
                .content_length()
                .is_some_and(|advertised| advertised != segment_length)
            {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::Integrity,
                    "媒体 Content-Length 与 Content-Range 片段长度不一致",
                    true,
                ));
            }
            (segment_length, Some(segment_length))
        } else {
            if let (Some(expected), Some(advertised)) =
                (source.expected_size, response.content_length())
            {
                if advertised != expected {
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::Integrity,
                        format!(
                            "媒体 Content-Length 与帖子元数据不一致: 期望 {expected}，实际 {advertised}"
                        ),
                        true,
                    ));
                }
            }
            let expected = source.expected_size.or_else(|| response.content_length());
            (
                expected
                    .unwrap_or(MAX_MEDIA_DOWNLOAD_BYTES)
                    .min(MAX_MEDIA_DOWNLOAD_BYTES),
                expected,
            )
        };
        Ok(MediaResponse::new(
            response,
            max_body_bytes,
            expected_body_bytes,
        ))
    }

    pub async fn validate_existing_media(
        &self,
        source: &MediaSource,
        path: &Path,
    ) -> Result<(), DanbooruError> {
        validate_file(path, source.expected_size, source.expected_md5.as_deref()).await
    }

    #[cfg(test)]
    pub async fn download(
        &self,
        request: &MediaDownloadRequest,
    ) -> Result<DownloadOutcome, DanbooruError> {
        match self
            .download_with_control(request, |_| DownloadControl::Continue)
            .await?
        {
            ControlledDownloadOutcome::Completed(outcome) => Ok(outcome),
            ControlledDownloadOutcome::Stopped { .. } => unreachable!("unconditional download"),
        }
    }

    pub async fn download_with_control<F>(
        &self,
        request: &MediaDownloadRequest,
        mut control: F,
    ) -> Result<ControlledDownloadOutcome, DanbooruError>
    where
        F: FnMut(DownloadProgress) -> DownloadControl,
    {
        if request
            .source
            .expected_size
            .is_some_and(|bytes| bytes > MAX_MEDIA_DOWNLOAD_BYTES)
        {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidResponse,
                "媒体文件超过 16 GiB 大小上限",
                false,
            ));
        }
        validate_filename_template(&request.filename_template)?;
        let filename = render_filename(
            &request.filename_template,
            FilenameContext {
                id: request.source.post_id,
                score: request.score,
                rating: &request.rating,
                extension: &request.source.extension,
            },
        )?;
        fs::create_dir_all(&request.destination_dir)
            .await
            .map_err(|error| io_error("无法创建下载目录", error))?;
        let destination_dir = fs::canonicalize(&request.destination_dir)
            .await
            .map_err(|error| io_error("无法规范化下载目录", error))?;
        let final_path = destination_dir.join(filename);
        let target_lock = download_path_lock(&final_path);
        let _target_guard = target_lock.lock().await;
        let part_path = part_path_for(&final_path)?;
        let max_total_bytes = request
            .source
            .expected_size
            .unwrap_or(MAX_MEDIA_DOWNLOAD_BYTES)
            .min(MAX_MEDIA_DOWNLOAD_BYTES);

        if fs::try_exists(&final_path)
            .await
            .map_err(|error| io_error("无法检查目标文件", error))?
        {
            validate_file(
                &final_path,
                request.source.expected_size,
                request.source.expected_md5.as_deref(),
            )
            .await?;
            let bytes_written = fs::metadata(&final_path)
                .await
                .map_err(|error| io_error("无法读取目标文件信息", error))?
                .len();
            return Ok(ControlledDownloadOutcome::Completed(DownloadOutcome {
                path: final_path,
                bytes_written,
                resumed: false,
                already_present: true,
            }));
        }

        let (mut file, mut existing) = open_secure_part_file(&part_path).await?;
        if existing > max_total_bytes {
            file.set_len(0)
                .await
                .map_err(|error| io_error("重置超限临时文件失败", error))?;
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidResponse,
                "媒体文件超过可信大小上限",
                false,
            ));
        }
        let resumed = existing > 0;
        let mut restarted = false;
        if control(DownloadProgress {
            bytes_written: existing,
            total_bytes: request.source.expected_size,
        }) == DownloadControl::Stop
        {
            return Ok(ControlledDownloadOutcome::Stopped {
                part_path,
                bytes_written: existing,
            });
        }

        loop {
            let range = (existing > 0).then(|| format!("bytes={existing}-"));
            let response = self
                .send_media(request.source.url.clone(), range.as_deref())
                .await?;
            let status = response.status();

            if status == StatusCode::RANGE_NOT_SATISFIABLE {
                if request.source.expected_size == Some(existing) {
                    break;
                }
                if restarted {
                    return Err(map_status(status, retry_after(&response)));
                }
                file.set_len(0)
                    .await
                    .map_err(|error| io_error("无法重置临时下载文件", error))?;
                file.seek(SeekFrom::Start(0))
                    .await
                    .map_err(|error| io_error("无法定位临时下载文件", error))?;
                existing = 0;
                restarted = true;
                continue;
            }
            if !(status.is_success() || status == StatusCode::PARTIAL_CONTENT) {
                return Err(map_status(status, retry_after(&response)));
            }
            if existing == 0 && status == StatusCode::PARTIAL_CONTENT {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::Integrity,
                    "完整媒体请求收到了未请求的局部响应",
                    true,
                ));
            }
            validate_content_type(
                &request.source.extension,
                response.headers().get(CONTENT_TYPE),
            )?;

            if existing > 0 && status == StatusCode::PARTIAL_CONTENT {
                validate_content_range(
                    response.headers().get(CONTENT_RANGE),
                    existing,
                    request.source.expected_size,
                )?;
            } else if existing > 0 {
                file.set_len(0)
                    .await
                    .map_err(|error| io_error("无法重置临时下载文件", error))?;
                file.seek(SeekFrom::Start(0))
                    .await
                    .map_err(|error| io_error("无法定位临时下载文件", error))?;
                existing = 0;
                restarted = true;
            }

            let advertised_length = response.content_length();
            if advertised_length
                .is_some_and(|length| existing.saturating_add(length) > max_total_bytes)
            {
                file.set_len(0)
                    .await
                    .map_err(|error| io_error("重置超限临时文件失败", error))?;
                return Err(DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    "媒体文件超过可信大小上限",
                    false,
                ));
            }
            if existing > 0 {
                file.seek(SeekFrom::End(0))
                    .await
                    .map_err(|error| io_error("无法定位临时下载文件", error))?;
            } else {
                file.set_len(0)
                    .await
                    .map_err(|error| io_error("无法重置临时下载文件", error))?;
                file.seek(SeekFrom::Start(0))
                    .await
                    .map_err(|error| io_error("无法定位临时下载文件", error))?;
            }

            let mut received = 0_u64;
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                DanbooruError::new(
                    DanbooruErrorKind::Network,
                    format!("媒体响应读取失败: {error}"),
                    true,
                )
            })? {
                if existing
                    .saturating_add(received)
                    .saturating_add(chunk.len() as u64)
                    > max_total_bytes
                {
                    file.set_len(0)
                        .await
                        .map_err(|error| io_error("重置超限临时文件失败", error))?;
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "媒体文件超过可信大小上限",
                        false,
                    ));
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|error| io_error("写入临时下载文件失败", error))?;
                received = received.saturating_add(chunk.len() as u64);
                let bytes_written = existing.saturating_add(received);
                if control(DownloadProgress {
                    bytes_written,
                    total_bytes: request.source.expected_size,
                }) == DownloadControl::Stop
                {
                    file.flush()
                        .await
                        .map_err(|error| io_error("刷新暂停下载文件失败", error))?;
                    file.sync_all()
                        .await
                        .map_err(|error| io_error("同步暂停下载文件失败", error))?;
                    return Ok(ControlledDownloadOutcome::Stopped {
                        part_path,
                        bytes_written,
                    });
                }
            }
            file.flush()
                .await
                .map_err(|error| io_error("刷新临时下载文件失败", error))?;
            file.sync_all()
                .await
                .map_err(|error| io_error("同步临时下载文件失败", error))?;

            if advertised_length.is_some_and(|length| length != received) {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::Integrity,
                    "媒体响应长度与 Content-Length 不一致",
                    true,
                ));
            }
            break;
        }

        if let Err(error) = validate_open_file(
            &mut file,
            request.source.expected_size,
            request.source.expected_md5.as_deref(),
        )
        .await
        {
            remove_if_exists(&part_path).await?;
            return Err(error);
        }

        let bytes_written = file
            .metadata()
            .await
            .map_err(|error| io_error("无法读取下载文件信息", error))?
            .len();
        drop(file);
        fs::rename(&part_path, &final_path)
            .await
            .map_err(|error| io_error("无法原子提交下载文件", error))?;

        Ok(ControlledDownloadOutcome::Completed(DownloadOutcome {
            path: final_path,
            bytes_written,
            resumed: resumed && !restarted,
            already_present: false,
        }))
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        params: &[(String, String)],
    ) -> Result<T, DanbooruError> {
        const MAX_API_JSON_BYTES: usize = 8 * 1024 * 1024;
        let mut url = self.base_url.join(path).map_err(|error| {
            DanbooruError::new(
                DanbooruErrorKind::InvalidRequest,
                format!("API 路径无效: {error}"),
                false,
            )
        })?;
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(key, value)| (key, value)));

        let mut retried_rate_limit = false;
        loop {
            self.rate_limiter.acquire().await;
            let mut request = self.client.get(url.clone());
            if !self.username.is_empty() && !self.api_key.is_empty() {
                request = request.basic_auth(self.username.as_ref(), Some(self.api_key.as_ref()));
            }
            let response = request.send().await.map_err(network_error)?;
            if response.status().is_redirection() {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    "Danbooru API 返回了未预期的重定向",
                    false,
                )
                .with_status(response.status()));
            }
            if response.status() == StatusCode::TOO_MANY_REQUESTS && !retried_rate_limit {
                let delay = retry_after(&response).unwrap_or(Duration::from_secs(1));
                if delay <= Duration::from_secs(60) {
                    retried_rate_limit = true;
                    sleep(delay).await;
                    continue;
                }
            }
            if !response.status().is_success() {
                return Err(map_status(response.status(), retry_after(&response)));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_API_JSON_BYTES as u64)
            {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    "Danbooru JSON 响应过大",
                    false,
                ));
            }
            let mut response = response;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| {
                DanbooruError::new(
                    DanbooruErrorKind::Network,
                    format!("Danbooru JSON 响应读取失败: {error}"),
                    true,
                )
            })? {
                if body.len().saturating_add(chunk.len()) > MAX_API_JSON_BYTES {
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "Danbooru JSON 响应过大",
                        false,
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            return serde_json::from_slice::<T>(&body).map_err(|error| {
                DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    format!("Danbooru JSON 响应无效: {error}"),
                    false,
                )
            });
        }
    }

    async fn send_media(
        &self,
        mut url: Url,
        range: Option<&str>,
    ) -> Result<Response, DanbooruError> {
        const MAX_REDIRECTS: usize = 5;
        let mut redirect_count = 0;
        let mut retried_rate_limit = false;
        loop {
            self.validate_media_url(url.as_str())?;
            self.rate_limiter.acquire().await;
            let mut request = self.client.get(url.clone());
            if let Some(range) = range {
                request = request.header(RANGE, range);
            }
            let response = request.send().await.map_err(network_error)?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS && !retried_rate_limit {
                let delay = retry_after(&response).unwrap_or(Duration::from_secs(1));
                if delay <= Duration::from_secs(60) {
                    retried_rate_limit = true;
                    sleep(delay).await;
                    continue;
                }
            }
            if !response.status().is_redirection() {
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_MEDIA_DOWNLOAD_BYTES)
                {
                    return Err(DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "媒体文件超过 16 GiB 大小上限",
                        false,
                    ));
                }
                return Ok(response);
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(DanbooruError::new(
                    DanbooruErrorKind::InvalidResponse,
                    "媒体重定向次数过多",
                    false,
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    DanbooruError::new(
                        DanbooruErrorKind::InvalidResponse,
                        "媒体重定向缺少 Location",
                        false,
                    )
                })?;
            url = url.join(location).map_err(|_| {
                DanbooruError::new(
                    DanbooruErrorKind::UnsafeMediaUrl,
                    "媒体重定向地址无效",
                    false,
                )
            })?;
            self.validate_media_url(url.as_str())?;
            redirect_count += 1;
            retried_rate_limit = false;
        }
    }
}

fn download_path_lock(path: &Path) -> Arc<Mutex<()>> {
    let key = download_lock_key(path);
    let registry = DOWNLOAD_PATH_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

#[cfg(windows)]
fn download_lock_key(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
fn download_lock_key(path: &Path) -> PathBuf {
    path.to_owned()
}

fn parse_base_url(candidate: &str) -> Result<Url, DanbooruError> {
    let mut url = Url::parse(candidate).map_err(|_| {
        DanbooruError::new(
            DanbooruErrorKind::InvalidRequest,
            "Danbooru base URL 无效",
            false,
        )
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidRequest,
            "Danbooru base URL 不能包含凭据、查询或片段",
            false,
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        DanbooruError::new(
            DanbooruErrorKind::InvalidRequest,
            "Danbooru base URL 缺少主机名",
            false,
        )
    })?;
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback_host(host)) {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidRequest,
            "Danbooru base URL 必须使用 HTTPS；测试仅允许 loopback HTTP",
            false,
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn network_error(error: reqwest::Error) -> DanbooruError {
    DanbooruError::new(
        DanbooruErrorKind::Network,
        format!("Danbooru 网络请求失败: {error}"),
        true,
    )
}

fn retry_after(response: &Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value).ok().map(|retry_at| {
        retry_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
    })
}

fn map_status(status: StatusCode, retry_after: Option<Duration>) -> DanbooruError {
    let (kind, message, retryable) = match status.as_u16() {
        400 | 424 => (
            DanbooruErrorKind::InvalidQuery,
            "Danbooru 查询语法无效",
            false,
        ),
        401 => (
            DanbooruErrorKind::InvalidCredentials,
            "Danbooru 凭据无效",
            false,
        ),
        403 => (
            DanbooruErrorKind::Forbidden,
            "没有权限访问该 Danbooru 资源",
            false,
        ),
        404 => (DanbooruErrorKind::NotFound, "Danbooru 资源不存在", false),
        410 => (
            DanbooruErrorKind::PageLimit,
            "Danbooru 页码超出账户权限",
            false,
        ),
        422 => (
            DanbooruErrorKind::TagLimit,
            "查询标签数量超出账户权限",
            false,
        ),
        429 => (
            DanbooruErrorKind::RateLimited,
            "Danbooru 请求速率受限",
            true,
        ),
        502..=504 => (
            DanbooruErrorKind::UpstreamUnavailable,
            "Danbooru 服务暂时不可用",
            true,
        ),
        _ if status.is_server_error() => (
            DanbooruErrorKind::UpstreamUnavailable,
            "Danbooru 服务返回服务器错误",
            true,
        ),
        _ => (
            DanbooruErrorKind::InvalidResponse,
            "Danbooru 返回未预期的 HTTP 状态",
            false,
        ),
    };
    DanbooruError::new(kind, message, retryable)
        .with_status(status)
        .with_retry_after(retry_after)
}

fn normalized_extension(extension: Option<&str>) -> Result<String, DanbooruError> {
    let extension = extension
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let extension = if extension == "jpeg" {
        "jpg".to_owned()
    } else {
        extension
    };
    if extension == "swf" {
        return Err(DanbooruError::new(
            DanbooruErrorKind::UnsupportedMedia,
            "SWF 不会被下载或执行",
            false,
        ));
    }
    if !SUPPORTED_MEDIA_EXTENSIONS.contains(&extension.as_str()) && extension != "zip" {
        return Err(DanbooruError::new(
            DanbooruErrorKind::UnsupportedMedia,
            format!("不支持的媒体格式: {extension}"),
            false,
        ));
    }
    Ok(extension)
}

fn extension_from_url(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .path_segments()?
        .next_back()?
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

fn is_static_image_extension(extension: &str) -> bool {
    matches!(extension, "jpg" | "png" | "webp" | "gif" | "avif")
}

fn select_asset_variant<F>(post: &Post, predicate: F) -> Result<&MediaAssetVariant, DanbooruError>
where
    F: Fn(&MediaAssetVariant) -> bool,
{
    post.media_asset
        .as_ref()
        .into_iter()
        .flat_map(|asset| asset.variants.iter())
        .filter(|item| item.url.as_ref().is_some_and(|url| !url.is_empty()))
        .filter(|item| predicate(item))
        .max_by_key(|item| u64::from(item.width.unwrap_or(0)) * u64::from(item.height.unwrap_or(0)))
        .ok_or_else(|| {
            DanbooruError::new(
                DanbooruErrorKind::UnsupportedMedia,
                "帖子缺少请求的媒体 variant",
                false,
            )
        })
}

fn validate_content_type(
    extension: &str,
    value: Option<&reqwest::header::HeaderValue>,
) -> Result<(), DanbooruError> {
    let Some(content_type) = value.and_then(|value| value.to_str().ok()) else {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidResponse,
            "媒体响应缺少有效的 Content-Type",
            false,
        ));
    };
    let content_type = content_type.split(';').next().unwrap_or_default().trim();
    let allowed = match extension {
        "jpg" => content_type == "image/jpeg",
        "png" => content_type == "image/png",
        "webp" => content_type == "image/webp",
        "gif" => content_type == "image/gif",
        "avif" => content_type == "image/avif",
        "mp4" => content_type == "video/mp4",
        "webm" => content_type == "video/webm",
        "zip" => matches!(
            content_type,
            "application/zip" | "application/x-zip-compressed"
        ),
        _ => false,
    };
    if !allowed {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidResponse,
            format!("媒体 MIME 与扩展名不匹配: {content_type} / {extension}"),
            false,
        ));
    }
    Ok(())
}

fn validate_content_range(
    value: Option<&reqwest::header::HeaderValue>,
    expected_start: u64,
    expected_total: Option<u64>,
) -> Result<(), DanbooruError> {
    let range = parse_content_range(value)?;
    if range.start != expected_start {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Integrity,
            "Range 响应起点与临时文件长度不一致",
            true,
        ));
    }
    if expected_total.is_some_and(|expected| expected != range.total) {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Integrity,
            "Range 响应总长度与帖子元数据不一致",
            true,
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedContentRange {
    start: u64,
    end: u64,
    total: u64,
}

fn parse_content_range(
    value: Option<&reqwest::header::HeaderValue>,
) -> Result<ParsedContentRange, DanbooruError> {
    let parsed = value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('/'))
        .and_then(|(range, total)| {
            let (start, end) = range.split_once('-')?;
            Some(ParsedContentRange {
                start: start.parse().ok()?,
                end: end.parse().ok()?,
                total: total.parse().ok()?,
            })
        })
        .filter(|range| {
            range.start <= range.end
                && range.end < range.total
                && range.total <= MAX_MEDIA_DOWNLOAD_BYTES
        });
    parsed.ok_or_else(|| {
        DanbooruError::new(
            DanbooruErrorKind::InvalidResponse,
            "Range 响应缺少有效的 Content-Range",
            false,
        )
    })
}

pub fn validate_filename_template(template: &str) -> Result<(), DanbooruError> {
    if template.is_empty()
        || template.len() > 240
        || template.contains('/')
        || template.contains('\\')
        || template
            .chars()
            .any(|character| character.is_control() || "<>:\"|?*".contains(character))
    {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "文件名模板包含不允许的字符",
            false,
        ));
    }
    if !template.contains("{id}") || !template.contains("{ext}") {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "文件名模板必须包含 {id} 和 {ext}",
            false,
        ));
    }

    let mut remainder = template;
    while let Some(open) = remainder.find('{') {
        let after_open = &remainder[open..];
        let Some(close) = after_open.find('}') else {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidTemplate,
                "文件名模板包含未闭合的占位符",
                false,
            ));
        };
        let token = &after_open[..=close];
        if !matches!(token, "{id}" | "{score}" | "{rating}" | "{ext}") {
            return Err(DanbooruError::new(
                DanbooruErrorKind::InvalidTemplate,
                format!("不支持的文件名占位符: {token}"),
                false,
            ));
        }
        remainder = &after_open[close + 1..];
    }
    if remainder.contains('}') {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "文件名模板包含多余的右花括号",
            false,
        ));
    }
    Ok(())
}

pub fn render_filename(
    template: &str,
    context: FilenameContext<'_>,
) -> Result<String, DanbooruError> {
    validate_filename_template(template)?;
    if !matches!(context.rating, "g" | "s" | "q" | "e") {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "rating 必须是 g、s、q 或 e",
            false,
        ));
    }
    let extension = normalized_extension(Some(context.extension))?;
    if extension == "zip" && context.extension != "zip" {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "文件扩展名无效",
            false,
        ));
    }
    let filename = template
        .replace("{id}", &context.id.to_string())
        .replace("{score}", &context.score.to_string())
        .replace("{rating}", context.rating)
        .replace("{ext}", &extension);
    validate_rendered_filename(&filename)?;
    if filename == "." || filename == ".." || filename.starts_with('.') {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "文件名模板不能生成隐藏或相对路径文件",
            false,
        ));
    }
    Ok(filename)
}

fn validate_rendered_filename(filename: &str) -> Result<(), DanbooruError> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "渲染后的文件名必须是单个普通路径组件",
            false,
        ));
    }
    if filename.ends_with('.') || filename.ends_with(' ') {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "渲染后的文件名不能以点或空格结尾",
            false,
        ));
    }
    if is_windows_reserved_filename(filename) {
        return Err(DanbooruError::new(
            DanbooruErrorKind::InvalidTemplate,
            "渲染后的文件名使用了 Windows 保留设备名",
            false,
        ));
    }
    Ok(())
}

fn is_windows_reserved_filename(filename: &str) -> bool {
    let basename = filename
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || basename
            .strip_prefix("COM")
            .or_else(|| basename.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn part_path_for(final_path: &Path) -> Result<PathBuf, DanbooruError> {
    let filename = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            DanbooruError::new(DanbooruErrorKind::InvalidTemplate, "目标文件名无效", false)
        })?;
    Ok(final_path.with_file_name(format!("{filename}.part")))
}

async fn open_secure_part_file(path: &Path) -> Result<(File, u64), DanbooruError> {
    let path = path.to_owned();
    let (file, existing) = tokio::task::spawn_blocking(move || open_secure_part_file_sync(&path))
        .await
        .map_err(|error| {
            DanbooruError::new(
                DanbooruErrorKind::Io,
                format!("打开临时下载文件的后台任务失败: {error}"),
                true,
            )
        })??;
    Ok((File::from_std(file), existing))
}

fn open_secure_part_file_sync(path: &Path) -> Result<(std::fs::File, u64), DanbooruError> {
    let mut create_options = std::fs::OpenOptions::new();
    create_options.read(true).write(true).create_new(true);
    configure_no_follow(&mut create_options);

    let file = match create_options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let path_metadata = std::fs::symlink_metadata(path)
                .map_err(|error| io_error("无法检查临时下载路径", error))?;
            validate_secure_part_metadata(&path_metadata)?;
            let mut existing_options = std::fs::OpenOptions::new();
            existing_options.read(true).write(true);
            configure_no_follow(&mut existing_options);
            existing_options
                .open(path)
                .map_err(|error| io_error("拒绝不安全的临时下载文件", error))?
        }
        Err(error) => return Err(io_error("无法原子创建临时下载文件", error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error("无法读取临时下载文件信息", error))?;
    validate_secure_part_metadata(&metadata)?;
    validate_secure_part_handle(&file)?;
    Ok((file, metadata.len()))
}

#[cfg(unix)]
fn configure_no_follow(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_NOFOLLOW: i32 = 0o400000;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const O_NOFOLLOW: i32 = 0x100;
    options.custom_flags(O_NOFOLLOW);
}

#[cfg(windows)]
fn configure_no_follow(options: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn configure_no_follow(_options: &mut std::fs::OpenOptions) {}

fn validate_secure_part_metadata(metadata: &std::fs::Metadata) -> Result<(), DanbooruError> {
    if !metadata.file_type().is_file() {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Io,
            "临时下载路径不是普通文件",
            false,
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::Io,
                "临时下载文件存在额外硬链接",
                false,
            ));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(DanbooruError::new(
                DanbooruErrorKind::Io,
                "临时下载路径不能是重解析点",
                false,
            ));
        }
    }

    Ok(())
}

#[cfg(windows)]
fn validate_secure_part_handle(file: &std::fs::File) -> Result<(), DanbooruError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFileType, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_TYPE_DISK,
    };

    let handle = file.as_raw_handle();
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Io,
            "临时下载句柄不是磁盘文件",
            false,
        ));
    }

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(io_error(
            "无法验证临时下载文件句柄",
            std::io::Error::last_os_error(),
        ));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Io,
            "临时下载句柄不能指向重解析点",
            false,
        ));
    }
    if information.nNumberOfLinks != 1 {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Io,
            "临时下载文件存在额外硬链接",
            false,
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_secure_part_handle(_file: &std::fs::File) -> Result<(), DanbooruError> {
    Ok(())
}

async fn validate_file(
    path: &Path,
    expected_size: Option<u64>,
    expected_md5: Option<&str>,
) -> Result<(), DanbooruError> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| io_error("无法读取下载文件信息", error))?;
    if expected_size.is_some_and(|size| size != metadata.len()) {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Integrity,
            format!(
                "下载文件长度不匹配: 期望 {expected_size:?}，实际 {}",
                metadata.len()
            ),
            true,
        ));
    }
    if let Some(expected_md5) = expected_md5.filter(|value| !value.is_empty()) {
        let actual = md5_file(path).await?;
        if !actual.eq_ignore_ascii_case(expected_md5) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::Integrity,
                format!("下载文件 MD5 不匹配: 期望 {expected_md5}，实际 {actual}"),
                true,
            ));
        }
    }
    Ok(())
}

async fn validate_open_file(
    file: &mut File,
    expected_size: Option<u64>,
    expected_md5: Option<&str>,
) -> Result<(), DanbooruError> {
    let metadata = file
        .metadata()
        .await
        .map_err(|error| io_error("无法读取下载文件信息", error))?;
    if expected_size.is_some_and(|size| size != metadata.len()) {
        return Err(DanbooruError::new(
            DanbooruErrorKind::Integrity,
            format!(
                "下载文件长度不匹配: 期望 {expected_size:?}，实际 {}",
                metadata.len()
            ),
            true,
        ));
    }
    if let Some(expected_md5) = expected_md5.filter(|value| !value.is_empty()) {
        file.seek(SeekFrom::Start(0))
            .await
            .map_err(|error| io_error("无法定位下载文件进行校验", error))?;
        let mut hasher = Md5::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|error| io_error("读取下载文件进行校验失败", error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected_md5) {
            return Err(DanbooruError::new(
                DanbooruErrorKind::Integrity,
                format!("下载文件 MD5 不匹配: 期望 {expected_md5}，实际 {actual}"),
                true,
            ));
        }
    }
    Ok(())
}

async fn md5_file(path: &Path) -> Result<String, DanbooruError> {
    let mut file = File::open(path)
        .await
        .map_err(|error| io_error("无法打开下载文件进行校验", error))?;
    let mut hasher = Md5::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("读取下载文件进行校验失败", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn remove_if_exists(path: &Path) -> Result<(), DanbooruError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("无法移除临时下载文件", error)),
    }
}

fn io_error(context: &str, error: std::io::Error) -> DanbooruError {
    DanbooruError::new(
        DanbooruErrorKind::Io,
        format!("{context}: {error}"),
        error.kind() == std::io::ErrorKind::Interrupted,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostQuery {
    pub tags: String,
    pub page: String,
    pub limit: u16,
}

impl Default for PostQuery {
    fn default() -> Self {
        Self {
            tags: String::new(),
            page: "1".into(),
            limit: 40,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostPage {
    pub posts: Vec<Post>,
    pub page: String,
    pub limit: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Post {
    pub id: u64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub uploader_id: Option<u64>,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub rating: String,
    #[serde(default)]
    pub image_width: u32,
    #[serde(default)]
    pub image_height: u32,
    #[serde(default)]
    pub tag_string: String,
    #[serde(default)]
    pub tag_string_general: String,
    #[serde(default)]
    pub tag_string_artist: String,
    #[serde(default)]
    pub tag_string_copyright: String,
    #[serde(default)]
    pub tag_string_character: String,
    #[serde(default)]
    pub tag_string_meta: String,
    #[serde(default)]
    pub file_ext: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_url: Option<String>,
    #[serde(default)]
    pub large_file_url: Option<String>,
    #[serde(default)]
    pub preview_file_url: Option<String>,
    #[serde(default)]
    pub fav_count: u64,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub media_asset: Option<MediaAsset>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaAsset {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub variants: Vec<MediaAssetVariant>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaAssetVariant {
    #[serde(default, rename = "type")]
    pub variant_type: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub file_ext: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutocompleteItem {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub category: Option<u8>,
    #[serde(default)]
    pub post_count: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TagCategoryResponse {
    #[serde(default)]
    category: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaVariant {
    Preview,
    Sample,
    Large,
    Original,
    UgoiraWebm,
    UgoiraZip,
}

#[derive(Debug, Clone)]
pub struct MediaSource {
    pub post_id: u64,
    pub variant: MediaVariant,
    pub url: Url,
    pub extension: String,
    pub expected_size: Option<u64>,
    pub expected_md5: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct FilenameContext<'a> {
    pub id: u64,
    pub score: i64,
    pub rating: &'a str,
    pub extension: &'a str,
}

#[derive(Debug, Clone)]
pub struct MediaDownloadRequest {
    pub source: MediaSource,
    pub destination_dir: PathBuf,
    pub filename_template: String,
    pub score: i64,
    pub rating: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOutcome {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub resumed: bool,
    pub already_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadControl {
    Continue,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub bytes_written: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlledDownloadOutcome {
    Completed(DownloadOutcome),
    Stopped {
        part_path: PathBuf,
        bytes_written: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{HeaderMap, Uri};
    use axum::response::{IntoResponse, Response as AxumResponse};
    use axum::routing::get;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    #[derive(Clone, Default)]
    struct QueryCapture {
        uri: Arc<StdMutex<Option<String>>>,
        authorization: Arc<StdMutex<Option<String>>>,
    }

    async fn capture_posts(
        State(capture): State<QueryCapture>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Json<Vec<Post>> {
        *capture.uri.lock().expect("query capture lock") = Some(uri.to_string());
        *capture.authorization.lock().expect("auth capture lock") = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Json(Vec::new())
    }

    async fn spawn_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("run mock server");
        });
        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn tag_category_lookup_uses_the_shared_danbooru_client() {
        let router = Router::new().route(
            "/tags.json",
            get(|| async { Json(serde_json::json!([{ "category": 1 }])) }),
        );
        let (endpoint, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: endpoint,
            requests_per_second: 1_000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let category = client.tag_category("john_doe").await.unwrap();

        server.abort();
        assert_eq!(category, Some(1));
    }

    async fn rate_limited_once(State(counter): State<Arc<AtomicUsize>>) -> AxumResponse {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, "0")],
                "slow down",
            )
                .into_response();
        }
        Json(Vec::<Post>::new()).into_response()
    }

    async fn serve_oversized_json() -> AxumResponse {
        let padding = "x".repeat(8 * 1024 * 1024);
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"padding":"{padding}"}}"#)))
            .unwrap()
    }

    #[derive(Clone)]
    struct MediaCapture {
        bytes: Arc<Vec<u8>>,
        ranges: Arc<StdMutex<Vec<Option<String>>>>,
    }

    async fn serve_resumable_media(
        State(capture): State<MediaCapture>,
        headers: HeaderMap,
    ) -> AxumResponse {
        let range = headers
            .get(RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        capture.ranges.lock().unwrap().push(range.clone());
        let start = range
            .as_deref()
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.strip_suffix('-'))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let status = if start > 0 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        let mut response = axum::http::Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "image/jpeg");
        if start > 0 {
            response = response.header(
                CONTENT_RANGE,
                format!(
                    "bytes {start}-{}/{}",
                    capture.bytes.len() - 1,
                    capture.bytes.len()
                ),
            );
        }
        response
            .body(Body::from(capture.bytes[start..].to_vec()))
            .unwrap()
    }

    async fn serve_counted_media(State(counter): State<Arc<AtomicUsize>>) -> AxumResponse {
        counter.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "image/jpeg")
            .body(Body::from("same target payload"))
            .unwrap()
    }

    async fn serve_unknown_length_media() -> AxumResponse {
        let chunks = tokio_stream::iter([
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"1234")),
            Ok(axum::body::Bytes::from_static(b"5678")),
        ]);
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "image/jpeg")
            .body(Body::from_stream(chunks))
            .unwrap()
    }

    async fn serve_slow_resumable_media(
        State(capture): State<MediaCapture>,
        headers: HeaderMap,
    ) -> AxumResponse {
        let range = headers
            .get(RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        capture.ranges.lock().unwrap().push(range.clone());
        let start = range
            .as_deref()
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.strip_suffix('-'))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let payload = capture.bytes[start..].to_vec();
        let stream = async_stream::stream! {
            for chunk in payload.chunks(4) {
                yield Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk));
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        };
        let status = if start > 0 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };
        let mut response = axum::http::Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "image/jpeg");
        if start > 0 {
            response = response.header(
                CONTENT_RANGE,
                format!(
                    "bytes {start}-{}/{}",
                    capture.bytes.len() - 1,
                    capture.bytes.len()
                ),
            );
        }
        response.body(Body::from_stream(stream)).unwrap()
    }

    async fn serve_wrong_declared_length_media() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(reqwest::header::CONTENT_LENGTH, "8")
            .body(Body::from("12345678"))
            .unwrap()
    }

    async fn serve_partial_with_wrong_total() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(CONTENT_RANGE, "bytes 2-5/8")
            .header(reqwest::header::CONTENT_LENGTH, "4")
            .body(Body::from("3456"))
            .unwrap()
    }

    async fn serve_partial_with_wrong_segment_length() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(CONTENT_RANGE, "bytes 2-3/6")
            .header(reqwest::header::CONTENT_LENGTH, "4")
            .body(Body::from("3456"))
            .unwrap()
    }

    async fn serve_resumable_media_with_wrong_total() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(CONTENT_RANGE, "bytes 6-10/99")
            .header(reqwest::header::CONTENT_LENGTH, "5")
            .body(Body::from("world"))
            .unwrap()
    }

    async fn serve_partial_over_global_total() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(
                CONTENT_RANGE,
                format!("bytes 0-0/{}", MAX_MEDIA_DOWNLOAD_BYTES + 1),
            )
            .header(reqwest::header::CONTENT_LENGTH, "1")
            .body(Body::from("x"))
            .unwrap()
    }

    async fn serve_unrequested_partial_media() -> AxumResponse {
        axum::http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "image/jpeg")
            .header(CONTENT_RANGE, "bytes 10-14/15")
            .header(reqwest::header::CONTENT_LENGTH, "5")
            .body(Body::from("world"))
            .unwrap()
    }

    async fn rate_limited_media_once(State(counter): State<Arc<AtomicUsize>>) -> AxumResponse {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(RETRY_AFTER, "0")],
                "slow down",
            )
                .into_response();
        }
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "image/jpeg")
            .body(Body::from("media bytes"))
            .unwrap()
    }

    async fn future_http_date_rate_limit(State(counter): State<Arc<AtomicUsize>>) -> AxumResponse {
        counter.fetch_add(1, Ordering::SeqCst);
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(RETRY_AFTER, "Wed, 21 Oct 2099 07:28:00 GMT")],
            "slow down",
        )
            .into_response()
    }

    #[test]
    fn config_debug_redacts_api_key() {
        let config = DanbooruClientConfig {
            api_key: "top-secret".into(),
            ..DanbooruClientConfig::default()
        };

        let debug = format!("{config:?}");

        assert!(!debug.contains("top-secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[tokio::test]
    async fn posts_preserve_native_query_and_use_basic_auth() {
        let capture = QueryCapture::default();
        let router = Router::new()
            .route("/posts.json", get(capture_posts))
            .with_state(capture.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            username: "demo-user".into(),
            api_key: "demo-key".into(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        client
            .posts(&PostQuery {
                tags: "rating:e order:score -ai_generated".into(),
                page: "a12345".into(),
                limit: 40,
            })
            .await
            .unwrap();

        let uri = capture.uri.lock().unwrap().clone().expect("captured URI");
        let query = Url::parse(&format!("http://example.test{uri}"))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            query.get("tags").map(String::as_str),
            Some("rating:e order:score -ai_generated")
        );
        assert_eq!(query.get("page").map(String::as_str), Some("a12345"));
        assert_eq!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Basic ZGVtby11c2VyOmRlbW8ta2V5")
        );
        server.abort();
    }

    #[test]
    fn media_url_rejects_untrusted_hosts_and_plain_http() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();

        let host_error = client
            .validate_media_url("https://example.com/data/secret.jpg")
            .unwrap_err();
        let scheme_error = client
            .validate_media_url("http://cdn.donmai.us/data/image.jpg")
            .unwrap_err();

        assert_eq!(host_error.kind, DanbooruErrorKind::UnsafeMediaUrl);
        assert_eq!(scheme_error.kind, DanbooruErrorKind::UnsafeMediaUrl);
    }

    #[test]
    fn render_filename_rejects_rating_outside_danbooru_domain() {
        let error = render_filename(
            "{id}_{rating}.{ext}",
            FilenameContext {
                id: 42,
                score: 9,
                rating: "../outside",
                extension: "jpg",
            },
        )
        .unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidTemplate);
    }

    #[test]
    fn rendered_filename_must_be_one_normal_path_component() {
        let error = validate_rendered_filename("nested/file.jpg").unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidTemplate);
    }

    #[test]
    fn rendered_filename_rejects_windows_reserved_device_basename_with_extension() {
        let error = render_filename(
            "CON.{ext}.{id}",
            FilenameContext {
                id: 42,
                score: 9,
                rating: "s",
                extension: "jpg",
            },
        )
        .unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidTemplate);
    }

    #[test]
    fn rendered_filename_rejects_windows_trailing_dot() {
        let error = render_filename(
            "{id}.{ext}.",
            FilenameContext {
                id: 42,
                score: 9,
                rating: "s",
                extension: "jpg",
            },
        )
        .unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidTemplate);
    }

    #[test]
    fn rendered_filename_rejects_windows_trailing_space() {
        let error = render_filename(
            "{id}.{ext} ",
            FilenameContext {
                id: 42,
                score: 9,
                rating: "s",
                extension: "jpg",
            },
        )
        .unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidTemplate);
    }

    #[test]
    fn invalid_proxy_is_fail_closed() {
        let error = DanbooruClient::new(DanbooruClientConfig {
            proxy_url: Some(":// definitely-not-a-proxy".into()),
            ..DanbooruClientConfig::default()
        })
        .unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn retries_one_429_after_retry_after_delay() {
        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/posts.json", get(rate_limited_once))
            .with_state(counter.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let page = client.posts(&PostQuery::default()).await.unwrap();

        assert!(page.posts.is_empty());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn rejects_api_json_larger_than_the_bounded_response_limit() {
        let router = Router::new().route("/oversized.json", get(serve_oversized_json));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let error = match client.get_json::<Value>("oversized.json", &[]).await {
            Err(error) => error,
            Ok(_) => panic!("oversized JSON must be rejected before deserialization"),
        };

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
        assert!(error.message.contains("过大"));
        server.abort();
    }

    #[tokio::test]
    async fn media_request_retries_one_short_429_response() {
        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/media.jpg", get(rate_limited_media_once))
            .with_state(counter.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = client
            .send_media(Url::parse(&format!("{base_url}/media.jpg")).unwrap(), None)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn media_request_honors_http_date_retry_after_without_short_retry() {
        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/media.jpg", get(future_http_date_rate_limit))
            .with_state(counter.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = client
            .send_media(Url::parse(&format!("{base_url}/media.jpg")).unwrap(), None)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn sample_media_falls_back_to_large_for_posts_without_asset_variants() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let post = Post {
            id: 41,
            large_file_url: Some("https://cdn.donmai.us/fallback-large.jpg".into()),
            preview_file_url: Some("https://cdn.donmai.us/fallback-preview.jpg".into()),
            ..Post::default()
        };

        let source = client.resolve_media(&post, MediaVariant::Sample).unwrap();

        assert_eq!(
            source.url.as_str(),
            "https://cdn.donmai.us/fallback-large.jpg"
        );
        assert_eq!(source.extension, "jpg");
    }

    #[test]
    fn mp4_sample_resolves_to_the_largest_static_poster_variant() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let post = Post {
            id: 42,
            file_ext: Some("mp4".into()),
            file_url: Some("https://cdn.donmai.us/original/video.mp4".into()),
            large_file_url: Some("https://cdn.donmai.us/original/video.mp4".into()),
            preview_file_url: Some("https://cdn.donmai.us/180x180/poster.jpg".into()),
            media_asset: Some(MediaAsset {
                variants: vec![
                    MediaAssetVariant {
                        variant_type: "360x360".into(),
                        url: Some("https://cdn.donmai.us/360x360/poster.jpg".into()),
                        file_ext: Some("jpg".into()),
                        width: Some(360),
                        height: Some(202),
                        ..MediaAssetVariant::default()
                    },
                    MediaAssetVariant {
                        variant_type: "720x720".into(),
                        url: Some("https://cdn.donmai.us/720x720/poster.webp".into()),
                        file_ext: Some("webp".into()),
                        width: Some(720),
                        height: Some(404),
                        ..MediaAssetVariant::default()
                    },
                    MediaAssetVariant {
                        variant_type: "original".into(),
                        url: Some("https://cdn.donmai.us/original/video.mp4".into()),
                        file_ext: Some("mp4".into()),
                        width: Some(1280),
                        height: Some(720),
                        ..MediaAssetVariant::default()
                    },
                ],
                ..MediaAsset::default()
            }),
            ..Post::default()
        };

        let source = client.resolve_media(&post, MediaVariant::Sample).unwrap();

        assert_eq!(
            source.url.as_str(),
            "https://cdn.donmai.us/720x720/poster.webp"
        );
        assert_eq!(source.extension, "webp");
    }

    #[test]
    fn legacy_mp4_sample_falls_back_to_the_static_preview_not_the_video() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let post = Post {
            id: 43,
            file_ext: Some("mp4".into()),
            large_file_url: Some("https://cdn.donmai.us/original/video.mp4".into()),
            preview_file_url: Some("https://cdn.donmai.us/preview/poster.jpg".into()),
            ..Post::default()
        };

        let source = client.resolve_media(&post, MediaVariant::Sample).unwrap();

        assert_eq!(
            source.url.as_str(),
            "https://cdn.donmai.us/preview/poster.jpg"
        );
        assert_eq!(source.extension, "jpg");
    }

    #[test]
    fn media_resolution_rejects_a_known_file_over_the_global_limit() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let post = Post {
            id: 42,
            file_ext: Some("webm".into()),
            file_size: Some(16 * 1024 * 1024 * 1024 + 1),
            file_url: Some("https://cdn.donmai.us/oversized.webm".into()),
            ..Post::default()
        };

        let error = client
            .resolve_media(&post, MediaVariant::Original)
            .expect_err("oversized media must be rejected before streaming");

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
        assert!(error.message.contains("大小上限"));
    }

    #[test]
    fn media_content_type_is_required() {
        let error = validate_content_type("jpg", None)
            .expect_err("缺失 Content-Type 的媒体响应必须 fail-closed");

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
        assert!(error.message.contains("Content-Type"));
    }

    #[test]
    fn generic_binary_content_type_is_not_accepted_for_trusted_media_extension() {
        let value = reqwest::header::HeaderValue::from_static("application/octet-stream");

        let error = validate_content_type("jpg", Some(&value))
            .expect_err("泛化 MIME 不能代替可验证的扩展名映射");

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
    }

    #[tokio::test]
    async fn unknown_length_proxy_stream_stops_at_the_trusted_expected_size() {
        use tokio_stream::StreamExt;

        let router = Router::new().route("/media.jpg", get(serve_unknown_length_media));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
            extension: "jpg".into(),
            expected_size: Some(6),
            expected_md5: None,
        };

        let response = client.open_media(&source, None).await.unwrap();
        let stream = response.bytes_stream();
        tokio::pin!(stream);
        let mut received = 0_u64;
        let mut rejected = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => received += chunk.len() as u64,
                Err(_) => {
                    rejected = true;
                    break;
                }
            }
        }

        assert!(rejected, "未知长度响应超出预期大小时必须中断");
        assert!(received <= 6, "不能向下游交付超过可信上限的字节");
        server.abort();
    }

    #[tokio::test]
    async fn proxy_rejects_declared_length_that_disagrees_with_post_metadata() {
        let router = Router::new().route("/media.jpg", get(serve_wrong_declared_length_media));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
            extension: "jpg".into(),
            expected_size: Some(6),
            expected_md5: None,
        };

        let error = match client.open_media(&source, None).await {
            Err(error) => error,
            Ok(_) => panic!("Content-Length 必须与帖子元数据一致"),
        };

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
        server.abort();
    }

    #[tokio::test]
    async fn proxy_rejects_partial_total_that_disagrees_with_post_metadata() {
        let router = Router::new().route("/media.jpg", get(serve_partial_with_wrong_total));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
            extension: "jpg".into(),
            expected_size: Some(6),
            expected_md5: None,
        };

        let error = match client.open_media(&source, Some("bytes=2-")).await {
            Err(error) => error,
            Ok(_) => panic!("Content-Range 总长度必须与帖子元数据一致"),
        };

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
        server.abort();
    }

    #[tokio::test]
    async fn proxy_rejects_partial_content_length_outside_the_declared_segment() {
        let router =
            Router::new().route("/media.jpg", get(serve_partial_with_wrong_segment_length));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
            extension: "jpg".into(),
            expected_size: Some(6),
            expected_md5: None,
        };

        let error = match client.open_media(&source, Some("bytes=2-")).await {
            Err(error) => error,
            Ok(_) => panic!("Content-Length 必须与 Content-Range 片段长度一致"),
        };

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
        server.abort();
    }

    #[tokio::test]
    async fn existing_media_validation_rejects_a_truncated_file() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("42.jpg");
        fs::write(&path, b"short").await.unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse("https://cdn.donmai.us/42.jpg").unwrap(),
            extension: "jpg".into(),
            expected_size: Some(10),
            expected_md5: None,
        };

        let error = client
            .validate_existing_media(&source, &path)
            .await
            .expect_err("被截断的已有文件不能被 skip_existing 接受");

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
    }

    #[tokio::test]
    async fn existing_media_validation_rejects_an_md5_mismatch() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("42.jpg");
        fs::write(&path, b"same size, wrong bytes").await.unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Original,
            url: Url::parse("https://cdn.donmai.us/42.jpg").unwrap(),
            extension: "jpg".into(),
            expected_size: Some(b"same size, wrong bytes".len() as u64),
            expected_md5: Some("00000000000000000000000000000000".into()),
        };

        let error = client
            .validate_existing_media(&source, &path)
            .await
            .expect_err("MD5 不匹配的已有文件不能被 skip_existing 接受");

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
    }

    #[tokio::test]
    async fn unknown_length_download_stops_when_chunks_exceed_expected_size() {
        let router = Router::new().route("/media.jpg", get(serve_unknown_length_media));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(6),
                expected_md5: None,
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let error = client
            .download(&request)
            .await
            .expect_err("未知长度下载不能读取超过帖子预期大小的块");

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
        assert!(error.message.contains("上限"));
        server.abort();
    }

    #[tokio::test]
    async fn resumed_download_rejects_content_range_total_outside_post_metadata() {
        let router = Router::new().route("/media.jpg", get(serve_resumable_media_with_wrong_total));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("42_score_9.jpg.part"), b"hello ")
            .await
            .unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(11),
                expected_md5: Some("5eb63bbbe01eeed093cb22bb8f5acdc3".into()),
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let error = client
            .download(&request)
            .await
            .expect_err("Content-Range 总长度必须与帖子元数据一致");

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
        server.abort();
    }

    #[tokio::test]
    async fn controlled_download_stops_with_a_part_and_resumes_with_range() {
        let capture = MediaCapture {
            bytes: Arc::new(b"twelve-bytes".to_vec()),
            ranges: Arc::new(StdMutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/media.jpg", get(serve_slow_resumable_media))
            .with_state(capture.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(capture.bytes.len() as u64),
                expected_md5: None,
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let stopped = client
            .download_with_control(&request, |progress| {
                if progress.bytes_written >= 4 {
                    DownloadControl::Stop
                } else {
                    DownloadControl::Continue
                }
            })
            .await
            .unwrap();

        assert!(matches!(stopped, ControlledDownloadOutcome::Stopped { .. }));
        let part = directory.path().join("42_score_9.jpg.part");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 4);
        assert!(!directory.path().join("42_score_9.jpg").exists());

        let completed = client.download(&request).await.unwrap();
        assert!(completed.resumed);
        assert_eq!(
            std::fs::read(completed.path).unwrap(),
            capture.bytes.as_ref().clone()
        );
        assert_eq!(
            *capture.ranges.lock().unwrap(),
            vec![None, Some("bytes=4-".to_string())]
        );
        server.abort();
    }

    #[tokio::test]
    async fn proxy_rejects_partial_total_above_the_global_media_limit() {
        let router = Router::new().route("/media.jpg", get(serve_partial_over_global_total));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let source = MediaSource {
            post_id: 42,
            variant: MediaVariant::Preview,
            url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
            extension: "jpg".into(),
            expected_size: None,
            expected_md5: None,
        };

        let error = match client.open_media(&source, Some("bytes=0-0")).await {
            Err(error) => error,
            Ok(_) => panic!("Content-Range 总长度不能超过全局媒体上限"),
        };

        assert_eq!(error.kind, DanbooruErrorKind::InvalidResponse);
        server.abort();
    }

    #[tokio::test]
    async fn full_download_rejects_an_unrequested_partial_response() {
        let router = Router::new().route("/media.jpg", get(serve_unrequested_partial_media));
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(5),
                expected_md5: None,
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let error = client
            .download(&request)
            .await
            .expect_err("完整下载请求不能接受上游自发的局部响应");

        assert_eq!(error.kind, DanbooruErrorKind::Integrity);
        server.abort();
    }

    #[tokio::test]
    async fn resumes_to_part_validates_md5_and_atomically_renames() {
        let bytes = Arc::new(b"hello world".to_vec());
        let capture = MediaCapture {
            bytes: bytes.clone(),
            ranges: Arc::new(StdMutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/media.jpg", get(serve_resumable_media))
            .with_state(capture.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let part_path = directory.path().join("42_score_9.jpg.part");
        fs::write(&part_path, b"hello ").await.unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(bytes.len() as u64),
                expected_md5: Some("5eb63bbbe01eeed093cb22bb8f5acdc3".into()),
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let outcome = client.download(&request).await.unwrap();

        assert!(outcome.resumed);
        assert_eq!(fs::read(&outcome.path).await.unwrap(), bytes.as_slice());
        assert!(!fs::try_exists(part_path).await.unwrap());
        assert_eq!(
            capture.ranges.lock().unwrap().as_slice(),
            &[Some("bytes=6-".into())]
        );
        server.abort();
    }

    #[tokio::test]
    async fn secure_part_open_rejects_non_regular_path() {
        let directory = tempfile::tempdir().unwrap();
        let part_path = directory.path().join("42_score_9.jpg.part");
        fs::create_dir(&part_path).await.unwrap();

        let error = open_secure_part_file(&part_path).await.unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::Io);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn secure_part_open_rejects_windows_hard_link_without_touching_source() {
        let directory = tempfile::tempdir().unwrap();
        let outside_path = directory.path().join("outside.jpg");
        let part_path = directory.path().join("42_score_9.jpg.part");
        let original = b"outside bytes must stay unchanged";
        fs::write(&outside_path, original).await.unwrap();
        fs::hard_link(&outside_path, &part_path).await.unwrap();

        let error = open_secure_part_file(&part_path).await.unwrap_err();

        assert_eq!(error.kind, DanbooruErrorKind::Io);
        assert_eq!(fs::read(&outside_path).await.unwrap(), original);
    }

    #[tokio::test]
    async fn concurrent_downloads_to_same_target_are_serialized() {
        let requests = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/media.jpg", get(serve_counted_media))
            .with_state(requests.clone());
        let (base_url, server) = spawn_server(router).await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1000,
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let request = MediaDownloadRequest {
            source: MediaSource {
                post_id: 42,
                variant: MediaVariant::Original,
                url: Url::parse(&format!("{base_url}/media.jpg")).unwrap(),
                extension: "jpg".into(),
                expected_size: Some(b"same target payload".len() as u64),
                expected_md5: None,
            },
            destination_dir: directory.path().to_owned(),
            filename_template: DEFAULT_FILENAME_TEMPLATE.into(),
            score: 9,
            rating: "s".into(),
        };

        let (first, second) = tokio::join!(client.download(&request), client.download(&request));

        let first = first.unwrap();
        let second = second.unwrap();
        assert_ne!(first.already_present, second.already_present);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn ugoira_recommends_webm_and_original_zip_but_swf_is_rejected() {
        let client = DanbooruClient::new(DanbooruClientConfig::default()).unwrap();
        let post = Post {
            id: 7,
            file_ext: Some("zip".into()),
            file_url: Some("https://cdn.donmai.us/original.zip".into()),
            media_asset: Some(MediaAsset {
                variants: vec![MediaAssetVariant {
                    variant_type: "sample".into(),
                    url: Some("https://cdn.donmai.us/playable.webm".into()),
                    file_ext: Some("webm".into()),
                    ..MediaAssetVariant::default()
                }],
                ..MediaAsset::default()
            }),
            ..Post::default()
        };

        let media = client.recommended_media(&post).unwrap();
        let swf = Post {
            id: 8,
            file_ext: Some("swf".into()),
            file_url: Some("https://cdn.donmai.us/legacy.swf".into()),
            ..Post::default()
        };

        assert_eq!(media.len(), 2);
        assert_eq!(media[0].extension, "webm");
        assert_eq!(media[1].extension, "zip");
        assert_eq!(
            client
                .resolve_media(&swf, MediaVariant::Original)
                .unwrap_err()
                .kind,
            DanbooruErrorKind::UnsupportedMedia
        );
    }
}
