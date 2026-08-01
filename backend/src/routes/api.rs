use crate::app_paths::AppPaths;
use crate::config::{
    apply_vllm_base_url_override, load_settings, migrate_legacy_secret, migrate_legacy_settings,
    save_settings, PublicConfig, StoredSettings,
};
use crate::database::{
    Database, DownloadTaskHistoryCursor, MediaFileInput, MediaFileRecord, PostRecordInput,
    PostTagInput, QuarantineInput, QuarantineRecord, RootRecord, TaskItemInput,
};
use crate::media_root::{normalize_windows_path, MediaRoot};
use crate::secrets::{SecretKind, SecretManager, SystemCredentialVault};
use crate::services::danbooru::{
    validate_filename_template, AutocompleteItem, ControlledDownloadOutcome, DanbooruClient,
    DanbooruClientConfig, DanbooruError, DanbooruErrorKind, DownloadControl, DownloadProgress,
    MediaDownloadRequest, MediaVariant, Post, PostQuery,
};
use crate::services::image_processor::{
    apply_heic_conversion, apply_quarantine, apply_tag_pipeline, collect_tag_pipeline_tokens,
    is_quarantine_dir_name, plan_delete_by_tag, plan_delete_by_tag_selected, plan_exact_duplicates,
    plan_exact_duplicates_selected, plan_heic_conversion, plan_integrity_check,
    plan_integrity_check_selected, plan_near_duplicates, plan_near_duplicates_selected,
    plan_tag_pipeline_classified, resize_to_jpeg_with_quarantine,
    restore_quarantine as restore_batch, rollback_heic_conversion, rollback_tag_pipeline,
    ArtistPrefix, TagPipelineConfig, ToolManifest, VerifiedMediaRoot,
};
use crate::services::vllm::{
    VllmBatchItem, VllmBatchResult, VllmError, VllmErrorKind, VllmHealth, VllmOutputOptions,
    VllmRetryItem, VllmService, VllmServiceConfig, VllmTagSuccess,
};
use crate::tasks::{
    task_from_record, SqliteTaskStore, TaskFailure, TaskManager, TaskManagerError, TaskSnapshot,
    TaskStatus,
};
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
pub struct ApiSuccess<T: Serialize> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ApiFailureEnvelope {
    error: ApiFailure,
    request_id: String,
}

#[derive(Debug, Serialize)]
struct ApiFailure {
    code: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<Value>,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    retryable: bool,
    fields: Option<Value>,
}

impl ApiError {
    fn not_found(code: &str, message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: code.to_string(),
            message: message.to_string(),
            retryable: false,
            fields: None,
        }
    }

    fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            fields: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error".to_string(),
            message: message.into(),
            retryable: false,
            fields: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = uuid::Uuid::new_v4().to_string();
        let body = ApiFailureEnvelope {
            error: ApiFailure {
                code: self.code,
                message: self.message,
                retryable: self.retryable,
                fields: self.fields,
            },
            request_id,
        };
        (self.status, Json(body)).into_response()
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    database: &'static str,
    uptime_seconds: u64,
}

type PersistentTaskManager = TaskManager<SqliteTaskStore>;

#[derive(Clone, Default)]
struct RootWriteCoordinator {
    locks: Arc<StdMutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl RootWriteCoordinator {
    async fn acquire(
        &self,
        root_path: &Path,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, std::io::Error> {
        let canonical = std::fs::canonicalize(root_path)?;
        let key = platform_path_key(&canonical);
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .map_err(|_| std::io::Error::other("root write lock poisoned"))?;
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(key, Arc::downgrade(&lock));
                lock
            }
        };
        Ok(lock.lock_owned().await)
    }
}

#[derive(Clone)]
pub struct AppState {
    settings_path: PathBuf,
    settings: Arc<RwLock<StoredSettings>>,
    pub(crate) secrets: Arc<SecretManager>,
    database: Arc<Database>,
    tasks: Arc<PersistentTaskManager>,
    danbooru: Arc<RwLock<DanbooruClient>>,
    danbooru_posts: Arc<StdMutex<HashMap<u64, CachedDanbooruPost>>>,
    active_workers: Arc<Mutex<HashSet<String>>>,
    root_writes: RootWriteCoordinator,
    root_registry: Arc<Mutex<()>>,
    thumbnail_cache_dir: PathBuf,
    worker_slots: Arc<Semaphore>,
    started_at: Instant,
}

#[derive(Clone)]
struct CachedDanbooruPost {
    post: Post,
    cached_at: Instant,
}

const DANBOORU_POST_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const DANBOORU_POST_CACHE_LIMIT: usize = 500;

impl AppState {
    fn open_internal(
        paths: AppPaths,
        secrets: SecretManager,
        migrate_legacy: bool,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.data_dir)
            .map_err(|error| format!("无法创建数据目录 {}: {error}", paths.data_dir.display()))?;
        let secrets = Arc::new(secrets);
        if migrate_legacy {
            migrate_legacy_database(&paths)?;
            migrate_legacy_secrets(&paths, &secrets);
        }
        let settings_path = paths.data_dir.join("app_settings.json");
        if migrate_legacy {
            let project_root = legacy_project_root(&paths);
            migrate_legacy_settings(
                &settings_path,
                &project_root.join("config.json"),
                &project_root.join("vllm_config.json"),
            )?;
        }
        let thumbnail_cache_dir = paths.data_dir.join("cache").join("thumbnails");
        let mut settings = load_settings(&settings_path)?;
        let vllm_base_url_override = std::env::var("VLLM_BASE_URL").ok();
        apply_vllm_base_url_override(&mut settings, vllm_base_url_override.as_deref())
            .map_err(|error| format!("VLLM_BASE_URL 无效: {}", error.message))?;
        let danbooru = build_danbooru_client(&settings, &secrets)?;
        let database = Arc::new(
            Database::open(&paths.data_dir.join("danbooru_tool.db"))
                .map_err(|error| format!("无法打开 SQLite: {error}"))?,
        );
        database
            .recover_interrupted_tasks()
            .map_err(|error| format!("无法恢复中断任务: {error}"))?;
        let tasks = Arc::new(TaskManager::new(SqliteTaskStore::new(database.clone())));
        tasks
            .recover_interrupted()
            .map_err(|error| format!("无法确认待停止任务: {error}"))?;
        Ok(Self {
            settings_path,
            settings: Arc::new(RwLock::new(settings)),
            secrets,
            database,
            tasks,
            danbooru: Arc::new(RwLock::new(danbooru)),
            danbooru_posts: Arc::new(StdMutex::new(HashMap::new())),
            active_workers: Arc::new(Mutex::new(HashSet::new())),
            root_writes: RootWriteCoordinator::default(),
            root_registry: Arc::new(Mutex::new(())),
            thumbnail_cache_dir,
            worker_slots: Arc::new(Semaphore::new(4)),
            started_at: Instant::now(),
        })
    }

    fn cache_danbooru_posts(&self, posts: &[Post]) {
        let now = Instant::now();
        let mut cache = self
            .danbooru_posts
            .lock()
            .expect("Danbooru post cache lock poisoned");
        cache.retain(|_, entry| {
            now.saturating_duration_since(entry.cached_at) <= DANBOORU_POST_CACHE_TTL
        });
        for post in posts {
            cache.insert(
                post.id,
                CachedDanbooruPost {
                    post: post.clone(),
                    cached_at: now,
                },
            );
        }
        while cache.len() > DANBOORU_POST_CACHE_LIMIT {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.cached_at)
                .map(|(id, _)| *id)
            else {
                break;
            };
            cache.remove(&oldest);
        }
    }

    fn cached_danbooru_post(&self, id: u64) -> Option<Post> {
        let now = Instant::now();
        let mut cache = self
            .danbooru_posts
            .lock()
            .expect("Danbooru post cache lock poisoned");
        cache.retain(|_, entry| {
            now.saturating_duration_since(entry.cached_at) <= DANBOORU_POST_CACHE_TTL
        });
        let entry = cache.get_mut(&id)?;
        entry.cached_at = now;
        Some(entry.post.clone())
    }

    fn clear_danbooru_post_cache(&self) {
        self.danbooru_posts
            .lock()
            .expect("Danbooru post cache lock poisoned")
            .clear();
    }
}

fn isolated_mode_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value == Some(std::ffi::OsStr::new("1"))
}

fn build_danbooru_client(
    settings: &StoredSettings,
    secrets: &SecretManager,
) -> Result<DanbooruClient, String> {
    let api_key = secrets
        .get_for_internal_use(SecretKind::Danbooru)
        .unwrap_or(None)
        .unwrap_or_default();
    DanbooruClient::new(DanbooruClientConfig {
        username: settings.danbooru_username.clone(),
        api_key,
        proxy_url: settings.proxy_url.clone(),
        ..DanbooruClientConfig::default()
    })
    .map_err(|error| error.to_string())
}

fn migrate_legacy_database(paths: &AppPaths) -> Result<(), String> {
    let target = paths.data_dir.join("danbooru_tool.db");
    let legacy = Path::new(env!("CARGO_MANIFEST_DIR")).join("danbooru_tool.db");
    migrate_legacy_database_from(&legacy, &target)
}

fn migrate_legacy_database_from(legacy: &Path, target: &Path) -> Result<(), String> {
    if !target.exists() && legacy.exists() && target != legacy {
        let source = rusqlite::Connection::open_with_flags(
            legacy,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|error| format!("无法只读打开旧数据库 {}: {error}", legacy.display()))?;
        let mut destination = rusqlite::Connection::open(target)
            .map_err(|error| format!("无法创建迁移数据库 {}: {error}", target.display()))?;
        let backup_result =
            rusqlite::backup::Backup::new(&source, &mut destination).and_then(|backup| {
                backup.run_to_completion(100, std::time::Duration::from_millis(10), None)
            });
        if let Err(error) = backup_result {
            drop(destination);
            let _ = std::fs::remove_file(target);
            return Err(format!(
                "无法将旧数据库 {} 一致迁移到 {}: {error}",
                legacy.display(),
                target.display()
            ));
        }
    }
    Ok(())
}

fn migrate_legacy_secrets(paths: &AppPaths, secrets: &SecretManager) {
    let project_root = legacy_project_root(paths);
    for (path, field_path, kind) in [
        (
            project_root.join("config.json"),
            &["download", "api_key"][..],
            SecretKind::Danbooru,
        ),
        (
            project_root.join("vllm_config.json"),
            &["api_key"][..],
            SecretKind::Vllm,
        ),
    ] {
        if let Err(error) = migrate_legacy_secret(&path, field_path, kind, secrets) {
            tracing::warn!(file = %path.display(), %error, "旧密钥尚未迁移；文件保持原样");
        }
    }
}

fn legacy_project_root(paths: &AppPaths) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(&paths.data_dir)
        .to_path_buf()
}

pub fn router() -> Router {
    let paths = AppPaths::from_env().expect("运行路径配置无效");
    let isolated = isolated_mode_enabled(std::env::var_os("APP_ISOLATED_MODE").as_deref());
    let secrets = if isolated {
        tracing::warn!("隔离运行模式已启用：跳过旧数据迁移并仅使用会话密钥");
        SecretManager::session_only()
    } else {
        SecretManager::with_vault(Arc::new(SystemCredentialVault))
    };
    let state = AppState::open_internal(paths, secrets, !isolated).expect("应用状态初始化失败");
    router_with_state(state)
}

pub fn router_with_state(state: AppState) -> Router {
    match state.tasks.snapshot() {
        Ok(tasks) => {
            for task in tasks
                .into_iter()
                .filter(|task| task.status == TaskStatus::Queued)
            {
                let worker_state = state.clone();
                tokio::spawn(async move {
                    spawn_task_worker(worker_state, task.id).await;
                });
            }
        }
        Err(error) => {
            tracing::error!(%error, "无法读取启动任务队列");
        }
    }
    Router::new()
        .route("/api/health", get(health))
        .route("/api/vllm/health", get(vllm_health))
        .route("/api/config", get(get_config).put(update_config))
        .route(
            "/api/secrets/{kind}",
            axum::routing::put(put_secret).delete(delete_secret),
        )
        .route("/api/library/roots", get(list_roots).post(create_root))
        .route(
            "/api/library/roots/{id}",
            axum::routing::put(update_root).delete(delete_root),
        )
        .route(
            "/api/library/roots/{id}/directories",
            get(list_root_directories).post(create_root_directory),
        )
        .route("/api/library/items", get(list_library_items))
        .route("/api/library/items/{id}", get(library_item))
        .route("/api/library/media/{id}/{variant}", get(library_media))
        .route(
            "/api/library/quarantine",
            get(list_quarantine).delete(purge_quarantine),
        )
        .route(
            "/api/library/quarantine/{id}/restore",
            axum::routing::post(restore_quarantine),
        )
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", get(task_detail))
        .route("/api/downloads/history", get(download_history))
        .route("/api/tasks/events", get(task_events))
        .route("/api/tasks/{id}/{action}", axum::routing::post(task_action))
        .route("/api/danbooru/posts", get(danbooru_posts))
        .route("/api/danbooru/posts/{id}", get(danbooru_post))
        .route(
            "/api/danbooru/posts/{id}/media/{variant}",
            get(danbooru_media),
        )
        .route("/api/danbooru/autocomplete", get(danbooru_autocomplete))
        .route("/api/danbooru/count", get(danbooru_count))
        .with_state(state)
}

async fn health(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<HealthResponse>>, ApiError> {
    state.database.health_check().map_err(|_| ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "database_unavailable".to_string(),
        message: "SQLite 健康检查失败".to_string(),
        retryable: true,
        fields: None,
    })?;
    Ok(Json(ApiSuccess {
        data: HealthResponse {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
            database: "ok",
            uptime_seconds: state.started_at.elapsed().as_secs(),
        },
        meta: None,
    }))
}

async fn vllm_health(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<VllmHealth>>, ApiError> {
    let settings = state.settings.read().await.clone();
    let api_key = state
        .secrets
        .get_for_internal_use(SecretKind::Vllm)
        .map_err(|_| ApiError::internal("无法读取 vLLM 凭据"))?;
    let service = VllmService::new(
        VllmServiceConfig {
            endpoint: settings.vllm_base_url,
            allowed_hosts: settings.vllm_allowed_hosts,
            model: settings.vllm_model,
            system_prompt: settings.vllm_system_prompt,
            tag_mode: settings.vllm_tag_mode,
            concurrency: settings.vllm_concurrency,
            timeout_seconds: 5,
            ..VllmServiceConfig::default()
        },
        api_key,
    )
    .map_err(|error| ApiError::bad_request("invalid_vllm_config", error.message))?;
    Ok(Json(ApiSuccess {
        data: service.health().await,
        meta: None,
    }))
}

async fn get_config(State(state): State<AppState>) -> Json<ApiSuccess<PublicConfig>> {
    let settings = state.settings.read().await;
    let danbooru_configured = state
        .secrets
        .status(SecretKind::Danbooru)
        .map(|status| status.configured)
        .unwrap_or(false);
    let vllm_configured = state
        .secrets
        .status(SecretKind::Vllm)
        .map(|status| status.configured)
        .unwrap_or(false);
    Json(ApiSuccess {
        data: PublicConfig::from_settings(&settings, danbooru_configured, vllm_configured),
        meta: None,
    })
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateConfigRequest {
    danbooru_username: String,
    vllm_base_url: String,
    vllm_allowed_hosts: Vec<String>,
    #[serde(default)]
    vllm_model: Option<String>,
    #[serde(default)]
    vllm_system_prompt: Option<String>,
    #[serde(default)]
    vllm_tag_mode: Option<crate::services::vllm::TagWriteMode>,
    #[serde(default)]
    vllm_language: Option<crate::services::vllm::VllmLanguage>,
    #[serde(default)]
    vllm_max_tags: Option<usize>,
    #[serde(default)]
    vllm_max_length: Option<usize>,
    #[serde(default)]
    vllm_verify_danbooru: Option<bool>,
    #[serde(default)]
    vllm_reference_existing: Option<bool>,
    #[serde(default)]
    vllm_concurrency: Option<usize>,
    proxy_url: Option<String>,
    download_concurrency: usize,
    filename_template: String,
    ugoira_policy: crate::config::UgoiraPolicy,
    blur_sensitive_media: bool,
}

async fn update_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<ApiSuccess<PublicConfig>>, ApiError> {
    let mut updated = state.settings.read().await.clone();
    updated.danbooru_username = request.danbooru_username;
    updated.vllm_base_url = request.vllm_base_url;
    updated.vllm_allowed_hosts = request.vllm_allowed_hosts;
    if let Some(model) = request.vllm_model {
        updated.vllm_model = model;
    }
    if let Some(system_prompt) = request.vllm_system_prompt {
        updated.vllm_system_prompt = system_prompt;
    }
    if let Some(tag_mode) = request.vllm_tag_mode {
        updated.vllm_tag_mode = tag_mode;
    }
    if let Some(language) = request.vllm_language {
        updated.vllm_language = language;
    }
    if let Some(max_tags) = request.vllm_max_tags {
        updated.vllm_max_tags = max_tags;
    }
    if let Some(max_length) = request.vllm_max_length {
        updated.vllm_max_length = max_length;
    }
    if let Some(verify) = request.vllm_verify_danbooru {
        updated.vllm_verify_danbooru = verify;
    }
    if let Some(reference) = request.vllm_reference_existing {
        updated.vllm_reference_existing = reference;
    }
    if let Some(concurrency) = request.vllm_concurrency {
        updated.vllm_concurrency = concurrency;
    }
    updated.proxy_url = request.proxy_url;
    updated.download_concurrency = request.download_concurrency;
    updated.filename_template = request.filename_template;
    updated.ugoira_policy = request.ugoira_policy;
    updated.blur_sensitive_media = request.blur_sensitive_media;
    updated
        .validate()
        .map_err(|error| ApiError::bad_request("invalid_config", error.message))?;
    let client = build_danbooru_client(&updated, &state.secrets)
        .map_err(|error| ApiError::bad_request("invalid_network_config", error))?;
    save_settings(&state.settings_path, &updated).map_err(ApiError::internal)?;
    *state.settings.write().await = updated;
    *state.danbooru.write().await = client;
    state.clear_danbooru_post_cache();
    Ok(get_config(State(state)).await)
}

#[derive(Debug, serde::Deserialize)]
struct SecretRequest {
    secret: String,
}

#[derive(Debug, Serialize)]
struct SecretResponse {
    configured: bool,
    storage: &'static str,
}

async fn put_secret(
    State(state): State<AppState>,
    AxumPath(kind): AxumPath<String>,
    Json(request): Json<SecretRequest>,
) -> Result<Json<ApiSuccess<SecretResponse>>, ApiError> {
    let kind = parse_secret_kind(&kind)?;
    if request.secret.is_empty() || request.secret.len() > 4_096 {
        return Err(ApiError::bad_request(
            "invalid_secret",
            "密钥长度必须在 1..=4096 字节之间",
        ));
    }
    let storage = if state.secrets.set_persistent(kind, &request.secret).is_ok() {
        "system"
    } else {
        state
            .secrets
            .set_session(kind, &request.secret)
            .map_err(|_| ApiError::internal("无法保存会话密钥"))?;
        "session"
    };
    if kind == SecretKind::Danbooru {
        refresh_danbooru_client(&state).await?;
    }
    Ok(Json(ApiSuccess {
        data: SecretResponse {
            configured: true,
            storage,
        },
        meta: None,
    }))
}

async fn delete_secret(
    State(state): State<AppState>,
    AxumPath(kind): AxumPath<String>,
) -> Result<Json<ApiSuccess<SecretResponse>>, ApiError> {
    let kind = parse_secret_kind(&kind)?;
    state
        .secrets
        .delete(kind)
        .map_err(|_| ApiError::internal("无法删除系统凭据"))?;
    if kind == SecretKind::Danbooru {
        refresh_danbooru_client(&state).await?;
    }
    Ok(Json(ApiSuccess {
        data: SecretResponse {
            configured: false,
            storage: "none",
        },
        meta: None,
    }))
}

fn parse_secret_kind(kind: &str) -> Result<SecretKind, ApiError> {
    match kind {
        "danbooru" => Ok(SecretKind::Danbooru),
        "vllm" => Ok(SecretKind::Vllm),
        _ => Err(ApiError::not_found("secret_kind_not_found", "未知密钥类型")),
    }
}

async fn refresh_danbooru_client(state: &AppState) -> Result<(), ApiError> {
    let settings = state.settings.read().await.clone();
    let client = build_danbooru_client(&settings, &state.secrets)
        .map_err(|error| ApiError::bad_request("invalid_network_config", error))?;
    *state.danbooru.write().await = client;
    state.clear_danbooru_post_cache();
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct RootRequest {
    name: String,
    windows_path: Option<String>,
    linux_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct RootResponse {
    id: String,
    name: String,
    windows_path: Option<String>,
    linux_path: Option<String>,
    indexed: bool,
    media_count: usize,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct RootRemovalResponse {
    id: String,
}

#[derive(Debug, Serialize)]
struct RootDirectoryListResponse {
    directories: Vec<String>,
    truncated: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRootDirectoryRequest {
    relative_path: String,
}

#[derive(Debug, Serialize)]
struct RootDirectoryResponse {
    relative_path: String,
}

async fn list_roots(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<Vec<RootResponse>>>, ApiError> {
    let roots = state
        .database
        .list_roots()
        .map_err(|error| ApiError::internal(format!("无法读取媒体根: {error}")))?;
    let roots = roots
        .into_iter()
        .map(|root| root_response(&state.database, root))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiSuccess {
        data: roots,
        meta: None,
    }))
}

async fn list_root_directories(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<RootDirectoryListResponse>>, ApiError> {
    const DIRECTORY_LIMIT: usize = 500;
    const MAX_DEPTH: usize = 16;

    let root = state
        .database
        .get_root(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let verified = VerifiedMediaRoot::open(current_platform_path(&root)?).map_err(|error| {
        ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
    })?;
    let mut directories = Vec::new();
    let walker = WalkDir::new(verified.path())
        .min_depth(1)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !name.eq_ignore_ascii_case(".danbooru-quarantine"))
        });
    for entry in walker {
        let entry = entry.map_err(|error| {
            ApiError::bad_request(
                "directory_unavailable",
                format!("无法读取媒体库文件夹: {error}"),
            )
        })?;
        if !entry.file_type().is_dir() {
            continue;
        }
        if directories.len() == DIRECTORY_LIMIT {
            return Ok(Json(ApiSuccess {
                data: RootDirectoryListResponse {
                    directories,
                    truncated: true,
                },
                meta: None,
            }));
        }
        let relative = entry
            .path()
            .strip_prefix(verified.path())
            .map_err(|_| ApiError::bad_request("directory_outside_root", "文件夹越过媒体根目录"))?;
        directories.push(relative.to_string_lossy().replace('\\', "/"));
    }
    directories.sort_by_key(|path| path.to_lowercase());
    Ok(Json(ApiSuccess {
        data: RootDirectoryListResponse {
            directories,
            truncated: false,
        },
        meta: None,
    }))
}

async fn create_root_directory(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<CreateRootDirectoryRequest>,
) -> Result<(StatusCode, Json<ApiSuccess<RootDirectoryResponse>>), ApiError> {
    let relative_path = normalize_task_relative_directory(&request.relative_path)?;
    if relative_path.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_relative_directory",
            "请输入库内子文件夹名称",
        ));
    }
    let root = state
        .database
        .get_root(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let verified = VerifiedMediaRoot::open(current_platform_path(&root)?).map_err(|error| {
        ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
    })?;
    let _write_guard = state
        .root_writes
        .acquire(verified.path())
        .await
        .map_err(|error| ApiError::internal(format!("无法锁定媒体根: {error}")))?;
    let destination = verified
        .resolve(Path::new(&relative_path))
        .map_err(|error| {
            ApiError::bad_request(
                "invalid_relative_directory",
                format!("无法使用该子文件夹: {error}"),
            )
        })?;
    std::fs::create_dir_all(&destination).map_err(|error| {
        ApiError::bad_request(
            "directory_create_failed",
            format!("无法创建子文件夹: {error}"),
        )
    })?;
    let canonical = std::fs::canonicalize(&destination).map_err(|error| {
        ApiError::bad_request(
            "directory_create_failed",
            format!("无法确认新建子文件夹: {error}"),
        )
    })?;
    if !canonical.starts_with(verified.path()) || !canonical.is_dir() {
        return Err(ApiError::bad_request(
            "directory_outside_root",
            "新建子文件夹越过媒体根目录",
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(ApiSuccess {
            data: RootDirectoryResponse { relative_path },
            meta: None,
        }),
    ))
}

async fn create_root(
    State(state): State<AppState>,
    Json(request): Json<RootRequest>,
) -> Result<(StatusCode, Json<ApiSuccess<RootResponse>>), ApiError> {
    let request = validate_root_request(request)?;
    let canonical_root = validate_current_platform_root(&request)?;
    validate_root_does_not_overlap(&state.database, &canonical_root, None)?;
    let _write_guard = state
        .root_writes
        .acquire(&canonical_root)
        .await
        .map_err(|error| ApiError::internal(format!("无法锁定媒体根: {error}")))?;
    let _registry_guard = state.root_registry.lock().await;
    validate_root_does_not_overlap(&state.database, &canonical_root, None)?;
    let root = state
        .database
        .create_root(
            &uuid::Uuid::new_v4().to_string(),
            &request.name,
            request.windows_path.as_deref(),
            request.linux_path.as_deref(),
        )
        .map_err(|error| ApiError::internal(format!("无法注册媒体根: {error}")))?;
    let response = root_response(&state.database, root)?;
    Ok((
        StatusCode::CREATED,
        Json(ApiSuccess {
            data: response,
            meta: None,
        }),
    ))
}

async fn update_root(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<RootRequest>,
) -> Result<Json<ApiSuccess<RootResponse>>, ApiError> {
    let existing = state
        .database
        .get_root(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let existing_canonical =
        std::fs::canonicalize(current_platform_path(&existing)?).map_err(|error| {
            ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
        })?;
    let request = validate_root_request(request)?;
    let canonical_root = validate_current_platform_root(&request)?;
    validate_root_does_not_overlap(&state.database, &canonical_root, Some(&id))?;
    let current_path_changed = !platform_paths_equal(&existing_canonical, &canonical_root);
    if current_path_changed && root_has_bound_state(&state, &id)? {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "root_path_in_use".to_string(),
            message: "媒体根已有媒体、隔离项或未结束任务，不能改指向其他目录".to_string(),
            retryable: false,
            fields: None,
        });
    }
    let _existing_guard = state
        .root_writes
        .acquire(&existing_canonical)
        .await
        .map_err(|error| ApiError::internal(format!("无法锁定媒体根: {error}")))?;
    let _replacement_guard = if current_path_changed {
        Some(
            state
                .root_writes
                .acquire(&canonical_root)
                .await
                .map_err(|error| ApiError::internal(format!("无法锁定新媒体根: {error}")))?,
        )
    } else {
        None
    };
    let _registry_guard = state.root_registry.lock().await;
    let latest = state
        .database
        .get_root(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let latest_canonical =
        std::fs::canonicalize(current_platform_path(&latest)?).map_err(|error| {
            ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
        })?;
    if !platform_paths_equal(&latest_canonical, &existing_canonical) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "root_changed_retry".to_string(),
            message: "媒体根在本次编辑期间已变化，请刷新后重试".to_string(),
            retryable: true,
            fields: None,
        });
    }
    validate_root_does_not_overlap(&state.database, &canonical_root, Some(&id))?;
    if current_path_changed && root_has_bound_state(&state, &id)? {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "root_path_in_use".to_string(),
            message: "媒体根已有媒体、隔离项或未结束任务，不能改指向其他目录".to_string(),
            retryable: false,
            fields: None,
        });
    }
    let indexing_status = if current_path_changed {
        "not_indexed"
    } else {
        &latest.indexing_status
    };
    let root = state
        .database
        .update_root(
            &id,
            &request.name,
            request.windows_path.as_deref(),
            request.linux_path.as_deref(),
            indexing_status,
        )
        .map_err(|error| ApiError::internal(format!("无法更新媒体根: {error}")))?;
    Ok(Json(ApiSuccess {
        data: root_response(&state.database, root)?,
        meta: None,
    }))
}

async fn delete_root(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<RootRemovalResponse>>, ApiError> {
    let _registry_guard = state.root_registry.lock().await;
    state
        .database
        .get_root(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    if root_has_active_tasks(&state, &id)? {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "root_task_in_progress".to_string(),
            message: "媒体根仍有未结束任务，完成或取消后再移除".to_string(),
            retryable: true,
            fields: None,
        });
    }
    let removed = state
        .database
        .remove_root_catalog(&id)
        .map_err(|error| ApiError::internal(format!("无法移除媒体根: {error}")))?;
    if !removed {
        return Err(ApiError::not_found("root_not_found", "媒体根不存在"));
    }
    Ok(Json(ApiSuccess {
        data: RootRemovalResponse { id },
        meta: None,
    }))
}

fn root_has_bound_state(state: &AppState, root_id: &str) -> Result<bool, ApiError> {
    if state
        .database
        .count_media_files(root_id)
        .map_err(|error| ApiError::internal(format!("无法检查媒体根: {error}")))?
        > 0
    {
        return Ok(true);
    }
    if !state
        .database
        .list_quarantine(root_id, false)
        .map_err(|error| ApiError::internal(format!("无法检查隔离区: {error}")))?
        .is_empty()
    {
        return Ok(true);
    }
    root_has_active_tasks(state, root_id)
}

fn root_has_active_tasks(state: &AppState, root_id: &str) -> Result<bool, ApiError> {
    Ok(state
        .tasks
        .snapshot()
        .map_err(map_task_manager_error)?
        .into_iter()
        .any(|task| {
            task.payload.get("root_id").and_then(Value::as_str) == Some(root_id)
                && !matches!(
                    task.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
        }))
}

fn validate_root_request(mut request: RootRequest) -> Result<RootRequest, ApiError> {
    request.name = request.name.trim().to_string();
    if request.name.is_empty() || request.name.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_root_name",
            "媒体根名称长度必须在 1..=100 之间",
        ));
    }
    request.windows_path = request
        .windows_path
        .filter(|path| !path.trim().is_empty())
        .map(|path| {
            normalize_windows_path(path.trim()).map_err(|_| {
                ApiError::bad_request("invalid_windows_path", "Windows 路径无效或不安全")
            })
        })
        .transpose()?;
    request.linux_path = request
        .linux_path
        .filter(|path| !path.trim().is_empty())
        .map(|path| path.trim().to_string());
    if request.windows_path.is_none() && request.linux_path.is_none() {
        return Err(ApiError::bad_request(
            "missing_root_path",
            "至少需要一个平台路径",
        ));
    }
    if let Some(path) = request.linux_path.as_deref() {
        if path.contains('\0') || !Path::new(path).is_absolute() {
            return Err(ApiError::bad_request(
                "invalid_linux_path",
                "Linux 路径必须是安全的绝对路径",
            ));
        }
    }
    Ok(request)
}

fn current_platform_root_path(request: &RootRequest) -> Option<&str> {
    #[cfg(windows)]
    return request.windows_path.as_deref();
    #[cfg(not(windows))]
    return request.linux_path.as_deref();
}

fn validate_current_platform_root(request: &RootRequest) -> Result<PathBuf, ApiError> {
    let path = current_platform_root_path(request).ok_or_else(|| {
        ApiError::bad_request("missing_platform_path", "缺少当前平台的媒体根路径")
    })?;
    let link_metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ApiError::bad_request("root_unavailable", "当前平台的媒体根路径不可访问"))?;
    if metadata_is_link_or_reparse_point(&link_metadata) {
        return Err(ApiError::bad_request(
            "unsafe_root_link",
            "媒体根不能是符号链接或重解析点",
        ));
    }
    if !link_metadata.is_dir() {
        return Err(ApiError::bad_request(
            "root_not_directory",
            "媒体根必须是目录",
        ));
    }
    std::fs::canonicalize(path)
        .map_err(|_| ApiError::bad_request("root_unavailable", "媒体根无法规范化"))
}

#[cfg(windows)]
fn metadata_is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_root_does_not_overlap(
    database: &Database,
    canonical_root: &Path,
    ignored_root_id: Option<&str>,
) -> Result<(), ApiError> {
    let roots = database
        .list_roots()
        .map_err(|error| ApiError::internal(format!("无法读取媒体根: {error}")))?;
    for existing in roots {
        if ignored_root_id == Some(existing.id.as_str()) {
            continue;
        }
        #[cfg(windows)]
        let path = existing.windows_path.as_deref();
        #[cfg(not(windows))]
        let path = existing.linux_path.as_deref();
        let Some(path) = path else {
            continue;
        };
        let Ok(existing_canonical) = std::fs::canonicalize(path) else {
            continue;
        };
        if platform_paths_overlap(canonical_root, &existing_canonical) {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                code: "overlapping_media_root".to_string(),
                message: "媒体根不能与已注册目录相同、互相包含或嵌套".to_string(),
                retryable: false,
                fields: None,
            });
        }
    }
    Ok(())
}

#[cfg(windows)]
fn platform_paths_overlap(left: &Path, right: &Path) -> bool {
    let left = platform_path_key(left);
    let right = platform_path_key(right);
    left == right
        || left.starts_with(&format!("{right}\\"))
        || right.starts_with(&format!("{left}\\"))
}

#[cfg(not(windows))]
fn platform_paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(windows)]
fn platform_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[cfg(not(windows))]
fn platform_path_key(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('/').to_string()
}

fn platform_paths_equal(left: &Path, right: &Path) -> bool {
    platform_path_key(left) == platform_path_key(right)
}

fn root_response(database: &Database, root: RootRecord) -> Result<RootResponse, ApiError> {
    let media_count = database
        .count_media_files(&root.id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .max(0) as usize;
    Ok(RootResponse {
        id: root.id,
        name: root.name,
        windows_path: root.windows_path,
        linux_path: root.linux_path,
        indexed: root.indexing_status == "indexed",
        media_count,
        created_at: root.created_at,
        updated_at: root.updated_at,
    })
}

#[derive(Debug, serde::Deserialize)]
struct LibraryItemsQuery {
    root_id: String,
    #[serde(default, rename = "q")]
    query: String,
    cursor: Option<String>,
    #[serde(default = "default_library_limit")]
    limit: usize,
}

fn default_library_limit() -> usize {
    60
}

#[derive(Debug, Serialize)]
struct LibraryPageResponse {
    items: Vec<LocalMediaResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    total: i64,
}

#[derive(Debug, Serialize)]
struct LocalMediaResponse {
    id: String,
    root_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_id: Option<i64>,
    filename: String,
    relative_path: String,
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<String>,
    tags: Vec<String>,
    created_at: String,
}

async fn list_library_items(
    State(state): State<AppState>,
    Query(query): Query<LibraryItemsQuery>,
) -> Result<Json<ApiSuccess<LibraryPageResponse>>, ApiError> {
    if !(1..=200).contains(&query.limit) {
        return Err(ApiError::bad_request(
            "invalid_page_limit",
            "图库分页数量必须在 1..=200",
        ));
    }
    if query.query.len() > 4_096 {
        return Err(ApiError::bad_request(
            "query_too_long",
            "查询最长为 4096 字节",
        ));
    }
    state
        .database
        .get_root(&query.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let page = state
        .database
        .list_library_media(
            &query.root_id,
            query.cursor.as_deref(),
            query.limit,
            &query.query,
        )
        .map_err(|error| ApiError::internal(format!("无法读取图库: {error}")))?;
    let items = page
        .items
        .into_iter()
        .map(|media| local_media_response(&state.database, media))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiSuccess {
        data: LibraryPageResponse {
            items,
            next_cursor: page.next_cursor,
            total: page.total,
        },
        meta: None,
    }))
}

fn local_media_response(
    database: &Database,
    media: MediaFileRecord,
) -> Result<LocalMediaResponse, ApiError> {
    let filename = Path::new(&media.relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&media.relative_path)
        .to_string();
    let metadata = media
        .post_id
        .map(|post_id| database.get_post_library_metadata(post_id))
        .transpose()
        .map_err(|error| ApiError::internal(format!("无法读取帖子标签: {error}")))?
        .flatten();
    let rating = metadata.as_ref().map(|metadata| metadata.rating.clone());
    let tags = metadata
        .map(|metadata| metadata.tags.into_iter().map(|tag| tag.name).collect())
        .unwrap_or_default();
    Ok(LocalMediaResponse {
        id: media.id,
        root_id: media.root_id,
        post_id: media.post_id,
        filename,
        relative_path: media.relative_path,
        mime_type: media.mime_type,
        width: media.width,
        height: media.height,
        duration: media.duration,
        size_bytes: media.byte_size,
        rating,
        tags,
        created_at: media.created_at,
    })
}

async fn library_item(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<LocalMediaResponse>>, ApiError> {
    let media = state
        .database
        .get_media_file(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .filter(|media| media.status == "active")
        .ok_or_else(|| ApiError::not_found("media_not_found", "媒体不存在"))?;
    Ok(Json(ApiSuccess {
        data: local_media_response(&state.database, media)?,
        meta: None,
    }))
}

async fn library_media(
    State(state): State<AppState>,
    AxumPath((id, variant)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !matches!(variant.as_str(), "file" | "thumbnail") {
        return Err(ApiError::not_found(
            "library_media_variant_not_found",
            "未知本地媒体 variant",
        ));
    }
    let media = state
        .database
        .get_media_file(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .filter(|media| media.status == "active")
        .ok_or_else(|| ApiError::not_found("media_not_found", "媒体不存在"))?;
    let root = state
        .database
        .get_root(&media.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let root_path = current_platform_path(&root)?;
    let safe_root = MediaRoot::new(root.id.clone(), root_path)
        .map_err(|_| ApiError::not_found("root_unavailable", "媒体根当前不可访问"))?;
    debug_assert_eq!(safe_root.id, media.root_id);
    let path = safe_root
        .resolve_existing_file(Path::new(&media.relative_path))
        .map_err(|_| ApiError::not_found("media_unavailable", "媒体文件不可访问"))?;
    let (path, cache_control) = if variant == "thumbnail" {
        if !media.mime_type.starts_with("image/") {
            return Ok(library_thumbnail_placeholder(&method));
        }
        let cache_dir = state.thumbnail_cache_dir.clone();
        let cache_key = format!(
            "{}\0{}\0{}\0{}",
            media.id, media.relative_path, media.byte_size, media.updated_at
        );
        let source = path.clone();
        let thumbnail = tokio::task::spawn_blocking(move || {
            generate_cached_thumbnail(&source, &cache_dir, &cache_key)
        })
        .await
        .map_err(|error| ApiError::internal(format!("缩略图任务异常: {error}")))?
        .map_err(|error| ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "thumbnail_unavailable".to_string(),
            message: error,
            retryable: false,
            fields: None,
        })?;
        (thumbnail, "private, max-age=86400")
    } else {
        (path, "private, max-age=300")
    };

    let mut request = Request::builder().method(method).uri("/");
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let request = request
        .body(Body::empty())
        .map_err(|error| ApiError::internal(format!("无法创建文件请求: {error}")))?;
    let mut response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(|never| match never {})?
        .map(Body::new);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_str(cache_control).expect("static cache-control value is valid"),
    );
    Ok(response)
}

fn generate_cached_thumbnail(
    source: &Path,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<PathBuf, String> {
    const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
    const MAX_PIXELS: u64 = 100_000_000;
    let metadata =
        std::fs::symlink_metadata(source).map_err(|error| format!("无法读取源图片: {error}"))?;
    if !metadata.is_file() || metadata_is_link_or_reparse_point(&metadata) {
        return Err("源媒体不是安全的普通文件".to_string());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err("源图片超过缩略图大小上限".to_string());
    }
    let (width, height) =
        image::image_dimensions(source).map_err(|error| format!("无法读取图片尺寸: {error}"))?;
    if width == 0 || height == 0 || u64::from(width).saturating_mul(u64::from(height)) > MAX_PIXELS
    {
        return Err("源图片像素数量超过缩略图安全上限".to_string());
    }
    std::fs::create_dir_all(cache_dir).map_err(|error| format!("无法创建缩略图缓存: {error}"))?;
    let mut digest = Sha256::new();
    digest.update(cache_key.as_bytes());
    let destination = cache_dir.join(format!("{}.jpg", hex::encode(digest.finalize())));
    if let Ok(cached) = std::fs::symlink_metadata(&destination) {
        if cached.is_file() && !metadata_is_link_or_reparse_point(&cached) && cached.len() > 0 {
            return Ok(destination);
        }
        return Err("缩略图缓存路径不安全".to_string());
    }

    let image = image::ImageReader::open(source)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|error| format!("无法打开源图片: {error}"))?
        .decode()
        .map_err(|error| format!("无法解码源图片: {error}"))?;
    let thumbnail = image.thumbnail(480, 480);
    let temporary = cache_dir.join(format!(".{}.part", uuid::Uuid::new_v4()));
    let write_result = (|| -> Result<(), String> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("无法创建缩略图临时文件: {error}"))?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 82)
            .encode_image(&thumbnail)
            .map_err(|error| format!("无法编码缩略图: {error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("无法同步缩略图: {error}"))?;
        drop(output);
        match std::fs::rename(&temporary, &destination) {
            Ok(()) => Ok(()),
            Err(_) if destination.is_file() => Ok(()),
            Err(error) => Err(format!("无法提交缩略图: {error}")),
        }
    })();
    let _ = std::fs::remove_file(&temporary);
    write_result?;
    Ok(destination)
}

fn library_thumbnail_placeholder(method: &Method) -> Response {
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="480" height="320" viewBox="0 0 480 320"><rect width="480" height="320" fill="#f1f3f5"/><circle cx="240" cy="160" r="42" fill="#ffffff" stroke="#d1d5db"/><path d="M228 136l34 24-34 24z" fill="#2563eb"/></svg>"##;
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(SVG)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .header(header::CONTENT_LENGTH, SVG.len())
        .body(body)
        .expect("static thumbnail placeholder response is valid")
}

fn current_platform_path(root: &RootRecord) -> Result<&str, ApiError> {
    #[cfg(windows)]
    let path = root.windows_path.as_deref();
    #[cfg(not(windows))]
    let path = root.linux_path.as_deref();
    path.ok_or_else(|| ApiError::bad_request("missing_platform_path", "媒体根未配置当前平台路径"))
}

#[derive(Debug, serde::Deserialize)]
struct RootIdQuery {
    root_id: String,
}

#[derive(Debug, Serialize)]
struct QuarantineResponse {
    id: String,
    root_id: String,
    original_relative_path: String,
    quarantine_relative_path: String,
    size_bytes: u64,
    reason: String,
    created_at: String,
}

async fn list_quarantine(
    State(state): State<AppState>,
    Query(query): Query<RootIdQuery>,
) -> Result<Json<ApiSuccess<Vec<QuarantineResponse>>>, ApiError> {
    let root = state
        .database
        .get_root(&query.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let safe_root = VerifiedMediaRoot::open(current_platform_path(&root)?)
        .map_err(|_| ApiError::not_found("root_unavailable", "媒体根当前不可访问"))?;
    let records = state
        .database
        .list_quarantine(&query.root_id, false)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let entries = records
        .into_iter()
        .map(|record| quarantine_response(&safe_root, record))
        .collect();
    Ok(Json(ApiSuccess {
        data: entries,
        meta: None,
    }))
}

async fn restore_quarantine(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<QuarantineResponse>>, ApiError> {
    let record = state
        .database
        .get_quarantine(&id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .filter(|record| record.restored_at.is_none())
        .ok_or_else(|| ApiError::not_found("quarantine_not_found", "隔离记录不存在"))?;
    let root = state
        .database
        .get_root(&record.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let safe_root = VerifiedMediaRoot::open(current_platform_path(&root)?)
        .map_err(|_| ApiError::not_found("root_unavailable", "媒体根当前不可访问"))?;
    let _root_write = state
        .root_writes
        .acquire(safe_root.path())
        .await
        .map_err(|error| ApiError::internal(format!("无法锁定媒体根: {error}")))?;
    let source = safe_root
        .resolve_existing_file(Path::new(&record.quarantine_relative_path))
        .map_err(|_| ApiError::not_found("quarantine_file_missing", "隔离文件不存在"))?;
    let destination = safe_root
        .resolve(Path::new(&record.original_relative_path))
        .map_err(|error| ApiError::bad_request("unsafe_restore_path", error.to_string()))?;
    if destination.exists() {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "restore_conflict".to_string(),
            message: "原位置已有文件，拒绝覆盖".to_string(),
            retryable: false,
            fields: None,
        });
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| ApiError::internal(format!("无法创建恢复目录: {error}")))?;
    }
    let destination = safe_root
        .resolve(Path::new(&record.original_relative_path))
        .map_err(|error| ApiError::bad_request("unsafe_restore_path", error.to_string()))?;
    std::fs::rename(&source, &destination)
        .map_err(|error| ApiError::internal(format!("无法恢复隔离文件: {error}")))?;
    let restored = match state.database.mark_quarantine_restored(&id) {
        Ok(restored) => restored,
        Err(database_error) => {
            std::fs::rename(&destination, &source).map_err(|rollback_error| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "restore_compensation_failed".to_string(),
                message: format!(
                    "恢复记录写入失败，且无法把文件移回隔离区: {database_error}; {rollback_error}"
                ),
                retryable: false,
                fields: None,
            })?;
            return Err(ApiError::internal(database_error.to_string()));
        }
    };
    Ok(Json(ApiSuccess {
        data: quarantine_response(&safe_root, restored),
        meta: None,
    }))
}

fn quarantine_response(
    root: &VerifiedMediaRoot,
    record: crate::database::QuarantineRecord,
) -> QuarantineResponse {
    let size_bytes = root
        .resolve(Path::new(&record.quarantine_relative_path))
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    QuarantineResponse {
        id: record.id,
        root_id: record.root_id,
        original_relative_path: record.original_relative_path,
        quarantine_relative_path: record.quarantine_relative_path,
        size_bytes,
        reason: record.reason,
        created_at: record.quarantined_at,
    }
}

#[derive(Debug, Serialize)]
struct PurgeResponse {
    purged: usize,
}

async fn purge_quarantine(
    State(state): State<AppState>,
    Query(query): Query<RootIdQuery>,
) -> Result<Json<ApiSuccess<PurgeResponse>>, ApiError> {
    let root = state
        .database
        .get_root(&query.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let safe_root = VerifiedMediaRoot::open(current_platform_path(&root)?)
        .map_err(|_| ApiError::not_found("root_unavailable", "媒体根当前不可访问"))?;
    let _root_write = state
        .root_writes
        .acquire(safe_root.path())
        .await
        .map_err(|error| ApiError::internal(format!("无法锁定媒体根: {error}")))?;
    let records = state
        .database
        .list_quarantine(&query.root_id, false)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let mut checked = Vec::with_capacity(records.len());
    for record in records {
        let relative = Path::new(&record.quarantine_relative_path);
        if !relative.components().next().is_some_and(|component| {
            matches!(component, std::path::Component::Normal(name) if is_quarantine_dir_name(name))
        }) {
            return Err(ApiError::bad_request(
                "unsafe_quarantine_path",
                "隔离记录不在隐藏隔离区内",
            ));
        }
        let path = safe_root
            .resolve_existing_file(relative)
            .map_err(|_| ApiError::not_found("quarantine_file_missing", "隔离文件不存在"))?;
        checked.push((record, path));
    }

    let mut purged = 0;
    for (record, path) in checked {
        if purge_registered_quarantine_file_with(&state.database, &record, &path, |path| {
            std::fs::remove_file(path)
        })? {
            purged += 1;
        }
    }
    Ok(Json(ApiSuccess {
        data: PurgeResponse { purged },
        meta: None,
    }))
}

fn purge_registered_quarantine_file_with<F>(
    database: &Database,
    record: &QuarantineRecord,
    path: &Path,
    remove_file: F,
) -> Result<bool, ApiError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let purged = database
        .purge_quarantine_record(&record.id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if !purged {
        return Ok(false);
    }
    if let Err(file_error) = remove_file(path) {
        database
            .restore_purged_quarantine_record(record)
            .map_err(|database_error| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "purge_compensation_failed".to_string(),
                message: format!(
                    "隔离文件清理失败，且无法恢复数据库记录: {file_error}; {database_error}"
                ),
                retryable: false,
                fields: None,
            })?;
        return Err(ApiError::internal(format!(
            "无法清理隔离文件: {file_error}"
        )));
    }
    Ok(true)
}

#[derive(Debug, Serialize)]
struct TaskSnapshotResponse {
    tasks: Vec<TaskSummaryResponse>,
    last_event_id: u64,
}

#[derive(Debug, Clone, Serialize)]
struct TaskSummaryResponse {
    id: String,
    kind: String,
    status: &'static str,
    revision: u64,
    title: String,
    progress: TaskProgressResponse,
    failures: Vec<TaskFailureResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<Value>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct TaskProgressResponse {
    completed: u64,
    total: u64,
    bytes_downloaded: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_bytes: Option<u64>,
    speed_bytes_per_sec: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    eta_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskFailureResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<String>,
    code: String,
    message: String,
    retryable: bool,
}

#[derive(Debug, serde::Deserialize)]
struct DownloadHistoryQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct DownloadHistoryPage {
    items: Vec<DownloadHistoryRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct DownloadHistoryRecord {
    id: String,
    task_id: String,
    status: &'static str,
    source_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_name: Option<String>,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_seconds: Option<u64>,
    total_items: u64,
    completed_items: u64,
    skipped_items: u64,
    failed_items: u64,
    bytes_processed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    can_repeat: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_request: Option<CreateTaskRequest>,
}

async fn download_history(
    State(state): State<AppState>,
    Query(query): Query<DownloadHistoryQuery>,
) -> Result<Json<ApiSuccess<DownloadHistoryPage>>, ApiError> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request(
            "invalid_history_limit",
            "下载记录分页大小必须在 1..=100 之间",
        ));
    }
    let (task_cursor, legacy_task_id, legacy_before) = match query.cursor.as_deref() {
        Some(cursor) if cursor.starts_with("legacy:") => {
            let before = cursor["legacy:".len()..]
                .parse::<i64>()
                .map_err(|_| ApiError::bad_request("invalid_history_cursor", "下载记录游标无效"))?;
            (None, None, Some(before))
        }
        Some(cursor) if cursor.starts_with("task:") => {
            let value = &cursor["task:".len()..];
            match decode_download_task_cursor(value) {
                Some(cursor) => (Some(cursor), None, None),
                None => (None, Some(value), None),
            }
        }
        Some(cursor) => (None, Some(cursor), None),
        None => (None, None, None),
    };
    let mut items = Vec::with_capacity(limit);
    let mut has_more_current = false;
    let mut next_task_cursor = None;
    if legacy_before.is_none() {
        let cursor = if let Some(cursor) = task_cursor {
            Some(cursor)
        } else if let Some(task_id) = legacy_task_id {
            Some(
                state
                    .database
                    .get_terminal_download_task_cursor(task_id)
                    .map_err(|error| ApiError::internal(format!("无法读取下载记录游标: {error}")))?
                    .ok_or_else(|| {
                        ApiError::bad_request("invalid_history_cursor", "下载记录游标无效")
                    })?,
            )
        } else {
            None
        };
        let mut records = state
            .database
            .list_terminal_download_tasks(cursor.as_ref(), limit + 1)
            .map_err(|error| ApiError::internal(format!("无法读取下载记录: {error}")))?;
        has_more_current = records.len() > limit;
        records.truncate(limit);
        if has_more_current {
            let last = records
                .last()
                .ok_or_else(|| ApiError::internal("下载记录分页缺少游标锚点"))?;
            next_task_cursor = Some(encode_download_task_cursor(&DownloadTaskHistoryCursor {
                updated_at: last.updated_at.clone(),
                id: last.id.clone(),
            })?);
        }
        for record in records {
            let task = task_from_record(record)
                .ok_or_else(|| ApiError::internal("SQLite 中的下载任务状态或内容无法解析"))?;
            items.push(download_history_record(&state.database, task)?);
        }
    }

    let mut last_legacy_id = None;
    let mut has_more_legacy = false;
    if !has_more_current {
        let remaining = limit - items.len();
        let legacy = state
            .database
            .list_legacy_download_history(legacy_before, remaining.saturating_add(1).max(1))
            .map_err(|error| ApiError::internal(format!("无法读取旧版下载记录: {error}")))?;
        has_more_legacy = if remaining == 0 {
            !legacy.is_empty()
        } else {
            legacy.len() > remaining
        };
        for record in legacy.into_iter().take(remaining) {
            last_legacy_id = Some(record.id);
            items.push(legacy_download_history_record(record));
        }
    }
    let next_cursor = if has_more_current {
        next_task_cursor
    } else if has_more_legacy {
        Some(
            last_legacy_id
                .map(|id| format!("legacy:{id}"))
                .unwrap_or_else(|| format!("legacy:{}", i64::MAX)),
        )
    } else {
        None
    };
    Ok(Json(ApiSuccess {
        data: DownloadHistoryPage { items, next_cursor },
        meta: None,
    }))
}

fn encode_download_task_cursor(cursor: &DownloadTaskHistoryCursor) -> Result<String, ApiError> {
    let payload = serde_json::to_vec(cursor)
        .map_err(|error| ApiError::internal(format!("无法生成下载记录游标: {error}")))?;
    Ok(format!("task:{}", URL_SAFE_NO_PAD.encode(payload)))
}

fn decode_download_task_cursor(value: &str) -> Option<DownloadTaskHistoryCursor> {
    let payload = URL_SAFE_NO_PAD.decode(value).ok()?;
    let cursor = serde_json::from_slice::<DownloadTaskHistoryCursor>(&payload).ok()?;
    let timestamp = cursor.updated_at.as_bytes();
    let timestamp_is_valid = timestamp.len() == 19
        && [4, 7].into_iter().all(|index| timestamp[index] == b'-')
        && timestamp[10] == b' '
        && [13, 16].into_iter().all(|index| timestamp[index] == b':')
        && timestamp
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit());
    (timestamp_is_valid && !cursor.id.is_empty() && cursor.id.len() <= 128).then_some(cursor)
}

fn download_history_record(
    database: &Database,
    task: TaskSnapshot,
) -> Result<DownloadHistoryRecord, ApiError> {
    let request = serde_json::from_value::<CreateTaskRequest>(task.payload.clone()).ok();
    let root_name = if let Some(request) = request.as_ref() {
        database
            .get_root(&request.root_id)
            .map_err(|error| ApiError::internal(format!("无法读取下载目标: {error}")))?
            .map(|root| root.name)
    } else {
        None
    };
    let source_label = request
        .as_ref()
        .and_then(|request| request.source.as_ref())
        .map(|source| match source {
            DownloadSource::Query { query } => query.clone(),
            DownloadSource::PostIds { post_ids } => format!("{} 个指定帖子", post_ids.len()),
        })
        .unwrap_or_else(|| "未知来源".to_string());
    let terminal = matches!(
        task.status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    );
    let item_counts = database
        .task_item_counts(&task.id)
        .map_err(|error| ApiError::internal(format!("无法统计下载任务项目: {error}")))?;
    let has_item_records = item_counts.total > 0;
    let skipped_items = if has_item_records {
        item_counts.skipped
    } else {
        task.result
            .as_ref()
            .and_then(|result| result.get("skipped"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let failed_items = if has_item_records {
        item_counts.failed
    } else {
        task.result
            .as_ref()
            .and_then(|result| result.get("failed"))
            .and_then(Value::as_u64)
            .unwrap_or_else(|| u64::from(task.error.is_some()))
    };
    let repeat_request = (terminal && root_name.is_some())
        .then_some(request)
        .flatten();
    Ok(DownloadHistoryRecord {
        id: task.id.clone(),
        task_id: task.id,
        status: task_status_response(task.status),
        source_label,
        root_name,
        created_at: format_unix_timestamp(task.created_at),
        finished_at: terminal.then(|| format_unix_timestamp(task.updated_at)),
        duration_seconds: terminal.then(|| task.updated_at.saturating_sub(task.created_at)),
        total_items: if has_item_records {
            item_counts.total
        } else {
            task.total_items.unwrap_or(0)
        },
        completed_items: if has_item_records {
            item_counts.completed
        } else {
            task.completed_items
        },
        skipped_items,
        failed_items,
        bytes_processed: if has_item_records {
            item_counts.completed_bytes
        } else {
            task.bytes_processed
        },
        error_message: task.error.map(|error| error.message),
        can_repeat: repeat_request.is_some(),
        repeat_request,
    })
}

fn legacy_download_history_record(
    record: crate::database::DownloadRecord,
) -> DownloadHistoryRecord {
    let status = if record.status == "completed" {
        "completed"
    } else if record.status.starts_with("cancelled") {
        "cancelled"
    } else {
        "failed"
    };
    let source_label = if record.tags.trim().is_empty() {
        "旧版下载".to_string()
    } else {
        record.tags
    };
    let error_message = record
        .status
        .strip_prefix("failed:")
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string);
    DownloadHistoryRecord {
        id: format!("legacy:{}", record.id),
        task_id: record.task_id,
        status,
        source_label,
        root_name: None,
        created_at: sqlite_timestamp_response(&record.started_at),
        finished_at: record.finished_at.as_deref().map(sqlite_timestamp_response),
        duration_seconds: None,
        total_items: record
            .total_images
            .saturating_add(record.failed_count)
            .try_into()
            .unwrap_or_default(),
        completed_items: record.total_images.try_into().unwrap_or_default(),
        skipped_items: 0,
        failed_items: record.failed_count.try_into().unwrap_or_default(),
        bytes_processed: 0,
        error_message,
        can_repeat: false,
        repeat_request: None,
    }
}

fn sqlite_timestamp_response(timestamp: &str) -> String {
    if timestamp.contains('T') {
        timestamp.to_string()
    } else {
        format!("{}Z", timestamp.replace(' ', "T"))
    }
}

async fn list_tasks(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<TaskSnapshotResponse>>, ApiError> {
    let (tasks, last_event_id) = state
        .tasks
        .snapshot_with_sequence()
        .map_err(map_task_manager_error)?;
    Ok(Json(ApiSuccess {
        data: TaskSnapshotResponse {
            tasks: tasks
                .into_iter()
                .map(|task| task_summary_response(&state.database, task))
                .collect(),
            last_event_id,
        },
        meta: None,
    }))
}

#[derive(Debug, serde::Deserialize)]
struct TaskDetailQuery {
    item_status: Option<String>,
    item_cursor: Option<String>,
    item_limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TaskDetailResponse {
    task: TaskSummaryResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    item_counts: TaskItemCountsResponse,
    items: Vec<TaskItemResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct TaskItemCountsResponse {
    total: u64,
    queued: u64,
    completed: u64,
    skipped: u64,
    failed: u64,
    retryable_failed: u64,
    completed_bytes: u64,
}

#[derive(Debug, Serialize)]
struct TaskItemResponse {
    item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_id: Option<u64>,
    status: String,
    attempts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<TaskItemErrorResponse>,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct TaskItemErrorResponse {
    code: String,
    message: String,
    retryable: bool,
}

async fn task_detail(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TaskDetailQuery>,
) -> Result<Json<ApiSuccess<TaskDetailResponse>>, ApiError> {
    let item_limit = query.item_limit.unwrap_or(50);
    if !(1..=100).contains(&item_limit) {
        return Err(ApiError::bad_request(
            "invalid_task_item_limit",
            "任务项目分页大小必须在 1..=100 之间",
        ));
    }
    if query
        .item_status
        .as_deref()
        .is_some_and(|status| !matches!(status, "queued" | "completed" | "skipped" | "failed"))
    {
        return Err(ApiError::bad_request(
            "invalid_task_item_status",
            "任务项目状态筛选无效",
        ));
    }
    let after_id = query
        .item_cursor
        .as_deref()
        .map(decode_task_item_cursor)
        .transpose()?
        .flatten();
    let task = state
        .tasks
        .get(&id)
        .map_err(map_task_manager_error)?
        .ok_or_else(|| ApiError::not_found("task_not_found", "任务不存在"))?;
    let counts = state
        .database
        .task_item_counts(&id)
        .map_err(|error| ApiError::internal(format!("无法统计任务项目: {error}")))?;
    let page = state
        .database
        .list_task_items_page(&id, query.item_status.as_deref(), after_id, item_limit)
        .map_err(|error| ApiError::internal(format!("无法读取任务项目: {error}")))?;
    let items = page
        .items
        .into_iter()
        .map(task_item_response)
        .collect::<Result<Vec<_>, _>>()?;
    let result = sanitize_task_result(&task);
    Ok(Json(ApiSuccess {
        data: TaskDetailResponse {
            task: task_summary_response(&state.database, task),
            result,
            item_counts: TaskItemCountsResponse {
                total: counts.total,
                queued: counts.queued,
                completed: counts.completed,
                skipped: counts.skipped,
                failed: counts.failed,
                retryable_failed: counts.retryable_failed,
                completed_bytes: counts.completed_bytes,
            },
            items,
            next_cursor: page.next_cursor.map(encode_task_item_cursor),
        },
        meta: None,
    }))
}

fn encode_task_item_cursor(id: i64) -> String {
    URL_SAFE_NO_PAD.encode(id.to_be_bytes())
}

fn decode_task_item_cursor(cursor: &str) -> Result<Option<i64>, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("invalid_task_item_cursor", "任务项目游标无效"))?;
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| ApiError::bad_request("invalid_task_item_cursor", "任务项目游标无效"))?;
    let id = i64::from_be_bytes(bytes);
    if id <= 0 {
        return Err(ApiError::bad_request(
            "invalid_task_item_cursor",
            "任务项目游标无效",
        ));
    }
    Ok(Some(id))
}

fn task_item_response(item: crate::database::TaskItemRecord) -> Result<TaskItemResponse, ApiError> {
    if !matches!(
        item.status.as_str(),
        "queued" | "completed" | "skipped" | "failed"
    ) {
        return Err(ApiError::internal("SQLite 中的任务项目状态无效"));
    }
    let result = sanitize_task_item_result(item.result.as_ref());
    let error = item.error.as_ref().map(task_item_error_response);
    Ok(TaskItemResponse {
        item_id: item.item_key,
        post_id: item.payload.get("post_id").and_then(Value::as_u64),
        status: item.status,
        attempts: item.attempts.max(0) as u64,
        result,
        error,
        updated_at: sqlite_timestamp_response(&item.updated_at),
    })
}

fn sanitize_task_item_result(result: Option<&Value>) -> Option<Value> {
    let object = result?.as_object()?;
    let mut safe = serde_json::Map::new();
    for key in ["bytes", "recovered", "sidecar_written"] {
        if let Some(value) = object.get(key).filter(|value| {
            matches!(
                (key, *value),
                ("bytes", Value::Number(_))
                    | ("recovered", Value::Bool(_))
                    | ("sidecar_written", Value::Bool(_))
            )
        }) {
            safe.insert(key.to_string(), value.clone());
        }
    }
    if let Some(reason) = object.get("reason").and_then(Value::as_str) {
        safe.insert(
            "reason".to_string(),
            Value::String(reason.chars().take(256).collect()),
        );
    }
    for key in ["variants", "media_ids", "tags"] {
        if let Some(values) = object.get(key).and_then(Value::as_array) {
            let values = values
                .iter()
                .filter_map(Value::as_str)
                .take(32)
                .map(|value| Value::String(value.chars().take(512).collect()))
                .collect::<Vec<_>>();
            safe.insert(key.to_string(), Value::Array(values));
        }
    }
    (!safe.is_empty()).then_some(Value::Object(safe))
}

fn sanitize_task_result(task: &TaskSnapshot) -> Option<Value> {
    let object = task.result.as_ref()?.as_object()?;
    let mut safe = serde_json::Map::new();
    let mut copy_count = |key: &str| {
        if let Some(value) = object.get(key).filter(|value| value.as_u64().is_some()) {
            safe.insert(key.to_string(), value.clone());
        }
    };
    match task.kind.as_str() {
        "download" => {
            for key in ["downloaded", "skipped", "failed", "bytes"] {
                copy_count(key);
            }
        }
        "index_library" => {
            for key in ["indexed", "deleted"] {
                copy_count(key);
            }
        }
        "exact_dedup" | "near_dedup" | "integrity_scan" | "delete_by_tag" => {
            copy_count("moved");
        }
        "tag_pipeline" => copy_count("changed"),
        "vllm_tag" => {
            if let Some(successes) = object.get("successes").and_then(Value::as_array) {
                safe.insert(
                    "tagged".to_string(),
                    Value::Number(serde_json::Number::from(
                        u64::try_from(successes.len()).unwrap_or(u64::MAX),
                    )),
                );
            }
        }
        "resize" | "heic_convert" => {
            if let Some(items) = object.get("items").and_then(Value::as_array) {
                safe.insert(
                    "processed".to_string(),
                    Value::Number(serde_json::Number::from(
                        u64::try_from(items.len()).unwrap_or(u64::MAX),
                    )),
                );
                let mut safe_items = Vec::new();
                for item in items.iter().take(20) {
                    let Some(item) = item.as_object() else {
                        continue;
                    };
                    let mut safe_item = serde_json::Map::new();
                    if let Some(media_id) = item.get("media_id").and_then(Value::as_str) {
                        safe_item.insert(
                            "media_id".to_string(),
                            Value::String(media_id.chars().take(128).collect()),
                        );
                    }
                    for key in ["width", "height", "bytes"] {
                        if let Some(value) = item.get(key).filter(|value| value.as_u64().is_some())
                        {
                            safe_item.insert(key.to_string(), value.clone());
                        }
                    }
                    if !safe_item.is_empty() {
                        safe_items.push(Value::Object(safe_item));
                    }
                }
                safe.insert("items".to_string(), Value::Array(safe_items));
            }
        }
        _ => {}
    }
    (!safe.is_empty()).then_some(Value::Object(safe))
}

fn task_item_error_response(error: &Value) -> TaskItemErrorResponse {
    TaskItemErrorResponse {
        code: error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("task_item_failed")
            .chars()
            .take(128)
            .collect(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("任务项目失败")
            .chars()
            .take(4_096)
            .collect(),
        retryable: error
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTaskRequest {
    #[serde(rename = "type")]
    kind: String,
    root_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relative_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<DownloadSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    concurrency: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filename_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skip_existing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_sidecar_txt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    static_images_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prioritize_score: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prioritize_resolution: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batch_filter: Option<BatchDownloadFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_policy: Option<MediaPolicyRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DownloadSource {
    Query { query: String },
    PostIds { post_ids: Vec<u64> },
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchDownloadFilter {
    include_tags: Vec<String>,
    exclude_tags: Vec<String>,
    minimum_score: i64,
    #[serde(default)]
    minimum_resolution: u32,
}

const BATCH_VERIFICATION_TAG_LIMIT: usize = 2;
const BATCH_VERIFICATION_POST_IDS_PER_REQUEST: usize = 100;

fn split_batch_verification_groups(
    filter: &BatchDownloadFilter,
    anchor_tag: &str,
) -> Vec<Vec<String>> {
    let mut remaining = filter
        .include_tags
        .iter()
        .filter(|tag| tag.as_str() != anchor_tag)
        .cloned()
        .collect::<Vec<_>>();
    remaining.extend(filter.exclude_tags.iter().map(|tag| format!("-{tag}")));
    remaining
        .chunks(BATCH_VERIFICATION_TAG_LIMIT)
        .map(|group| group.to_vec())
        .collect()
}

fn segmented_batch_anchor_query(filter: &BatchDownloadFilter, anchor_tag: &str) -> String {
    let resolution = (filter.minimum_resolution > 0)
        .then(|| format!("width:>={0} height:>={0}", filter.minimum_resolution));
    [
        anchor_tag.to_string(),
        format!("score:>={}", filter.minimum_score),
        resolution.unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

fn segmented_batch_verification_query(post_ids: &[u64], tag_group: &[String]) -> String {
    let post_ids = post_ids
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    std::iter::once(format!("id:{post_ids}"))
        .chain(tag_group.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

async fn select_segmented_batch_anchor(
    client: &DanbooruClient,
    filter: &BatchDownloadFilter,
) -> String {
    let mut selected = filter
        .include_tags
        .first()
        .cloned()
        .expect("validated batch filter always has an include tag");
    let mut smallest_count = u64::MAX;
    for tag in &filter.include_tags {
        match client.count(tag).await {
            Ok(count) if count < smallest_count => {
                selected = tag.clone();
                smallest_count = count;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(%error, tag, "无法统计批量下载锚点标签，沿用可用标签");
            }
        }
    }
    selected
}

async fn verify_segmented_batch_candidates(
    client: &DanbooruClient,
    candidates: Vec<Post>,
    filter: &BatchDownloadFilter,
    anchor_tag: &str,
) -> Result<Vec<Post>, TaskFailure> {
    let mut accepted = candidates;
    for tag_group in split_batch_verification_groups(filter, anchor_tag) {
        if accepted.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(accepted.len());
        for post_ids in accepted.chunks(BATCH_VERIFICATION_POST_IDS_PER_REQUEST) {
            let tags = segmented_batch_verification_query(
                &post_ids.iter().map(|post| post.id).collect::<Vec<_>>(),
                &tag_group,
            );
            let mut page = client
                .posts(&PostQuery {
                    tags,
                    page: "1".to_string(),
                    limit: post_ids.len() as u16,
                })
                .await
                .map_err(danbooru_task_failure)?;
            next.append(&mut page.posts);
        }
        accepted = next;
    }
    Ok(accepted)
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaPolicyRequest {
    original: bool,
    ugoira: crate::config::UgoiraPolicy,
}

async fn create_task(
    State(state): State<AppState>,
    Json(mut request): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<ApiSuccess<TaskSummaryResponse>>), ApiError> {
    normalize_download_task_directory(&mut request)?;
    expand_relative_directory_selection(&state, &mut request)?;
    validate_task_request(&state, &request)?;
    let payload = serde_json::to_value(&request)
        .map_err(|error| ApiError::internal(format!("无法编码任务请求: {error}")))?;
    let task = state
        .tasks
        .create(request.kind.clone(), payload)
        .map_err(map_task_manager_error)?;
    if let Some(DownloadSource::PostIds { post_ids }) = request.source.as_ref() {
        let mut seen = HashSet::new();
        let items = post_ids
            .iter()
            .copied()
            .filter(|post_id| seen.insert(*post_id))
            .map(|post_id| TaskItemInput {
                item_key: format!("post:{post_id}"),
                status: "queued".to_string(),
                payload: serde_json::json!({ "post_id": post_id }),
                result: None,
                error: None,
                attempts: 0,
            })
            .collect::<Vec<_>>();
        if let Err(error) = state.database.ensure_task_items(&task.id, &items) {
            tracing::error!(task_id = %task.id, %error, "无法持久化下载任务项目");
            if let Err(status_error) = state.tasks.fail(
                &task.id,
                TaskFailure {
                    code: "task_item_persistence_failed".to_string(),
                    message: "无法持久化下载任务项目".to_string(),
                    retryable: true,
                },
            ) {
                tracing::error!(task_id = %task.id, %status_error, "无法持久化任务项目失败状态");
            }
            return Err(ApiError::internal("无法持久化下载任务项目"));
        }
    }
    if request.kind == "vllm_tag" {
        let media_ids = validated_task_media_ids(request.options.as_ref())?;
        let items = media_ids
            .into_iter()
            .map(|media_id| TaskItemInput {
                item_key: format!("media:{media_id}"),
                status: "queued".to_string(),
                payload: serde_json::json!({ "media_id": media_id }),
                result: None,
                error: None,
                attempts: 0,
            })
            .collect::<Vec<_>>();
        if let Err(error) = state.database.ensure_task_items(&task.id, &items) {
            tracing::error!(task_id = %task.id, %error, "无法持久化 vLLM 任务项目");
            if let Err(status_error) = state.tasks.fail(
                &task.id,
                TaskFailure {
                    code: "task_item_persistence_failed".to_string(),
                    message: "无法持久化 vLLM 任务项目".to_string(),
                    retryable: true,
                },
            ) {
                tracing::error!(task_id = %task.id, %status_error, "无法持久化任务项目失败状态");
            }
            return Err(ApiError::internal("无法持久化 vLLM 任务项目"));
        }
    }
    spawn_task_worker(state.clone(), task.id.clone()).await;
    Ok((
        StatusCode::CREATED,
        Json(ApiSuccess {
            data: task_summary_response(&state.database, task),
            meta: None,
        }),
    ))
}

fn normalize_download_task_directory(request: &mut CreateTaskRequest) -> Result<(), ApiError> {
    if request.kind != "download" {
        return Ok(());
    }
    request.relative_directory = request
        .relative_directory
        .as_deref()
        .map(normalize_task_relative_directory)
        .transpose()?
        .filter(|path| !path.is_empty());
    Ok(())
}

fn validate_task_request(state: &AppState, request: &CreateTaskRequest) -> Result<(), ApiError> {
    const KINDS: &[&str] = &[
        "download",
        "index_library",
        "integrity_scan",
        "exact_dedup",
        "near_dedup",
        "resize",
        "heic_convert",
        "delete_by_tag",
        "tag_pipeline",
        "vllm_tag",
    ];
    if !KINDS.contains(&request.kind.as_str()) {
        return Err(ApiError::bad_request("invalid_task_type", "未知任务类型"));
    }
    state
        .database
        .get_root(&request.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    if request
        .options
        .as_ref()
        .is_some_and(contains_forbidden_task_option)
    {
        return Err(ApiError::bad_request(
            "forbidden_task_override",
            "任务不能覆盖服务地址、allowlist 或密钥",
        ));
    }

    if request.kind != "download" {
        if request.relative_directory.is_some()
            || request.source.is_some()
            || request.limit.is_some()
            || request.concurrency.is_some()
            || request.filename_template.is_some()
            || request.keep_sidecar_txt.is_some()
            || request.static_images_only.is_some()
            || request.prioritize_score.is_some()
            || request.prioritize_resolution.is_some()
            || request.batch_filter.is_some()
            || request.media_policy.is_some()
        {
            return Err(ApiError::bad_request(
                "invalid_task_fields",
                "该工具任务包含下载专用字段",
            ));
        }
        if matches!(
            request.kind.as_str(),
            "resize" | "heic_convert" | "tag_pipeline" | "vllm_tag"
        ) {
            let media_ids = validated_task_media_ids(request.options.as_ref())?;
            if matches!(request.kind.as_str(), "heic_convert" | "tag_pipeline") {
                let options = request
                    .options
                    .as_ref()
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        ApiError::bad_request("invalid_task_options", "工具 options 必须是对象")
                    })?;
                let valid_keys: &[&str] = if request.kind == "tag_pipeline" {
                    &["media_ids", "artist_prefix"]
                } else {
                    &["media_ids"]
                };
                if options
                    .keys()
                    .any(|key| !valid_keys.contains(&key.as_str()))
                {
                    return Err(ApiError::bad_request(
                        "invalid_task_options",
                        "该工具包含不支持的选项，不允许覆盖转换器或文件路径",
                    ));
                }
                if request.kind == "tag_pipeline" {
                    parse_artist_prefix(request.options.as_ref())?;
                }
            }
            let mut unsupported_vllm_media = Vec::new();
            for media_id in media_ids {
                let media = state
                    .database
                    .get_media_file(&media_id)
                    .map_err(|error| ApiError::internal(error.to_string()))?
                    .filter(|media| media.status == "active")
                    .ok_or_else(|| {
                        ApiError::not_found("media_not_found", "任务媒体不存在或不可用")
                    })?;
                if media.root_id != request.root_id {
                    return Err(ApiError::bad_request(
                        "media_root_mismatch",
                        "任务媒体不属于指定根目录",
                    ));
                }
                if request.kind == "vllm_tag"
                    && !is_supported_vllm_media(&media.relative_path, &media.mime_type)
                {
                    unsupported_vllm_media.push(media_id);
                }
            }
            if !unsupported_vllm_media.is_empty() {
                return Err(ApiError {
                    status: StatusCode::BAD_REQUEST,
                    code: "unsupported_vllm_media".to_string(),
                    message: "视觉打标仅支持 PNG、JPEG、BMP、WebP 和 GIF 静态图片".to_string(),
                    retryable: false,
                    fields: Some(serde_json::json!({
                        "media_ids": {
                            "code": "unsupported_media_type",
                            "invalid_ids": unsupported_vllm_media,
                        }
                    })),
                });
            }
        } else if matches!(
            request.kind.as_str(),
            "integrity_scan" | "exact_dedup" | "near_dedup" | "delete_by_tag"
        ) && request
            .options
            .as_ref()
            .and_then(|options| options.get("media_ids"))
            .is_some()
        {
            for media_id in validated_task_media_ids(request.options.as_ref())? {
                let media = state
                    .database
                    .get_media_file(&media_id)
                    .map_err(|error| ApiError::internal(error.to_string()))?
                    .filter(|media| media.status == "active")
                    .ok_or_else(|| {
                        ApiError::not_found("media_not_found", "任务媒体不存在或不可用")
                    })?;
                if media.root_id != request.root_id {
                    return Err(ApiError::bad_request(
                        "media_root_mismatch",
                        "任务媒体不属于指定根目录",
                    ));
                }
            }
        }
        return Ok(());
    }

    if request.options.is_some() {
        return Err(ApiError::bad_request(
            "invalid_task_fields",
            "下载任务不能包含工具专用 options",
        ));
    }

    let source = request
        .source
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("missing_source", "下载任务缺少 source"))?;
    match source {
        DownloadSource::Query { query } if query.len() <= 4_096 => {}
        DownloadSource::Query { .. } => {
            return Err(ApiError::bad_request(
                "query_too_long",
                "查询最长为 4096 字节",
            ));
        }
        DownloadSource::PostIds { post_ids }
            if !post_ids.is_empty()
                && post_ids.len() <= 10_000
                && post_ids.iter().all(|id| *id > 0) => {}
        DownloadSource::PostIds { .. } => {
            return Err(ApiError::bad_request(
                "invalid_post_ids",
                "post_ids 数量必须在 1..=10000 且 ID 大于 0",
            ));
        }
    }
    if let Some(filter) = request.batch_filter.as_ref() {
        if !matches!(source, DownloadSource::Query { .. }) {
            return Err(ApiError::bad_request(
                "invalid_batch_filter",
                "分段标签验证仅支持查询下载",
            ));
        }
        validate_batch_download_filter(filter)?;
    }
    if !matches!(request.limit, Some(1..)) {
        return Err(ApiError::bad_request(
            "invalid_download_limit",
            "下载数量必须大于 0",
        ));
    }
    if !matches!(request.concurrency, Some(1..=32)) {
        return Err(ApiError::bad_request(
            "invalid_concurrency",
            "下载并发必须在 1..=32",
        ));
    }
    let template = request.filename_template.as_deref().ok_or_else(|| {
        ApiError::bad_request("missing_filename_template", "下载任务缺少文件名模板")
    })?;
    validate_filename_template(template)
        .map_err(|error| ApiError::bad_request("invalid_filename_template", error.message))?;
    if !request
        .media_policy
        .as_ref()
        .is_some_and(|policy| policy.original)
    {
        return Err(ApiError::bad_request(
            "invalid_media_policy",
            "当前下载策略必须包含原始媒体",
        ));
    }
    Ok(())
}

fn validate_batch_download_filter(filter: &BatchDownloadFilter) -> Result<(), ApiError> {
    const MAX_BATCH_TAGS: usize = 64;
    if filter.include_tags.is_empty()
        || filter.include_tags.len() > MAX_BATCH_TAGS
        || filter.exclude_tags.len() > MAX_BATCH_TAGS
        || !(-1_000_000..=1_000_000).contains(&filter.minimum_score)
        || filter.minimum_resolution > 8_192
        || filter.minimum_resolution % 512 != 0
    {
        return Err(ApiError::bad_request(
            "invalid_batch_filter",
            "分段标签验证参数无效",
        ));
    }
    let mut seen = HashSet::new();
    for tag in filter.include_tags.iter().chain(&filter.exclude_tags) {
        if tag.is_empty()
            || tag.len() > 255
            || tag.starts_with('-')
            || tag
                .chars()
                .any(|character| character.is_whitespace() || character == ':')
            || !seen.insert(tag)
        {
            return Err(ApiError::bad_request(
                "invalid_batch_filter",
                "分段标签验证仅接受不重复的普通标签",
            ));
        }
    }
    Ok(())
}

fn contains_forbidden_task_option(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "api_key" | "secret" | "token" | "endpoint" | "base_url" | "allowed_hosts"
            ) || contains_forbidden_task_option(value)
        }),
        Value::Array(items) => items.iter().any(contains_forbidden_task_option),
        _ => false,
    }
}

fn validated_task_media_ids(options: Option<&Value>) -> Result<Vec<String>, ApiError> {
    let ids = options
        .and_then(|options| options.get("media_ids"))
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("missing_media_ids", "任务必须明确携带 media_ids"))?;
    if ids.is_empty() || ids.len() > 10_000 {
        return Err(ApiError::bad_request(
            "invalid_media_ids",
            "media_ids 数量必须在 1..=10000",
        ));
    }
    let mut unique = HashSet::with_capacity(ids.len());
    let mut validated = Vec::with_capacity(ids.len());
    for id in ids {
        let id = id
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or_else(|| ApiError::bad_request("invalid_media_ids", "media ID 格式无效"))?;
        if unique.insert(id) {
            validated.push(id.to_string());
        }
    }
    if validated.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_media_ids",
            "media_ids 不能为空",
        ));
    }
    Ok(validated)
}

fn expand_relative_directory_selection(
    state: &AppState,
    request: &mut CreateTaskRequest,
) -> Result<(), ApiError> {
    if matches!(request.kind.as_str(), "download" | "index_library") {
        return Ok(());
    }
    let Some(options) = request.options.as_mut().and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let Some(directory_value) = options.get("relative_directory").cloned() else {
        return Ok(());
    };
    if options.contains_key("media_ids") {
        return Err(ApiError::bad_request(
            "ambiguous_media_selection",
            "media_ids 与 relative_directory 只能选择一种",
        ));
    }
    let directory = directory_value
        .as_str()
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_relative_directory",
                "relative_directory 必须是字符串",
            )
        })
        .and_then(normalize_task_relative_directory)?;
    if state
        .database
        .get_root(&request.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .is_none()
    {
        return Err(ApiError::not_found("root_not_found", "媒体根不存在"));
    }
    let mut media = state
        .database
        .list_active_media_in_directory(&request.root_id, &directory, 10_001)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    media.retain(|item| match request.kind.as_str() {
        "vllm_tag" => is_supported_vllm_media(&item.relative_path, &item.mime_type),
        "heic_convert" => is_supported_heic_media(&item.relative_path, &item.mime_type),
        "resize" => item.mime_type.starts_with("image/"),
        _ => true,
    });
    if media.len() > 10_000 {
        return Err(ApiError::bad_request(
            "directory_selection_too_large",
            "单个目录批处理最多 10000 项，请缩小目录范围",
        ));
    }
    if media.is_empty() {
        return Err(ApiError::bad_request(
            "empty_directory_selection",
            "目录内没有适用于该工具的已索引媒体",
        ));
    }
    options.remove("relative_directory");
    options.insert(
        "media_ids".to_string(),
        Value::Array(
            media
                .into_iter()
                .map(|item| Value::String(item.id))
                .collect(),
        ),
    );
    Ok(())
}

fn normalize_task_relative_directory(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.len() > 4_096 {
        return Err(ApiError::bad_request(
            "invalid_relative_directory",
            "相对目录最长为 4096 字节",
        ));
    }
    if value.is_empty() || value == "." {
        return Ok(String::new());
    }
    let normalized = value.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err(ApiError::bad_request(
            "invalid_relative_directory",
            "目录必须是媒体根内的相对路径",
        ));
    }
    let normalized = normalized.trim_end_matches('/');
    let parts = normalized.split('/').collect::<Vec<_>>();
    if parts.is_empty()
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || part.eq_ignore_ascii_case(".danbooru-quarantine")
        })
    {
        return Err(ApiError::bad_request(
            "invalid_relative_directory",
            "相对目录不能包含空段、点段、父目录或隔离区",
        ));
    }
    Ok(parts.join("/"))
}

fn parse_artist_prefix(options: Option<&Value>) -> Result<ArtistPrefix, ApiError> {
    match options
        .and_then(|options| options.get("artist_prefix"))
        .and_then(Value::as_str)
    {
        None | Some("artist") => Ok(ArtistPrefix::Artist),
        Some("at") => Ok(ArtistPrefix::At),
        Some(_) => Err(ApiError::bad_request(
            "invalid_artist_prefix",
            "artist_prefix 只能是 artist 或 at",
        )),
    }
}

fn is_supported_vllm_media(relative_path: &str, mime_type: &str) -> bool {
    let extension = Path::new(relative_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let mime_type = mime_type.trim().to_ascii_lowercase();
    match extension.as_deref() {
        Some("png") => mime_type == "image/png",
        Some("jpg" | "jpeg") => mime_type == "image/jpeg",
        Some("bmp") => matches!(mime_type.as_str(), "image/bmp" | "image/x-ms-bmp"),
        Some("webp") => mime_type == "image/webp",
        Some("gif") => mime_type == "image/gif",
        _ => false,
    }
}

async fn task_action(
    State(state): State<AppState>,
    AxumPath((id, action)): AxumPath<(String, String)>,
) -> Result<Json<ApiSuccess<TaskSummaryResponse>>, ApiError> {
    let should_start = matches!(action.as_str(), "resume" | "retry" | "confirm");
    let task = match action.as_str() {
        "pause" => state.tasks.pause(&id),
        "resume" => state.tasks.resume(&id),
        "cancel" => state.tasks.cancel(&id),
        "retry" => state.tasks.retry(&id),
        "confirm" => state.tasks.confirm(&id),
        _ => {
            return Err(ApiError::not_found("task_action_not_found", "未知任务动作"));
        }
    }
    .map_err(map_task_manager_error)?;
    if action == "retry" && matches!(task.kind.as_str(), "download" | "vllm_tag") {
        if let Err(error) = state.database.requeue_retryable_task_items(&id) {
            tracing::error!(task_id = %id, %error, "无法重新排入可重试任务项目");
            if let Err(status_error) = state.tasks.fail(
                &id,
                TaskFailure {
                    code: "task_item_persistence_failed".to_string(),
                    message: "无法重新排入可重试任务项目".to_string(),
                    retryable: true,
                },
            ) {
                tracing::error!(task_id = %id, %status_error, "无法补偿任务项目重试状态");
            }
            return Err(ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "task_item_persistence_failed".to_string(),
                message: "无法重新排入可重试任务项目".to_string(),
                retryable: true,
                fields: None,
            });
        }
    }
    if should_start {
        spawn_task_worker(state.clone(), task.id.clone()).await;
    }
    Ok(Json(ApiSuccess {
        data: task_summary_response(&state.database, task),
        meta: None,
    }))
}

fn map_task_manager_error(error: TaskManagerError) -> ApiError {
    match error {
        TaskManagerError::NotFound => ApiError::not_found("task_not_found", "任务不存在"),
        TaskManagerError::InvalidTransition { from, to } => ApiError {
            status: StatusCode::CONFLICT,
            code: "invalid_task_transition".to_string(),
            message: format!("任务不能从 {from:?} 切换到 {to:?}"),
            retryable: false,
            fields: None,
        },
        TaskManagerError::Persistence { .. } => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "task_persistence_failed".to_string(),
            message: "任务持久化失败，请稍后重试".to_string(),
            retryable: true,
            fields: None,
        },
    }
}

enum WorkerOutcome {
    Complete(Value),
    Stopped,
}

async fn spawn_task_worker(state: AppState, task_id: String) {
    {
        let mut active = state.active_workers.lock().await;
        if !active.insert(task_id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        loop {
            let worker_slot = match state.worker_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let outcome = async {
                let task = state.tasks.start(&task_id).map_err(|error| TaskFailure {
                    code: "task_start_failed".to_string(),
                    message: error.to_string(),
                    retryable: false,
                })?;
                match task.kind.as_str() {
                    "download" => run_download_task(&state, &task).await,
                    "index_library" => run_index_task(&state, &task).await,
                    "exact_dedup" | "integrity_scan" | "near_dedup" | "delete_by_tag" => {
                        run_tool_task(&state, &task).await
                    }
                    "vllm_tag" => run_vllm_task(&state, &task).await,
                    "resize" => run_resize_task(&state, &task).await,
                    "heic_convert" => run_heic_task(&state, &task).await,
                    "tag_pipeline" => run_tag_pipeline_task(&state, &task).await,
                    _ => Err(TaskFailure {
                        code: "task_not_implemented".to_string(),
                        message: "该工具任务尚未接入执行器".to_string(),
                        retryable: false,
                    }),
                }
            }
            .await;

            match outcome {
                Ok(WorkerOutcome::Complete(result)) => match state.tasks.get(&task_id) {
                    Ok(Some(task)) if task.status == TaskStatus::Running => {
                        if let Err(error) = state.tasks.complete(&task_id, result) {
                            tracing::error!(task_id = %task_id, %error, "无法持久化任务完成状态");
                        }
                    }
                    Err(error) => {
                        tracing::error!(task_id = %task_id, %error, "无法读取任务完成状态");
                    }
                    _ => {}
                },
                Ok(WorkerOutcome::Stopped) => {}
                Err(failure) => match state.tasks.get(&task_id) {
                    Ok(Some(task)) if task.status == TaskStatus::Running => {
                        if let Err(error) = state.tasks.fail(&task_id, failure) {
                            tracing::error!(task_id = %task_id, %error, "无法持久化任务失败状态");
                        }
                    }
                    Err(error) => {
                        tracing::error!(task_id = %task_id, %error, "无法读取任务失败状态");
                    }
                    _ => {}
                },
            }

            match state.tasks.get(&task_id) {
                Ok(Some(task))
                    if matches!(task.status, TaskStatus::Pausing | TaskStatus::Cancelling) =>
                {
                    if let Err(error) = state.tasks.acknowledge_stop(&task_id) {
                        tracing::error!(task_id = %task_id, %error, "无法持久化任务停止确认");
                    }
                }
                Err(error) => {
                    tracing::error!(task_id = %task_id, %error, "无法读取任务停止请求状态");
                }
                _ => {}
            }

            drop(worker_slot);
            let mut active = state.active_workers.lock().await;
            active.remove(&task_id);
            let should_restart = match state.tasks.get(&task_id) {
                Ok(Some(task)) => task.status == TaskStatus::Queued,
                Ok(None) => false,
                Err(error) => {
                    tracing::error!(task_id = %task_id, %error, "无法读取待重启任务状态");
                    false
                }
            };
            if should_restart {
                active.insert(task_id.clone());
            }
            drop(active);
            if !should_restart {
                break;
            }
        }
    });
}

async fn run_vllm_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let requested_media_ids = validated_task_media_ids(request.options.as_ref())
        .map_err(api_task_failure)?
        .into_iter()
        .collect::<HashSet<_>>();
    let persisted_items = state
        .database
        .list_task_items(&task.id)
        .map_err(database_task_failure)?;
    let mut media_ids = Vec::new();
    let mut item_keys = HashMap::new();
    for item in persisted_items
        .into_iter()
        .filter(|item| item.status == "queued")
    {
        let media_id = item
            .payload
            .get("media_id")
            .and_then(Value::as_str)
            .filter(|media_id| requested_media_ids.contains(*media_id))
            .ok_or_else(|| TaskFailure {
                code: "invalid_vllm_task_item".to_string(),
                message: "vLLM 任务项目缺少有效媒体 ID".to_string(),
                retryable: false,
            })?
            .to_string();
        if item_keys.insert(media_id.clone(), item.item_key).is_some() {
            return Err(TaskFailure {
                code: "duplicate_vllm_task_item".to_string(),
                message: format!("vLLM 任务包含重复媒体项目: {media_id}"),
                retryable: false,
            });
        }
        media_ids.push(media_id);
    }
    if media_ids.is_empty() {
        let counts = state
            .database
            .task_item_counts(&task.id)
            .map_err(database_task_failure)?;
        if counts.failed > 0 {
            return Err(vllm_items_failure(counts));
        }
        return Ok(WorkerOutcome::Complete(serde_json::json!({
            "successes": [],
            "retry_manifest": { "items": [] }
        })));
    }
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let verified_root =
        VerifiedMediaRoot::open(current_platform_path(&root).map_err(api_task_failure)?)
            .map_err(tool_task_failure)?;
    let _root_write = state
        .root_writes
        .acquire(verified_root.path())
        .await
        .map_err(root_write_task_failure)?;
    if worker_was_stopped(state, &task.id) {
        return Ok(WorkerOutcome::Stopped);
    }
    let settings = state.settings.read().await.clone();
    let mut items = Vec::with_capacity(media_ids.len());
    let mut quarantine_by_media = HashMap::new();
    let mut sidecar_targets = HashSet::new();
    let quarantine_batch = format!("vllm-{}", task.id);
    for media_id in media_ids {
        let media = state
            .database
            .get_media_file(&media_id)
            .map_err(database_task_failure)?
            .filter(|media| media.status == "active" && media.root_id == request.root_id)
            .ok_or_else(|| TaskFailure {
                code: "media_not_found".to_string(),
                message: format!("媒体 {media_id} 不存在或不可用"),
                retryable: false,
            })?;
        let image_path = verified_root
            .resolve_existing_file(Path::new(&media.relative_path))
            .map_err(tool_task_failure)?;
        let sidecar_path = image_path.with_extension("txt");
        let sidecar_relative = sidecar_path
            .strip_prefix(verified_root.path())
            .map_err(|_| TaskFailure {
                code: "unsafe_sidecar_path".to_string(),
                message: format!("媒体 {media_id} 的标签路径不在媒体根内"),
                retryable: false,
            })?
            .to_path_buf();
        if !sidecar_targets.insert(platform_path_key(&sidecar_path)) {
            return Err(TaskFailure {
                code: "duplicate_sidecar_target".to_string(),
                message: format!("多个媒体指向同一个标签文件: {}", sidecar_relative.display()),
                retryable: false,
            });
        }
        let (sidecar_quarantine_path, existing_tags) =
            match std::fs::symlink_metadata(&sidecar_path) {
                Ok(metadata) => {
                    if !metadata.file_type().is_file()
                        || metadata_is_link_or_reparse_point(&metadata)
                        || metadata.len() > 8 * 1024 * 1024
                    {
                        return Err(TaskFailure {
                            code: "unsafe_sidecar_file".to_string(),
                            message: format!(
                                "拒绝替换不安全的标签文件: {}",
                                sidecar_relative.display()
                            ),
                            retryable: false,
                        });
                    }
                    let quarantine_relative = PathBuf::from(".danbooru-quarantine")
                        .join(&quarantine_batch)
                        .join(&sidecar_relative);
                    let quarantine_path = verified_root
                        .resolve(&quarantine_relative)
                        .map_err(tool_task_failure)?;
                    quarantine_by_media.insert(
                        media_id.clone(),
                        QuarantineInput {
                            id: uuid::Uuid::new_v4().to_string(),
                            root_id: request.root_id.clone(),
                            media_file_id: None,
                            original_relative_path: sidecar_relative
                                .to_string_lossy()
                                .replace('\\', "/"),
                            quarantine_relative_path: quarantine_relative
                                .to_string_lossy()
                                .replace('\\', "/"),
                            reason: "vllm_sidecar_replaced".to_string(),
                            sha256: None,
                        },
                    );
                    let existing_tags = if settings.vllm_reference_existing {
                        Some(std::fs::read_to_string(&sidecar_path).map_err(|error| {
                            TaskFailure {
                                code: "sidecar_read_failed".to_string(),
                                message: error.to_string(),
                                retryable: false,
                            }
                        })?)
                    } else {
                        None
                    };
                    (Some(quarantine_path), existing_tags)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
                Err(error) => {
                    return Err(TaskFailure {
                        code: "sidecar_metadata_failed".to_string(),
                        message: error.to_string(),
                        retryable: false,
                    });
                }
            };
        items.push(VllmBatchItem {
            media_id,
            image_path,
            existing_tags,
            sidecar_quarantine_path,
        });
    }

    let output = VllmOutputOptions {
        language: settings.vllm_language,
        max_tags: settings.vllm_max_tags,
        max_length: settings.vllm_max_length,
        verify_danbooru: settings.vllm_verify_danbooru,
        reference_existing: settings.vllm_reference_existing,
    };
    let verify_danbooru =
        output.verify_danbooru && output.language == crate::services::vllm::VllmLanguage::Danbooru;
    let config = VllmServiceConfig {
        endpoint: settings.vllm_base_url,
        allowed_hosts: settings.vllm_allowed_hosts,
        model: settings.vllm_model,
        system_prompt: settings.vllm_system_prompt,
        tag_mode: settings.vllm_tag_mode,
        concurrency: settings.vllm_concurrency,
        batch_limit: items.len().clamp(1, 10_000),
        ..VllmServiceConfig::default()
    };
    let api_key = state
        .secrets
        .get_for_internal_use(SecretKind::Vllm)
        .map_err(|_| TaskFailure {
            code: "secret_unavailable".to_string(),
            message: "无法从系统凭据库读取 vLLM 密钥".to_string(),
            retryable: true,
        })?;
    let wave_size = config.concurrency;
    let mut service = VllmService::new(config, api_key)
        .map_err(vllm_task_failure)?
        .with_output_options(output)
        .map_err(vllm_task_failure)?;
    if verify_danbooru {
        service = service.with_danbooru_client(state.danbooru.read().await.clone());
    }
    let total = state
        .database
        .task_item_counts(&task.id)
        .map_err(database_task_failure)?
        .total;
    let started = Instant::now();
    let mut pending = items.into_iter();
    let mut successes = Vec::<VllmTagSuccess>::new();
    let mut failures = Vec::<VllmRetryItem>::new();
    loop {
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        let wave = pending.by_ref().take(wave_size).collect::<Vec<_>>();
        if wave.is_empty() {
            break;
        }
        let wave_media_ids = wave
            .iter()
            .map(|item| item.media_id.clone())
            .collect::<HashSet<_>>();
        let result = service.tag_batch(wave).await.map_err(vllm_task_failure)?;
        commit_vllm_wave(
            state,
            &task.id,
            &verified_root,
            &item_keys,
            &quarantine_by_media,
            &wave_media_ids,
            &result,
        )
        .await?;
        successes.extend(result.successes);
        failures.extend(result.retry_manifest.items);
        let counts = state
            .database
            .task_item_counts(&task.id)
            .map_err(database_task_failure)?;
        if !report_download_progress(
            state,
            &task.id,
            counts.completed,
            counts.total.max(total),
            0,
            started,
        )? {
            return Ok(WorkerOutcome::Stopped);
        }
    }
    let counts = state
        .database
        .task_item_counts(&task.id)
        .map_err(database_task_failure)?;
    if counts.failed > 0 {
        return Err(vllm_items_failure(counts));
    }
    Ok(WorkerOutcome::Complete(serde_json::json!({
        "successes": successes,
        "retry_manifest": {
            "created_at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            "items": failures,
        }
    })))
}

async fn commit_vllm_wave(
    state: &AppState,
    task_id: &str,
    verified_root: &VerifiedMediaRoot,
    item_keys: &HashMap<String, String>,
    quarantine_by_media: &HashMap<String, QuarantineInput>,
    wave_media_ids: &HashSet<String>,
    result: &VllmBatchResult,
) -> Result<(), TaskFailure> {
    let mut quarantine_entries = result
        .successes
        .iter()
        .filter_map(|success| quarantine_by_media.get(&success.media_id).cloned())
        .collect::<Vec<_>>();
    let registration_result = (|| -> Result<(), TaskFailure> {
        for entry in &mut quarantine_entries {
            let path = verified_root
                .resolve_existing_file(Path::new(&entry.quarantine_relative_path))
                .map_err(tool_task_failure)?;
            let contents = std::fs::read(path).map_err(|error| TaskFailure {
                code: "quarantine_hash_failed".to_string(),
                message: error.to_string(),
                retryable: false,
            })?;
            entry.sha256 = Some(hex::encode(Sha256::digest(contents)));
        }
        state
            .database
            .quarantine_media_batch(&quarantine_entries)
            .map(|_| ())
            .map_err(database_task_failure)
    })();
    if let Err(registration_error) = registration_result {
        let rollback_root = verified_root.path().to_path_buf();
        let rollback_entries = quarantine_entries.clone();
        let rollback = tokio::task::spawn_blocking(move || {
            rollback_vllm_sidecar_replacements(&rollback_root, &rollback_entries)
        })
        .await
        .map_err(join_task_failure)?;
        if let Err(rollback_error) = rollback {
            return Err(TaskFailure {
                code: "vllm_quarantine_rollback_incomplete".to_string(),
                message: format!(
                    "标签隔离登记失败，且文件回滚不完整（{}）: {}; {rollback_error}",
                    registration_error.code, registration_error.message
                ),
                retryable: false,
            });
        }
        return Err(registration_error);
    }
    let mut terminal_media_ids = HashSet::new();
    for success in &result.successes {
        let Some(item_key) = item_keys.get(&success.media_id) else {
            return Err(TaskFailure {
                code: "unexpected_vllm_result".to_string(),
                message: "vLLM 返回了不属于当前队列的媒体结果".to_string(),
                retryable: false,
            });
        };
        let item_result = serde_json::json!({
            "media_ids": [success.media_id.clone()],
            "tags": success.tags.clone(),
            "sidecar_written": success.sidecar_written
        });
        if !state
            .database
            .finish_task_item(task_id, item_key, "completed", Some(&item_result), None)
            .map_err(database_task_failure)?
        {
            return Err(TaskFailure {
                code: "vllm_task_item_commit_failed".to_string(),
                message: format!("无法提交 vLLM 媒体项目: {}", success.media_id),
                retryable: true,
            });
        }
        terminal_media_ids.insert(success.media_id.clone());
    }
    for failure in &result.retry_manifest.items {
        let Some(item_key) = item_keys.get(&failure.media_id) else {
            continue;
        };
        let error = serde_json::json!({
            "code": vllm_item_error_code(failure.code),
            "message": redact_vllm_item_message(&failure.message, verified_root.path()),
            "retryable": failure.retryable
        });
        if !state
            .database
            .finish_task_item(task_id, item_key, "failed", None, Some(&error))
            .map_err(database_task_failure)?
        {
            return Err(TaskFailure {
                code: "vllm_task_item_commit_failed".to_string(),
                message: format!("无法提交 vLLM 媒体失败项目: {}", failure.media_id),
                retryable: true,
            });
        }
        terminal_media_ids.insert(failure.media_id.clone());
    }
    for media_id in wave_media_ids {
        if terminal_media_ids.contains(media_id) {
            continue;
        }
        let error = serde_json::json!({
            "code": "vllm_worker_lost",
            "message": "vLLM 工作线程未返回该媒体的结果",
            "retryable": true
        });
        let item_key = item_keys
            .get(media_id)
            .expect("queued vLLM media has a task item key");
        if !state
            .database
            .finish_task_item(task_id, item_key, "failed", None, Some(&error))
            .map_err(database_task_failure)?
        {
            return Err(TaskFailure {
                code: "vllm_task_item_commit_failed".to_string(),
                message: format!("无法提交丢失的 vLLM 媒体项目: {media_id}"),
                retryable: true,
            });
        }
    }
    Ok(())
}

fn rollback_vllm_sidecar_replacements(
    root_path: &Path,
    entries: &[QuarantineInput],
) -> Result<(), String> {
    let root = VerifiedMediaRoot::open(root_path).map_err(|error| error.to_string())?;
    let quarantine_root = root
        .resolve(Path::new(".danbooru-quarantine"))
        .map_err(|error| error.to_string())?;
    for entry in entries.iter().rev() {
        let quarantined = root
            .resolve_existing_file(Path::new(&entry.quarantine_relative_path))
            .map_err(|error| error.to_string())?;
        let original_relative = Path::new(&entry.original_relative_path);
        let replacement = root
            .resolve(original_relative)
            .map_err(|error| error.to_string())?;
        if replacement.exists() {
            let metadata =
                std::fs::symlink_metadata(&replacement).map_err(|error| error.to_string())?;
            if !metadata.file_type().is_file() || metadata_is_link_or_reparse_point(&metadata) {
                return Err(format!(
                    "拒绝删除不安全的新标签文件: {}",
                    original_relative.display()
                ));
            }
            std::fs::remove_file(&replacement).map_err(|error| error.to_string())?;
        }
        let destination = root
            .resolve(original_relative)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&quarantined, destination).map_err(|error| error.to_string())?;
        let mut parent = quarantined.parent();
        while let Some(directory) = parent {
            if platform_paths_equal(directory, &quarantine_root)
                || !directory.starts_with(&quarantine_root)
            {
                break;
            }
            match std::fs::remove_dir(directory) {
                Ok(()) => parent = directory.parent(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent = directory.parent();
                }
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    break;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    Ok(())
}

fn api_task_failure(error: ApiError) -> TaskFailure {
    TaskFailure {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}

fn vllm_task_failure(error: VllmError) -> TaskFailure {
    TaskFailure {
        code: format!("vllm_{:?}", error.kind).to_ascii_lowercase(),
        message: error.message,
        retryable: error.retryable,
    }
}

fn vllm_item_error_code(kind: VllmErrorKind) -> String {
    let kind = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{kind:?}").to_ascii_lowercase());
    format!("vllm_{kind}")
}

fn redact_vllm_item_message(message: &str, media_root: &Path) -> String {
    let root = media_root.to_string_lossy();
    let mut sanitized = message.to_string();
    let candidates = [
        root.to_string(),
        root.replace('\\', "/"),
        root.replace('/', "\\"),
    ];
    for candidate in candidates {
        if candidate.len() > 2 {
            sanitized = sanitized.replace(&candidate, "<media-root>");
        }
    }
    sanitized.chars().take(4_096).collect()
}

fn vllm_items_failure(counts: crate::database::TaskItemCounts) -> TaskFailure {
    TaskFailure {
        code: "vllm_items_failed".to_string(),
        message: format!(
            "vLLM 打标完成 {} 项，失败 {} 项",
            counts.completed, counts.failed
        ),
        retryable: counts.retryable_failed > 0,
    }
}

async fn run_resize_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let media_ids = validated_task_media_ids(request.options.as_ref()).map_err(api_task_failure)?;
    let max_size = request
        .options
        .as_ref()
        .and_then(|options| options.get("max_size"))
        .and_then(Value::as_u64)
        .unwrap_or(1_216);
    let quality = request
        .options
        .as_ref()
        .and_then(|options| options.get("quality"))
        .and_then(Value::as_u64)
        .unwrap_or(90);
    if !(1..=8_192).contains(&max_size) || !(1..=100).contains(&quality) {
        return Err(TaskFailure {
            code: "invalid_resize_options".to_string(),
            message: "max_size 必须在 1..=8192，quality 必须在 1..=100".to_string(),
            retryable: false,
        });
    }
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = PathBuf::from(current_platform_path(&root).map_err(api_task_failure)?);
    let _root_write = state
        .root_writes
        .acquire(&root_path)
        .await
        .map_err(root_write_task_failure)?;
    if worker_was_stopped(state, &task.id) {
        return Ok(WorkerOutcome::Stopped);
    }
    let mut media_records = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let media = state
            .database
            .get_media_file(&media_id)
            .map_err(database_task_failure)?
            .filter(|media| media.status == "active" && media.root_id == request.root_id)
            .ok_or_else(|| TaskFailure {
                code: "media_not_found".to_string(),
                message: format!("媒体 {media_id} 不存在或不可用"),
                retryable: false,
            })?;
        if !media.mime_type.starts_with("image/") {
            return Err(TaskFailure {
                code: "unsupported_resize_media".to_string(),
                message: format!("媒体 {media_id} 不是可缩放图片"),
                retryable: false,
            });
        }
        media_records.push(media);
    }

    let started = Instant::now();
    let total = media_records.len() as u64;
    let operation = async {
        let mut results = Vec::with_capacity(media_records.len());
        for (index, media) in media_records.into_iter().enumerate() {
            if worker_was_stopped(state, &task.id) {
                return Ok(WorkerOutcome::Stopped);
            }
            let source_relative = PathBuf::from(&media.relative_path);
            let batch_id = format!("resize-{}-{}", task.id, index);
            let worker_root = root_path.clone();
            let worker_batch = batch_id.clone();
            let result = tokio::task::spawn_blocking(move || {
                let root = VerifiedMediaRoot::open(worker_root)?;
                resize_to_jpeg_with_quarantine(
                    &root,
                    &source_relative,
                    max_size as u32,
                    quality as u8,
                    &worker_batch,
                )
            })
            .await
            .map_err(join_task_failure)?
            .map_err(tool_task_failure)?;
            let output_relative = result.output_relative.to_string_lossy().replace('\\', "/");
            let output_size = std::fs::metadata(root_path.join(&result.output_relative))
                .map_err(|error| TaskFailure {
                    code: "resize_output_missing".to_string(),
                    message: error.to_string(),
                    retryable: false,
                })?
                .len();
            let quarantine_entry = QuarantineInput {
                id: uuid::Uuid::new_v4().to_string(),
                root_id: request.root_id.clone(),
                media_file_id: None,
                original_relative_path: media.relative_path.clone(),
                quarantine_relative_path: PathBuf::from(".danbooru-quarantine")
                    .join(&batch_id)
                    .join(&media.relative_path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                reason: "resize_original".to_string(),
                sha256: media.sha256.clone(),
            };
            let replacement = MediaFileInput {
                id: media.id.clone(),
                root_id: media.root_id.clone(),
                post_id: media.post_id,
                relative_path: output_relative,
                variant: media.variant.clone(),
                mime_type: "image/jpeg".to_string(),
                byte_size: i64::try_from(output_size).unwrap_or(i64::MAX),
                sha256: None,
                md5: None,
                width: Some(i64::from(result.width)),
                height: Some(i64::from(result.height)),
                duration: None,
            };
            if let Err(database_error) = state
                .database
                .quarantine_and_replace_media(&quarantine_entry, &replacement)
            {
                let rollback_root = root_path.clone();
                let rollback_output = result.output_relative.clone();
                let rollback_batch = batch_id.clone();
                let rollback = tokio::task::spawn_blocking(move || {
                    let root = VerifiedMediaRoot::open(rollback_root)?;
                    let output = root.resolve(&rollback_output)?;
                    if output.exists() {
                        let checked_output = root.resolve_existing_file(&rollback_output)?;
                        std::fs::remove_file(checked_output).map_err(
                            crate::services::image_processor::ToolError::Io,
                        )?;
                    }
                    restore_batch(&root, &rollback_batch)
                })
                .await
                .map_err(join_task_failure)?
                .map_err(tool_task_failure)?;
                if rollback.restored != 1
                    || rollback.remaining != 0
                    || !rollback.conflicts.is_empty()
                {
                    return Err(TaskFailure {
                        code: "resize_rollback_incomplete".to_string(),
                        message: format!(
                            "缩放数据库写入失败，且文件回滚不完整（已恢复 {}，剩余 {}）: {database_error}",
                            rollback.restored, rollback.remaining
                        ),
                        retryable: false,
                    });
                }
                return Err(database_task_failure(database_error));
            }
            results.push(serde_json::json!({
                "media_id": media.id,
                "relative_path": result.output_relative,
                "width": result.width,
                "height": result.height,
                "quarantine_batch": result.quarantine_batch,
            }));
            if !report_download_progress(state, &task.id, index as u64 + 1, total, 0, started)? {
                return Ok(WorkerOutcome::Stopped);
            }
        }
        Ok(WorkerOutcome::Complete(
            serde_json::json!({ "items": results }),
        ))
    }
    .await;
    operation
}

async fn run_tag_pipeline_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let media_ids = validated_task_media_ids(request.options.as_ref()).map_err(api_task_failure)?;
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = PathBuf::from(current_platform_path(&root).map_err(api_task_failure)?);
    let mut media_paths = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let media = state
            .database
            .get_media_file(&media_id)
            .map_err(database_task_failure)?
            .filter(|media| media.status == "active" && media.root_id == request.root_id)
            .ok_or_else(|| TaskFailure {
                code: "media_not_found".to_string(),
                message: format!("媒体 {media_id} 不存在或不可用"),
                retryable: false,
            })?;
        media_paths.push(PathBuf::from(media.relative_path));
    }

    if let Some(preview) = &task.preview {
        let manifest: ToolManifest =
            serde_json::from_value(preview.clone()).map_err(|error| TaskFailure {
                code: "invalid_tool_manifest".to_string(),
                message: error.to_string(),
                retryable: false,
            })?;
        let _root_write = state
            .root_writes
            .acquire(&root_path)
            .await
            .map_err(root_write_task_failure)?;
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        let worker_root = root_path.clone();
        let manifest_for_apply = manifest.clone();
        let result = tokio::task::spawn_blocking(move || {
            let root = VerifiedMediaRoot::open(worker_root)?;
            apply_tag_pipeline(&root, &manifest_for_apply)
        })
        .await
        .map_err(join_task_failure)?
        .map_err(tool_task_failure)?;
        let entries = result
            .paths
            .iter()
            .map(|relative| {
                let relative_path = relative.to_string_lossy().replace('\\', "/");
                let sha256 = manifest
                    .file_fingerprints
                    .iter()
                    .find(|fingerprint| fingerprint.relative_path == *relative)
                    .map(|fingerprint| fingerprint.sha256.clone());
                QuarantineInput {
                    id: uuid::Uuid::new_v4().to_string(),
                    root_id: request.root_id.clone(),
                    media_file_id: None,
                    original_relative_path: relative_path,
                    quarantine_relative_path: PathBuf::from(".danbooru-quarantine")
                        .join(&result.batch_id)
                        .join(relative)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    reason: "tag_pipeline_original".to_string(),
                    sha256,
                }
            })
            .collect::<Vec<_>>();
        if let Err(database_error) = state.database.quarantine_media_batch(&entries) {
            let rollback_root = root_path;
            let rollback_result = result.clone();
            let rollback = tokio::task::spawn_blocking(move || {
                let root = VerifiedMediaRoot::open(rollback_root)?;
                rollback_tag_pipeline(&root, &rollback_result)
            })
            .await
            .map_err(join_task_failure)?
            .map_err(tool_task_failure)?;
            if rollback.restored != result.changed
                || rollback.remaining != 0
                || !rollback.conflicts.is_empty()
            {
                return Err(TaskFailure {
                    code: "tag_pipeline_rollback_incomplete".to_string(),
                    message: format!(
                        "标签隔离记录写入失败，且文件回滚不完整（已恢复 {}，剩余 {}）: {database_error}",
                        rollback.restored, rollback.remaining
                    ),
                    retryable: false,
                });
            }
            tracing::error!(task_id = %task.id, %database_error, "无法持久化标签流水线隔离记录，文件已回滚");
            return Err(TaskFailure {
                code: "tag_pipeline_persistence_failed".to_string(),
                message: "无法保存标签处理记录，原标签文件已恢复".to_string(),
                retryable: false,
            });
        }
        return Ok(WorkerOutcome::Complete(serde_json::json!({
            "batch_id": result.batch_id,
            "changed": result.changed,
            "paths": result.paths,
        })));
    }
    let artist_prefix = parse_artist_prefix(request.options.as_ref()).map_err(api_task_failure)?;
    let token_root = root_path.clone();
    let token_media = media_paths.clone();
    let tags = tokio::task::spawn_blocking(move || {
        let root = VerifiedMediaRoot::open(token_root)?;
        collect_tag_pipeline_tokens(&root, &token_media)
    })
    .await
    .map_err(join_task_failure)?
    .map_err(tool_task_failure)?;
    let categories = resolve_tag_categories(state, tags).await?;
    let config = TagPipelineConfig {
        artist_prefix,
        categories,
    };
    let mut manifest = tokio::task::spawn_blocking(move || {
        let root = VerifiedMediaRoot::open(root_path)?;
        plan_tag_pipeline_classified(&root, &media_paths, config)
    })
    .await
    .map_err(join_task_failure)?
    .map_err(tool_task_failure)?;
    manifest.batch_id = format!("tag-{}", task.id);
    if manifest.candidates.is_empty() {
        return Ok(WorkerOutcome::Complete(serde_json::json!({
            "changed": 0,
            "paths": [],
        })));
    }
    state
        .tasks
        .await_confirmation(
            &task.id,
            serde_json::to_value(&manifest).map_err(|error| TaskFailure {
                code: "manifest_encoding_failed".to_string(),
                message: error.to_string(),
                retryable: false,
            })?,
        )
        .map_err(task_manager_task_failure)?;
    Ok(WorkerOutcome::Stopped)
}

async fn resolve_tag_categories(
    state: &AppState,
    tags: std::collections::BTreeSet<String>,
) -> Result<BTreeMap<String, Option<i64>>, TaskFailure> {
    let mut categories = BTreeMap::new();
    let mut pending = Vec::new();
    for tag in tags {
        match state
            .database
            .find_known_tag_category(&tag)
            .map_err(database_task_failure)?
        {
            Some(category) => {
                categories.insert(tag, category);
            }
            None => pending.push(tag),
        }
    }

    let client = state.danbooru.read().await.clone();
    let mut pending = pending.into_iter();
    let mut lookups = JoinSet::new();
    for _ in 0..8 {
        let Some(tag) = pending.next() else {
            break;
        };
        let client = client.clone();
        lookups.spawn(async move {
            let result = client.tag_category(&tag).await;
            (tag, result)
        });
    }
    while let Some(joined) = lookups.join_next().await {
        if let Ok((tag, result)) = joined {
            match result {
                Ok(category) => {
                    state
                        .database
                        .set_tag_category(&tag, category)
                        .map_err(database_task_failure)?;
                    categories.insert(tag, category);
                }
                Err(error) => {
                    tracing::warn!(tag = %tag, %error, "Danbooru 标签分类查询失败，本次按普通标签保留");
                    categories.insert(tag, None);
                }
            }
        }
        if let Some(tag) = pending.next() {
            let client = client.clone();
            lookups.spawn(async move {
                let result = client.tag_category(&tag).await;
                (tag, result)
            });
        }
    }
    Ok(categories)
}

async fn run_heic_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let media_ids = validated_task_media_ids(request.options.as_ref()).map_err(api_task_failure)?;
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = PathBuf::from(current_platform_path(&root).map_err(api_task_failure)?);
    let mut media_records = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let media = state
            .database
            .get_media_file(&media_id)
            .map_err(database_task_failure)?
            .filter(|media| media.status == "active" && media.root_id == request.root_id)
            .ok_or_else(|| TaskFailure {
                code: "media_not_found".to_string(),
                message: format!("媒体 {media_id} 不存在或不可用"),
                retryable: false,
            })?;
        if !is_supported_heic_media(&media.relative_path, &media.mime_type) {
            return Err(TaskFailure {
                code: "unsupported_heic_media".to_string(),
                message: format!("媒体 {media_id} 不是已注册的 HEIC/HEIF 图片"),
                retryable: false,
            });
        }
        media_records.push(media);
    }

    if let Some(preview) = &task.preview {
        let manifest: ToolManifest =
            serde_json::from_value(preview.clone()).map_err(|error| TaskFailure {
                code: "invalid_tool_manifest".to_string(),
                message: error.to_string(),
                retryable: false,
            })?;
        let media_by_relative = media_records
            .into_iter()
            .map(|media| (media.relative_path.replace('\\', "/"), media))
            .collect::<HashMap<_, _>>();
        if manifest.candidates.iter().any(|candidate| {
            !media_by_relative
                .contains_key(&candidate.relative_path.to_string_lossy().replace('\\', "/"))
        }) {
            return Err(TaskFailure {
                code: "heic_media_mapping_failed".to_string(),
                message: "HEIC 预检与已注册媒体不一致".to_string(),
                retryable: false,
            });
        }
        let _root_write = state
            .root_writes
            .acquire(&root_path)
            .await
            .map_err(root_write_task_failure)?;
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        let worker_root = root_path.clone();
        let manifest_for_apply = manifest.clone();
        let result = tokio::task::spawn_blocking(move || {
            let root = VerifiedMediaRoot::open(worker_root)?;
            apply_heic_conversion(&root, &manifest_for_apply)
        })
        .await
        .map_err(join_task_failure)?
        .map_err(heic_task_failure)?;
        let mut database_replacements = Vec::with_capacity(result.items.len());
        for item in &result.items {
            let original_relative = item.original_relative.to_string_lossy().replace('\\', "/");
            let media = media_by_relative
                .get(&original_relative)
                .ok_or_else(|| TaskFailure {
                    code: "heic_media_mapping_failed".to_string(),
                    message: "HEIC 转换结果与已注册媒体不一致".to_string(),
                    retryable: false,
                })?;
            let sha256 = manifest
                .file_fingerprints
                .iter()
                .find(|fingerprint| fingerprint.relative_path == item.original_relative)
                .map(|fingerprint| fingerprint.sha256.clone());
            database_replacements.push((
                QuarantineInput {
                    id: uuid::Uuid::new_v4().to_string(),
                    root_id: request.root_id.clone(),
                    media_file_id: None,
                    original_relative_path: original_relative.clone(),
                    quarantine_relative_path: PathBuf::from(".danbooru-quarantine")
                        .join(&result.batch_id)
                        .join(&item.original_relative)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    reason: "heic_original".to_string(),
                    sha256,
                },
                MediaFileInput {
                    id: media.id.clone(),
                    root_id: media.root_id.clone(),
                    post_id: media.post_id,
                    relative_path: item.output_relative.to_string_lossy().replace('\\', "/"),
                    variant: media.variant.clone(),
                    mime_type: "image/jpeg".to_string(),
                    byte_size: i64::try_from(item.byte_size).unwrap_or(i64::MAX),
                    sha256: None,
                    md5: None,
                    width: Some(i64::from(item.width)),
                    height: Some(i64::from(item.height)),
                    duration: None,
                },
            ));
        }
        if let Err(database_error) = state
            .database
            .quarantine_and_replace_media_batch(&database_replacements)
        {
            let rollback_root = root_path;
            let rollback_result = result.clone();
            tokio::task::spawn_blocking(move || {
                let root = VerifiedMediaRoot::open(rollback_root)?;
                rollback_heic_conversion(&root, &rollback_result)
            })
            .await
            .map_err(join_task_failure)?
            .map_err(heic_task_failure)?;
            tracing::error!(task_id = %task.id, %database_error, "无法持久化 HEIC 转换记录，文件已回滚");
            return Err(TaskFailure {
                code: "heic_persistence_failed".to_string(),
                message: "无法保存 HEIC 转换记录，原图已恢复".to_string(),
                retryable: false,
            });
        }
        return Ok(WorkerOutcome::Complete(serde_json::json!({
            "batch_id": result.batch_id,
            "items": result.items.iter().zip(database_replacements.iter()).map(|(item, (_, replacement))| serde_json::json!({
                "media_id": replacement.id,
                "relative_path": item.output_relative,
                "width": item.width,
                "height": item.height,
                "bytes": item.byte_size,
            })).collect::<Vec<_>>(),
        })));
    }

    let media_paths = media_records
        .iter()
        .map(|media| PathBuf::from(&media.relative_path))
        .collect::<Vec<_>>();
    let mut manifest = tokio::task::spawn_blocking(move || {
        let root = VerifiedMediaRoot::open(root_path)?;
        plan_heic_conversion(&root, &media_paths)
    })
    .await
    .map_err(join_task_failure)?
    .map_err(heic_task_failure)?;
    manifest.batch_id = format!("heic-{}", task.id);
    state
        .tasks
        .await_confirmation(
            &task.id,
            serde_json::to_value(&manifest).map_err(|error| TaskFailure {
                code: "manifest_encoding_failed".to_string(),
                message: error.to_string(),
                retryable: false,
            })?,
        )
        .map_err(task_manager_task_failure)?;
    Ok(WorkerOutcome::Stopped)
}

fn is_supported_heic_media(relative_path: &str, mime_type: &str) -> bool {
    let extension = Path::new(relative_path)
        .extension()
        .and_then(|extension| extension.to_str());
    let has_heic_extension = extension.is_some_and(|extension| {
        matches!(extension.to_ascii_lowercase().as_str(), "heic" | "heif")
    });
    let supported_mime = matches!(
        mime_type.trim().to_ascii_lowercase().as_str(),
        "image/heic" | "image/heif" | "application/octet-stream"
    );
    has_heic_extension && supported_mime
}

#[derive(Debug)]
struct IndexedMediaCandidate {
    relative_path: String,
    extension: String,
    byte_size: u64,
    width: Option<u32>,
    height: Option<u32>,
    post_id: Option<u64>,
    score: i64,
    tags: Vec<String>,
}

struct IndexingStatusGuard {
    database: Arc<Database>,
    root: RootRecord,
    armed: bool,
}

impl IndexingStatusGuard {
    fn new(database: Arc<Database>, root: RootRecord) -> Self {
        Self {
            database,
            root,
            armed: true,
        }
    }

    fn mark_indexed(&mut self) -> Result<(), TaskFailure> {
        self.database
            .update_root(
                &self.root.id,
                &self.root.name,
                self.root.windows_path.as_deref(),
                self.root.linux_path.as_deref(),
                "indexed",
            )
            .map_err(database_task_failure)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for IndexingStatusGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = self.database.update_root(
            &self.root.id,
            &self.root.name,
            self.root.windows_path.as_deref(),
            self.root.linux_path.as_deref(),
            "not_indexed",
        ) {
            tracing::error!(root_id = %self.root.id, %error, "无法清理中断的索引状态");
        }
    }
}

async fn run_index_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = current_platform_path(&root)
        .map(PathBuf::from)
        .map_err(|error| TaskFailure {
            code: error.code,
            message: error.message,
            retryable: false,
        })?;
    let _root_write = state
        .root_writes
        .acquire(&root_path)
        .await
        .map_err(root_write_task_failure)?;
    if worker_was_stopped(state, &task.id) {
        return Ok(WorkerOutcome::Stopped);
    }
    state
        .database
        .update_root(
            &root.id,
            &root.name,
            root.windows_path.as_deref(),
            root.linux_path.as_deref(),
            "indexing",
        )
        .map_err(database_task_failure)?;
    let mut indexing_status = IndexingStatusGuard::new(state.database.clone(), root.clone());
    let candidates = tokio::task::spawn_blocking(move || scan_media_root(&root_path))
        .await
        .map_err(join_task_failure)??;
    let scanned_paths = candidates
        .iter()
        .map(|candidate| candidate.relative_path.clone())
        .collect::<HashSet<_>>();
    let total = candidates.len() as u64;
    let mut indexed = 0_u64;
    let started = Instant::now();
    for candidate in candidates {
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        if let Some(post_id) = candidate.post_id {
            let tag_string = candidate.tags.join(" ");
            state
                .database
                .insert_local_post_with_tags_if_missing(
                    &PostRecordInput {
                        id: post_id as i64,
                        md5: None,
                        rating: "unknown".to_string(),
                        score: candidate.score,
                        fav_count: 0,
                        width: i64::from(candidate.width.unwrap_or(0)),
                        height: i64::from(candidate.height.unwrap_or(0)),
                        file_ext: Some(candidate.extension.clone()),
                        file_size: Some(i64::try_from(candidate.byte_size).unwrap_or(i64::MAX)),
                        source: None,
                        duration: None,
                        status: "local".to_string(),
                        tag_string: tag_string.clone(),
                        tag_string_general: tag_string,
                        tag_string_character: String::new(),
                        tag_string_copyright: String::new(),
                        tag_string_artist: String::new(),
                        tag_string_meta: String::new(),
                    },
                    &candidate
                        .tags
                        .iter()
                        .map(|tag| PostTagInput::new(tag, 0))
                        .collect::<Vec<_>>(),
                )
                .map_err(database_task_failure)?;
        }
        let mut id_hash = Sha256::new();
        id_hash.update(request.root_id.as_bytes());
        id_hash.update([0]);
        id_hash.update(candidate.relative_path.as_bytes());
        let id = format!("indexed-{}", hex::encode(id_hash.finalize()));
        state
            .database
            .upsert_media_file(&MediaFileInput {
                id,
                root_id: request.root_id.clone(),
                post_id: candidate.post_id.map(|id| id as i64),
                relative_path: candidate.relative_path,
                variant: "original".to_string(),
                mime_type: media_mime_type(&candidate.extension).to_string(),
                byte_size: i64::try_from(candidate.byte_size).unwrap_or(i64::MAX),
                sha256: None,
                md5: None,
                width: candidate.width.map(i64::from),
                height: candidate.height.map(i64::from),
                duration: None,
            })
            .map_err(database_task_failure)?;
        indexed += 1;
        if !report_download_progress(state, &task.id, indexed, total, 0, started)? {
            return Ok(WorkerOutcome::Stopped);
        }
    }
    let removed = state
        .database
        .remove_missing_active_media_files(&request.root_id, &scanned_paths)
        .map_err(database_task_failure)?;
    indexing_status.mark_indexed()?;
    Ok(WorkerOutcome::Complete(serde_json::json!({
        "indexed": indexed,
        "moved": 0,
        "deleted": removed,
    })))
}

fn scan_media_root(root_path: &Path) -> Result<Vec<IndexedMediaCandidate>, TaskFailure> {
    const EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "gif", "avif", "mp4", "webm", "zip", "heic", "heif",
    ];
    let root = std::fs::canonicalize(root_path).map_err(|error| TaskFailure {
        code: "root_unavailable".to_string(),
        message: error.to_string(),
        retryable: true,
    })?;
    let mut candidates = Vec::new();
    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|error| TaskFailure {
            code: "index_walk_failed".to_string(),
            message: error.to_string(),
            retryable: true,
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(&root).map_err(|_| TaskFailure {
            code: "index_outside_root".to_string(),
            message: "索引路径越过媒体根".to_string(),
            retryable: false,
        })?;
        if relative.components().next().is_some_and(|component| {
            matches!(component, std::path::Component::Normal(name) if is_quarantine_dir_name(name))
        }) {
            continue;
        }
        let extension = relative
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let canonical = std::fs::canonicalize(entry.path()).map_err(|error| TaskFailure {
            code: "index_file_unavailable".to_string(),
            message: error.to_string(),
            retryable: true,
        })?;
        if !canonical.starts_with(&root) {
            return Err(TaskFailure {
                code: "index_symlink_escape".to_string(),
                message: "索引文件通过链接越过媒体根".to_string(),
                retryable: false,
            });
        }
        let metadata = canonical.metadata().map_err(|error| TaskFailure {
            code: "index_metadata_failed".to_string(),
            message: error.to_string(),
            retryable: true,
        })?;
        let (width, height) = image::image_dimensions(&canonical)
            .map(|(width, height)| (Some(width), Some(height)))
            .unwrap_or((None, None));
        let stem = relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let post_id = stem
            .split(|character: char| !character.is_ascii_digit())
            .next()
            .filter(|digits| !digits.is_empty())
            .and_then(|digits| digits.parse().ok());
        let score = stem
            .split_once("_score_")
            .and_then(|(_, tail)| {
                tail.split(|character: char| !character.is_ascii_digit() && character != '-')
                    .next()
            })
            .and_then(|score| score.parse().ok())
            .unwrap_or(0);
        let sidecar = canonical.with_extension("txt");
        let tags = read_sidecar_tags(&root, &sidecar);
        candidates.push(IndexedMediaCandidate {
            relative_path: relative.to_string_lossy().replace('\\', "/"),
            extension,
            byte_size: metadata.len(),
            width,
            height,
            post_id,
            score,
            tags,
        });
    }
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(candidates)
}

fn read_sidecar_tags(root: &Path, sidecar: &Path) -> Vec<String> {
    let Ok(canonical) = std::fs::canonicalize(sidecar) else {
        return Vec::new();
    };
    if !canonical.starts_with(root) {
        return Vec::new();
    }
    std::fs::symlink_metadata(&canonical)
        .ok()
        .filter(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
        .and_then(|_| std::fs::read_to_string(canonical).ok())
        .map(|contents| {
            contents
                .split(|character: char| character == ',' || character.is_whitespace())
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn run_tool_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = current_platform_path(&root)
        .map(PathBuf::from)
        .map_err(|error| TaskFailure {
            code: error.code,
            message: error.message,
            retryable: false,
        })?;
    let selected_media = selected_task_media_paths(state, &request)?;

    match task.kind.as_str() {
        "near_dedup" => {
            let distance = request
                .options
                .as_ref()
                .and_then(|options| options.get("distance"))
                .and_then(Value::as_u64)
                .unwrap_or(8);
            if !(1..=32).contains(&distance) {
                return Err(TaskFailure {
                    code: "invalid_phash_distance".to_string(),
                    message: "感知哈希距离必须在 1..=32".to_string(),
                    retryable: false,
                });
            }
            if let Some(preview) = &task.preview {
                let manifest: ToolManifest =
                    serde_json::from_value(preview.clone()).map_err(|error| TaskFailure {
                        code: "invalid_tool_manifest".to_string(),
                        message: error.to_string(),
                        retryable: false,
                    })?;
                return apply_tool_manifest(state, &task.id, &request.root_id, root_path, manifest)
                    .await;
            }
            let manifest = tokio::task::spawn_blocking(move || {
                let root = VerifiedMediaRoot::open(root_path)?;
                match selected_media {
                    Some(media) => plan_near_duplicates_selected(&root, distance as u32, &media),
                    None => plan_near_duplicates(&root, distance as u32),
                }
            })
            .await
            .map_err(join_task_failure)?
            .map_err(tool_task_failure)?;
            if manifest.candidates.is_empty() {
                return Ok(WorkerOutcome::Complete(serde_json::json!({
                    "batch_id": manifest.batch_id,
                    "moved": 0,
                    "paths": [],
                    "pairs": manifest.pairs,
                })));
            }
            state
                .tasks
                .await_confirmation(
                    &task.id,
                    serde_json::to_value(&manifest).map_err(|error| TaskFailure {
                        code: "manifest_encoding_failed".to_string(),
                        message: error.to_string(),
                        retryable: false,
                    })?,
                )
                .map_err(task_manager_task_failure)?;
            Ok(WorkerOutcome::Stopped)
        }
        "exact_dedup" | "integrity_scan" | "delete_by_tag" => {
            if let Some(preview) = &task.preview {
                let manifest: ToolManifest =
                    serde_json::from_value(preview.clone()).map_err(|error| TaskFailure {
                        code: "invalid_tool_manifest".to_string(),
                        message: error.to_string(),
                        retryable: false,
                    })?;
                apply_tool_manifest(state, &task.id, &request.root_id, root_path, manifest).await
            } else {
                let tag = request
                    .options
                    .as_ref()
                    .and_then(|options| options.get("tag"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let manifest =
                    plan_manifest_in_worker(root_path, &task.kind, tag, selected_media).await?;
                state
                    .tasks
                    .await_confirmation(
                        &task.id,
                        serde_json::to_value(&manifest).map_err(|error| TaskFailure {
                            code: "manifest_encoding_failed".to_string(),
                            message: error.to_string(),
                            retryable: false,
                        })?,
                    )
                    .map_err(task_manager_task_failure)?;
                Ok(WorkerOutcome::Stopped)
            }
        }
        _ => unreachable!("tool kind filtered by caller"),
    }
}

async fn plan_manifest_in_worker(
    root_path: PathBuf,
    kind: &str,
    tag: Option<String>,
    selected_media: Option<Vec<PathBuf>>,
) -> Result<ToolManifest, TaskFailure> {
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        let root = VerifiedMediaRoot::open(root_path)?;
        match kind.as_str() {
            "exact_dedup" => match selected_media.as_deref() {
                Some(media) => plan_exact_duplicates_selected(&root, media),
                None => plan_exact_duplicates(&root),
            },
            "integrity_scan" => match selected_media.as_deref() {
                Some(media) => plan_integrity_check_selected(&root, media),
                None => plan_integrity_check(&root),
            },
            "delete_by_tag" => {
                let tag = tag.as_deref().ok_or_else(|| {
                    crate::services::image_processor::ToolError::InvalidManifest(
                        "按标签隔离任务缺少 tag".to_string(),
                    )
                })?;
                match selected_media.as_deref() {
                    Some(media) => plan_delete_by_tag_selected(&root, tag, media),
                    None => plan_delete_by_tag(&root, tag),
                }
            }
            _ => unreachable!(),
        }
    })
    .await
    .map_err(join_task_failure)?
    .map_err(tool_task_failure)
}

fn selected_task_media_paths(
    state: &AppState,
    request: &CreateTaskRequest,
) -> Result<Option<Vec<PathBuf>>, TaskFailure> {
    if request
        .options
        .as_ref()
        .and_then(|options| options.get("media_ids"))
        .is_none()
    {
        return Ok(None);
    }
    let media_ids = validated_task_media_ids(request.options.as_ref()).map_err(api_task_failure)?;
    let mut paths = Vec::with_capacity(media_ids.len());
    for media_id in media_ids {
        let media = state
            .database
            .get_media_file(&media_id)
            .map_err(database_task_failure)?
            .filter(|media| media.status == "active" && media.root_id == request.root_id)
            .ok_or_else(|| TaskFailure {
                code: "media_not_found".to_string(),
                message: format!("媒体 {media_id} 不存在或不可用"),
                retryable: false,
            })?;
        paths.push(PathBuf::from(media.relative_path));
    }
    Ok(Some(paths))
}

async fn apply_tool_manifest(
    state: &AppState,
    task_id: &str,
    root_id: &str,
    root_path: PathBuf,
    manifest: ToolManifest,
) -> Result<WorkerOutcome, TaskFailure> {
    let _root_write = state
        .root_writes
        .acquire(&root_path)
        .await
        .map_err(root_write_task_failure)?;
    if worker_was_stopped(state, task_id) {
        return Ok(WorkerOutcome::Stopped);
    }

    let operation = async {
        let manifest_for_apply = manifest.clone();
        let root_path_for_apply = root_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            let root = VerifiedMediaRoot::open(root_path_for_apply)?;
            apply_quarantine(&root, &manifest_for_apply)
        })
        .await
        .map_err(join_task_failure)?
        .map_err(tool_task_failure)?;

        let entries = result
            .paths
            .iter()
            .map(|relative| {
                let original_relative_path = relative.to_string_lossy().replace('\\', "/");
                let candidate = manifest.candidates.iter().find(|candidate| {
                    candidate.relative_path == *relative
                        || candidate.companion_paths.contains(relative)
                });
                let media_file_id = state
                    .database
                    .find_media_by_root_path(root_id, &original_relative_path)?
                    .map(|media| media.id);
                let sha256 = manifest
                    .file_fingerprints
                    .iter()
                    .find(|fingerprint| fingerprint.relative_path == *relative)
                    .map(|fingerprint| fingerprint.sha256.clone());
                Ok(QuarantineInput {
                    id: uuid::Uuid::new_v4().to_string(),
                    root_id: root_id.to_string(),
                    media_file_id,
                    original_relative_path,
                    quarantine_relative_path: PathBuf::from(".danbooru-quarantine")
                        .join(&manifest.batch_id)
                        .join(relative)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    reason: candidate
                        .map(|candidate| candidate.reason.clone())
                        .unwrap_or_else(|| "quarantined".to_string()),
                    sha256,
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>();
        let database_result = entries
            .and_then(|entries| state.database.quarantine_media_batch(&entries).map(|_| ()));
        if let Err(database_error) = database_result {
            let batch_id = manifest.batch_id.clone();
            let expected_moved = result.moved;
            let restore_result = tokio::task::spawn_blocking(move || {
                let root = VerifiedMediaRoot::open(root_path)?;
                restore_batch(&root, &batch_id)
            })
            .await
            .map_err(join_task_failure)?
            .map_err(tool_task_failure)?;
            if restore_result.restored != expected_moved
                || restore_result.remaining != 0
                || !restore_result.conflicts.is_empty()
            {
                return Err(TaskFailure {
                    code: "quarantine_rollback_incomplete".to_string(),
                    message: format!(
                        "隔离数据库写入失败，且文件回滚不完整（已恢复 {}，剩余 {}）: {database_error}",
                        restore_result.restored, restore_result.remaining
                    ),
                    retryable: false,
                });
            }
            return Err(database_task_failure(database_error));
        }

        Ok(WorkerOutcome::Complete(serde_json::json!({
            "batch_id": result.batch_id,
            "moved": result.moved,
            "paths": result.paths,
        })))
    }
    .await;
    operation
}

fn tool_task_failure(error: crate::services::image_processor::ToolError) -> TaskFailure {
    TaskFailure {
        code: "tool_error".to_string(),
        message: error.to_string(),
        retryable: false,
    }
}

fn heic_task_failure(error: crate::services::image_processor::ToolError) -> TaskFailure {
    use crate::services::image_processor::ToolError;
    let code = match error {
        ToolError::ConverterUnavailable => "heic_converter_unavailable",
        ToolError::ConversionFailed => "heic_conversion_failed",
        ToolError::ConversionTimedOut => "heic_conversion_timed_out",
        _ => "heic_tool_error",
    };
    TaskFailure {
        code: code.to_string(),
        message: error.to_string(),
        retryable: false,
    }
}

fn join_task_failure(error: tokio::task::JoinError) -> TaskFailure {
    TaskFailure {
        code: "worker_join_error".to_string(),
        message: error.to_string(),
        retryable: true,
    }
}

fn root_write_task_failure(error: std::io::Error) -> TaskFailure {
    TaskFailure {
        code: "root_write_lock_failed".to_string(),
        message: format!("无法锁定媒体根: {error}"),
        retryable: true,
    }
}

fn task_manager_task_failure(error: TaskManagerError) -> TaskFailure {
    match error {
        TaskManagerError::Persistence { .. } => TaskFailure {
            code: "task_persistence_failed".to_string(),
            message: "任务持久化失败".to_string(),
            retryable: true,
        },
        error => TaskFailure {
            code: "task_transition_failed".to_string(),
            message: error.to_string(),
            retryable: false,
        },
    }
}

fn sort_posts_for_download(
    posts: &mut [crate::services::danbooru::Post],
    prioritize_score: bool,
    prioritize_resolution: bool,
) {
    if !prioritize_score && !prioritize_resolution {
        return;
    }
    posts.sort_by(|left, right| {
        let left_pixels = u64::from(left.image_width).saturating_mul(u64::from(left.image_height));
        let right_pixels =
            u64::from(right.image_width).saturating_mul(u64::from(right.image_height));
        prioritize_resolution
            .then(|| right_pixels.cmp(&left_pixels))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                prioritize_score
                    .then(|| right.score.cmp(&left.score))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| right.id.cmp(&left.id))
    });
}

fn meets_minimum_resolution(image_width: u32, image_height: u32, minimum_resolution: u32) -> bool {
    minimum_resolution == 0
        || image_width >= minimum_resolution && image_height >= minimum_resolution
}

fn is_static_image_post(post: &Post) -> bool {
    matches!(
        post.file_ext
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "avif")
    )
}

async fn run_download_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let root = state
        .database
        .get_root(&request.root_id)
        .map_err(database_task_failure)?
        .ok_or_else(|| TaskFailure {
            code: "root_not_found".to_string(),
            message: "媒体根不存在".to_string(),
            retryable: false,
        })?;
    let root_path = current_platform_path(&root)
        .map(PathBuf::from)
        .map_err(|error| TaskFailure {
            code: error.code,
            message: error.message,
            retryable: false,
        })?;
    let verified_root = VerifiedMediaRoot::open(&root_path).map_err(|error| TaskFailure {
        code: "root_unavailable".to_string(),
        message: error.to_string(),
        retryable: true,
    })?;
    let _root_write = state
        .root_writes
        .acquire(verified_root.path())
        .await
        .map_err(root_write_task_failure)?;
    if worker_was_stopped(state, &task.id) {
        return Ok(WorkerOutcome::Stopped);
    }
    let output_dir = if let Some(relative_directory) = request.relative_directory.as_deref() {
        let destination = verified_root
            .resolve(Path::new(relative_directory))
            .map_err(|error| TaskFailure {
                code: "invalid_relative_directory".to_string(),
                message: format!("无法使用下载子文件夹: {error}"),
                retryable: false,
            })?;
        std::fs::create_dir_all(&destination).map_err(|error| TaskFailure {
            code: "directory_create_failed".to_string(),
            message: format!("无法创建下载子文件夹: {error}"),
            retryable: false,
        })?;
        let canonical = std::fs::canonicalize(&destination).map_err(|error| TaskFailure {
            code: "directory_create_failed".to_string(),
            message: format!("无法确认下载子文件夹: {error}"),
            retryable: false,
        })?;
        if !canonical.starts_with(verified_root.path()) || !canonical.is_dir() {
            return Err(TaskFailure {
                code: "download_outside_root".to_string(),
                message: "下载子文件夹越过媒体根".to_string(),
                retryable: false,
            });
        }
        canonical
    } else {
        verified_root.path().to_path_buf()
    };
    let client = state.danbooru.read().await.clone();
    let target = request.limit.unwrap_or(1);
    let concurrency = usize::from(request.concurrency.unwrap_or(1)).clamp(1, 32);
    let template = request.filename_template.clone().unwrap_or_default();
    let skip_existing = request.skip_existing.unwrap_or(true);
    let keep_sidecar_txt = request.keep_sidecar_txt.unwrap_or(true);
    let static_images_only = request.static_images_only.unwrap_or(false);
    let destination = DownloadDestination {
        root_dir: verified_root.path().to_path_buf(),
        output_dir,
        keep_sidecar_txt,
        static_images_only,
    };
    let prioritize_score = request.prioritize_score.unwrap_or(false);
    let prioritize_resolution = request.prioritize_resolution.unwrap_or(false);
    let batch_filter = request.batch_filter.clone();
    let policy = request
        .media_policy
        .as_ref()
        .map(|policy| policy.ugoira)
        .unwrap_or_default();
    let started = Instant::now();
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut failed = 0_u64;
    let mut last_item_failure = None;
    let mut bytes = 0_u64;
    let persisted_counts = state
        .database
        .task_item_counts(&task.id)
        .map_err(database_task_failure)?;
    if persisted_counts.total > 0 {
        downloaded = persisted_counts.completed;
        skipped = persisted_counts.skipped;
        failed = persisted_counts.failed;
        bytes = persisted_counts.completed_bytes;
    }
    if !report_download_progress(state, &task.id, downloaded, target, bytes, started)? {
        return Ok(WorkerOutcome::Stopped);
    }

    match request.source.ok_or_else(|| TaskFailure {
        code: "missing_source".to_string(),
        message: "下载任务缺少 source".to_string(),
        retryable: false,
    })? {
        DownloadSource::PostIds { post_ids } => {
            let tracked_items = state
                .database
                .list_task_items(&task.id)
                .map_err(database_task_failure)?;
            let has_tracked_items = !tracked_items.is_empty();
            let queued_post_ids = tracked_items
                .into_iter()
                .filter(|item| item.status == "queued")
                .filter_map(|item| item.payload.get("post_id").and_then(Value::as_u64))
                .collect::<HashSet<_>>();
            let mut seen = HashSet::new();
            let mut post_ids = post_ids.into_iter().filter(|post_id| {
                seen.insert(*post_id) && (!has_tracked_items || queued_post_ids.contains(post_id))
            });
            let mut workers = JoinSet::new();

            while downloaded < target {
                while workers.len() < concurrency
                    && downloaded.saturating_add(workers.len() as u64) < target
                {
                    let Some(post_id) = post_ids.next() else {
                        break;
                    };
                    let worker_state = state.clone();
                    let worker_client = client.clone();
                    let task_id = task.id.clone();
                    let task_started = started;
                    let root_id = request.root_id.clone();
                    let destination = destination.clone();
                    let template = template.clone();
                    workers.spawn(async move {
                        download_post_id(
                            worker_state,
                            worker_client,
                            task_id,
                            task_started,
                            root_id,
                            destination,
                            template,
                            policy,
                            skip_existing,
                            post_id,
                        )
                        .await
                    });
                }

                let Some(result) = workers.join_next().await else {
                    break;
                };
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        workers.abort_all();
                        return Err(join_task_failure(error));
                    }
                };
                match result {
                    Ok(PostDownloadOutcome::Downloaded(downloaded_bytes)) => {
                        downloaded += 1;
                        bytes = bytes.saturating_add(downloaded_bytes);
                        if !report_download_progress(
                            state, &task.id, downloaded, target, bytes, started,
                        )? {
                            workers.abort_all();
                            return Ok(WorkerOutcome::Stopped);
                        }
                    }
                    Ok(PostDownloadOutcome::Skipped) => skipped += 1,
                    Ok(PostDownloadOutcome::Stopped) => {
                        workers.abort_all();
                        return Ok(WorkerOutcome::Stopped);
                    }
                    Err(error) => {
                        failed = failed.saturating_add(1);
                        last_item_failure = Some(error);
                    }
                }
            }
        }
        DownloadSource::Query { query } => {
            let mut active_query = query.clone();
            let mut custom_order = query
                .split_whitespace()
                .any(|token| token.starts_with("order:"));
            let mut segmented_filter: Option<(BatchDownloadFilter, String)> = None;
            let mut page_token = "1".to_string();
            for _ in 0..1_000 {
                if downloaded >= target {
                    break;
                }
                if worker_was_stopped(state, &task.id) {
                    return Ok(WorkerOutcome::Stopped);
                }
                let page = match client
                    .posts(&PostQuery {
                        tags: active_query.clone(),
                        page: page_token.clone(),
                        limit: 200,
                    })
                    .await
                {
                    Ok(page) => page,
                    Err(error)
                        if error.kind == DanbooruErrorKind::TagLimit
                            && segmented_filter.is_none()
                            && batch_filter.is_some() =>
                    {
                        let filter = batch_filter
                            .as_ref()
                            .expect("batch filter checked above")
                            .clone();
                        let anchor_tag = select_segmented_batch_anchor(&client, &filter).await;
                        tracing::info!(
                            task_id = %task.id,
                            anchor_tag,
                            "查询标签超额，切换到分段远程验证"
                        );
                        active_query = segmented_batch_anchor_query(&filter, &anchor_tag);
                        custom_order = false;
                        page_token = "1".to_string();
                        segmented_filter = Some((filter, anchor_tag));
                        continue;
                    }
                    Err(error) => return Err(danbooru_task_failure(error)),
                };
                if page.posts.is_empty() {
                    break;
                }
                let last_id = page.posts.last().map(|post| post.id);
                let tracked_post_statuses = state
                    .database
                    .list_task_items(&task.id)
                    .map_err(database_task_failure)?
                    .into_iter()
                    .filter_map(|item| {
                        item.payload
                            .get("post_id")
                            .and_then(Value::as_u64)
                            .map(|post_id| (post_id, item.status))
                    })
                    .collect::<HashMap<_, _>>();
                let candidates = page
                    .posts
                    .into_iter()
                    .filter(|post| {
                        tracked_post_statuses
                            .get(&post.id)
                            .is_none_or(|status| status == "queued")
                    })
                    .collect::<Vec<_>>();
                let mut posts = if let Some((filter, anchor_tag)) = segmented_filter.as_ref() {
                    verify_segmented_batch_candidates(&client, candidates, filter, anchor_tag)
                        .await?
                } else {
                    candidates
                };
                let minimum_resolution = batch_filter
                    .as_ref()
                    .map_or(0, |filter| filter.minimum_resolution);
                posts.retain(|post| {
                    meets_minimum_resolution(
                        post.image_width,
                        post.image_height,
                        minimum_resolution,
                    ) && (!static_images_only || is_static_image_post(post))
                });
                sort_posts_for_download(&mut posts, prioritize_score, prioritize_resolution);
                let mut posts = posts.into_iter();
                let mut workers = JoinSet::new();
                while downloaded < target {
                    while workers.len() < concurrency
                        && downloaded.saturating_add(workers.len() as u64) < target
                    {
                        let Some(post) = posts.next() else {
                            break;
                        };
                        state
                            .database
                            .ensure_task_items(
                                &task.id,
                                &[TaskItemInput {
                                    item_key: format!("post:{}", post.id),
                                    status: "queued".to_string(),
                                    payload: serde_json::json!({ "post_id": post.id }),
                                    result: None,
                                    error: None,
                                    attempts: 0,
                                }],
                            )
                            .map_err(database_task_failure)?;
                        let worker_state = state.clone();
                        let worker_client = client.clone();
                        let task_id = task.id.clone();
                        let task_started = started;
                        let root_id = request.root_id.clone();
                        let destination = destination.clone();
                        let template = template.clone();
                        workers.spawn(async move {
                            download_tracked_known_post(
                                worker_state,
                                worker_client,
                                task_id,
                                task_started,
                                root_id,
                                destination,
                                template,
                                policy,
                                skip_existing,
                                post,
                            )
                            .await
                        });
                    }

                    let Some(result) = workers.join_next().await else {
                        break;
                    };
                    let result = match result {
                        Ok(result) => result,
                        Err(error) => {
                            workers.abort_all();
                            return Err(join_task_failure(error));
                        }
                    };
                    match result {
                        Ok(PostDownloadOutcome::Downloaded(downloaded_bytes)) => {
                            downloaded += 1;
                            bytes = bytes.saturating_add(downloaded_bytes);
                            if !report_download_progress(
                                state, &task.id, downloaded, target, bytes, started,
                            )? {
                                workers.abort_all();
                                return Ok(WorkerOutcome::Stopped);
                            }
                        }
                        Ok(PostDownloadOutcome::Skipped) => skipped += 1,
                        Ok(PostDownloadOutcome::Stopped) => {
                            workers.abort_all();
                            return Ok(WorkerOutcome::Stopped);
                        }
                        Err(error) => {
                            failed = failed.saturating_add(1);
                            last_item_failure = Some(error);
                        }
                    }
                }
                page_token = if custom_order {
                    page_token
                        .parse::<u64>()
                        .unwrap_or(1)
                        .saturating_add(1)
                        .to_string()
                } else if let Some(last_id) = last_id {
                    format!("b{last_id}")
                } else {
                    break;
                };
            }
        }
    }

    if worker_was_stopped(state, &task.id) {
        return Ok(WorkerOutcome::Stopped);
    }
    if downloaded < target {
        if let Some(failure) = last_item_failure {
            return Err(failure);
        }
        if failed > 0 {
            return Err(TaskFailure {
                code: "download_items_failed".to_string(),
                message: format!("{failed} 个下载项目失败，无法达到成功新增目标"),
                retryable: false,
            });
        }
    }
    if downloaded != target
        && !report_download_progress(state, &task.id, downloaded, downloaded, bytes, started)?
    {
        return Ok(WorkerOutcome::Stopped);
    }
    Ok(WorkerOutcome::Complete(serde_json::json!({
        "downloaded": downloaded,
        "skipped": skipped,
        "failed": failed,
        "bytes": bytes,
    })))
}

#[derive(Debug)]
enum PostDownloadOutcome {
    Downloaded(u64),
    Skipped,
    Stopped,
}

struct DownloadedMediaRegistration {
    record: MediaFileInput,
    newly_created: bool,
}

#[derive(Debug, Clone)]
struct DownloadDestination {
    root_dir: PathBuf,
    output_dir: PathBuf,
    keep_sidecar_txt: bool,
    static_images_only: bool,
}

#[allow(clippy::too_many_arguments)]
async fn download_post_id(
    state: AppState,
    client: DanbooruClient,
    task_id: String,
    task_started: Instant,
    root_id: String,
    destination: DownloadDestination,
    template: String,
    policy: crate::config::UgoiraPolicy,
    skip_existing: bool,
    post_id: u64,
) -> Result<PostDownloadOutcome, TaskFailure> {
    if worker_was_stopped(&state, &task_id) {
        return Ok(PostDownloadOutcome::Stopped);
    }
    let outcome = async {
        let post = client.post(post_id).await.map_err(danbooru_task_failure)?;
        download_known_post(
            state.clone(),
            client,
            task_id.clone(),
            task_started,
            root_id,
            destination,
            template,
            policy,
            skip_existing,
            post,
        )
        .await
    }
    .await;
    persist_download_task_item(&state, &task_id, post_id, &outcome)?;
    outcome
}

fn persist_download_task_item(
    state: &AppState,
    task_id: &str,
    post_id: u64,
    outcome: &Result<PostDownloadOutcome, TaskFailure>,
) -> Result<(), TaskFailure> {
    let item_key = format!("post:{post_id}");
    match outcome {
        Ok(PostDownloadOutcome::Downloaded(_)) => {}
        Ok(PostDownloadOutcome::Skipped) => {
            state
                .database
                .finish_task_item(
                    task_id,
                    &item_key,
                    "skipped",
                    Some(&serde_json::json!({ "reason": "already_exists" })),
                    None,
                )
                .map_err(database_task_failure)?;
        }
        Ok(PostDownloadOutcome::Stopped) => {}
        Err(failure) => {
            let error = serde_json::to_value(failure).map_err(|error| TaskFailure {
                code: "task_item_encoding_failed".to_string(),
                message: error.to_string(),
                retryable: false,
            })?;
            state
                .database
                .finish_task_item(task_id, &item_key, "failed", None, Some(&error))
                .map_err(database_task_failure)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_tracked_known_post(
    state: AppState,
    client: DanbooruClient,
    task_id: String,
    task_started: Instant,
    root_id: String,
    destination: DownloadDestination,
    template: String,
    policy: crate::config::UgoiraPolicy,
    skip_existing: bool,
    post: Post,
) -> Result<PostDownloadOutcome, TaskFailure> {
    let post_id = post.id;
    let outcome = download_known_post(
        state.clone(),
        client,
        task_id.clone(),
        task_started,
        root_id,
        destination,
        template,
        policy,
        skip_existing,
        post,
    )
    .await;
    persist_download_task_item(&state, &task_id, post_id, &outcome)?;
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn download_known_post(
    state: AppState,
    client: DanbooruClient,
    task_id: String,
    task_started: Instant,
    root_id: String,
    destination: DownloadDestination,
    template: String,
    policy: crate::config::UgoiraPolicy,
    skip_existing: bool,
    post: Post,
) -> Result<PostDownloadOutcome, TaskFailure> {
    if worker_was_stopped(&state, &task_id) {
        return Ok(PostDownloadOutcome::Stopped);
    }
    if destination.static_images_only && !is_static_image_post(&post) {
        return Ok(PostDownloadOutcome::Skipped);
    }
    download_post_to_destination(
        &state,
        &client,
        Some(&task_id),
        Some(task_started),
        &root_id,
        &destination,
        &template,
        policy,
        skip_existing,
        &post,
    )
    .await
}

fn worker_was_stopped(state: &AppState, task_id: &str) -> bool {
    match state.tasks.get(task_id) {
        Ok(task) => task.is_none_or(|task| task.status != TaskStatus::Running),
        Err(error) => {
            tracing::error!(task_id, %error, "无法读取任务运行状态");
            true
        }
    }
}

fn report_download_progress(
    state: &AppState,
    task_id: &str,
    completed: u64,
    total: u64,
    bytes: u64,
    started: Instant,
) -> Result<bool, TaskFailure> {
    let speed = task_average_speed(bytes, started.elapsed());
    for _ in 0..4 {
        let task = match state.tasks.get(task_id) {
            Ok(Some(task)) => task,
            Ok(None) => return Ok(false),
            Err(error) => return Err(task_manager_task_failure(error)),
        };
        match task.status {
            TaskStatus::Queued => match state.tasks.start(task_id) {
                Ok(_) | Err(TaskManagerError::InvalidTransition { .. }) => continue,
                Err(TaskManagerError::NotFound) => return Ok(false),
                Err(error @ TaskManagerError::Persistence { .. }) => {
                    return Err(task_manager_task_failure(error));
                }
            },
            TaskStatus::Running => match state
                .tasks
                .progress(task_id, completed, total, bytes, speed)
            {
                Ok(_) => return Ok(true),
                Err(TaskManagerError::InvalidTransition { .. }) => continue,
                Err(TaskManagerError::NotFound) => return Ok(false),
                Err(error @ TaskManagerError::Persistence { .. }) => {
                    return Err(task_manager_task_failure(error));
                }
            },
            TaskStatus::Pausing
            | TaskStatus::Paused
            | TaskStatus::Cancelling
            | TaskStatus::AwaitingConfirmation
            | TaskStatus::Completed
            | TaskStatus::Failed
            | TaskStatus::Cancelled => return Ok(false),
        }
    }
    Err(TaskFailure {
        code: "task_progress_race".to_string(),
        message: "任务状态在进度提交期间持续变化".to_string(),
        retryable: true,
    })
}

fn task_average_speed(bytes: u64, elapsed: Duration) -> u64 {
    let elapsed_seconds = elapsed.as_secs_f64().max(0.001);
    (bytes as f64 / elapsed_seconds) as u64
}

fn report_download_chunk_progress(
    state: &AppState,
    task_id: &str,
    baseline_bytes: u64,
    progress: DownloadProgress,
    task_started: Instant,
) -> DownloadControl {
    let snapshot = match state.tasks.get(task_id) {
        Ok(Some(task)) if task.status == TaskStatus::Running => task,
        Ok(_) => return DownloadControl::Stop,
        Err(error) => {
            tracing::error!(task_id, %error, "无法读取流式下载任务状态");
            return DownloadControl::Stop;
        }
    };
    let bytes = snapshot
        .bytes_processed
        .max(baseline_bytes.saturating_add(progress.bytes_written));
    let speed = task_average_speed(bytes, task_started.elapsed());
    // Use the task-wide average speed, not the speed of this individual request.
    // For the first item there is no completed-item average yet, so the current
    // response size is the only sensible estimate.  This still gives the UI an
    // ETA while the first transfer is in flight without letting concurrent
    // downloads overwrite the rate with their own short-lived measurements.
    let eta = (speed > 0).then(|| {
        let total_items = snapshot
            .total_items
            .unwrap_or(snapshot.completed_items.saturating_add(1));
        let remaining_after_current =
            total_items.saturating_sub(snapshot.completed_items.saturating_add(1));
        let current_remaining = progress
            .total_bytes
            .unwrap_or(progress.bytes_written)
            .saturating_sub(progress.bytes_written);
        let average_item_bytes = if snapshot.completed_items > 0 {
            snapshot.bytes_processed / snapshot.completed_items
        } else {
            progress.total_bytes.unwrap_or(progress.bytes_written)
        };
        let estimated_remaining_bytes = current_remaining
            .saturating_add(average_item_bytes.saturating_mul(remaining_after_current));
        estimated_remaining_bytes.div_ceil(speed)
    });
    match state.tasks.stream_progress(task_id, bytes, speed, eta) {
        Ok(_) => DownloadControl::Continue,
        Err(TaskManagerError::InvalidTransition { .. } | TaskManagerError::NotFound) => {
            DownloadControl::Stop
        }
        Err(error @ TaskManagerError::Persistence { .. }) => {
            tracing::error!(task_id, %error, "无法保存流式下载进度");
            DownloadControl::Stop
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn download_post(
    state: &AppState,
    client: &DanbooruClient,
    task_id: Option<&str>,
    root_id: &str,
    destination_dir: &Path,
    template: &str,
    policy: crate::config::UgoiraPolicy,
    skip_existing: bool,
    post: &Post,
) -> Result<PostDownloadOutcome, TaskFailure> {
    let destination = DownloadDestination {
        root_dir: destination_dir.to_path_buf(),
        output_dir: destination_dir.to_path_buf(),
        keep_sidecar_txt: true,
        static_images_only: false,
    };
    download_post_to_destination(
        state,
        client,
        task_id,
        None,
        root_id,
        &destination,
        template,
        policy,
        skip_existing,
        post,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn download_post_to_destination(
    state: &AppState,
    client: &DanbooruClient,
    task_id: Option<&str>,
    task_started: Option<Instant>,
    root_id: &str,
    destination: &DownloadDestination,
    template: &str,
    policy: crate::config::UgoiraPolicy,
    skip_existing: bool,
    post: &Post,
) -> Result<PostDownloadOutcome, TaskFailure> {
    let verified_root =
        VerifiedMediaRoot::open(&destination.root_dir).map_err(|error| TaskFailure {
            code: "root_unavailable".to_string(),
            message: error.to_string(),
            retryable: true,
        })?;
    let destination_dir =
        std::fs::canonicalize(&destination.output_dir).map_err(|error| TaskFailure {
            code: "directory_unavailable".to_string(),
            message: format!("下载子文件夹当前不可访问: {error}"),
            retryable: true,
        })?;
    if !destination_dir.starts_with(verified_root.path()) || !destination_dir.is_dir() {
        return Err(TaskFailure {
            code: "download_outside_root".to_string(),
            message: "下载子文件夹越过媒体根".to_string(),
            retryable: false,
        });
    }
    let mut sources = if post.file_ext.as_deref() == Some("zip") {
        match policy {
            crate::config::UgoiraPolicy::WebmAndZip => vec![
                client
                    .resolve_media(post, MediaVariant::UgoiraWebm)
                    .map_err(danbooru_task_failure)?,
                client
                    .resolve_media(post, MediaVariant::UgoiraZip)
                    .map_err(danbooru_task_failure)?,
            ],
            crate::config::UgoiraPolicy::WebmOnly => vec![client
                .resolve_media(post, MediaVariant::UgoiraWebm)
                .map_err(danbooru_task_failure)?],
            crate::config::UgoiraPolicy::ZipOnly => vec![client
                .resolve_media(post, MediaVariant::UgoiraZip)
                .map_err(danbooru_task_failure)?],
        }
    } else {
        client
            .recommended_media(post)
            .map_err(danbooru_task_failure)?
    };
    if skip_existing {
        let mut missing = Vec::with_capacity(sources.len());
        for source in sources {
            let existing = state
                .database
                .find_active_media_for_download(
                    root_id,
                    Some(post.id as i64),
                    source.expected_md5.as_deref(),
                    media_variant_name(source.variant),
                )
                .map_err(database_task_failure)?;
            if let Some(media) = existing {
                if let Ok(path) =
                    verified_root.resolve_existing_file(Path::new(&media.relative_path))
                {
                    client
                        .validate_existing_media(&source, &path)
                        .await
                        .map_err(danbooru_task_failure)?;
                    continue;
                }
            }
            missing.push(source);
        }
        sources = missing;
    }
    if sources.is_empty() {
        return Ok(PostDownloadOutcome::Skipped);
    }
    let mut downloaded_files = Vec::with_capacity(sources.len());
    let mut total_bytes = 0_u64;
    let mut created_new_file = false;
    for source in sources {
        let variant = source.variant;
        let md5 = source.expected_md5.clone();
        let extension = source.extension.clone();
        let transfer_started = task_started.unwrap_or_else(Instant::now);
        let baseline_bytes = task_id
            .and_then(|id| state.tasks.get(id).ok().flatten())
            .map_or(0, |task| task.bytes_processed);
        let transfer = client
            .download_with_control(
                &MediaDownloadRequest {
                    source,
                    destination_dir: destination_dir.clone(),
                    filename_template: template.to_string(),
                    score: post.score,
                    rating: normalized_post_rating(&post.rating),
                },
                |progress| {
                    task_id.map_or(DownloadControl::Continue, |task_id| {
                        report_download_chunk_progress(
                            state,
                            task_id,
                            baseline_bytes,
                            progress,
                            transfer_started,
                        )
                    })
                },
            )
            .await;
        let outcome = match transfer {
            Ok(ControlledDownloadOutcome::Completed(outcome)) => outcome,
            Ok(ControlledDownloadOutcome::Stopped { part_path, .. }) => {
                if task_id.is_some_and(|id| task_is_cancelling(state, id)) {
                    if let Err(error) = std::fs::remove_file(&part_path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            tracing::warn!(path = %part_path.display(), %error, "取消下载后无法清理临时文件");
                        }
                    }
                }
                if let Err(rollback_error) =
                    rollback_new_download_files(verified_root.path(), &downloaded_files)
                {
                    return Err(TaskFailure {
                        code: "download_pause_rollback_incomplete".to_string(),
                        message: format!("下载停止后先前新文件回滚不完整（{rollback_error}）"),
                        retryable: false,
                    });
                }
                return Ok(PostDownloadOutcome::Stopped);
            }
            Err(error) => {
                let failure = danbooru_task_failure(error);
                if let Err(rollback_error) =
                    rollback_new_download_files(verified_root.path(), &downloaded_files)
                {
                    return Err(TaskFailure {
                        code: "download_variant_rollback_incomplete".to_string(),
                        message: format!(
                            "下载后续媒体版本失败，且先前新文件回滚不完整（{rollback_error}）: {}",
                            failure.message
                        ),
                        retryable: false,
                    });
                }
                return Err(failure);
            }
        };
        if !outcome.already_present {
            created_new_file = true;
            total_bytes = total_bytes.saturating_add(outcome.bytes_written);
        }
        let relative_path = outcome
            .path
            .strip_prefix(verified_root.path())
            .map_err(|_| TaskFailure {
                code: "download_outside_root".to_string(),
                message: "下载结果越过媒体根".to_string(),
                retryable: false,
            })?
            .to_string_lossy()
            .replace('\\', "/");
        downloaded_files.push(DownloadedMediaRegistration {
            record: MediaFileInput {
                id: format!("{root_id}:{}:{}", post.id, media_variant_name(variant)),
                root_id: root_id.to_string(),
                post_id: Some(post.id as i64),
                relative_path,
                variant: media_variant_name(variant).to_string(),
                mime_type: media_mime_type(&extension).to_string(),
                byte_size: i64::try_from(outcome.bytes_written).unwrap_or(i64::MAX),
                sha256: None,
                md5,
                width: Some(i64::from(post.image_width)),
                height: Some(i64::from(post.image_height)),
                duration: post.duration,
            },
            newly_created: !outcome.already_present,
        });
    }

    let tags = post_tag_inputs(post);
    let media_records = downloaded_files
        .iter()
        .map(|download| download.record.clone())
        .collect::<Vec<_>>();
    let database_result = if let Some(task_id) = task_id {
        let (item_status, item_result) = if created_new_file {
            ("completed", serde_json::json!({ "bytes": total_bytes }))
        } else {
            ("skipped", serde_json::json!({ "reason": "already_exists" }))
        };
        state
            .database
            .register_downloaded_post_and_finish_task_item(
                task_id,
                &format!("post:{}", post.id),
                item_status,
                &item_result,
                &post_record_input(post),
                &tags,
                &media_records,
            )
    } else {
        state
            .database
            .register_downloaded_post(&post_record_input(post), &tags, &media_records)
    };
    if let Err(database_error) = database_result {
        if let Err(rollback_error) =
            rollback_new_download_files(verified_root.path(), &downloaded_files)
        {
            return Err(TaskFailure {
                code: "download_database_rollback_incomplete".to_string(),
                message: format!(
                    "下载数据库写入失败，且新文件回滚不完整（{rollback_error}）: {database_error}"
                ),
                retryable: false,
            });
        }
        return Err(database_task_failure(database_error));
    }
    if !destination.keep_sidecar_txt {
        remove_download_sidecars(&verified_root, &downloaded_files);
    }
    Ok(if created_new_file {
        PostDownloadOutcome::Downloaded(total_bytes)
    } else {
        PostDownloadOutcome::Skipped
    })
}

fn task_is_cancelling(state: &AppState, task_id: &str) -> bool {
    state
        .tasks
        .get(task_id)
        .ok()
        .flatten()
        .is_some_and(|task| task.status == TaskStatus::Cancelling)
}

fn remove_download_sidecars(root: &VerifiedMediaRoot, downloads: &[DownloadedMediaRegistration]) {
    for download in downloads.iter().filter(|item| item.newly_created) {
        let sidecar_relative = PathBuf::from(&download.record.relative_path).with_extension("txt");
        let sidecar = match root.resolve(&sidecar_relative) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(
                    path = %sidecar_relative.display(),
                    %error,
                    "无法解析需清理的同名 TXT 标签文件"
                );
                continue;
            }
        };
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata_is_link_or_reparse_point(&metadata) =>
            {
                if let Err(error) = std::fs::remove_file(&sidecar) {
                    tracing::warn!(path = %sidecar_relative.display(), %error, "无法清理同名 TXT 标签文件");
                }
            }
            Ok(_) => tracing::warn!(
                path = %sidecar_relative.display(),
                "拒绝清理不安全的同名 TXT 标签路径"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %sidecar_relative.display(),
                %error,
                "无法检查同名 TXT 标签文件"
            ),
        }
    }
}

fn rollback_new_download_files(
    destination_dir: &Path,
    downloads: &[DownloadedMediaRegistration],
) -> Result<(), String> {
    let root = VerifiedMediaRoot::open(destination_dir).map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for download in downloads.iter().rev().filter(|item| item.newly_created) {
        let relative = Path::new(&download.record.relative_path);
        let candidate = match root.resolve(relative) {
            Ok(path) => path,
            Err(error) => {
                failures.push(format!("{}: {error}", download.record.relative_path));
                continue;
            }
        };
        if !candidate.exists() {
            continue;
        }
        match root
            .resolve_existing_file(relative)
            .and_then(|path| std::fs::remove_file(path).map_err(Into::into))
        {
            Ok(()) => {}
            Err(error) => failures.push(format!("{}: {error}", download.record.relative_path)),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn post_record_input(post: &Post) -> PostRecordInput {
    PostRecordInput {
        id: post.id as i64,
        md5: post.md5.clone(),
        rating: normalized_post_rating(&post.rating),
        score: post.score,
        fav_count: i64::try_from(post.fav_count).unwrap_or(i64::MAX),
        width: i64::from(post.image_width),
        height: i64::from(post.image_height),
        file_ext: post.file_ext.clone(),
        file_size: post
            .file_size
            .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
        source: (!post.source.is_empty()).then(|| post.source.clone()),
        duration: post.duration,
        status: if post.file_url.is_some() {
            "available"
        } else {
            "restricted"
        }
        .to_string(),
        tag_string: post.tag_string.clone(),
        tag_string_general: post.tag_string_general.clone(),
        tag_string_character: post.tag_string_character.clone(),
        tag_string_copyright: post.tag_string_copyright.clone(),
        tag_string_artist: post.tag_string_artist.clone(),
        tag_string_meta: post.tag_string_meta.clone(),
    }
}

fn post_tag_inputs(post: &Post) -> Vec<PostTagInput> {
    let mut tags = Vec::new();
    for (source, category) in [
        (&post.tag_string_general, 0),
        (&post.tag_string_artist, 1),
        (&post.tag_string_copyright, 3),
        (&post.tag_string_character, 4),
        (&post.tag_string_meta, 5),
    ] {
        tags.extend(
            source
                .split_whitespace()
                .map(|tag| PostTagInput::new(tag, category)),
        );
    }
    tags
}

fn normalized_post_rating(rating: &str) -> String {
    match rating {
        "g" | "s" | "q" | "e" => rating.to_string(),
        _ => "unknown".to_string(),
    }
}

fn media_variant_name(variant: MediaVariant) -> &'static str {
    match variant {
        MediaVariant::Preview => "preview",
        MediaVariant::Sample => "sample",
        MediaVariant::Large => "large",
        MediaVariant::Original => "original",
        MediaVariant::UgoiraWebm => "ugoira_webm",
        MediaVariant::UgoiraZip => "ugoira_zip",
    }
}

fn media_mime_type(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn danbooru_task_failure(error: DanbooruError) -> TaskFailure {
    TaskFailure {
        code: danbooru_error_code(error.kind).to_string(),
        message: error.message,
        retryable: error.retryable,
    }
}

fn database_task_failure(error: rusqlite::Error) -> TaskFailure {
    TaskFailure {
        code: "database_error".to_string(),
        message: error.to_string(),
        retryable: false,
    }
}

#[derive(Debug, serde::Deserialize)]
struct TaskEventsQuery {
    #[serde(default)]
    after: u64,
}

#[derive(Debug, Serialize)]
struct TaskEventResponse {
    sequence: u64,
    task_id: String,
    revision: u64,
    event_type: &'static str,
    task: TaskSummaryResponse,
}

#[derive(Debug, Serialize)]
struct TaskResyncResponse {
    sequence: u64,
    reason: &'static str,
}

async fn task_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TaskEventsQuery>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let tasks = state.tasks.clone();
    let database = state.database.clone();
    let mut receiver = tasks.subscribe();
    let requested_sequence = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(query.after);
    let stream = async_stream::stream! {
        let mut last_sequence = requested_sequence;
        loop {
            let replay = tasks.replay_after(last_sequence);
            if replay.requires_resync {
                last_sequence = replay.latest_sequence;
                yield Ok(task_sse_resync_event(last_sequence));
                continue;
            }
            for event in replay.events {
                if event.sequence <= last_sequence {
                    continue;
                }
                last_sequence = event.sequence;
                yield Ok(task_sse_event(&database, event));
            }

            match receiver.recv().await {
                Ok(event) if event.sequence > last_sequence => {
                    last_sequence = event.sequence;
                    yield Ok(task_sse_event(&database, event));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn task_sse_resync_event(sequence: u64) -> Event {
    Event::default()
        .event("resync")
        .id(sequence.to_string())
        .json_data(TaskResyncResponse {
            sequence,
            reason: "replay_gap",
        })
        .expect("task resync event serialization is infallible")
}

fn task_sse_event(database: &Database, event: crate::tasks::TaskEvent) -> Event {
    let response = TaskEventResponse {
        sequence: event.sequence,
        task_id: event.task_id,
        revision: event.revision,
        event_type: if event.event == "created" {
            "created"
        } else {
            "updated"
        },
        task: task_summary_response(database, event.task),
    };
    Event::default()
        .id(response.sequence.to_string())
        .json_data(response)
        .expect("task event serialization is infallible")
}

#[derive(Debug, serde::Deserialize)]
struct DanbooruPostsQuery {
    #[serde(default, rename = "q")]
    query: String,
    #[serde(default = "default_page")]
    page: String,
    #[serde(default = "default_post_limit")]
    limit: u16,
}

fn default_page() -> String {
    "1".to_string()
}

fn default_post_limit() -> u16 {
    40
}

#[derive(Debug, Serialize)]
struct DanbooruPostsResponse {
    posts: Vec<DanbooruPostResponse>,
    page: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_page: Option<String>,
}

#[derive(Debug, Serialize)]
struct DanbooruPostResponse {
    id: u64,
    rating: String,
    score: i64,
    fav_count: u64,
    image_width: u32,
    image_height: u32,
    file_ext: String,
    file_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    is_video: bool,
    is_ugoira: bool,
    restricted: bool,
    downloaded: bool,
    tags: DanbooruTagsResponse,
}

#[derive(Debug, Serialize)]
struct DanbooruTagsResponse {
    general: Vec<String>,
    artist: Vec<String>,
    copyright: Vec<String>,
    character: Vec<String>,
    meta: Vec<String>,
}

async fn danbooru_posts(
    State(state): State<AppState>,
    Query(query): Query<DanbooruPostsQuery>,
) -> Result<Json<ApiSuccess<DanbooruPostsResponse>>, ApiError> {
    if query.query.len() > 4_096 {
        return Err(ApiError::bad_request(
            "query_too_long",
            "查询最长为 4096 字节",
        ));
    }
    let client = state.danbooru.read().await.clone();
    let page = client
        .posts(&PostQuery {
            tags: query.query.clone(),
            page: query.page.clone(),
            limit: query.limit,
        })
        .await
        .map_err(map_danbooru_error)?;
    state.cache_danbooru_posts(&page.posts);
    let numeric_page = query.page.parse::<u64>().unwrap_or(1);
    let custom_order = query
        .query
        .split_whitespace()
        .any(|token| token.starts_with("order:"));
    let first_post_id = page.posts.first().map(|post| post.id);
    let last_post_id = page.posts.last().map(|post| post.id);
    let next_page = if page.posts.len() == usize::from(query.limit) {
        if custom_order {
            Some(numeric_page.saturating_add(1).to_string())
        } else {
            last_post_id.map(|id| format!("b{id}"))
        }
    } else {
        None
    };
    let previous_page = if query.page.parse::<u64>().is_ok() {
        (numeric_page > 1).then(|| (numeric_page - 1).to_string())
    } else {
        first_post_id.map(|id| format!("a{id}"))
    };
    let posts = page
        .posts
        .into_iter()
        .map(|post| danbooru_post_response(&state.database, post))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiSuccess {
        data: DanbooruPostsResponse {
            posts,
            page: numeric_page,
            next_page,
            previous_page,
        },
        meta: None,
    }))
}

async fn danbooru_post(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<u64>,
) -> Result<Json<ApiSuccess<DanbooruPostResponse>>, ApiError> {
    let client = state.danbooru.read().await.clone();
    let post = client.post(id).await.map_err(map_danbooru_error)?;
    state.cache_danbooru_posts(std::slice::from_ref(&post));
    Ok(Json(ApiSuccess {
        data: danbooru_post_response(&state.database, post)?,
        meta: None,
    }))
}

async fn danbooru_media(
    State(state): State<AppState>,
    AxumPath((id, variant)): AxumPath<(u64, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let variant = parse_media_variant(&variant)?;
    let range = headers
        .get(header::RANGE)
        .map(|value| value.to_str())
        .transpose()
        .map_err(|_| ApiError::bad_request("invalid_range", "Range 请求头无效"))?;
    if range.is_some_and(|value| {
        value.len() > 128
            || !value.starts_with("bytes=")
            || value.contains(',')
            || value.chars().any(char::is_whitespace)
    }) {
        return Err(ApiError::bad_request(
            "invalid_range",
            "仅支持单个 bytes Range",
        ));
    }

    let client = state.danbooru.read().await.clone();
    let post = if let Some(post) = state.cached_danbooru_post(id) {
        post
    } else {
        let post = client.post(id).await.map_err(map_danbooru_error)?;
        state.cache_danbooru_posts(std::slice::from_ref(&post));
        post
    };
    let source = client
        .resolve_media(&post, variant)
        .map_err(map_danbooru_error)?;
    let upstream = client
        .open_media(&source, range)
        .await
        .map_err(map_danbooru_error)?;
    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder
        .header(header::CACHE_CONTROL, "private, max-age=300")
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|error| ApiError::internal(format!("无法创建媒体响应: {error}")))
}

fn parse_media_variant(value: &str) -> Result<MediaVariant, ApiError> {
    match value {
        "preview" => Ok(MediaVariant::Preview),
        "sample" => Ok(MediaVariant::Sample),
        "large" => Ok(MediaVariant::Large),
        "original" => Ok(MediaVariant::Original),
        "ugoira_webm" => Ok(MediaVariant::UgoiraWebm),
        "ugoira_zip" => Ok(MediaVariant::UgoiraZip),
        _ => Err(ApiError::not_found(
            "media_variant_not_found",
            "未知媒体 variant",
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
struct AutocompleteQuery {
    #[serde(rename = "q")]
    query: String,
}

#[derive(Debug, Serialize)]
struct AutocompleteResponse {
    value: String,
    label: String,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_count: Option<u64>,
}

async fn danbooru_autocomplete(
    State(state): State<AppState>,
    Query(query): Query<AutocompleteQuery>,
) -> Result<Json<ApiSuccess<Vec<AutocompleteResponse>>>, ApiError> {
    let client = state.danbooru.read().await.clone();
    let items = client
        .autocomplete(&query.query, 10)
        .await
        .map_err(map_danbooru_error)?;
    Ok(Json(ApiSuccess {
        data: items.into_iter().map(autocomplete_response).collect(),
        meta: None,
    }))
}

fn autocomplete_response(item: AutocompleteItem) -> AutocompleteResponse {
    let category = match item.category {
        Some(1) => "artist",
        Some(3) => "copyright",
        Some(4) => "character",
        Some(5 | 6) => "meta",
        Some(0) => "general",
        _ => "query",
    };
    AutocompleteResponse {
        value: item.value,
        label: item.label,
        category,
        post_count: item.post_count,
    }
}

#[derive(Debug, Serialize)]
struct DanbooruCountResponse {
    count: u64,
    exact: bool,
}

async fn danbooru_count(
    State(state): State<AppState>,
    Query(query): Query<AutocompleteQuery>,
) -> Result<Json<ApiSuccess<DanbooruCountResponse>>, ApiError> {
    if query.query.len() > 4_096 {
        return Err(ApiError::bad_request(
            "query_too_long",
            "查询最长为 4096 字节",
        ));
    }
    let client = state.danbooru.read().await.clone();
    let count = client
        .count(&query.query)
        .await
        .map_err(map_danbooru_error)?;
    Ok(Json(ApiSuccess {
        data: DanbooruCountResponse { count, exact: true },
        meta: None,
    }))
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split_whitespace().map(str::to_string).collect()
}

fn danbooru_post_response(
    database: &Database,
    post: Post,
) -> Result<DanbooruPostResponse, ApiError> {
    let downloaded = database
        .find_media_by_post_or_md5(Some(post.id as i64), post.md5.as_deref())
        .map_err(|error| ApiError::internal(error.to_string()))?
        .is_some();
    let extension = post
        .file_ext
        .clone()
        .unwrap_or_default()
        .to_ascii_lowercase();
    Ok(DanbooruPostResponse {
        id: post.id,
        rating: normalized_post_rating(&post.rating),
        score: post.score,
        fav_count: post.fav_count,
        image_width: post.image_width,
        image_height: post.image_height,
        file_ext: extension.clone(),
        file_size: post.file_size.unwrap_or(0),
        duration: post.duration,
        source: (!post.source.is_empty()).then_some(post.source),
        is_video: matches!(extension.as_str(), "mp4" | "webm"),
        is_ugoira: extension == "zip",
        restricted: post.file_url.is_none(),
        downloaded,
        tags: DanbooruTagsResponse {
            general: split_tags(&post.tag_string_general),
            artist: split_tags(&post.tag_string_artist),
            copyright: split_tags(&post.tag_string_copyright),
            character: split_tags(&post.tag_string_character),
            meta: split_tags(&post.tag_string_meta),
        },
    })
}

fn map_danbooru_error(error: DanbooruError) -> ApiError {
    let status = match error.kind {
        DanbooruErrorKind::InvalidRequest
        | DanbooruErrorKind::InvalidQuery
        | DanbooruErrorKind::PageLimit
        | DanbooruErrorKind::TagLimit
        | DanbooruErrorKind::InvalidTemplate
        | DanbooruErrorKind::UnsupportedMedia => StatusCode::BAD_REQUEST,
        DanbooruErrorKind::InvalidCredentials => StatusCode::UNAUTHORIZED,
        DanbooruErrorKind::Forbidden => StatusCode::FORBIDDEN,
        DanbooruErrorKind::NotFound => StatusCode::NOT_FOUND,
        DanbooruErrorKind::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        DanbooruErrorKind::UpstreamUnavailable | DanbooruErrorKind::Network => {
            StatusCode::BAD_GATEWAY
        }
        DanbooruErrorKind::UnsafeMediaUrl
        | DanbooruErrorKind::InvalidResponse
        | DanbooruErrorKind::Integrity
        | DanbooruErrorKind::Io => StatusCode::BAD_GATEWAY,
    };
    ApiError {
        status,
        code: danbooru_error_code(error.kind).to_string(),
        message: error.message,
        retryable: error.retryable,
        fields: None,
    }
}

fn danbooru_error_code(kind: DanbooruErrorKind) -> &'static str {
    match kind {
        DanbooruErrorKind::NotFound => "danbooru_not_found",
        DanbooruErrorKind::InvalidRequest => "danbooru_invalidrequest",
        DanbooruErrorKind::InvalidQuery => "danbooru_invalidquery",
        DanbooruErrorKind::InvalidCredentials => "danbooru_invalidcredentials",
        DanbooruErrorKind::Forbidden => "danbooru_forbidden",
        DanbooruErrorKind::PageLimit => "danbooru_pagelimit",
        DanbooruErrorKind::TagLimit => "danbooru_taglimit",
        DanbooruErrorKind::RateLimited => "danbooru_ratelimited",
        DanbooruErrorKind::UpstreamUnavailable => "danbooru_upstreamunavailable",
        DanbooruErrorKind::Network => "danbooru_network",
        DanbooruErrorKind::InvalidResponse => "danbooru_invalidresponse",
        DanbooruErrorKind::UnsafeMediaUrl => "danbooru_unsafemediaurl",
        DanbooruErrorKind::UnsupportedMedia => "danbooru_unsupportedmedia",
        DanbooruErrorKind::InvalidTemplate => "danbooru_invalidtemplate",
        DanbooruErrorKind::Integrity => "danbooru_integrity",
        DanbooruErrorKind::Io => "danbooru_io",
    }
}

fn task_summary_response(database: &Database, task: TaskSnapshot) -> TaskSummaryResponse {
    let title = task
        .payload
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| task_title(&task.kind).to_string());
    let mut failures = task
        .error
        .iter()
        .map(|failure| TaskFailureResponse {
            item_id: None,
            code: failure.code.clone(),
            message: failure.message.clone(),
            retryable: failure.retryable,
        })
        .collect::<Vec<_>>();
    match database.list_task_items_page(&task.id, Some("failed"), None, 20) {
        Ok(page) => {
            failures.extend(page.items.into_iter().filter_map(|item| {
                item.error.as_ref().map(|error| {
                    let error = task_item_error_response(error);
                    TaskFailureResponse {
                        item_id: Some(item.item_key),
                        code: error.code,
                        message: error.message,
                        retryable: error.retryable,
                    }
                })
            }));
        }
        Err(error) => {
            tracing::error!(task_id = %task.id, %error, "无法读取任务项目失败摘要");
        }
    }
    TaskSummaryResponse {
        id: task.id,
        kind: task.kind,
        status: task_status_response(task.status),
        revision: task.revision,
        title,
        progress: TaskProgressResponse {
            completed: task.completed_items,
            total: task.total_items.unwrap_or(0),
            bytes_downloaded: task.bytes_processed,
            total_bytes: None,
            speed_bytes_per_sec: task.speed_bytes_per_sec,
            eta_seconds: task.eta_seconds,
        },
        failures,
        preview: task.preview,
        created_at: format_unix_timestamp(task.created_at),
        updated_at: format_unix_timestamp(task.updated_at),
    }
}

fn task_status_response(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Pausing => "pausing",
        TaskStatus::Paused => "paused",
        TaskStatus::Cancelling => "cancelling",
        TaskStatus::AwaitingConfirmation => "awaiting_confirmation",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn task_title(kind: &str) -> &'static str {
    match kind {
        "download" => "Danbooru 下载",
        "index_library" => "索引本地图库",
        "integrity_scan" => "完整性检查",
        "exact_dedup" => "精确去重预检",
        "near_dedup" => "近似图片预检",
        "resize" => "调整图片尺寸",
        "heic_convert" => "转换 HEIC",
        "delete_by_tag" => "按标签隔离",
        "tag_pipeline" => "标签处理",
        "vllm_tag" => "vLLM 视觉打标",
        _ => "后台任务",
    }
}

fn format_unix_timestamp(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
fn test_router() -> (Router, AppState, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let data_dir = directory.path().join("data");
    let static_dir = directory.path().join("static");
    let data_dir_string = data_dir.to_string_lossy().into_owned();
    let static_dir_string = static_dir.to_string_lossy().into_owned();
    let paths = AppPaths::from_values(
        Some("127.0.0.1"),
        Some("8888"),
        Some(&data_dir_string),
        Some(&static_dir_string),
    )
    .unwrap();
    let state = AppState::open_internal(paths, SecretManager::session_only(), false).unwrap();
    (router_with_state(state.clone()), state, directory)
}

#[cfg(test)]
mod security_contract_tests {
    use super::{
        apply_tool_manifest, download_post, is_static_image_post, isolated_mode_enabled,
        meets_minimum_resolution, migrate_legacy_database_from, normalize_task_relative_directory,
        parse_media_variant, purge_registered_quarantine_file_with, run_resize_task,
        segmented_batch_anchor_query, segmented_batch_verification_query, sort_posts_for_download,
        spawn_task_worker, split_batch_verification_groups, task_average_speed, test_router,
        validate_batch_download_filter, validate_task_request, BatchDownloadFilter,
        CreateTaskRequest, DownloadSource, MediaPolicyRequest,
    };
    use crate::models::DownloadConfig;
    use crate::secrets::SecretKind;
    use crate::services::danbooru::{
        DanbooruClient, DanbooruClientConfig, MediaAsset, MediaAssetVariant, Post,
    };
    use crate::services::image_processor::{plan_delete_by_tag, VerifiedMediaRoot};
    use crate::tasks::{TaskFailure, TaskStatus};
    use axum::body::{to_bytes, Body};
    use axum::extract::Path as AxumPath;
    use axum::http::{Request, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;
    use tower::ServiceExt;

    #[test]
    fn isolated_runtime_mode_requires_an_explicit_one_value() {
        assert!(isolated_mode_enabled(Some(std::ffi::OsStr::new("1"))));
        assert!(!isolated_mode_enabled(None));
        assert!(!isolated_mode_enabled(Some(std::ffi::OsStr::new("true"))));
    }

    #[test]
    fn high_resolution_download_sort_uses_score_for_equal_pixel_counts() {
        let mut posts = vec![
            Post {
                id: 1,
                image_width: 1920,
                image_height: 1080,
                score: 100,
                ..Post::default()
            },
            Post {
                id: 2,
                image_width: 3840,
                image_height: 2160,
                score: 10,
                ..Post::default()
            },
            Post {
                id: 3,
                image_width: 3840,
                image_height: 2160,
                score: 50,
                ..Post::default()
            },
        ];

        sort_posts_for_download(&mut posts, true, true);

        assert_eq!(
            posts.iter().map(|post| post.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn minimum_resolution_requires_both_image_dimensions_to_meet_the_shortest_edge() {
        assert!(meets_minimum_resolution(2048, 3072, 2048));
        assert!(!meets_minimum_resolution(1536, 4096, 2048));
        assert!(meets_minimum_resolution(320, 240, 0));
    }

    #[test]
    fn static_image_only_excludes_gif_video_and_ugoira_media() {
        assert!(is_static_image_post(&Post {
            file_ext: Some("png".into()),
            ..Post::default()
        }));
        assert!(!is_static_image_post(&Post {
            file_ext: Some("gif".into()),
            ..Post::default()
        }));
        assert!(!is_static_image_post(&Post {
            file_ext: Some("mp4".into()),
            ..Post::default()
        }));
        assert!(!is_static_image_post(&Post {
            file_ext: Some("zip".into()),
            ..Post::default()
        }));
    }

    #[test]
    fn score_priority_sorts_a_download_page_without_remote_ordering() {
        let mut posts = vec![
            Post {
                id: 1,
                score: 10,
                ..Post::default()
            },
            Post {
                id: 2,
                score: 50,
                ..Post::default()
            },
        ];

        sort_posts_for_download(&mut posts, true, false);

        assert_eq!(
            posts.iter().map(|post| post.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn batch_filter_splits_remaining_tags_into_account_safe_verification_groups() {
        let filter = BatchDownloadFilter {
            include_tags: vec![
                "1girl".to_string(),
                "solo".to_string(),
                "blue_hair".to_string(),
            ],
            exclude_tags: vec![
                "comic".to_string(),
                "watermark".to_string(),
                "text".to_string(),
            ],
            minimum_score: 20,
            minimum_resolution: 0,
        };

        assert_eq!(
            split_batch_verification_groups(&filter, "1girl"),
            vec![
                vec!["solo".to_string(), "blue_hair".to_string()],
                vec!["-comic".to_string(), "-watermark".to_string()],
                vec!["-text".to_string()],
            ]
        );
    }

    #[test]
    fn batch_filter_uses_one_anchor_and_free_score_filter_for_candidate_pages() {
        let filter = BatchDownloadFilter {
            include_tags: vec!["1girl".to_string(), "solo".to_string()],
            exclude_tags: vec!["comic".to_string()],
            minimum_score: 20,
            minimum_resolution: 2048,
        };

        assert_eq!(
            segmented_batch_anchor_query(&filter, "1girl"),
            "1girl score:>=20 width:>=2048 height:>=2048"
        );
    }

    #[test]
    fn batch_filter_rejects_a_resolution_that_is_not_a_slider_step() {
        let filter = BatchDownloadFilter {
            include_tags: vec!["1girl".to_string()],
            exclude_tags: vec![],
            minimum_score: 0,
            minimum_resolution: 513,
        };

        assert!(validate_batch_download_filter(&filter).is_err());
    }

    #[test]
    fn download_task_accepts_a_limit_above_ten_thousand() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("large-download-limit");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-large-download-limit",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let request = CreateTaskRequest {
            kind: "download".to_string(),
            root_id: "root-large-download-limit".to_string(),
            relative_directory: None,
            source: Some(DownloadSource::Query {
                query: "1girl score:>=0".to_string(),
            }),
            batch_filter: Some(BatchDownloadFilter {
                include_tags: vec!["1girl".to_string()],
                exclude_tags: vec![],
                minimum_score: 0,
                minimum_resolution: 0,
            }),
            limit: Some(10_001),
            concurrency: Some(1),
            filename_template: Some("{id}.{ext}".to_string()),
            skip_existing: Some(true),
            keep_sidecar_txt: Some(true),
            static_images_only: Some(false),
            prioritize_score: Some(false),
            prioritize_resolution: Some(false),
            media_policy: Some(MediaPolicyRequest {
                original: true,
                ugoira: crate::config::UgoiraPolicy::WebmAndZip,
            }),
            options: None,
        };

        assert!(validate_task_request(&state, &request).is_ok());
    }

    #[test]
    fn segmented_batch_verification_uses_free_post_ids_with_at_most_two_tags() {
        assert_eq!(
            segmented_batch_verification_query(
                &[100, 101],
                &["solo".to_string(), "-comic".to_string()],
            ),
            "id:100,101 solo -comic"
        );
    }

    #[test]
    fn task_speed_is_derived_from_the_whole_task_window_not_one_transfer() {
        assert_eq!(task_average_speed(6_000, Duration::from_secs(3)), 2_000);
    }

    #[test]
    fn clear_large_media_variant_is_available_through_the_safe_proxy() {
        assert!(matches!(
            parse_media_variant("large").expect("large preview variant"),
            crate::services::danbooru::MediaVariant::Large
        ));
    }

    #[test]
    fn legacy_database_migration_includes_uncheckpointed_wal_data() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("legacy.db");
        let target = directory.path().join("migrated.db");
        let source = rusqlite::Connection::open(&legacy).unwrap();
        source
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE migration_probe (value TEXT NOT NULL);
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO migration_probe (value) VALUES (?1)",
                ["present-only-in-wal"],
            )
            .unwrap();
        assert!(legacy.with_extension("db-wal").metadata().unwrap().len() > 0);

        migrate_legacy_database_from(&legacy, &target).unwrap();

        let migrated = rusqlite::Connection::open(&target).unwrap();
        let value: String = migrated
            .query_row("SELECT value FROM migration_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "present-only-in-wal");
    }

    async fn mock_danbooru_posts(
        capture: Arc<StdMutex<Option<String>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn posts(
            axum::extract::State(capture): axum::extract::State<Arc<StdMutex<Option<String>>>>,
            uri: Uri,
        ) -> Json<serde_json::Value> {
            *capture.lock().unwrap() = Some(uri.to_string());
            Json(serde_json::json!([{
                "id": 42,
                "rating": "q",
                "score": 17,
                "fav_count": 3,
                "image_width": 1200,
                "image_height": 800,
                "file_ext": "jpg",
                "file_size": 1234,
                "file_url": "http://127.0.0.1/private-original.jpg",
                "preview_file_url": "http://127.0.0.1/preview.jpg",
                "source": "https://example.invalid/source",
                "tag_string_general": "blue_eyes solo",
                "tag_string_artist": "artist_name",
                "tag_string_copyright": "series_name",
                "tag_string_character": "character_name",
                "tag_string_meta": "highres"
            }]))
        }

        let app = Router::new()
            .route("/posts.json", get(posts))
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_danbooru_media(
        range_capture: Arc<StdMutex<Option<String>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn post(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 42,
                "rating": "s",
                "file_ext": "jpg",
                "file_size": 4,
                "file_url": format!("http://{authority}/media.jpg")
            }))
        }

        async fn media(
            axum::extract::State(capture): axum::extract::State<Arc<StdMutex<Option<String>>>>,
            headers: axum::http::HeaderMap,
        ) -> impl axum::response::IntoResponse {
            *capture.lock().unwrap() = headers
                .get("range")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            (
                StatusCode::PARTIAL_CONTENT,
                [
                    ("content-type", "image/jpeg"),
                    ("content-range", "bytes 1-3/4"),
                    ("accept-ranges", "bytes"),
                ],
                "bcd",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/media.jpg", get(media))
            .with_state(range_capture);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_danbooru_download() -> (String, tokio::task::JoinHandle<()>) {
        async fn post(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 42,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "jpg",
                "file_size": 4,
                "file_url": format!("http://{authority}/media.jpg"),
                "tag_string": "cat solo",
                "tag_string_general": "cat solo"
            }))
        }
        async fn media() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "image/jpeg"), ("content-length", "4")],
                "jpeg",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/media.jpg", get(media));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_download_with_failed_first_post() -> (String, tokio::task::JoinHandle<()>) {
        async fn post(
            headers: axum::http::HeaderMap,
            AxumPath(filename): AxumPath<String>,
        ) -> Response {
            if filename == "1.json" {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "message": "temporary upstream failure" })),
                )
                    .into_response();
            }
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 2,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "jpg",
                "file_size": 4,
                "file_url": format!("http://{authority}/media.jpg"),
                "tag_string": "cat solo",
                "tag_string_general": "cat solo"
            }))
            .into_response()
        }
        async fn media() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "image/jpeg"), ("content-length", "4")],
                "jpeg",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/media.jpg", get(media));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_query_with_existing_first(
        media_requests: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn posts(
            headers: axum::http::HeaderMap,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
        ) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let (id, path) = if query
                .as_deref()
                .is_some_and(|value| value.contains("page=b2"))
            {
                (1, "media/1.jpg")
            } else {
                (2, "media/2.jpg")
            };
            Json(serde_json::json!([{
                "id": id,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "jpg",
                "file_size": 4,
                "file_url": format!("http://{authority}/{path}")
            }]))
        }
        async fn media(
            axum::extract::State(media_requests): axum::extract::State<Arc<AtomicUsize>>,
        ) -> impl axum::response::IntoResponse {
            media_requests.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                [("content-type", "image/jpeg"), ("content-length", "4")],
                "jpeg",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts.json", get(posts))
            .route("/media/{filename}", get(media))
            .with_state(media_requests);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_ugoira_zip_only() -> (String, tokio::task::JoinHandle<()>) {
        async fn post(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 42,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "zip",
                "file_size": 3,
                "file_url": format!("http://{authority}/ugoira.zip"),
                "media_asset": { "variants": [] }
            }))
        }
        async fn media() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/zip"), ("content-length", "3")],
                "zip",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/ugoira.zip", get(media));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_ugoira_variants(
        webm_requests: Arc<AtomicUsize>,
        zip_requests: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn post(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 42,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "zip",
                "file_size": 3,
                "file_url": format!("http://{authority}/ugoira.zip"),
                "media_asset": {
                    "variants": [{
                        "type": "720x720",
                        "url": format!("http://{authority}/ugoira.webm"),
                        "file_ext": "webm",
                        "file_size": 4
                    }]
                }
            }))
        }
        async fn webm(
            axum::extract::State((webm_requests, _)): axum::extract::State<(
                Arc<AtomicUsize>,
                Arc<AtomicUsize>,
            )>,
        ) -> impl axum::response::IntoResponse {
            webm_requests.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                [("content-type", "video/webm"), ("content-length", "4")],
                "webm",
            )
        }
        async fn zip(
            axum::extract::State((_, zip_requests)): axum::extract::State<(
                Arc<AtomicUsize>,
                Arc<AtomicUsize>,
            )>,
        ) -> impl axum::response::IntoResponse {
            zip_requests.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                [("content-type", "application/zip"), ("content-length", "3")],
                "zip",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/ugoira.webm", get(webm))
            .route("/ugoira.zip", get(zip))
            .with_state((webm_requests, zip_requests));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_concurrent_downloads(
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn posts(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!([
                {
                    "id": 1,
                    "rating": "g",
                    "file_ext": "jpg",
                    "file_size": 4,
                    "file_url": format!("http://{authority}/media/1.jpg")
                },
                {
                    "id": 2,
                    "rating": "g",
                    "file_ext": "jpg",
                    "file_size": 4,
                    "file_url": format!("http://{authority}/media/2.jpg")
                }
            ]))
        }
        async fn post(
            axum::extract::Path(filename): axum::extract::Path<String>,
            headers: axum::http::HeaderMap,
        ) -> Json<serde_json::Value> {
            let id = filename.trim_end_matches(".json").parse::<u64>().unwrap();
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": id,
                "rating": "g",
                "file_ext": "jpg",
                "file_size": 4,
                "file_url": format!("http://{authority}/media/{id}.jpg")
            }))
        }
        async fn media(
            axum::extract::State((active, maximum)): axum::extract::State<(
                Arc<AtomicUsize>,
                Arc<AtomicUsize>,
            )>,
        ) -> impl axum::response::IntoResponse {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                [("content-type", "image/jpeg"), ("content-length", "4")],
                "jpeg",
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts.json", get(posts))
            .route("/posts/{filename}", get(post))
            .route("/media/{filename}", get(media))
            .with_state((active, maximum));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_slow_resumable_download(
        chunks_sent: Arc<AtomicUsize>,
        ranges: Arc<StdMutex<Vec<Option<String>>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        type SlowDownloadState = (Arc<AtomicUsize>, Arc<StdMutex<Vec<Option<String>>>>);

        async fn post(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!({
                "id": 42,
                "rating": "g",
                "score": 9,
                "image_width": 2,
                "image_height": 2,
                "file_ext": "jpg",
                "file_size": 40,
                "file_url": format!("http://{authority}/media.jpg")
            }))
        }
        async fn media(
            axum::extract::State((chunks_sent, ranges)): axum::extract::State<SlowDownloadState>,
            headers: axum::http::HeaderMap,
        ) -> Response {
            let range = headers
                .get(axum::http::header::RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            ranges.lock().unwrap().push(range.clone());
            let start = range
                .as_deref()
                .and_then(|value| value.strip_prefix("bytes="))
                .and_then(|value| value.strip_suffix('-'))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let payload = b"0123456789012345678901234567890123456789";
            let remaining = payload[start..].to_vec();
            let remaining_len = remaining.len();
            let stream = async_stream::stream! {
                for chunk in remaining.chunks(4) {
                    chunks_sent.fetch_add(1, Ordering::SeqCst);
                    yield Ok::<_, std::convert::Infallible>(axum::body::Bytes::copy_from_slice(chunk));
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                }
            };
            let mut response = Response::builder()
                .status(if start == 0 {
                    StatusCode::OK
                } else {
                    StatusCode::PARTIAL_CONTENT
                })
                .header("content-type", "image/jpeg")
                .header("content-length", remaining_len.to_string())
                .header("accept-ranges", "bytes");
            if start > 0 {
                response = response.header("content-range", format!("bytes {start}-39/40"));
            }
            response.body(Body::from_stream(stream)).unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/posts/{filename}", get(post))
            .route("/media.jpg", get(media))
            .with_state((chunks_sent, ranges));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/"), server)
    }

    async fn mock_vllm_tags() -> (String, tokio::task::JoinHandle<()>) {
        async fn completions() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "choices": [{ "message": { "content": "<tag>cat, solo</tag>" } }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/v1/chat/completions", axum::routing::post(completions));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    async fn mock_vllm_health() -> (String, tokio::task::JoinHandle<()>) {
        async fn models() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "data": [{ "id": "local/vision-model" }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/v1/models", get(models));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    async fn mock_vllm_partial_deterministic_failure() -> (String, tokio::task::JoinHandle<()>) {
        async fn completions(
            axum::extract::State(calls): axum::extract::State<Arc<AtomicUsize>>,
        ) -> Response {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Json(serde_json::json!({
                    "choices": [{ "message": { "content": "<tag>cat, solo</tag>" } }]
                }))
                .into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": {"message": "invalid image input"}})),
                )
                    .into_response()
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/chat/completions", axum::routing::post(completions))
            .with_state(calls);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    async fn mock_vllm_slow_tags(calls: Arc<AtomicUsize>) -> (String, tokio::task::JoinHandle<()>) {
        async fn completions(
            axum::extract::State(calls): axum::extract::State<Arc<AtomicUsize>>,
        ) -> Json<serde_json::Value> {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            Json(serde_json::json!({
                "choices": [{ "message": { "content": "<tag>cat, solo</tag>" } }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/chat/completions", axum::routing::post(completions))
            .with_state(calls);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    async fn mock_vllm_capture_request(
        capture: Arc<StdMutex<Option<serde_json::Value>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        async fn completions(
            axum::extract::State(capture): axum::extract::State<
                Arc<StdMutex<Option<serde_json::Value>>>,
            >,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            *capture.lock().unwrap() = Some(body);
            Json(serde_json::json!({
                "choices": [{ "message": { "content": "<tag>cat, solo</tag>" } }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/chat/completions", axum::routing::post(completions))
            .with_state(capture);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1"), server)
    }

    #[tokio::test]
    async fn arbitrary_path_file_endpoint_is_removed() {
        let (application, _state, _directory) = test_router();
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/images/file?path=Cargo.toml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn config_endpoint_never_returns_secret_values() {
        let (application, state, _directory) = test_router();
        state
            .secrets
            .set_session(SecretKind::Danbooru, "do-not-return-this")
            .unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["danbooru_api_key_configured"], true);
        assert!(!String::from_utf8_lossy(&body).contains("do-not-return-this"));
        assert!(json["data"].get("api_key").is_none());
    }

    #[tokio::test]
    async fn vllm_health_endpoint_reports_models_from_the_configured_local_service() {
        let (application, state, _directory) = test_router();
        let (endpoint, server) = mock_vllm_health().await;
        state.settings.write().await.vllm_base_url = endpoint;

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/vllm/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        server.abort();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["available"], true);
        assert_eq!(json["data"]["models"][0], "local/vision-model");
    }

    #[tokio::test]
    async fn secret_endpoint_stores_key_without_echoing_it() {
        let (application, _state, _directory) = test_router();
        let response = application
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/secrets/danbooru")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"secret":"private-value"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["configured"], true);
        assert!(!String::from_utf8_lossy(&body).contains("private-value"));
    }

    #[tokio::test]
    async fn config_update_accepts_only_editable_fields_and_returns_secret_status() {
        let (application, _state, _directory) = test_router();
        let response = application
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "danbooru_username": "local-user",
                            "vllm_base_url": "http://127.0.0.1:8000/v1",
                            "vllm_allowed_hosts": [],
                            "proxy_url": null,
                            "download_concurrency": 8,
                            "filename_template": "{id}_score_{score}.{ext}",
                            "ugoira_policy": "webm_and_zip",
                            "blur_sensitive_media": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["data"]["danbooru_username"], "local-user");
        assert!(json["data"]["danbooru_api_key_configured"].is_boolean());
        assert!(json["data"]["vllm_api_key_configured"].is_boolean());
    }

    #[tokio::test]
    async fn config_update_persists_vllm_execution_settings() {
        let (application, state, _directory) = test_router();
        let response = application
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "danbooru_username": "",
                            "vllm_base_url": "http://127.0.0.1:8000/v1",
                            "vllm_allowed_hosts": [],
                            "vllm_model": "local/custom-vision",
                            "vllm_system_prompt": "return custom tags",
                            "vllm_tag_mode": "append",
                            "vllm_concurrency": 3,
                            "proxy_url": null,
                            "download_concurrency": 8,
                            "filename_template": "{id}_score_{score}.{ext}",
                            "ugoira_policy": "webm_and_zip",
                            "blur_sensitive_media": false
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["vllm_model"], "local/custom-vision");
        assert_eq!(json["data"]["vllm_system_prompt"], "return custom tags");
        assert_eq!(json["data"]["vllm_tag_mode"], "append");
        assert_eq!(json["data"]["vllm_concurrency"], 3);
        let stored = crate::config::load_settings(&state.settings_path).unwrap();
        assert_eq!(stored.vllm_model, "local/custom-vision");
        assert_eq!(
            stored.vllm_tag_mode,
            crate::services::vllm::TagWriteMode::Append
        );
        assert_eq!(stored.vllm_concurrency, 3);
        assert!(!stored.blur_sensitive_media);
    }

    #[tokio::test]
    async fn invalid_network_config_is_not_persisted_before_client_validation() {
        let (application, state, _directory) = test_router();
        let before = std::fs::read_to_string(&state.settings_path).unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "danbooru_username": "",
                            "vllm_base_url": "http://127.0.0.1:8000/v1",
                            "vllm_allowed_hosts": [],
                            "proxy_url": "mailto:invalid-proxy@example.test",
                            "download_concurrency": 8,
                            "filename_template": "{id}_score_{score}.{ext}",
                            "ugoira_policy": "webm_and_zip",
                            "blur_sensitive_media": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(&state.settings_path).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn registering_a_root_does_not_implicitly_index_media() {
        let (application, _state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("1.jpg"), b"not-indexed-yet").unwrap();
        let body = serde_json::json!({
            "name": "测试图库",
            "windows_path": media.to_string_lossy(),
            "linux_path": null,
        });

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/library/roots")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let response_body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&response_body).unwrap();

        assert_eq!(json["data"]["indexed"], false);
        assert_eq!(json["data"]["media_count"], 0);
    }

    #[tokio::test]
    async fn library_directories_are_listed_as_root_relative_download_locations() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("directory-list-root");
        std::fs::create_dir_all(media.join("characters/alice")).unwrap();
        std::fs::create_dir_all(media.join("landscapes")).unwrap();
        std::fs::create_dir_all(media.join(".danbooru-quarantine/hidden")).unwrap();
        state
            .database
            .create_root(
                "directory-list-root",
                "下载图库",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/roots/directory-list-root/directories")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["data"]["directories"],
            serde_json::json!(["characters", "characters/alice", "landscapes"])
        );
        assert_eq!(json["data"]["truncated"], false);
    }

    #[tokio::test]
    async fn a_nested_download_directory_can_be_created_inside_a_registered_root() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("directory-create-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "directory-create-root",
                "下载图库",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/library/roots/directory-create-root/directories")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"relative_path": "角色/爱丽丝"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["data"]["relative_path"], "角色/爱丽丝");
        assert!(media.join("角色/爱丽丝").is_dir());
    }

    #[tokio::test]
    async fn media_roots_cannot_overlap_or_nest_on_the_current_platform() {
        let (application, _state, directory) = test_router();
        let parent = directory.path().join("media");
        let child = parent.join("nested");
        std::fs::create_dir_all(&child).unwrap();

        let request = |name: &str, path: &std::path::Path| {
            serde_json::json!({
                "name": name,
                "windows_path": path.to_string_lossy(),
                "linux_path": path.to_string_lossy(),
            })
        };
        let create = |body: serde_json::Value| {
            Request::builder()
                .method("POST")
                .uri("/api/library/roots")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        let parent_response = application
            .clone()
            .oneshot(create(request("父图库", &parent)))
            .await
            .unwrap();
        assert_eq!(parent_response.status(), StatusCode::CREATED);

        let nested_response = application
            .oneshot(create(request("嵌套图库", &child)))
            .await
            .unwrap();
        assert_eq!(nested_response.status(), StatusCode::CONFLICT);
        let body = to_bytes(nested_response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "overlapping_media_root");
    }

    #[tokio::test]
    async fn indexed_media_prevents_repointing_a_root_to_another_directory() {
        let (application, state, directory) = test_router();
        let original = directory.path().join("original-root");
        let replacement = directory.path().join("replacement-root");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        state
            .database
            .create_root(
                "fixed-root",
                "固定图库",
                Some(&original.to_string_lossy()),
                Some(&original.to_string_lossy()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "fixed-media".to_string(),
                root_id: "fixed-root".to_string(),
                post_id: None,
                relative_path: "1.jpg".to_string(),
                variant: "original".to_string(),
                mime_type: "image/jpeg".to_string(),
                byte_size: 1,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/library/roots/fixed-root")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "固定图库",
                            "windows_path": replacement.to_string_lossy(),
                            "linux_path": replacement.to_string_lossy(),
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "root_path_in_use");
    }

    #[tokio::test]
    async fn removing_a_media_root_forgets_catalog_records_but_keeps_files() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("removable-root");
        std::fs::create_dir_all(&media).unwrap();
        let image = media.join("keep.jpg");
        std::fs::write(&image, b"keep-this-file").unwrap();
        state
            .database
            .create_root(
                "removable-root",
                "可移除图库",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "removable-media".to_string(),
                root_id: "removable-root".to_string(),
                post_id: None,
                relative_path: "keep.jpg".to_string(),
                variant: "original".to_string(),
                mime_type: "image/jpeg".to_string(),
                byte_size: 14,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/library/roots/removable-root")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.database.get_root("removable-root").unwrap().is_none());
        assert!(state
            .database
            .get_media_file("removable-media")
            .unwrap()
            .is_none());
        assert_eq!(std::fs::read(image).unwrap(), b"keep-this-file");
    }

    #[tokio::test]
    async fn canonical_root_write_lock_serializes_equivalent_directory_paths() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("locked-root");
        std::fs::create_dir_all(&media).unwrap();
        let first = state.root_writes.acquire(&media).await.unwrap();
        let coordinator = state.root_writes.clone();
        let equivalent = media.join(".");
        let waiter = tokio::spawn(async move { coordinator.acquire(&equivalent).await });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), waiter)
                .await
                .is_err()
        );
        drop(first);
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.root_writes.acquire(&media),
        )
        .await
        .unwrap()
        .unwrap();
        drop(second);
    }

    #[tokio::test]
    async fn root_registration_is_serialized_around_overlap_validation_and_insert() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("registry-root");
        std::fs::create_dir_all(&media).unwrap();
        let registry_guard = state.root_registry.lock().await;
        let request = application.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/library/roots")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "串行注册",
                        "windows_path": media.to_string_lossy(),
                        "linux_path": media.to_string_lossy(),
                    })
                    .to_string(),
                ))
                .unwrap(),
        );
        tokio::pin!(request);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut request)
                .await
                .is_err()
        );
        drop(registry_guard);
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn root_update_does_not_hold_the_global_registry_while_waiting_for_a_writer() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("busy-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "busy-root",
                "Busy",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let writer = state.root_writes.acquire(&media).await.unwrap();
        let update = application.oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/library/roots/busy-root")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "Renamed",
                        "windows_path": media.to_string_lossy(),
                        "linux_path": media.to_string_lossy(),
                    })
                    .to_string(),
                ))
                .unwrap(),
        );
        tokio::pin!(update);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut update)
                .await
                .is_err()
        );

        assert!(state.root_registry.try_lock().is_ok());
        drop(writer);
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), update)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_startup_reschedules_persisted_queued_tasks() {
        let (_application, state, _directory) = test_router();
        let task = state
            .tasks
            .create("unsupported_startup_task", serde_json::json!({}))
            .unwrap();
        assert_eq!(
            state.tasks.get(&task.id).unwrap().unwrap().status,
            crate::tasks::TaskStatus::Queued
        );

        let _restarted_router = super::router_with_state(state.clone());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state
                    .tasks
                    .get(&task.id)
                    .unwrap()
                    .is_some_and(|snapshot| snapshot.status == crate::tasks::TaskStatus::Failed)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued task should be rescheduled when the router starts");
    }

    #[tokio::test]
    async fn global_worker_slots_keep_excess_tasks_queued() {
        let (_application, mut state, directory) = test_router();
        state.worker_slots = Arc::new(tokio::sync::Semaphore::new(1));
        for (id, name) in [("root-a", "a"), ("root-b", "b")] {
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            state
                .database
                .create_root(
                    id,
                    name,
                    Some(path.to_str().unwrap()),
                    Some(path.to_str().unwrap()),
                )
                .unwrap();
        }
        let lock_a = state
            .root_writes
            .acquire(&directory.path().join("a"))
            .await
            .unwrap();
        let lock_b = state
            .root_writes
            .acquire(&directory.path().join("b"))
            .await
            .unwrap();
        let first = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({"type":"index_library","root_id":"root-a"}),
            )
            .unwrap();
        let second = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({"type":"index_library","root_id":"root-b"}),
            )
            .unwrap();
        spawn_task_worker(state.clone(), first.id.clone()).await;
        spawn_task_worker(state.clone(), second.id.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let statuses = [first.id.as_str(), second.id.as_str()]
            .into_iter()
            .map(|id| state.tasks.get(id).unwrap().unwrap().status)
            .collect::<Vec<_>>();
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == crate::tasks::TaskStatus::Running)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == crate::tasks::TaskStatus::Queued)
                .count(),
            1
        );
        let _ = state.tasks.cancel(&first.id);
        let _ = state.tasks.cancel(&second.id);
        drop((lock_a, lock_b));
    }

    #[tokio::test]
    async fn task_snapshot_is_available_before_sse_subscription() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create("download", serde_json::json!({"title": "测试下载"}))
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["tasks"][0]["id"], task.id);
        assert_eq!(json["data"]["last_event_id"], 1);
    }

    #[tokio::test]
    async fn download_history_exposes_persisted_task_results_and_repeat_request() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("history-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "history-root",
                "历史图库",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "root_id": "history-root",
            "source": { "type": "query", "query": "cat_ears rating:g" },
            "limit": 3,
            "concurrency": 2,
            "filename_template": "{id}_score_{score}.{ext}",
            "skip_existing": true,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });
        let task = state.tasks.create("download", request.clone()).unwrap();
        state.tasks.start(&task.id).unwrap();
        state.tasks.progress(&task.id, 3, 3, 4096, 1024).unwrap();
        state
            .tasks
            .complete(
                &task.id,
                serde_json::json!({"downloaded": 3, "skipped": 2, "bytes": 4096}),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/downloads/history?limit=20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["items"][0]["task_id"], task.id);
        assert_eq!(
            json["data"]["items"][0]["source_label"],
            "cat_ears rating:g"
        );
        assert_eq!(json["data"]["items"][0]["root_name"], "历史图库");
        assert_eq!(json["data"]["items"][0]["completed_items"], 3);
        assert_eq!(json["data"]["items"][0]["skipped_items"], 2);
        assert_eq!(json["data"]["items"][0]["bytes_processed"], 4096);
        assert_eq!(json["data"]["items"][0]["can_repeat"], true);
        assert_eq!(json["data"]["items"][0]["repeat_request"], request);
    }

    #[tokio::test]
    async fn download_history_contains_only_terminal_download_tasks() {
        let (application, state, _directory) = test_router();
        let finished = state
            .tasks
            .create(
                "download",
                serde_json::json!({"type":"download","root_id":"missing"}),
            )
            .unwrap();
        state.tasks.start(&finished.id).unwrap();
        state
            .tasks
            .fail(
                &finished.id,
                TaskFailure {
                    code: "test_failure".to_string(),
                    message: "测试失败".to_string(),
                    retryable: false,
                },
            )
            .unwrap();
        let running = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();
        state.tasks.start(&running.id).unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/downloads/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids = json["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["task_id"].as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&finished.id.as_str()));
        assert!(!ids.contains(&running.id.as_str()));
    }

    #[tokio::test]
    async fn download_history_keeps_legacy_records_visible_without_exposing_paths() {
        let (application, state, _directory) = test_router();
        let config = DownloadConfig {
            tags: "legacy_tag rating:g".to_string(),
            limit: 4,
            save_path: "C:\\private\\legacy".to_string(),
            ..DownloadConfig::default()
        };
        state
            .database
            .start_download("legacy-visible", &config)
            .unwrap();
        state
            .database
            .finish_download("legacy-visible", 4, 1)
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/downloads/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(json["data"]["items"][0]["task_id"], "legacy-visible");
        assert_eq!(
            json["data"]["items"][0]["source_label"],
            "legacy_tag rating:g"
        );
        assert_eq!(json["data"]["items"][0]["completed_items"], 4);
        assert_eq!(json["data"]["items"][0]["failed_items"], 1);
        assert_eq!(json["data"]["items"][0]["can_repeat"], false);
        assert!(!text.contains("private"));
        assert!(!text.contains("save_path"));
    }

    #[tokio::test]
    async fn download_history_keyset_paginates_past_the_task_snapshot_limit() {
        let (application, state, _directory) = test_router();
        for index in 0..=1_000 {
            state
                .database
                .create_task(
                    &format!("history-page-{index:04}"),
                    "download",
                    &serde_json::json!({"type":"download","root_id":"missing"}),
                    "completed",
                )
                .unwrap();
        }

        let mut cursor = None;
        let mut task_ids = std::collections::HashSet::new();
        for _ in 0..20 {
            let uri = cursor.as_ref().map_or_else(
                || "/api/downloads/history?limit=100".to_string(),
                |cursor: &String| format!("/api/downloads/history?limit=100&cursor={cursor}"),
            );
            let response = application
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            for item in json["data"]["items"].as_array().unwrap() {
                let task_id = item["task_id"].as_str().unwrap();
                if task_id.starts_with("history-page-") {
                    task_ids.insert(task_id.to_string());
                }
            }
            cursor = json["data"]["next_cursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(task_ids.len(), 1_001);
        assert!(task_ids.contains("history-page-0000"));
    }

    #[tokio::test]
    async fn download_history_cursor_keeps_its_position_if_the_anchor_task_is_retried() {
        let (application, state, _directory) = test_router();
        let mut created = std::collections::HashSet::new();
        for _ in 0..2 {
            let task = state
                .tasks
                .create(
                    "download",
                    serde_json::json!({"type":"download","root_id":"missing"}),
                )
                .unwrap();
            state.tasks.start(&task.id).unwrap();
            state
                .tasks
                .fail(
                    &task.id,
                    TaskFailure {
                        code: "temporary".to_string(),
                        message: "temporary".to_string(),
                        retryable: true,
                    },
                )
                .unwrap();
            created.insert(task.id);
        }

        let first = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/downloads/history?limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first_body = to_bytes(first.into_body(), 64 * 1024).await.unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let anchor_id = first_json["data"]["items"][0]["task_id"].as_str().unwrap();
        let cursor = first_json["data"]["next_cursor"].as_str().unwrap();
        assert!(created.contains(anchor_id));
        state.tasks.retry(anchor_id).unwrap();

        let next = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/downloads/history?limit=1&cursor={cursor}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(next.status(), StatusCode::OK);
        let next_body = to_bytes(next.into_body(), 64 * 1024).await.unwrap();
        let next_json: serde_json::Value = serde_json::from_slice(&next_body).unwrap();
        assert_ne!(
            next_json["data"]["items"][0]["task_id"].as_str(),
            Some(anchor_id)
        );
    }

    #[tokio::test]
    async fn task_event_stream_replays_events_after_the_requested_sequence() {
        use tokio_stream::StreamExt;

        let (application, state, _directory) = test_router();
        let first = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();
        let second = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/events?after=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("SSE replay should arrive without waiting for a live event")
            .expect("SSE body should remain open")
            .expect("SSE frame should be valid");
        let text = String::from_utf8_lossy(&chunk);

        assert!(!text.contains(&first.id));
        assert!(text.contains(&second.id));
        assert!(text.contains("\"sequence\":2"));
        assert!(text.contains("\"event_type\":\"created\""));
    }

    #[tokio::test]
    async fn task_event_stream_honors_the_standard_last_event_id_header() {
        use tokio_stream::StreamExt;

        let (application, state, _directory) = test_router();
        let first = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();
        let second = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/events")
                    .header("last-event-id", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("SSE replay should arrive without a live event")
            .expect("SSE body should remain open")
            .expect("SSE frame should be valid");
        let text = String::from_utf8_lossy(&chunk);

        assert!(!text.contains(&first.id));
        assert!(text.contains(&second.id));
        assert!(text.contains("id: 2"));
    }

    #[tokio::test]
    async fn task_event_stream_requests_snapshot_resync_after_a_replay_gap() {
        use tokio_stream::StreamExt;

        let (application, state, _directory) = test_router();
        state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();
        state.tasks.clear_replay_for_test();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/events")
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("a replay gap must be reported immediately")
            .expect("SSE body should remain open")
            .expect("SSE frame should be valid");
        let text = String::from_utf8_lossy(&chunk);

        assert!(text.contains("event: resync"));
        assert!(text.contains("\"reason\":\"replay_gap\""));
        assert!(text.contains("\"sequence\":1"));
    }

    #[tokio::test]
    async fn danbooru_posts_forwards_native_query_without_leaking_cdn_urls() {
        let (application, state, _directory) = test_router();
        let capture = Arc::new(StdMutex::new(None));
        let (base_url, server) = mock_danbooru_posts(capture.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts?q=rating%3Aq+order%3Ascore&page=2&limit=40")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        server.abort();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["posts"][0]["id"], 42);
        assert_eq!(json["data"]["posts"][0]["tags"]["artist"][0], "artist_name");
        assert!(json["data"]["posts"][0].get("file_url").is_none());
        let upstream = capture.lock().unwrap().clone().unwrap();
        assert!(
            upstream.contains("tags=rating%3Aq+order%3Ascore")
                || upstream.contains("tags=rating%3Aq%20order%3Ascore")
        );
        assert!(upstream.contains("page=2"));
    }

    #[tokio::test]
    async fn missing_danbooru_post_returns_a_real_404_with_a_stable_code() {
        let (application, state, _directory) = test_router();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/posts/{filename}", get(|| async { StatusCode::NOT_FOUND })),
            )
            .await
            .unwrap();
        });
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url: format!("http://{address}/"),
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        server.abort();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"]["code"], "danbooru_not_found");
        assert_eq!(json["error"]["retryable"], false);
    }

    #[tokio::test]
    async fn danbooru_cursor_page_returns_both_native_navigation_tokens() {
        let (application, state, _directory) = test_router();
        let capture = Arc::new(StdMutex::new(None));
        let (base_url, server) = mock_danbooru_posts(capture).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts?q=cat&page=b100&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        server.abort();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["next_page"], "b42");
        assert_eq!(json["data"]["previous_page"], "a42");
    }

    #[test]
    fn unknown_upstream_rating_is_stored_as_fail_closed_unknown() {
        let post = crate::services::danbooru::Post {
            id: 42,
            rating: "unexpected".to_string(),
            ..Default::default()
        };

        assert_eq!(super::post_record_input(&post).rating, "unknown");
    }

    #[tokio::test]
    async fn danbooru_media_proxy_forwards_range_and_streams_partial_response() {
        let (application, state, _directory) = test_router();
        let range = Arc::new(StdMutex::new(None));
        let (base_url, server) = mock_danbooru_media(range.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts/42/media/original")
                    .header("range", "bytes=1-")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        server.abort();

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers.get("content-range").unwrap(), "bytes 1-3/4");
        assert_eq!(&body[..], b"bcd");
        assert_eq!(range.lock().unwrap().as_deref(), Some("bytes=1-"));
    }

    #[tokio::test]
    async fn loaded_post_metadata_is_reused_for_media_preview_without_a_second_api_lookup() {
        async fn posts(headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let authority = headers
                .get(axum::http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            Json(serde_json::json!([{
                "id": 42,
                "rating": "s",
                "file_ext": "jpg",
                "preview_file_url": format!("http://{authority}/preview.jpg")
            }]))
        }
        async fn post(
            axum::extract::State(calls): axum::extract::State<Arc<AtomicUsize>>,
        ) -> StatusCode {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR
        }
        async fn preview() -> impl IntoResponse {
            ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], "img")
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let upstream = Router::new()
            .route("/posts.json", get(posts))
            .route("/posts/42.json", get(post))
            .route("/preview.jpg", get(preview))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let (application, state, _directory) = test_router();
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url: format!("http://{address}/"),
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let list = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts?q=cat&page=1&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let media = application
            .oneshot(
                Request::builder()
                    .uri("/api/danbooru/posts/42/media/preview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = media.status();
        let body = to_bytes(media.into_body(), 64 * 1024).await.unwrap();
        server.abort();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"img");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn creating_download_task_persists_a_validated_snapshot() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root("root-1", "Library", None, Some(media.to_str().unwrap()))
            .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "source": { "type": "post_ids", "post_ids": [42] },
            "root_id": "root-1",
            "limit": 1,
            "concurrency": 8,
            "filename_template": "{id}_score_{score}.{ext}",
            "skip_existing": true,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["data"]["kind"], "download");
        assert_eq!(json["data"]["status"], "queued");
        assert_eq!(state.tasks.snapshot().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn creating_post_id_download_persists_unique_queued_items_before_worker_start() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media-items");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root("root-1", "Library", None, Some(media.to_str().unwrap()))
            .unwrap();
        let _worker_guard = state
            .worker_slots
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42, 42, 43] },
                            "root_id": "root-1",
                            "limit": 2,
                            "concurrency": 2,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap();

        let items = state.database.list_task_items(task_id).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.status == "queued"));
        assert_eq!(items[0].payload["post_id"], 42);
        assert_eq!(items[1].payload["post_id"], 43);
    }

    #[tokio::test]
    async fn creating_vllm_task_persists_unique_queued_media_items_before_worker_start() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-task-items");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-vllm-items",
                "vLLM items",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path) in [("media-1", "one.jpg"), ("media-2", "two.jpg")] {
            std::fs::write(media_root.join(relative_path), b"fixture").unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-vllm-items".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 7,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }
        let _worker_guard = state
            .worker_slots
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-items",
                            "options": { "media_ids": ["media-1", "media-1", "media-2"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap();

        let items = state.database.list_task_items(task_id).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.status == "queued"));
        assert_eq!(items[0].item_key, "media:media-1");
        assert_eq!(items[0].payload["media_id"], "media-1");
        assert_eq!(items[1].item_key, "media:media-2");
        assert_eq!(items[1].payload["media_id"], "media-2");
    }

    #[tokio::test]
    async fn creating_vllm_task_expands_a_safe_relative_directory_recursively() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-directory-items");
        std::fs::create_dir_all(media_root.join("people/nested")).unwrap();
        std::fs::create_dir_all(media_root.join("people-old")).unwrap();
        state
            .database
            .create_root(
                "root-vllm-directory",
                "vLLM directory",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path) in [
            ("direct", "people/direct.jpg"),
            ("nested", "people/nested/image.jpg"),
            ("sibling", "people-old/image.jpg"),
        ] {
            std::fs::write(media_root.join(relative_path), b"fixture").unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-vllm-directory".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 7,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }
        let _worker_guard = state
            .worker_slots
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-directory",
                            "options": { "relative_directory": "people" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::CREATED, "{json}");
        let task_id = json["data"]["id"].as_str().unwrap();
        let items = state.database.list_task_items(task_id).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.payload["media_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["direct", "nested"]
        );
    }

    #[tokio::test]
    async fn creating_vllm_task_rejects_non_static_images_before_persistence() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-invalid-media");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-vllm-invalid",
                "vLLM invalid media",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path, mime_type) in [
            ("video", "clip.mp4", "video/mp4"),
            ("ugoira", "animation.zip", "application/zip"),
            ("heic", "camera.heic", "image/heic"),
        ] {
            std::fs::write(media_root.join(relative_path), b"fixture").unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-vllm-invalid".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: mime_type.into(),
                    byte_size: 7,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-invalid",
                            "options": { "media_ids": ["video", "ugoira", "heic"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "unsupported_vllm_media");
        assert_eq!(
            json["error"]["fields"]["media_ids"]["invalid_ids"],
            serde_json::json!(["video", "ugoira", "heic"])
        );
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[tokio::test]
    async fn task_detail_returns_filtered_bounded_items_without_raw_payload_fields() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create(
                "download",
                serde_json::json!({"type": "download", "root_id": "root-1", "limit": 2}),
            )
            .unwrap();
        state
            .database
            .ensure_task_items(
                &task.id,
                &[
                    crate::database::TaskItemInput::new(
                        "post:1",
                        serde_json::json!({"post_id": 1, "cdn_url": "https://secret.invalid/a"}),
                    ),
                    crate::database::TaskItemInput::new(
                        "post:2",
                        serde_json::json!({"post_id": 2}),
                    ),
                ],
            )
            .unwrap();
        state
            .database
            .finish_task_item(
                &task.id,
                "post:1",
                "completed",
                Some(&serde_json::json!({
                    "bytes": 42,
                    "path": r"C:\private\secret.jpg"
                })),
                None,
            )
            .unwrap();
        state
            .database
            .finish_task_item(
                &task.id,
                "post:2",
                "failed",
                None,
                Some(&serde_json::json!({
                    "code": "danbooru_network",
                    "message": "网络失败",
                    "retryable": true,
                    "debug_path": r"C:\private\secret.jpg"
                })),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/tasks/{}?item_status=failed&item_limit=50",
                        task.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["item_counts"]["total"], 2);
        assert_eq!(json["data"]["item_counts"]["retryable_failed"], 1);
        assert_eq!(json["data"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"]["items"][0]["item_id"], "post:2");
        assert_eq!(json["data"]["items"][0]["post_id"], 2);
        assert_eq!(
            json["data"]["items"][0]["error"]["code"],
            "danbooru_network"
        );
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("secret.invalid"));
        assert!(!text.contains("private"));
    }

    #[tokio::test]
    async fn task_detail_returns_safe_task_specific_resize_summary() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create(
                "resize",
                serde_json::json!({"type": "resize", "root_id": "root-1"}),
            )
            .unwrap();
        state.tasks.start(&task.id).unwrap();
        state
            .tasks
            .complete(
                &task.id,
                serde_json::json!({
                    "items": [{
                        "media_id": "media-1",
                        "relative_path": r"C:\\private\\resized.jpg",
                        "width": 1024,
                        "height": 683,
                        "quarantine_batch": "batch-private"
                    }]
                }),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/tasks/{}", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["result"]["items"][0]["media_id"], "media-1");
        assert_eq!(json["data"]["result"]["processed"], 1);
        assert_eq!(json["data"]["result"]["items"][0]["width"], 1024);
        assert_eq!(json["data"]["result"]["items"][0]["height"], 683);
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("private"));
        assert!(!text.contains("quarantine_batch"));
    }

    #[tokio::test]
    async fn vllm_task_rejects_endpoint_or_secret_overrides_before_persistence() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-1",
                            "options": {
                                "media_ids": ["media-1"],
                                "endpoint": "https://attacker.invalid/v1",
                                "api_key": "must-not-persist"
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!String::from_utf8_lossy(&body).contains("must-not-persist"));
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[tokio::test]
    async fn download_task_rejects_secret_options_before_persistence() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("download-secret-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "root_id": "root-1",
                            "source": {"type": "query", "query": "cat"},
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": {"original": true, "ugoira": "webm_and_zip"},
                            "options": {"api_key": "must-not-persist"}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!String::from_utf8_lossy(&body).contains("must-not-persist"));
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[tokio::test]
    async fn task_request_rejects_unknown_top_level_secret_fields() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("download-unknown-secret-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "root_id": "root-1",
                            "source": {"type": "query", "query": "cat"},
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": {"original": true, "ugoira": "webm_and_zip"},
                            "api_key": "must-not-persist"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!String::from_utf8_lossy(&body).contains("must-not-persist"));
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[tokio::test]
    async fn download_task_rejects_tool_only_options() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("download-options-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-options",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let request = CreateTaskRequest {
            kind: "download".to_string(),
            root_id: "root-options".to_string(),
            relative_directory: None,
            source: Some(DownloadSource::Query {
                query: "cat".to_string(),
            }),
            batch_filter: None,
            limit: Some(1),
            concurrency: Some(1),
            filename_template: Some("{id}.{ext}".to_string()),
            skip_existing: Some(true),
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: Some(MediaPolicyRequest {
                original: true,
                ugoira: crate::config::UgoiraPolicy::WebmAndZip,
            }),
            options: Some(serde_json::json!({"preflight": true})),
        };

        let error = validate_task_request(&state, &request).unwrap_err();
        assert_eq!(error.code, "invalid_task_fields");
    }

    #[test]
    fn tag_pipeline_requires_controlled_media_ids_before_it_can_be_persisted() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("unsupported-tool-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-unsupported",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let request = CreateTaskRequest {
            kind: "tag_pipeline".to_string(),
            root_id: "root-unsupported".to_string(),
            relative_directory: None,
            source: None,
            batch_filter: None,
            limit: None,
            concurrency: None,
            filename_template: None,
            skip_existing: None,
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: None,
            options: None,
        };

        let error = validate_task_request(&state, &request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "missing_media_ids");
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[test]
    fn task_directory_selection_accepts_only_normalized_root_relative_paths() {
        assert_eq!(
            normalize_task_relative_directory("people\\portraits/").unwrap(),
            "people/portraits"
        );
        assert!(normalize_task_relative_directory("../outside").is_err());
        assert!(normalize_task_relative_directory("C:\\outside").is_err());
        assert!(normalize_task_relative_directory("//server/share").is_err());
        assert!(normalize_task_relative_directory(".danbooru-quarantine/items").is_err());
    }

    #[test]
    fn heic_media_accepts_octet_stream_when_extension_is_heif() {
        assert!(super::is_supported_heic_media(
            "portrait.heif",
            "application/octet-stream"
        ));
    }

    #[test]
    fn heic_media_rejects_mime_only_without_heic_extension() {
        assert!(!super::is_supported_heic_media("camera.bin", "image/heic"));
    }

    #[test]
    fn heic_conversion_requires_controlled_media_ids_before_it_can_be_persisted() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("heic-validation-root");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-heic-validation",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let request = CreateTaskRequest {
            kind: "heic_convert".to_string(),
            root_id: "root-heic-validation".to_string(),
            relative_directory: None,
            source: None,
            batch_filter: None,
            limit: None,
            concurrency: None,
            filename_template: None,
            skip_existing: None,
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: None,
            options: None,
        };

        let error = validate_task_request(&state, &request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "missing_media_ids");
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[test]
    fn heic_task_rejects_user_supplied_converter_commands() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("heic-command-root");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("selected.heic"), b"fake-heic").unwrap();
        state
            .database
            .create_root(
                "root-heic-command",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-heic-command".into(),
                root_id: "root-heic-command".into(),
                post_id: None,
                relative_path: "selected.heic".into(),
                variant: "original".into(),
                mime_type: "image/heic".into(),
                byte_size: 9,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        let request = CreateTaskRequest {
            kind: "heic_convert".to_string(),
            root_id: "root-heic-command".to_string(),
            relative_directory: None,
            source: None,
            batch_filter: None,
            limit: None,
            concurrency: None,
            filename_template: None,
            skip_existing: None,
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: None,
            options: Some(serde_json::json!({
                "media_ids": ["media-heic-command"],
                "converter": "attacker-controlled-command"
            })),
        };

        let error = validate_task_request(&state, &request).unwrap_err();

        assert_eq!(error.code, "invalid_task_options");
        assert!(state.tasks.snapshot().unwrap().is_empty());
    }

    #[test]
    fn unavailable_heic_converter_maps_to_stable_non_retryable_task_failure() {
        let failure = super::heic_task_failure(
            crate::services::image_processor::ToolError::ConverterUnavailable,
        );

        assert_eq!(failure.code, "heic_converter_unavailable");
        assert!(!failure.retryable);
        assert_eq!(failure.message, "未安装 heif-convert，无法转换 HEIC");
    }

    #[tokio::test]
    async fn heic_task_creates_relative_path_preflight_without_touching_original() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("heic-preflight-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("selected.heic"), b"fake-heic").unwrap();
        state
            .database
            .create_root(
                "root-heic-preflight",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-heic-preflight".into(),
                root_id: "root-heic-preflight".into(),
                post_id: None,
                relative_path: "selected.heic".into(),
                variant: "original".into(),
                mime_type: "image/heic".into(),
                byte_size: 9,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "heic_convert",
                            "root_id": "root-heic-preflight",
                            "options": { "media_ids": ["media-heic-preflight"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let task = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if task.status != TaskStatus::Queued && task.status != TaskStatus::Running {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            task.status,
            TaskStatus::AwaitingConfirmation,
            "{:?}",
            task.error
        );
        let preview = task
            .preview
            .unwrap()
            .to_string()
            .replace('\\', "/")
            .replace("//", "/");
        assert!(preview.contains("selected.heic"));
        assert!(!preview.contains(media.to_string_lossy().as_ref()));
        assert_eq!(
            std::fs::read(media.join("selected.heic")).unwrap(),
            b"fake-heic"
        );
        assert!(!media.join(".danbooru-quarantine").exists());
    }

    #[tokio::test]
    async fn tag_pipeline_creates_a_relative_path_only_preflight() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("tag-preflight-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("selected.jpg"), b"media").unwrap();
        std::fs::write(media.join("selected.txt"), "blue_hair,blue_hair").unwrap();
        state
            .database
            .create_root(
                "root-tag-preflight",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-tag-preflight".into(),
                root_id: "root-tag-preflight".into(),
                post_id: None,
                relative_path: "selected.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 5,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "tag_pipeline",
                            "root_id": "root-tag-preflight",
                            "options": { "media_ids": ["media-tag-preflight"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let task = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if task.status != TaskStatus::Queued && task.status != TaskStatus::Running {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            task.status,
            TaskStatus::AwaitingConfirmation,
            "{:?}",
            task.error
        );
        let preview = task.preview.unwrap().to_string();
        assert!(preview.contains("selected.txt"));
        assert!(!preview.contains(media.to_string_lossy().as_ref()));
        assert_eq!(
            std::fs::read_to_string(media.join("selected.txt")).unwrap(),
            "blue_hair,blue_hair"
        );
    }

    #[tokio::test]
    async fn integrity_directory_task_preflight_excludes_indexed_media_outside_the_directory() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("integrity-directory-media");
        std::fs::create_dir_all(media.join("selected")).unwrap();
        std::fs::create_dir_all(media.join("outside")).unwrap();
        state
            .database
            .create_root(
                "root-integrity-directory",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path) in [
            ("selected", "selected/broken.jpg"),
            ("outside", "outside/broken.jpg"),
        ] {
            std::fs::write(media.join(relative_path), b"").unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-integrity-directory".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 0,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "integrity_scan",
                            "root_id": "root-integrity-directory",
                            "options": { "preflight": true, "relative_directory": "selected" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let task = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if task.status != TaskStatus::Queued && task.status != TaskStatus::Running {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let preview = task
            .preview
            .unwrap()
            .to_string()
            .replace('\\', "/")
            .replace("//", "/");

        assert!(preview.contains("selected/broken.jpg"), "{preview}");
        assert!(!preview.contains("outside/broken.jpg"));
    }

    #[tokio::test]
    async fn confirmed_tag_pipeline_quarantines_and_registers_the_original_sidecar() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("tag-apply-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("selected.jpg"), b"media").unwrap();
        std::fs::write(media.join("selected.txt"), "blue_hair,blue_hair").unwrap();
        state
            .database
            .create_root(
                "root-tag-apply",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-tag-apply".into(),
                root_id: "root-tag-apply".into(),
                post_id: None,
                relative_path: "selected.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 5,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        let request = CreateTaskRequest {
            kind: "tag_pipeline".to_string(),
            root_id: "root-tag-apply".to_string(),
            relative_directory: None,
            source: None,
            batch_filter: None,
            limit: None,
            concurrency: None,
            filename_template: None,
            skip_existing: None,
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: None,
            options: Some(serde_json::json!({ "media_ids": ["media-tag-apply"] })),
        };
        let task = state
            .tasks
            .create("tag_pipeline", serde_json::to_value(request).unwrap())
            .unwrap();
        spawn_task_worker(state.clone(), task.id.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if state.tasks.get(&task.id).unwrap().unwrap().status
                    == TaskStatus::AwaitingConfirmation
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        state.tasks.confirm(&task.id).unwrap();
        spawn_task_worker(state.clone(), task.id.clone()).await;
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let current = state.tasks.get(&task.id).unwrap().unwrap();
                if matches!(current.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break current;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(
            std::fs::read_to_string(media.join("selected.txt")).unwrap(),
            "blue hair"
        );
        let quarantine = state
            .database
            .list_quarantine("root-tag-apply", false)
            .unwrap();
        assert_eq!(quarantine.len(), 1);
        assert_eq!(quarantine[0].original_relative_path, "selected.txt");
        assert_eq!(quarantine[0].reason, "tag_pipeline_original");
        assert_eq!(
            std::fs::read_to_string(media.join(&quarantine[0].quarantine_relative_path)).unwrap(),
            "blue_hair,blue_hair"
        );
        assert!(!completed
            .result
            .unwrap()
            .to_string()
            .contains(media.to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn tag_pipeline_database_failure_restores_the_original_without_temp_files() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("tag-database-failure-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("selected.jpg"), b"media").unwrap();
        std::fs::write(media.join("selected.txt"), "blue_hair,blue_hair").unwrap();
        state
            .database
            .create_root(
                "root-tag-database-failure",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-tag-database-failure".into(),
                root_id: "root-tag-database-failure".into(),
                post_id: None,
                relative_path: "selected.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 5,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        let request = CreateTaskRequest {
            kind: "tag_pipeline".to_string(),
            root_id: "root-tag-database-failure".to_string(),
            relative_directory: None,
            source: None,
            batch_filter: None,
            limit: None,
            concurrency: None,
            filename_template: None,
            skip_existing: None,
            keep_sidecar_txt: None,
            static_images_only: None,
            prioritize_score: None,
            prioritize_resolution: None,
            media_policy: None,
            options: Some(serde_json::json!({
                "media_ids": ["media-tag-database-failure"]
            })),
        };
        let task = state
            .tasks
            .create("tag_pipeline", serde_json::to_value(request).unwrap())
            .unwrap();
        spawn_task_worker(state.clone(), task.id.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if state.tasks.get(&task.id).unwrap().unwrap().status
                    == TaskStatus::AwaitingConfirmation
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let database =
            rusqlite::Connection::open(directory.path().join("data/danbooru_tool.db")).unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER reject_tag_pipeline_quarantine
                 BEFORE INSERT ON quarantine
                 WHEN NEW.reason='tag_pipeline_original'
                 BEGIN SELECT RAISE(ABORT, 'injected tag quarantine failure'); END;",
            )
            .unwrap();
        drop(database);
        state.tasks.confirm(&task.id).unwrap();
        spawn_task_worker(state.clone(), task.id.clone()).await;
        let failed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let current = state.tasks.get(&task.id).unwrap().unwrap();
                if matches!(current.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break current;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(
            failed.error.as_ref().map(|error| error.code.as_str()),
            Some("tag_pipeline_persistence_failed")
        );
        assert_eq!(
            std::fs::read_to_string(media.join("selected.txt")).unwrap(),
            "blue_hair,blue_hair"
        );
        assert!(state
            .database
            .list_quarantine("root-tag-database-failure", false)
            .unwrap()
            .is_empty());
        assert!(!media.join(".danbooru-quarantine").exists());
        assert!(std::fs::read_dir(&media)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")));
    }

    #[tokio::test]
    async fn vllm_partial_deterministic_failure_marks_task_and_item_failed() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-partial-failure");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-vllm-partial",
                "vLLM partial",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path, color) in [
            ("media-1", "one.png", [20, 40, 60]),
            ("media-2", "two.png", [80, 100, 120]),
        ] {
            image::RgbImage::from_pixel(2, 2, image::Rgb(color))
                .save(media_root.join(relative_path))
                .unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-vllm-partial".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: "image/png".into(),
                    byte_size: 4,
                    sha256: None,
                    md5: None,
                    width: Some(2),
                    height: Some(2),
                    duration: None,
                })
                .unwrap();
        }
        let (endpoint, server) = mock_vllm_partial_deterministic_failure().await;
        state.settings.write().await.vllm_base_url = endpoint;

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-partial",
                            "options": { "media_ids": ["media-1", "media-2"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(terminal.status, TaskStatus::Failed);
        assert_eq!(terminal.completed_items, 1);
        assert_eq!(terminal.total_items, Some(2));
        assert_eq!(terminal.progress, 0.5);
        assert_eq!(
            terminal.error.as_ref().map(|error| error.retryable),
            Some(false)
        );
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|item| item.status == "completed")
                .count(),
            1
        );
        let failed = items.iter().find(|item| item.status == "failed").unwrap();
        assert_eq!(
            failed.error.as_ref().unwrap()["code"],
            "vllm_invalid_request"
        );
        assert_eq!(failed.error.as_ref().unwrap()["retryable"], false);
        assert!(!failed
            .error
            .as_ref()
            .unwrap()
            .to_string()
            .contains(media_root.to_string_lossy().as_ref()));
        assert!(media_root.join("one.png").exists());
        assert!(media_root.join("two.png").exists());
    }

    #[tokio::test]
    async fn vllm_pause_between_concurrency_waves_leaves_unstarted_items_queued() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-wave-pause");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-vllm-wave-pause",
                "vLLM wave pause",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for index in 1..=3 {
            let filename = format!("{index}.png");
            image::RgbImage::from_pixel(8, 8, image::Rgb([index, 2, 3]))
                .save(media_root.join(&filename))
                .unwrap();
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: format!("media-{index}"),
                    root_id: "root-vllm-wave-pause".into(),
                    post_id: None,
                    relative_path: filename,
                    variant: "original".into(),
                    mime_type: "image/png".into(),
                    byte_size: 128,
                    sha256: None,
                    md5: None,
                    width: Some(8),
                    height: Some(8),
                    duration: None,
                })
                .unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let (endpoint, server) = mock_vllm_slow_tags(calls.clone()).await;
        {
            let mut settings = state.settings.write().await;
            settings.vllm_base_url = endpoint;
            settings.download_concurrency = 3;
            settings.vllm_concurrency = 1;
        }

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-wave-pause",
                            "options": { "media_ids": ["media-1", "media-2", "media-3"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap().to_string();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        state.tasks.pause(&task_id).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if state.tasks.get(&task_id).unwrap().unwrap().status == TaskStatus::Paused {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(items[0].status, "completed");
        assert_eq!(items[1].status, "queued");
        assert_eq!(items[2].status, "queued");
        server.abort();
    }

    #[tokio::test]
    async fn vllm_worker_uses_stored_model_prompt_and_tag_mode() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-stored-config");
        std::fs::create_dir_all(&media_root).unwrap();
        image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]))
            .save(media_root.join("image.png"))
            .unwrap();
        std::fs::write(media_root.join("image.txt"), "human_tag").unwrap();
        state
            .database
            .create_root(
                "root-vllm-stored-config",
                "vLLM stored config",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-config".into(),
                root_id: "root-vllm-stored-config".into(),
                post_id: None,
                relative_path: "image.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 128,
                sha256: None,
                md5: None,
                width: Some(8),
                height: Some(8),
                duration: None,
            })
            .unwrap();
        let capture = Arc::new(StdMutex::new(None));
        let (endpoint, server) = mock_vllm_capture_request(capture.clone()).await;
        {
            let mut settings = state.settings.write().await;
            settings.vllm_base_url = endpoint;
            settings.vllm_model = "local/custom-vision".to_string();
            settings.vllm_system_prompt = "return only verified tags".to_string();
            settings.vllm_tag_mode = crate::services::vllm::TagWriteMode::Append;
            settings.vllm_concurrency = 1;
        }

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-stored-config",
                            "options": { "media_ids": ["media-config"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let status = state.tasks.get(task_id).unwrap().unwrap().status;
                if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
                    assert_eq!(status, TaskStatus::Completed);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let request = capture.lock().unwrap().clone().unwrap();
        assert_eq!(request["model"], "local/custom-vision");
        assert_eq!(
            request["messages"][0]["content"],
            "return only verified tags"
        );
        assert_eq!(
            std::fs::read_to_string(media_root.join("image.txt")).unwrap(),
            "human_tag,\ncat,solo"
        );
        let detail_response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/tasks/{task_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let detail_body = to_bytes(detail_response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let detail: serde_json::Value = serde_json::from_slice(&detail_body).unwrap();
        assert_eq!(
            detail["data"]["items"][0]["result"]["tags"],
            serde_json::json!(["cat", "solo"])
        );
        assert_eq!(
            detail["data"]["items"][0]["result"]["sidecar_written"],
            true
        );
        server.abort();
    }

    #[tokio::test]
    async fn vllm_task_quarantines_existing_sidecar_before_atomic_replacement() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([20, 40, 60]))
            .save(media.join("selected.jpg"))
            .unwrap();
        std::fs::write(media.join("selected.txt"), "human_reviewed_tag").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-1".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "selected.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let (endpoint, server) = mock_vllm_tags().await;
        state.settings.write().await.vllm_base_url = endpoint;

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-1",
                            "options": { "media_ids": ["media-1"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(
            std::fs::read_to_string(media.join("selected.txt")).unwrap(),
            "cat,solo"
        );
        let quarantined = state.database.list_quarantine("root-1", false).unwrap();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].original_relative_path, "selected.txt");
        assert_eq!(quarantined[0].reason, "vllm_sidecar_replaced");
        assert_eq!(
            std::fs::read_to_string(media.join(&quarantined[0].quarantine_relative_path)).unwrap(),
            "human_reviewed_tag"
        );
        let result = completed.result.unwrap().to_string();
        assert!(!result.contains(media.to_string_lossy().as_ref()));
        assert!(result.contains("media-1"));
        let item = state
            .database
            .list_task_items(&task_id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            item.result.as_ref().unwrap()["tags"],
            serde_json::json!(["cat", "solo"])
        );
    }

    #[tokio::test]
    async fn vllm_task_rolls_back_sidecar_when_quarantine_registration_fails() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("vllm-rollback-media");
        std::fs::create_dir_all(&media).unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([20, 40, 60]))
            .save(media.join("selected.jpg"))
            .unwrap();
        std::fs::write(media.join("selected.txt"), "human_reviewed_tag").unwrap();
        state
            .database
            .create_root(
                "root-vllm-rollback",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-vllm-rollback".into(),
                root_id: "root-vllm-rollback".into(),
                post_id: None,
                relative_path: "selected.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let database =
            rusqlite::Connection::open(directory.path().join("data/danbooru_tool.db")).unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER reject_vllm_quarantine
                 BEFORE INSERT ON quarantine
                 WHEN NEW.reason = 'vllm_sidecar_replaced'
                 BEGIN SELECT RAISE(ABORT, 'injected quarantine failure'); END;",
            )
            .unwrap();
        drop(database);
        let (endpoint, server) = mock_vllm_tags().await;
        state.settings.write().await.vllm_base_url = endpoint;

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "vllm_tag",
                            "root_id": "root-vllm-rollback",
                            "options": { "media_ids": ["media-vllm-rollback"] }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(completed.status, crate::tasks::TaskStatus::Failed);
        assert_eq!(
            std::fs::read_to_string(media.join("selected.txt")).unwrap(),
            "human_reviewed_tag"
        );
        assert!(state
            .database
            .list_quarantine("root-vllm-rollback", false)
            .unwrap()
            .is_empty());
        assert!(!media
            .join(".danbooru-quarantine")
            .join(format!("vllm-{task_id}"))
            .exists());
    }

    #[tokio::test]
    async fn resize_task_uses_selected_media_ids_and_quarantines_the_original() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        image::RgbImage::from_pixel(8, 4, image::Rgb([20, 40, 60]))
            .save(media.join("selected.png"))
            .unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-1".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "selected.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 128,
                sha256: None,
                md5: None,
                width: Some(8),
                height: Some(4),
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "resize",
                            "root_id": "root-1",
                            "options": { "media_ids": ["media-1"], "max_size": 4 }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert!(!media.join("selected.png").exists());
        assert!(media.join("selected.jpg").is_file());
        let updated = state.database.get_media_file("media-1").unwrap().unwrap();
        assert_eq!(updated.relative_path, "selected.jpg");
        assert_eq!((updated.width, updated.height), (Some(4), Some(2)));
        assert_eq!(
            state
                .database
                .list_quarantine("root-1", false)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn resize_database_failure_removes_replacement_and_restores_original() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        image::RgbImage::from_pixel(8, 4, image::Rgb([20, 40, 60]))
            .save(media.join("selected.png"))
            .unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-1".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "selected.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 128,
                sha256: None,
                md5: None,
                width: Some(8),
                height: Some(4),
                duration: None,
            })
            .unwrap();
        let payload = serde_json::json!({
            "type": "resize",
            "root_id": "root-1",
            "options": { "media_ids": ["media-1"], "max_size": 4 }
        });
        let task = state.tasks.create("resize", payload).unwrap();
        let batch_id = format!("resize-{}-0", task.id);
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "pre-existing-conflict".into(),
                root_id: "root-1".into(),
                media_file_id: None,
                original_relative_path: "unrelated.png".into(),
                quarantine_relative_path: format!(".danbooru-quarantine/{batch_id}/selected.png"),
                reason: "pre-existing".into(),
                sha256: None,
            })
            .unwrap();
        let running = state.tasks.start(&task.id).unwrap();

        assert!(run_resize_task(&state, &running).await.is_err());

        assert!(media.join("selected.png").is_file());
        assert!(!media.join("selected.jpg").exists());
        let stored = state.database.get_media_file("media-1").unwrap().unwrap();
        assert_eq!(stored.relative_path, "selected.png");
        assert_eq!(stored.mime_type, "image/png");
        assert_eq!(stored.status, "active");
        let quarantine = state.database.list_quarantine("root-1", true).unwrap();
        assert_eq!(quarantine.len(), 1);
        assert_eq!(quarantine[0].id, "pre-existing-conflict");
    }

    #[tokio::test]
    async fn task_action_endpoint_reports_pause_as_pending_worker_ack() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create("download", serde_json::json!({}))
            .unwrap();
        state.tasks.start(&task.id).unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{}/pause", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["status"], "pausing");
        assert_eq!(
            state.tasks.get(&task.id).unwrap().unwrap().status,
            crate::tasks::TaskStatus::Pausing
        );
    }

    #[tokio::test]
    async fn retry_action_requeues_only_retryable_failed_download_items() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create(
                "download",
                serde_json::json!({"type":"download","root_id":"missing"}),
            )
            .unwrap();
        state
            .database
            .ensure_task_items(
                &task.id,
                &[
                    crate::database::TaskItemInput::new("post:1", serde_json::json!({"post_id":1})),
                    crate::database::TaskItemInput::new("post:2", serde_json::json!({"post_id":2})),
                ],
            )
            .unwrap();
        for (key, retryable) in [("post:1", true), ("post:2", false)] {
            state
                .database
                .finish_task_item(
                    &task.id,
                    key,
                    "failed",
                    None,
                    Some(&serde_json::json!({
                        "code":"download_failed",
                        "message":"failed",
                        "retryable":retryable
                    })),
                )
                .unwrap();
        }
        state.tasks.start(&task.id).unwrap();
        state
            .tasks
            .fail(
                &task.id,
                TaskFailure {
                    code: "download_failed".to_string(),
                    message: "failed".to_string(),
                    retryable: true,
                },
            )
            .unwrap();
        let _worker_guard = state
            .worker_slots
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{}/retry", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let items = state.database.list_task_items(&task.id).unwrap();
        assert_eq!(items[0].status, "queued");
        assert_eq!(items[1].status, "failed");
    }

    #[tokio::test]
    async fn retry_action_requeues_only_retryable_failed_vllm_items() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create(
                "vllm_tag",
                serde_json::json!({
                    "type":"vllm_tag",
                    "root_id":"missing",
                    "options":{"media_ids":["done","retry","deterministic"]}
                }),
            )
            .unwrap();
        state
            .database
            .ensure_task_items(
                &task.id,
                &[
                    crate::database::TaskItemInput::new(
                        "media:done",
                        serde_json::json!({"media_id":"done"}),
                    ),
                    crate::database::TaskItemInput::new(
                        "media:retry",
                        serde_json::json!({"media_id":"retry"}),
                    ),
                    crate::database::TaskItemInput::new(
                        "media:deterministic",
                        serde_json::json!({"media_id":"deterministic"}),
                    ),
                ],
            )
            .unwrap();
        state
            .database
            .finish_task_item(
                &task.id,
                "media:done",
                "completed",
                Some(&serde_json::json!({"media_ids":["done"]})),
                None,
            )
            .unwrap();
        for (key, retryable) in [("media:retry", true), ("media:deterministic", false)] {
            state
                .database
                .finish_task_item(
                    &task.id,
                    key,
                    "failed",
                    None,
                    Some(&serde_json::json!({
                        "code":"vllm_invalid_request",
                        "message":"failed",
                        "retryable":retryable
                    })),
                )
                .unwrap();
        }
        state.tasks.start(&task.id).unwrap();
        state
            .tasks
            .fail(
                &task.id,
                TaskFailure {
                    code: "vllm_items_failed".to_string(),
                    message: "failed".to_string(),
                    retryable: true,
                },
            )
            .unwrap();
        let _worker_guard = state
            .worker_slots
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{}/retry", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let items = state.database.list_task_items(&task.id).unwrap();
        assert_eq!(items[0].status, "completed");
        assert_eq!(items[1].status, "queued");
        assert!(items[1].error.is_none());
        assert_eq!(items[2].status, "failed");
        assert_eq!(items[2].error.as_ref().unwrap()["retryable"], false);
    }

    #[tokio::test]
    async fn worker_only_acknowledges_pause_after_crossing_the_commit_boundary() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("pause-boundary-root");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("42.jpg"), b"pending-index").unwrap();
        state
            .database
            .create_root(
                "pause-boundary-root",
                "Pause boundary",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let writer = state.root_writes.acquire(&media).await.unwrap();
        let task = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({
                    "type": "index_library",
                    "root_id": "pause-boundary-root"
                }),
            )
            .unwrap();
        spawn_task_worker(state.clone(), task.id.clone()).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state.tasks.get(&task.id).unwrap().unwrap().status
                    == crate::tasks::TaskStatus::Running
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let requested = state.tasks.pause(&task.id).unwrap();

        assert_eq!(requested.status, crate::tasks::TaskStatus::Pausing);
        assert_eq!(
            state
                .database
                .count_media_files("pause-boundary-root")
                .unwrap(),
            0
        );
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state.tasks.get(&task.id).unwrap().unwrap().status
                    == crate::tasks::TaskStatus::Paused
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker should acknowledge only after it has stopped writing");
        assert_eq!(
            state
                .database
                .count_media_files("pause-boundary-root")
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn library_items_are_cursor_paginated_by_root_without_paths_from_the_request() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("42.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-42".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/items?root_id=root-1&limit=60")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["items"][0]["id"], "media-42");
        assert_eq!(json["data"]["items"][0]["filename"], "42.jpg");
        assert_eq!(json["data"]["total"], 1);
        assert!(json["data"].get("next_cursor").is_none());
    }

    #[tokio::test]
    async fn library_item_detail_is_loaded_by_media_id_without_a_path_parameter() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("detail-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "detail-root",
                "Detail Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "detail-media-42".into(),
                root_id: "detail-root".into(),
                post_id: None,
                relative_path: "nested/42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/items/detail-media-42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["id"], "detail-media-42");
        assert_eq!(json["data"]["relative_path"], "nested/42.jpg");
        assert!(json.to_string().contains("detail-media-42"));
        assert!(!json
            .to_string()
            .contains(directory.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn library_thumbnail_is_a_bounded_cached_image_instead_of_the_original() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("thumbnail-media");
        std::fs::create_dir_all(&media).unwrap();
        let source = media.join("large.png");
        image::RgbImage::from_pixel(1_200, 800, image::Rgb([20, 80, 140]))
            .save(&source)
            .unwrap();
        let source_size = std::fs::metadata(&source).unwrap().len();
        state
            .database
            .create_root(
                "thumbnail-root",
                "Thumbnail Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "large-media".into(),
                root_id: "thumbnail-root".into(),
                post_id: None,
                relative_path: "large.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: i64::try_from(source_size).unwrap(),
                sha256: None,
                md5: None,
                width: Some(1_200),
                height: Some(800),
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/media/large-media/thumbnail")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/jpeg");
        let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let thumbnail = image::load_from_memory(&body).unwrap();
        assert!(thumbnail.width() <= 480);
        assert!(thumbnail.height() <= 480);
        assert_ne!((thumbnail.width(), thumbnail.height()), (1_200, 800));
    }

    #[tokio::test]
    async fn library_items_include_post_rating_and_classified_tags() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let post = crate::services::danbooru::Post {
            id: 42,
            rating: "q".into(),
            tag_string: "cat artist_name".into(),
            tag_string_general: "cat".into(),
            tag_string_artist: "artist_name".into(),
            ..Default::default()
        };
        state
            .database
            .upsert_post_with_tags(
                &super::post_record_input(&post),
                &super::post_tag_inputs(&post),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-42".into(),
                root_id: "root-1".into(),
                post_id: Some(42),
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/items?root_id=root-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["items"][0]["rating"], "q");
        assert_eq!(
            json["data"]["items"][0]["tags"],
            serde_json::json!(["cat", "artist_name"])
        );
    }

    #[tokio::test]
    async fn library_media_is_resolved_by_id_and_supports_range() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("42.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-42".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/media/media-42/file")
                    .header("range", "bytes=1-2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(&body[..], b"pe");
    }

    #[tokio::test]
    async fn quarantine_entry_restores_without_overwriting_the_destination() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        let quarantined = media.join(".danbooru-quarantine/batch-1");
        std::fs::create_dir_all(&quarantined).unwrap();
        std::fs::write(quarantined.join("42.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "entry-1".into(),
                root_id: "root-1".into(),
                media_file_id: None,
                original_relative_path: "42.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch-1/42.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/library/quarantine/entry-1/restore")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["id"], "entry-1");
        assert!(media.join("42.jpg").is_file());
        assert!(!quarantined.join("42.jpg").exists());
        assert!(state
            .database
            .list_quarantine("root-1", false)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn restore_database_failure_moves_the_file_back_to_quarantine() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        let quarantined = media.join(".danbooru-quarantine/batch-restore-failure");
        std::fs::create_dir_all(&quarantined).unwrap();
        std::fs::write(quarantined.join("42.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-restore-failure",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "entry-restore-failure".into(),
                root_id: "root-restore-failure".into(),
                media_file_id: None,
                original_relative_path: "42.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch-restore-failure/42.jpg"
                    .into(),
                reason: "duplicate".into(),
                sha256: None,
            })
            .unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("data/danbooru_tool.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_quarantine_restore
                 BEFORE UPDATE OF restored_at ON quarantine
                 BEGIN
                   SELECT RAISE(ABORT, 'forced restore database failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/library/quarantine/entry-restore-failure/restore")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(quarantined.join("42.jpg").is_file());
        assert!(!media.join("42.jpg").exists());
        assert_eq!(
            state
                .database
                .list_quarantine("root-restore-failure", false)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn manually_purging_quarantine_only_deletes_registered_quarantine_files() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        let quarantined = media.join(".danbooru-quarantine/batch-1");
        std::fs::create_dir_all(&quarantined).unwrap();
        std::fs::write(quarantined.join("42.jpg"), b"jpeg").unwrap();
        std::fs::write(media.join("keep.jpg"), b"keep").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "entry-1".into(),
                root_id: "root-1".into(),
                media_file_id: None,
                original_relative_path: "42.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch-1/42.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/library/quarantine?root_id=root-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["purged"], 1);
        assert!(!quarantined.join("42.jpg").exists());
        assert!(media.join("keep.jpg").exists());
        assert!(state
            .database
            .list_quarantine("root-1", false)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn purge_database_failure_keeps_the_registered_quarantine_file() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        let quarantined = media.join(".danbooru-quarantine/batch-purge-failure");
        std::fs::create_dir_all(&quarantined).unwrap();
        std::fs::write(quarantined.join("42.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-purge-failure",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "entry-purge-failure".into(),
                root_id: "root-purge-failure".into(),
                media_file_id: None,
                original_relative_path: "42.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch-purge-failure/42.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            })
            .unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("data/danbooru_tool.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_quarantine_purge
                 BEFORE DELETE ON quarantine
                 BEGIN
                   SELECT RAISE(ABORT, 'forced purge database failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        let response = application
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/library/quarantine?root_id=root-purge-failure")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(std::fs::read(quarantined.join("42.jpg")).unwrap(), b"jpeg");
        assert_eq!(
            state
                .database
                .list_quarantine("root-purge-failure", false)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn purge_file_failure_restores_the_database_record_and_media_status() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("media");
        let quarantined = media.join(".danbooru-quarantine/batch-file-failure");
        std::fs::create_dir_all(&quarantined).unwrap();
        let quarantine_file = quarantined.join("42.jpg");
        std::fs::write(&quarantine_file, b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-file-failure",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-file-failure".into(),
                root_id: "root-file-failure".into(),
                post_id: None,
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        let record = state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "entry-file-failure".into(),
                root_id: "root-file-failure".into(),
                media_file_id: Some("media-file-failure".into()),
                original_relative_path: "42.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch-file-failure/42.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            })
            .unwrap();

        let error = purge_registered_quarantine_file_with(
            &state.database,
            &record,
            &quarantine_file,
            |_| Err(std::io::Error::other("forced file removal failure")),
        )
        .unwrap_err();

        assert_eq!(error.code, "internal_error");
        assert!(quarantine_file.is_file());
        assert_eq!(
            state
                .database
                .list_quarantine("root-file-failure", false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state
                .database
                .get_media_file("media-file-failure")
                .unwrap()
                .unwrap()
                .status,
            "quarantined"
        );
    }

    #[tokio::test]
    async fn quarantine_database_failure_restores_the_whole_filesystem_batch() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        for name in ["a.jpg", "b.jpg"] {
            std::fs::write(media.join(name), b"same-image").unwrap();
            std::fs::write(media.join(name).with_extension("txt"), b"remove_me").unwrap();
        }
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        for (id, relative_path) in [("media-a", "a.jpg"), ("media-b", "b.jpg")] {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "root-1".into(),
                    post_id: None,
                    relative_path: relative_path.into(),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 10,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }

        let root =
            crate::services::image_processor::VerifiedMediaRoot::open(media.clone()).unwrap();
        let manifest =
            crate::services::image_processor::plan_delete_by_tag(&root, "remove_me").unwrap();
        state
            .database
            .quarantine_media(&crate::database::QuarantineInput {
                id: "pre-existing-conflict".into(),
                root_id: "root-1".into(),
                media_file_id: None,
                original_relative_path: "unrelated.jpg".into(),
                quarantine_relative_path: format!(
                    ".danbooru-quarantine/{}/b.jpg",
                    manifest.batch_id
                ),
                reason: "pre-existing".into(),
                sha256: None,
            })
            .unwrap();
        let task = state
            .tasks
            .create("delete_by_tag", serde_json::json!({}))
            .unwrap();
        state.tasks.start(&task.id).unwrap();

        assert!(
            apply_tool_manifest(&state, &task.id, "root-1", media.clone(), manifest)
                .await
                .is_err()
        );

        for name in ["a.jpg", "a.txt", "b.jpg", "b.txt"] {
            assert!(media.join(name).is_file(), "{name} was not restored");
        }
        let records = state.database.list_quarantine("root-1", true).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "pre-existing-conflict");
        for id in ["media-a", "media-b"] {
            assert_eq!(
                state.database.get_media_file(id).unwrap().unwrap().status,
                "active"
            );
        }
    }

    #[tokio::test]
    async fn quarantine_records_store_each_companion_files_own_sha256() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("sidecar-hash-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("image.jpg"), b"primary-content").unwrap();
        std::fs::write(media.join("image.txt"), b"remove_me").unwrap();
        state
            .database
            .create_root(
                "root-sidecar-hash",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let root = VerifiedMediaRoot::open(&media).unwrap();
        let manifest = plan_delete_by_tag(&root, "remove_me").unwrap();
        let task = state
            .tasks
            .create("delete_by_tag", serde_json::json!({}))
            .unwrap();
        state.tasks.start(&task.id).unwrap();

        apply_tool_manifest(&state, &task.id, "root-sidecar-hash", media, manifest)
            .await
            .unwrap();

        let records = state
            .database
            .list_quarantine("root-sidecar-hash", false)
            .unwrap();
        let sidecar = records
            .iter()
            .find(|record| record.original_relative_path == "image.txt")
            .expect("sidecar quarantine record");
        assert_eq!(
            sidecar.sha256.as_deref(),
            Some(hex::encode(Sha256::digest(b"remove_me")).as_str())
        );
    }

    #[tokio::test]
    async fn download_task_streams_media_and_registers_it_in_the_library() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "source": { "type": "post_ids", "post_ids": [42] },
            "root_id": "root-1",
            "limit": 1,
            "concurrency": 8,
            "filename_template": "{id}.{ext}",
            "skip_existing": true,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(std::fs::read(media.join("42.jpg")).unwrap(), b"jpeg");
        assert_eq!(state.database.count_media_files("root-1").unwrap(), 1);
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "completed");
        assert_eq!(items[0].result.as_ref().unwrap()["bytes"], 4);
    }

    #[tokio::test]
    async fn download_task_saves_into_a_safe_library_subdirectory() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("categorized-download-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "categorized-root",
                "分类下载",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "categorized-root",
                            "relative_directory": "角色/爱丽丝",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(
            std::fs::read(media.join("角色/爱丽丝/42.jpg")).unwrap(),
            b"jpeg"
        );
        let registered = state
            .database
            .find_active_media_for_download("categorized-root", Some(42), None, "original")
            .unwrap()
            .unwrap();
        assert_eq!(registered.relative_path, "角色/爱丽丝/42.jpg");
    }

    #[tokio::test]
    async fn download_task_pauses_between_chunks_and_resumes_the_part_with_range() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("resumable-task-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-resumable",
                "Resumable",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let chunks_sent = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(StdMutex::new(Vec::new()));
        let (base_url, server) =
            mock_slow_resumable_download(chunks_sent.clone(), ranges.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "root-resumable",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while chunks_sent.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let live = state.tasks.get(&task_id).unwrap().unwrap();
        assert!(
            live.bytes_processed > 0,
            "chunk bytes must be observable live"
        );
        assert!(
            live.speed_bytes_per_sec > 0,
            "chunk speed must be observable live"
        );
        assert!(
            live.eta_seconds.is_some(),
            "chunk ETA must be observable live"
        );

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{task_id}/pause"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_millis(350), async {
            while state.tasks.get(&task_id).unwrap().unwrap().status != TaskStatus::Paused {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("download worker must acknowledge pause at the next body chunk");

        assert!(!media.join("42.jpg").exists());
        let part_len = std::fs::metadata(media.join("42.jpg.part")).unwrap().len();
        assert!(part_len > 0 && part_len < 40);
        assert_eq!(
            state.database.list_task_items(&task_id).unwrap()[0].status,
            "queued"
        );

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{task_id}/resume"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(
            std::fs::read(media.join("42.jpg")).unwrap(),
            b"0123456789012345678901234567890123456789"
        );
        assert!(!media.join("42.jpg.part").exists());
        let captured = ranges.lock().unwrap();
        assert!(captured.len() >= 2);
        assert_eq!(
            captured[1].as_deref(),
            Some(format!("bytes={part_len}-").as_str())
        );
        assert_eq!(
            state.database.list_task_items(&task_id).unwrap()[0].status,
            "completed"
        );
    }

    #[tokio::test]
    async fn post_id_download_records_one_failure_and_continues_to_the_success_target() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media-continue-after-failure");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let (base_url, server) = mock_download_with_failed_first_post().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [1, 2] },
                            "root_id": "root-1",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(std::fs::read(media.join("2.jpg")).unwrap(), b"jpeg");
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].status, "failed");
        assert_eq!(items[1].status, "completed");

        let history = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/downloads/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(history.into_body(), 64 * 1024).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["data"]["items"][0]["total_items"], 2);
        assert_eq!(json["data"]["items"][0]["completed_items"], 1);
        assert_eq!(json["data"]["items"][0]["failed_items"], 1);

        let snapshot = application
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(snapshot.into_body(), 64 * 1024).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["data"]["tasks"][0]["failures"][0]["item_id"], "post:1");
        assert_eq!(
            json["data"]["tasks"][0]["failures"][0]["code"],
            "danbooru_upstreamunavailable"
        );
    }

    #[tokio::test]
    async fn existing_final_is_not_counted_as_a_new_query_download() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("existing-final-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("2.jpg"), b"jpeg").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        let media_requests = Arc::new(AtomicUsize::new(0));
        let (base_url, server) = mock_query_with_existing_first(media_requests.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "source": { "type": "query", "query": "cat" },
            "root_id": "root-1",
            "limit": 1,
            "concurrency": 1,
            "filename_template": "{id}.{ext}",
            "skip_existing": false,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(completed.result.as_ref().unwrap()["downloaded"], 1);
        assert_eq!(completed.result.as_ref().unwrap()["skipped"], 1);
        assert_eq!(std::fs::read(media.join("2.jpg")).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(media.join("1.jpg")).unwrap(), b"jpeg");
        assert_eq!(media_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn media_downloaded_in_one_root_does_not_skip_another_root() {
        let (application, state, directory) = test_router();
        let first_root = directory.path().join("first-media");
        let second_root = directory.path().join("second-media");
        std::fs::create_dir_all(&first_root).unwrap();
        std::fs::create_dir_all(&second_root).unwrap();
        std::fs::write(first_root.join("42.jpg"), b"jpeg").unwrap();
        for (id, name, path) in [
            ("root-1", "Library 1", &first_root),
            ("root-2", "Library 2", &second_root),
        ] {
            state
                .database
                .create_root(
                    id,
                    name,
                    Some(&path.to_string_lossy()),
                    Some(&path.to_string_lossy()),
                )
                .unwrap();
        }
        state
            .database
            .upsert_post_with_tags(
                &crate::database::PostRecordInput {
                    id: 42,
                    md5: None,
                    rating: "g".into(),
                    score: 9,
                    fav_count: 0,
                    width: 2,
                    height: 2,
                    file_ext: Some("jpg".into()),
                    file_size: Some(4),
                    source: None,
                    duration: None,
                    status: "available".into(),
                    tag_string: String::new(),
                    tag_string_general: String::new(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                &[],
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "root-1:42:original".into(),
                root_id: "root-1".into(),
                post_id: Some(42),
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "root-2",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(completed.status, crate::tasks::TaskStatus::Completed);
        assert_eq!(std::fs::read(second_root.join("42.jpg")).unwrap(), b"jpeg");
        assert!(state
            .database
            .find_active_media_for_download("root-2", Some(42), None, "original")
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn skip_existing_rejects_a_truncated_registered_file() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("truncated-existing-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("42.jpg"), b"bad").unwrap();
        state
            .database
            .create_root(
                "root-truncated",
                "Library",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        state
            .database
            .upsert_post_with_tags(
                &crate::database::PostRecordInput {
                    id: 42,
                    md5: None,
                    rating: "g".into(),
                    score: 9,
                    fav_count: 0,
                    width: 2,
                    height: 2,
                    file_ext: Some("jpg".into()),
                    file_size: Some(4),
                    source: None,
                    duration: None,
                    status: "available".into(),
                    tag_string: String::new(),
                    tag_string_general: String::new(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                &[],
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "root-truncated:42:original".into(),
                root_id: "root-truncated".into(),
                post_id: Some(42),
                relative_path: "42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let post = client.post(42).await.unwrap();

        let error = download_post(
            &state,
            &client,
            None,
            "root-truncated",
            &media,
            "{id}.{ext}",
            crate::config::UgoiraPolicy::default(),
            true,
            &post,
        )
        .await
        .unwrap_err();
        server.abort();

        assert_eq!(error.code, "danbooru_integrity");
        assert_eq!(std::fs::read(media.join("42.jpg")).unwrap(), b"bad");
    }

    #[tokio::test]
    async fn stale_download_record_with_missing_file_is_downloaded_again() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("stale-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        state
            .database
            .upsert_post_with_tags(
                &crate::database::PostRecordInput {
                    id: 42,
                    md5: None,
                    rating: "g".into(),
                    score: 9,
                    fav_count: 0,
                    width: 2,
                    height: 2,
                    file_ext: Some("jpg".into()),
                    file_size: Some(4),
                    source: None,
                    duration: None,
                    status: "available".into(),
                    tag_string: String::new(),
                    tag_string_general: String::new(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                &[],
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "root-1:42:original".into(),
                root_id: "root-1".into(),
                post_id: Some(42),
                relative_path: "missing.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "root-1",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(completed.status, crate::tasks::TaskStatus::Completed);
        assert_eq!(std::fs::read(media.join("42.jpg")).unwrap(), b"jpeg");
        assert_eq!(
            state
                .database
                .get_media_file("root-1:42:original")
                .unwrap()
                .unwrap()
                .relative_path,
            "42.jpg"
        );
    }

    #[tokio::test]
    async fn zip_only_ugoira_policy_does_not_require_a_webm_variant() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("zip-only-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        let (base_url, server) = mock_ugoira_zip_only().await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "root-1",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "zip_only" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(std::fs::read(media.join("42.zip")).unwrap(), b"zip");
        assert!(state
            .database
            .find_active_media_for_download("root-1", Some(42), None, "ugoira_zip")
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn webm_and_zip_policy_downloads_only_the_missing_ugoira_variant() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("partial-ugoira-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("saved.webm"), b"webm").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(&media.to_string_lossy()),
                Some(&media.to_string_lossy()),
            )
            .unwrap();
        state
            .database
            .upsert_post_with_tags(
                &crate::database::PostRecordInput {
                    id: 42,
                    md5: None,
                    rating: "g".into(),
                    score: 9,
                    fav_count: 0,
                    width: 2,
                    height: 2,
                    file_ext: Some("zip".into()),
                    file_size: Some(3),
                    source: None,
                    duration: None,
                    status: "available".into(),
                    tag_string: String::new(),
                    tag_string_general: String::new(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                &[],
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "root-1:42:ugoira_webm".into(),
                root_id: "root-1".into(),
                post_id: Some(42),
                relative_path: "saved.webm".into(),
                variant: "ugoira_webm".into(),
                mime_type: "video/webm".into(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(2),
                height: Some(2),
                duration: None,
            })
            .unwrap();
        let webm_requests = Arc::new(AtomicUsize::new(0));
        let zip_requests = Arc::new(AtomicUsize::new(0));
        let (base_url, server) =
            mock_ugoira_variants(webm_requests.clone(), zip_requests.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [42] },
                            "root_id": "root-1",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(webm_requests.load(Ordering::SeqCst), 0);
        assert_eq!(zip_requests.load(Ordering::SeqCst), 1);
        assert!(!media.join("42.webm").exists());
        assert_eq!(std::fs::read(media.join("42.zip")).unwrap(), b"zip");
        assert_eq!(state.database.count_media_files("root-1").unwrap(), 2);
    }

    #[tokio::test]
    async fn exact_dedup_requires_confirmation_then_moves_duplicate_to_quarantine() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("a.jpg"), b"same-bytes").unwrap();
        std::fs::write(media.join("b.jpg"), b"same-bytes").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        for name in ["a.jpg", "b.jpg"] {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: format!("media-{name}"),
                    root_id: "root-1".into(),
                    post_id: None,
                    relative_path: name.into(),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 10,
                    sha256: None,
                    md5: None,
                    width: None,
                    height: None,
                    duration: None,
                })
                .unwrap();
        }

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "exact_dedup",
                            "root_id": "root-1",
                            "options": { "preflight": true }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while state.tasks.get(&task_id).unwrap().unwrap().status
                != crate::tasks::TaskStatus::AwaitingConfirmation
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let snapshot = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let snapshot = to_bytes(snapshot.into_body(), 256 * 1024).await.unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
        assert_eq!(
            snapshot["data"]["tasks"][0]["preview"]["candidates"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "destructive confirmation must expose its preflight manifest",
        );
        application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{task_id}/confirm"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        let quarantined = state.database.list_quarantine("root-1", false).unwrap();
        assert_eq!(quarantined.len(), 1);
        let media_id = quarantined[0]
            .media_file_id
            .as_deref()
            .expect("registered media must remain linked to quarantine");
        assert_eq!(
            state
                .database
                .get_media_file(media_id)
                .unwrap()
                .unwrap()
                .status,
            "quarantined"
        );
        assert_eq!(
            usize::from(media.join("a.jpg").exists()) + usize::from(media.join("b.jpg").exists()),
            1
        );
    }

    #[tokio::test]
    async fn near_dedup_requires_confirmation_and_restores_without_overwriting() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("near-dedup-media");
        std::fs::create_dir_all(&media).unwrap();
        let image = image::GrayImage::from_pixel(32, 32, image::Luma([128]));
        for name in ["b.png", "a.png"] {
            image.save(media.join(name)).unwrap();
        }
        state
            .database
            .create_root(
                "root-near-dedup",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        for name in ["a.png", "b.png"] {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: format!("media-{name}"),
                    root_id: "root-near-dedup".into(),
                    post_id: None,
                    relative_path: name.into(),
                    variant: "original".into(),
                    mime_type: "image/png".into(),
                    byte_size: std::fs::metadata(media.join(name)).unwrap().len() as i64,
                    sha256: None,
                    md5: None,
                    width: Some(32),
                    height: Some(32),
                    duration: None,
                })
                .unwrap();
        }

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "near_dedup",
                            "root_id": "root-near-dedup",
                            "options": { "distance": 1 }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let preview = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if task.status == TaskStatus::AwaitingConfirmation {
                    break task.preview.unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(preview["pairs"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            preview["candidates"][0]["relative_path"],
            serde_json::json!("b.png")
        );
        assert!(media.join("a.png").is_file());
        assert!(media.join("b.png").is_file());

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{task_id}/confirm"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert!(media.join("a.png").is_file());
        assert!(!media.join("b.png").exists());

        let quarantined = state
            .database
            .list_quarantine("root-near-dedup", false)
            .unwrap();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].original_relative_path, "b.png");
        assert_eq!(quarantined[0].media_file_id.as_deref(), Some("media-b.png"));
        std::fs::write(media.join("b.png"), b"conflicting replacement").unwrap();
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/library/quarantine/{}/restore",
                        quarantined[0].id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read(media.join("b.png")).unwrap(),
            b"conflicting replacement"
        );
        assert_eq!(
            state
                .database
                .list_quarantine("root-near-dedup", false)
                .unwrap()
                .len(),
            1
        );

        std::fs::remove_file(media.join("b.png")).unwrap();
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/library/quarantine/{}/restore",
                        quarantined[0].id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(media.join("b.png").is_file());
        assert_eq!(
            state
                .database
                .get_media_file("media-b.png")
                .unwrap()
                .unwrap()
                .status,
            "active"
        );
    }

    #[tokio::test]
    async fn integrity_scan_requires_confirmation_before_quarantining_corrupt_media() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("integrity-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("broken.jpg"), b"not-a-jpeg").unwrap();
        state
            .database
            .create_root(
                "root-integrity",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-broken".into(),
                root_id: "root-integrity".into(),
                post_id: None,
                relative_path: "broken.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 10,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "integrity_scan",
                            "root_id": "root-integrity",
                            "options": { "preflight": true }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while state.tasks.get(&task_id).unwrap().unwrap().status
                != TaskStatus::AwaitingConfirmation
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(media.join("broken.jpg").exists());

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{task_id}/confirm"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert!(!media.join("broken.jpg").exists());
        let quarantined = state
            .database
            .list_quarantine("root-integrity", false)
            .unwrap();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].original_relative_path, "broken.jpg");
        assert!(quarantined[0].reason.starts_with("decode_failed:"));
    }

    #[tokio::test]
    async fn explicit_index_task_registers_existing_media_without_moving_it() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("42_score_9.jpg"), b"existing").unwrap();
        std::fs::write(media.join("42_score_9.txt"), "cat, solo").unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "downloaded-42".to_string(),
                root_id: "root-1".to_string(),
                post_id: None,
                relative_path: "42_score_9.jpg".to_string(),
                variant: "original".to_string(),
                mime_type: "image/jpeg".to_string(),
                byte_size: 7,
                sha256: Some("existing-sha".to_string()),
                md5: Some("existing-md5".to_string()),
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "type": "index_library", "root_id": "root-1" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(state.database.count_media_files("root-1").unwrap(), 1);
        let indexed = state
            .database
            .get_media_file("downloaded-42")
            .unwrap()
            .unwrap();
        assert_eq!(indexed.post_id, Some(42));
        assert_eq!(indexed.md5.as_deref(), Some("existing-md5"));
        assert_eq!(
            state
                .database
                .get_post_library_metadata(42)
                .unwrap()
                .unwrap()
                .rating,
            "unknown"
        );
        assert_eq!(
            state
                .database
                .get_root("root-1")
                .unwrap()
                .unwrap()
                .indexing_status,
            "indexed"
        );
        assert!(media.join("42_score_9.jpg").exists());
    }

    #[tokio::test]
    async fn refreshing_a_library_removes_records_for_files_deleted_outside_the_app() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("refresh-library-media");
        std::fs::create_dir_all(&media).unwrap();
        let image = media.join("external.jpg");
        std::fs::write(&image, b"external-file").unwrap();
        state
            .database
            .create_root(
                "refresh-root",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let initial = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({ "type": "index_library", "root_id": "refresh-root" }),
            )
            .unwrap();
        let initial = state.tasks.start(&initial.id).unwrap();
        super::run_index_task(&state, &initial).await.unwrap();
        assert_eq!(state.database.count_media_files("refresh-root").unwrap(), 1);

        std::fs::remove_file(image).unwrap();
        let refreshed = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({ "type": "index_library", "root_id": "refresh-root" }),
            )
            .unwrap();
        let refreshed = state.tasks.start(&refreshed.id).unwrap();
        super::run_index_task(&state, &refreshed).await.unwrap();

        assert_eq!(state.database.count_media_files("refresh-root").unwrap(), 0);
    }

    #[tokio::test]
    async fn failed_index_task_does_not_leave_the_root_stuck_as_indexing() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("failed-index-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("fixture.jpg"), b"fixture").unwrap();
        state
            .database
            .create_root(
                "root-index-failure",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("data/danbooru_tool.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_index_media
                 BEFORE INSERT ON media_files
                 WHEN NEW.root_id = 'root-index-failure'
                 BEGIN SELECT RAISE(ABORT, 'injected index failure'); END;",
            )
            .unwrap();
        drop(connection);

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "index_library",
                            "root_id": "root-index-failure"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let failed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(
            state
                .database
                .get_root("root-index-failure")
                .unwrap()
                .unwrap()
                .indexing_status,
            "not_indexed"
        );
    }

    #[test]
    fn index_sidecar_reader_rejects_a_canonical_path_outside_the_media_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sidecar = outside.path().join("secret.txt");
        std::fs::write(&sidecar, "must_not_be_indexed").unwrap();
        let canonical_root = std::fs::canonicalize(root.path()).unwrap();

        assert!(super::read_sidecar_tags(&canonical_root, &sidecar).is_empty());
    }

    #[tokio::test]
    async fn download_worker_honors_bounded_post_concurrency() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (base_url, server) = mock_concurrent_downloads(active, maximum.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "source": {
                "type": "post_ids",
                "post_ids": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
            },
            "root_id": "root-1",
            "limit": 16,
            "concurrency": 16,
            "filename_template": "{id}.{ext}",
            "skip_existing": true,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(maximum.load(Ordering::SeqCst), 16);
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(items.len(), 16);
        assert!(items.iter().all(|item| item.status == "completed"));
    }

    #[tokio::test]
    async fn download_database_failure_removes_new_file_and_rolls_back_post_metadata() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("database-rollback-media");
        std::fs::create_dir_all(&media).unwrap();
        let (base_url, server) = mock_danbooru_download().await;
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let post = client.post(42).await.unwrap();

        let result = download_post(
            &state,
            &client,
            None,
            "missing-root",
            &media,
            "{id}.{ext}",
            crate::config::UgoiraPolicy::default(),
            false,
            &post,
        )
        .await;
        server.abort();

        assert!(result.is_err());
        assert!(!media.join("42.jpg").exists(), "{result:?}");
        assert!(state
            .database
            .get_post_library_metadata(42)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn later_ugoira_variant_failure_removes_the_newly_downloaded_webm() {
        async fn webm() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "video/webm"), ("content-length", "4")],
                "webm",
            )
        }
        async fn zip_failure() -> StatusCode {
            StatusCode::INTERNAL_SERVER_ERROR
        }

        let (_application, state, directory) = test_router();
        let media = directory.path().join("ugoira-rollback-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "ugoira-root",
                "Ugoira",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/playable.webm", get(webm))
                    .route("/original.zip", get(zip_failure)),
            )
            .await
            .unwrap();
        });
        let base_url = format!("http://{address}/");
        let client = DanbooruClient::new(DanbooruClientConfig {
            base_url: base_url.clone(),
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let post = Post {
            id: 42,
            rating: "g".into(),
            file_ext: Some("zip".into()),
            file_url: Some(format!("{base_url}original.zip")),
            media_asset: Some(MediaAsset {
                variants: vec![MediaAssetVariant {
                    variant_type: "sample".into(),
                    url: Some(format!("{base_url}playable.webm")),
                    file_ext: Some("webm".into()),
                    ..MediaAssetVariant::default()
                }],
                ..MediaAsset::default()
            }),
            ..Post::default()
        };

        let result = download_post(
            &state,
            &client,
            None,
            "ugoira-root",
            &media,
            "{id}.{ext}",
            crate::config::UgoiraPolicy::WebmAndZip,
            false,
            &post,
        )
        .await;
        server.abort();

        assert!(result.is_err());
        assert!(!media.join("42.webm").exists(), "{result:?}");
        assert_eq!(state.database.count_media_files("ugoira-root").unwrap(), 0);
    }

    #[tokio::test]
    async fn query_download_worker_honors_bounded_media_concurrency() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (base_url, server) = mock_concurrent_downloads(active, maximum.clone()).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let request = serde_json::json!({
            "type": "download",
            "source": { "type": "query", "query": "cat order:score" },
            "root_id": "root-1",
            "limit": 2,
            "concurrency": 2,
            "filename_template": "{id}.{ext}",
            "skip_existing": true,
            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
        });
        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.status == "completed"));
    }

    #[tokio::test]
    async fn query_download_persists_only_items_that_enter_the_worker_queue() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("exact-query-items");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-exact-query-items",
                "Exact query items",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (base_url, server) = mock_concurrent_downloads(active, maximum).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "query", "query": "cat order:score" },
                            "root_id": "root-exact-query-items",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        server.abort();

        assert_eq!(
            completed.status,
            TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        let items = state.database.list_task_items(&task_id).unwrap();
        assert_eq!(
            items.len(),
            1,
            "unstarted page results must not become queued items"
        );
        assert_eq!(items[0].status, "completed");
    }

    #[tokio::test]
    async fn immediately_resumed_download_is_not_left_queued_without_a_worker() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-1",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (base_url, server) = mock_concurrent_downloads(active.clone(), maximum).await;
        *state.danbooru.write().await = DanbooruClient::new(DanbooruClientConfig {
            base_url,
            requests_per_second: 1_000,
            trusted_media_hosts: vec!["127.0.0.1".into()],
            ..DanbooruClientConfig::default()
        })
        .unwrap();
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "type": "download",
                            "source": { "type": "post_ids", "post_ids": [1] },
                            "root_id": "root-1",
                            "limit": 1,
                            "concurrency": 1,
                            "filename_template": "{id}.{ext}",
                            "skip_existing": true,
                            "media_policy": { "original": true, "ugoira": "webm_and_zip" }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let task_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        for action in ["pause", "resume"] {
            let response = application
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/tasks/{task_id}/{action}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let task = state.tasks.get(&task_id).unwrap().unwrap();
                if matches!(
                    task.status,
                    crate::tasks::TaskStatus::Completed | crate::tasks::TaskStatus::Failed
                ) {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("resumed task must retain or acquire a worker");
        server.abort();

        assert_eq!(
            completed.status,
            crate::tasks::TaskStatus::Completed,
            "{:?}",
            completed.error
        );
        assert_eq!(completed.completed_items, 1);
    }
}
