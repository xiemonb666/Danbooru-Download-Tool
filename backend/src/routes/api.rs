use crate::app_paths::AppPaths;
use crate::config::{
    apply_vllm_base_url_override, load_settings, migrate_legacy_secret, migrate_legacy_settings,
    save_settings, PublicConfig, StoredSettings,
};
use crate::database::{
    Database, DownloadTaskHistoryCursor, LibraryMediaFilters, LibraryResolutionRange,
    LibraryScoreRange, MediaFileInput, MediaFileRecord, PostRecordInput, PostTagInput,
    QuarantineInput, QuarantineRecord, RootRecord, TaskItemInput,
};
use crate::media_root::{normalize_windows_path, MediaRoot};
use crate::secrets::{SecretKind, SecretManager, SystemCredentialVault};
use crate::services::danbooru::{
    validate_filename_template, AutocompleteItem, ControlledDownloadOutcome, DanbooruClient,
    DanbooruClientConfig, DanbooruError, DanbooruErrorKind, DownloadControl, DownloadProgress,
    MediaDownloadRequest, MediaVariant, Post, PostQuery,
};
use crate::services::dataset_augmentation::{
    AnimeCropAnalysis, DatasetAugmentationConfig, DatasetAugmentationItemResult,
    DatasetAugmentationSource, DatasetAugmentationWorkspace, SmartCropConfig,
};
use crate::services::image_processor::{
    apply_heic_conversion, apply_quarantine, apply_tag_pipeline, collect_tag_pipeline_tokens,
    is_quarantine_dir_name, plan_delete_by_tag, plan_delete_by_tag_selected, plan_delete_selected,
    plan_exact_duplicates, plan_exact_duplicates_selected, plan_heic_conversion,
    plan_integrity_check, plan_integrity_check_selected, plan_near_duplicates,
    plan_near_duplicates_selected, plan_tag_pipeline_classified, resize_to_jpeg_with_quarantine,
    restore_quarantine as restore_batch, rollback_heic_conversion, rollback_tag_pipeline,
    ArtistPrefix, TagPipelineConfig, ToolManifest, VerifiedMediaRoot,
};
use crate::services::vllm::{
    TagWriteMode, VllmBatchItem, VllmBatchResult, VllmError, VllmErrorKind, VllmHealth,
    VllmOutputOptions, VllmRetryItem, VllmService, VllmServiceConfig, VllmTagSuccess,
};
use crate::tasks::{
    task_from_record, SqliteTaskStore, TaskFailure, TaskManager, TaskManagerError, TaskSnapshot,
    TaskStatus,
};
use crate::training::{
    augment_adapter_with_upstream_fields, builtin_adapters, parse_metric_line, serialize_toml,
    GpuLeaseManager, TrainingGalleryDataset, TrainingRequest, UpstreamParserField,
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
use encoding_rs::{GBK, SHIFT_JIS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use url::Url;
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

const VLLM_LOAD_REQUEST_TTL: Duration = Duration::from_secs(15 * 60);
const TRAINING_TELEMETRY_LAUNCHER: &[u8] =
    include_bytes!("../../../training_runtime/telemetry_launcher.py");
const TRAINING_ADAPTER_INSPECTOR: &[u8] =
    include_bytes!("../../../training_runtime/adapter_inspector.py");
const TRAINING_LORA_SVD_INSPECTOR: &[u8] =
    include_bytes!("../../../training_runtime/lora_svd_inspector.py");
const ANIME_CROP_WORKER: &[u8] = include_bytes!("../../../training_runtime/anime_crop_worker.py");
const KOHYA_RUNTIME_DIRECTORY: &str = "kohya-ss-v26.0.0";
const LORA_SVD_ANALYSIS_TTL: Duration = Duration::from_secs(30 * 60);
const LORA_SVD_ANALYSIS_CACHE_LIMIT: usize = 3;
const LORA_SVD_SUMMARY_SPECTRUM_LIMIT: usize = 4_096;

#[derive(Clone, Default)]
struct VllmLoadCoordinator {
    requested_at: Arc<StdMutex<Option<Instant>>>,
}

#[derive(Clone, Default)]
struct TrainingRuntimeInstallCoordinator {
    states: Arc<StdMutex<HashMap<String, TrainingRuntimeInstallState>>>,
}

#[derive(Debug, Clone, Default)]
struct TrainingRuntimeInstallState {
    active: bool,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct LoraSvdAnalysisCache {
    entries: Arc<StdMutex<HashMap<String, CachedLoraSvdAnalysis>>>,
}

#[derive(Clone)]
struct CachedLoraSvdAnalysis {
    expires_at: u64,
    payload: Value,
}

impl LoraSvdAnalysisCache {
    fn insert(&self, mut payload: Value) -> Result<(String, u64), String> {
        let id = uuid::Uuid::new_v4().to_string();
        let expires_at = training_now_epoch_seconds() + LORA_SVD_ANALYSIS_TTL.as_secs();
        let object = payload
            .as_object_mut()
            .ok_or_else(|| "SVD 分析器返回的根节点不是对象".to_string())?;
        object.insert("id".to_string(), Value::String(id.clone()));
        object.insert("expires_at".to_string(), Value::from(expires_at));
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "SVD 分析缓存锁定失败".to_string())?;
        let now = training_now_epoch_seconds();
        entries.retain(|_, entry| entry.expires_at > now);
        while entries.len() >= LORA_SVD_ANALYSIS_CACHE_LIMIT {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(id, _)| id.clone())
            {
                entries.remove(&oldest);
            } else {
                break;
            }
        }
        entries.insert(
            id.clone(),
            CachedLoraSvdAnalysis {
                expires_at,
                payload,
            },
        );
        Ok((id, expires_at))
    }

    fn get(&self, id: &str) -> Result<Option<CachedLoraSvdAnalysis>, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "SVD 分析缓存锁定失败".to_string())?;
        let now = training_now_epoch_seconds();
        entries.retain(|_, entry| entry.expires_at > now);
        Ok(entries.get(id).cloned())
    }
}

impl TrainingRuntimeInstallCoordinator {
    fn begin(&self, profile: &str) -> bool {
        let mut states = self
            .states
            .lock()
            .expect("training runtime install lock poisoned");
        let state = states.entry(profile.to_string()).or_default();
        if state.active {
            return false;
        }
        state.active = true;
        state.error = None;
        true
    }

    fn complete(&self, profile: &str, result: Result<(), String>) {
        let mut states = self
            .states
            .lock()
            .expect("training runtime install lock poisoned");
        let state = states.entry(profile.to_string()).or_default();
        state.active = false;
        state.error = result.err();
    }

    fn state(&self, profile: &str) -> TrainingRuntimeInstallState {
        self.states
            .lock()
            .expect("training runtime install lock poisoned")
            .get(profile)
            .cloned()
            .unwrap_or_default()
    }
}

impl VllmLoadCoordinator {
    fn begin(&self) -> bool {
        let mut requested_at = self
            .requested_at
            .lock()
            .expect("vLLM load coordinator lock poisoned");
        if requested_at.is_some_and(|started| started.elapsed() < VLLM_LOAD_REQUEST_TTL) {
            return false;
        }
        *requested_at = Some(Instant::now());
        true
    }

    fn clear(&self) {
        self.requested_at
            .lock()
            .expect("vLLM load coordinator lock poisoned")
            .take();
    }
}

#[derive(Debug, Serialize)]
struct VllmLoadResponse {
    state: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct VllmUnloadResponse {
    state: &'static str,
    message: String,
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
    vllm_launcher_root: Option<PathBuf>,
    vllm_loads: VllmLoadCoordinator,
    worker_slots: Arc<Semaphore>,
    training_root: PathBuf,
    training_leases: GpuLeaseManager,
    training_presets: TrainingPresetStore,
    training_runtime_installs: TrainingRuntimeInstallCoordinator,
    lora_svd_analyses: LoraSvdAnalysisCache,
    started_at: Instant,
}

#[derive(Clone)]
struct CachedDanbooruPost {
    post: Post,
    cached_at: Instant,
}

const DANBOORU_POST_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const DANBOORU_POST_CACHE_LIMIT: usize = 500;

#[derive(Clone)]
struct TrainingPresetStore {
    directory: PathBuf,
    write_lock: Arc<StdMutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrainingPresetRecord {
    id: String,
    name: String,
    training: TrainingRequest,
    created_at: u64,
    updated_at: u64,
    versions: Vec<TrainingPresetVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrainingPresetVersion {
    version: u32,
    saved_at: u64,
    name: String,
    training: TrainingRequest,
}

impl TrainingPresetStore {
    fn open(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("无法创建训练预设目录 {}: {error}", directory.display()))?;
        Ok(Self {
            directory,
            write_lock: Arc::new(StdMutex::new(())),
        })
    }

    fn list(&self) -> Result<Vec<TrainingPresetRecord>, String> {
        let mut presets = Vec::new();
        let entries = std::fs::read_dir(&self.directory)
            .map_err(|error| format!("无法读取训练预设目录: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取训练预设条目: {error}"))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let source = std::fs::read(entry.path())
                .map_err(|error| format!("无法读取训练预设: {error}"))?;
            let preset = serde_json::from_slice::<TrainingPresetRecord>(&source)
                .map_err(|error| format!("训练预设文件损坏: {error}"))?;
            presets.push(preset);
        }
        presets.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then(left.id.cmp(&right.id))
        });
        Ok(presets)
    }

    fn get(&self, id: &str) -> Result<Option<TrainingPresetRecord>, String> {
        validate_training_task_id(id).map_err(|error| error.message)?;
        let path = self.directory.join(format!("{id}.json"));
        let source = match std::fs::read(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法读取训练预设: {error}")),
        };
        serde_json::from_slice(&source)
            .map(Some)
            .map_err(|error| format!("训练预设文件损坏: {error}"))
    }

    fn create(
        &self,
        name: String,
        training: TrainingRequest,
    ) -> Result<TrainingPresetRecord, String> {
        let _guard = self
            .write_lock
            .lock()
            .expect("training preset lock poisoned");
        let now = training_now_epoch_seconds();
        let id = uuid::Uuid::new_v4().to_string();
        let preset = TrainingPresetRecord {
            id: id.clone(),
            name: name.clone(),
            training: training.clone(),
            created_at: now,
            updated_at: now,
            versions: vec![TrainingPresetVersion {
                version: 1,
                saved_at: now,
                name,
                training,
            }],
        };
        self.write_unlocked(&preset)?;
        Ok(preset)
    }

    fn update(
        &self,
        id: &str,
        name: String,
        training: TrainingRequest,
    ) -> Result<Option<TrainingPresetRecord>, String> {
        let _guard = self
            .write_lock
            .lock()
            .expect("training preset lock poisoned");
        validate_training_task_id(id).map_err(|error| error.message)?;
        let path = self.directory.join(format!("{id}.json"));
        let source = match std::fs::read(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法读取训练预设: {error}")),
        };
        let mut preset: TrainingPresetRecord = serde_json::from_slice(&source)
            .map_err(|error| format!("训练预设文件损坏: {error}"))?;
        let now = training_now_epoch_seconds();
        let version = preset
            .versions
            .last()
            .map(|version| version.version.saturating_add(1))
            .unwrap_or(1);
        preset.name = name.clone();
        preset.training = training.clone();
        preset.updated_at = now;
        preset.versions.push(TrainingPresetVersion {
            version,
            saved_at: now,
            name,
            training,
        });
        self.write_unlocked(&preset)?;
        Ok(Some(preset))
    }

    fn write_unlocked(&self, preset: &TrainingPresetRecord) -> Result<(), String> {
        let path = self.directory.join(format!("{}.json", preset.id));
        std::fs::write(
            path,
            serde_json::to_vec_pretty(preset)
                .map_err(|error| format!("无法序列化训练预设: {error}"))?,
        )
        .map_err(|error| format!("无法保存训练预设: {error}"))
    }
}

fn training_now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

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
        let vllm_launcher_root = find_vllm_launcher_root(&paths);
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
        let training_root = paths.data_dir.join("training");
        ensure_training_support_scripts(&training_root)?;
        let training_presets = TrainingPresetStore::open(training_root.join("presets"))?;
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
            vllm_launcher_root,
            vllm_loads: VllmLoadCoordinator::default(),
            worker_slots: Arc::new(Semaphore::new(4)),
            training_root,
            training_leases: GpuLeaseManager::default(),
            training_presets,
            training_runtime_installs: TrainingRuntimeInstallCoordinator::default(),
            lora_svd_analyses: LoraSvdAnalysisCache::default(),
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

fn ensure_training_support_scripts(training_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(training_root).map_err(|error| {
        format!(
            "无法创建训练运行时目录 {}: {error}",
            training_root.display()
        )
    })?;
    // The path picker must always open somewhere actionable.  Keep generated
    // training output separate from the immutable run snapshots/logs beneath
    // `training/runs`, and create it once during normal application setup.
    std::fs::create_dir_all(training_root.join("outputs")).map_err(|error| {
        format!(
            "无法创建训练输出目录 {}: {error}",
            training_root.join("outputs").display()
        )
    })?;
    for (name, contents, description) in [
        (
            "telemetry_launcher.py",
            TRAINING_TELEMETRY_LAUNCHER,
            "训练遥测桥接器",
        ),
        (
            "adapter_inspector.py",
            TRAINING_ADAPTER_INSPECTOR,
            "训练参数检查器",
        ),
        (
            "lora_svd_inspector.py",
            TRAINING_LORA_SVD_INSPECTOR,
            "LoRA 奇异值分析器",
        ),
        (
            "anime_crop_worker.py",
            ANIME_CROP_WORKER,
            "动漫智能裁剪检测器",
        ),
    ] {
        let script = training_root.join(name);
        let should_write = std::fs::read(&script)
            .map(|current| current != contents)
            .unwrap_or(true);
        if should_write {
            std::fs::write(&script, contents)
                .map_err(|error| format!("无法安装{description}: {error}"))?;
        }
    }
    Ok(())
}

fn installed_training_runtime_root(training_root: &Path) -> PathBuf {
    training_root.join(KOHYA_RUNTIME_DIRECTORY)
}

fn bundled_training_runtime_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("training_runtime")
        .join(KOHYA_RUNTIME_DIRECTORY)
}

fn install_bundled_training_runtime_source(destination: &Path) -> Result<(), String> {
    let source = bundled_training_runtime_source();
    let entrypoint = source.join("sd-scripts/sdxl_train_network.py");
    if !entrypoint.is_file() || !source.join("RUNTIME_MANIFEST.json").is_file() {
        return Err(format!(
            "内置 kohya_ss v26.0.0 源码不完整：{}",
            source.display()
        ));
    }
    if destination
        .join("sd-scripts/sdxl_train_network.py")
        .is_file()
        && destination.join("RUNTIME_MANIFEST.json").is_file()
    {
        return Ok(());
    }
    if destination.exists() {
        return Err(format!(
            "训练运行时目录不完整，拒绝覆盖以保护已有文件：{}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "训练运行时目录没有父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建训练运行时父目录 {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(KOHYA_RUNTIME_DIRECTORY);
    let staging = parent.join(format!(".{name}.{}.installing", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("无法创建训练运行时暂存目录 {}: {error}", staging.display()))?;
    for entry in WalkDir::new(&source).follow_links(false) {
        let entry = entry.map_err(|error| format!("无法读取内置训练源码: {error}"))?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .map_err(|error| format!("无法解析内置训练源码路径: {error}"))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = staging.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("无法创建运行时目录 {}: {error}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("无法创建运行时目录 {}: {error}", parent.display()))?;
            }
            std::fs::copy(entry.path(), &target).map_err(|error| {
                format!(
                    "无法安装内置训练源码 {} 到 {}: {error}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }
    std::fs::rename(&staging, destination).map_err(|error| {
        format!(
            "无法原子切换 kohya_ss v26.0.0 训练运行时 {}: {error}",
            destination.display()
        )
    })?;
    Ok(())
}

#[derive(Debug, Clone)]
struct TrainingRuntimeProfilePaths {
    python: PathBuf,
}

/// A resolved Python interpreter that may either be owned by the application
/// or discovered from the user's system.  The training source is deliberately
/// shared and pinned; choosing an existing environment must never rewrite it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedTrainingRuntimeProfile {
    id: String,
    label: String,
    kind: &'static str,
    python: PathBuf,
    managed: bool,
}

impl ResolvedTrainingRuntimeProfile {
    fn is_wsl(&self) -> bool {
        self.kind == "wsl"
    }
}

fn training_runtime_profile_paths(
    runtime_root: &Path,
    profile: &str,
) -> Result<TrainingRuntimeProfilePaths, String> {
    let python = match profile {
        "windows" => runtime_root.join("venv").join("Scripts").join("python.exe"),
        "wsl" => runtime_root.join("venv").join("bin").join("python"),
        _ => return Err("不支持的训练运行时配置档".to_string()),
    };
    Ok(TrainingRuntimeProfilePaths { python })
}

fn managed_training_runtime_profile(
    runtime_root: &Path,
    profile: &str,
) -> Result<ResolvedTrainingRuntimeProfile, String> {
    let paths = training_runtime_profile_paths(runtime_root, profile)?;
    let (label, kind) = match profile {
        "windows" => ("Windows 原生 Python", "windows"),
        "wsl" => ("WSL Python / CUDA", "wsl"),
        _ => return Err("不支持的内置训练运行时配置档".to_string()),
    };
    Ok(ResolvedTrainingRuntimeProfile {
        id: profile.to_string(),
        label: label.to_string(),
        kind,
        python: paths.python,
        managed: true,
    })
}

#[derive(Debug, Deserialize)]
struct CondaEnvironmentList {
    #[serde(default)]
    envs: Vec<String>,
    #[serde(default)]
    envs_details: HashMap<String, CondaEnvironmentDetail>,
}

#[derive(Debug, Deserialize)]
struct CondaEnvironmentDetail {
    #[serde(default)]
    name: String,
}

fn parse_conda_environment_profiles(
    source: &str,
) -> Result<Vec<ResolvedTrainingRuntimeProfile>, String> {
    let listing = serde_json::from_str::<CondaEnvironmentList>(source)
        .map_err(|error| format!("无法解析 Conda 环境列表: {error}"))?;
    let mut used_ids = HashSet::new();
    let mut profiles = Vec::new();
    for raw_path in listing.envs {
        // Conda always emits native Windows paths on the Windows host.  Slash
        // normalization makes the data portable for tests and for a backend
        // launched through a compatibility shell.
        let root = PathBuf::from(raw_path.replace('\\', "/"));
        let detail = listing.envs_details.get(&raw_path);
        let name = detail
            .map(|item| item.name.trim())
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                root.file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "base".to_string());
        let id = format!("conda:{name}");
        if !used_ids.insert(id.clone()) {
            continue;
        }
        profiles.push(ResolvedTrainingRuntimeProfile {
            id,
            label: format!("Conda · {name}"),
            kind: "conda",
            // A Conda environment keeps its interpreter at the environment
            // root.  `Scripts/` contains activation and package executables,
            // not the environment Python itself.
            python: root.join("python.exe"),
            managed: false,
        });
    }
    Ok(profiles)
}

fn conda_executable() -> Option<PathBuf> {
    for name in ["DANBOORU_TRAINING_CONDA_EXE", "CONDA_EXE"] {
        if let Some(path) = std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        for distribution in ["anaconda3", "miniconda3", "mambaforge", "miniforge3"] {
            candidates.push(home.join(distribution).join("Scripts").join("conda.exe"));
        }
    }
    for root in ["C:/ProgramData", "C:/"] {
        for distribution in ["anaconda3", "miniconda3", "mambaforge", "miniforge3"] {
            candidates.push(
                PathBuf::from(root)
                    .join(distribution)
                    .join("Scripts")
                    .join("conda.exe"),
            );
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn discover_conda_runtime_profiles() -> Result<Vec<ResolvedTrainingRuntimeProfile>, String> {
    let conda = conda_executable().ok_or_else(|| {
        "未找到 Conda；可设置 DANBOORU_TRAINING_CONDA_EXE 指向 conda.exe".to_string()
    })?;
    let output = Command::new(&conda)
        .args(["env", "list", "--json"])
        .output()
        .map_err(|error| format!("无法读取 Conda 环境列表（{}）: {error}", conda.display()))?;
    if !output.status.success() {
        return Err(format!(
            "Conda 环境列表读取失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_conda_environment_profiles(&String::from_utf8_lossy(&output.stdout))
}

fn external_venv_profile(
    root: &Path,
    used_ids: &mut HashSet<String>,
) -> Option<ResolvedTrainingRuntimeProfile> {
    let python = root.join("Scripts").join("python.exe");
    if !python.is_file() {
        return None;
    }
    let name = root.file_name()?.to_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let id = format!("venv:{name}");
    if !used_ids.insert(id.clone()) {
        return None;
    }
    Some(ResolvedTrainingRuntimeProfile {
        id,
        label: format!("Python venv · {name}"),
        kind: "venv",
        python,
        managed: false,
    })
}

fn discover_python_venv_profiles_in_roots(
    roots: &[PathBuf],
) -> Vec<ResolvedTrainingRuntimeProfile> {
    let mut profiles = Vec::new();
    let mut used_ids = HashSet::new();
    for root in roots {
        if let Some(profile) = external_venv_profile(root, &mut used_ids) {
            profiles.push(profile);
            continue;
        }
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(profile) = external_venv_profile(&path, &mut used_ids) {
                profiles.push(profile);
            }
        }
    }
    profiles
}

fn system_venv_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(active) = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from) {
        roots.push(active);
    }
    if let Some(workon_home) = std::env::var_os("WORKON_HOME").map(PathBuf::from) {
        roots.push(workon_home);
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.push(home.join(".virtualenvs"));
        roots.push(home.join("Envs"));
    }
    roots
}

fn discover_system_venv_runtime_profiles() -> Vec<ResolvedTrainingRuntimeProfile> {
    discover_python_venv_profiles_in_roots(&system_venv_search_roots())
}

fn available_training_runtime_profiles(
    training_root: &Path,
) -> Vec<ResolvedTrainingRuntimeProfile> {
    let runtime_root = installed_training_runtime_root(training_root);
    let mut profiles = ["windows", "wsl"]
        .into_iter()
        .filter_map(|profile| managed_training_runtime_profile(&runtime_root, profile).ok())
        .collect::<Vec<_>>();
    match discover_conda_runtime_profiles() {
        Ok(mut conda_profiles) => profiles.append(&mut conda_profiles),
        Err(error) => tracing::debug!(%error, "未发现可用的 Conda 训练环境"),
    }
    profiles.extend(discover_system_venv_runtime_profiles());
    profiles
}

fn resolve_training_runtime_profile(
    training_root: &Path,
    profile_id: &str,
) -> Result<ResolvedTrainingRuntimeProfile, String> {
    available_training_runtime_profiles(training_root)
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("未找到训练运行时配置档：{profile_id}"))
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct UpstreamParserExport {
    fields: Vec<UpstreamParserField>,
}

#[derive(Debug, Deserialize)]
struct UpstreamAdapterParserExport {
    id: String,
    fields: Vec<UpstreamParserField>,
}

#[derive(Debug, Deserialize)]
struct UpstreamParserCatalogExport {
    adapters: Vec<UpstreamAdapterParserExport>,
}

#[cfg(test)]
fn parse_upstream_parser_export(source: &str) -> Result<Vec<UpstreamParserField>, String> {
    serde_json::from_str::<UpstreamParserExport>(source)
        .map(|export| export.fields)
        .map_err(|error| format!("无法解析 kohya_ss 参数导出: {error}"))
}

fn parse_upstream_parser_catalog_export(
    source: &str,
) -> Result<HashMap<String, Vec<UpstreamParserField>>, String> {
    serde_json::from_str::<UpstreamParserCatalogExport>(source)
        .map(|catalog| {
            catalog
                .adapters
                .into_iter()
                .map(|adapter| (adapter.id, adapter.fields))
                .collect()
        })
        .map_err(|error| format!("无法解析 kohya_ss 参数目录: {error}"))
}

static UPSTREAM_FIELDS_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(u128, std::collections::HashMap<String, Vec<UpstreamParserField>>)>>,
> = std::sync::OnceLock::new();

fn upstream_fields_disk_cache_path(training_root: &Path) -> PathBuf {
    training_root.join("upstream_fields_cache.json")
}

fn read_upstream_fields_disk_cache(
    training_root: &Path,
    cache_key: u128,
) -> Option<HashMap<String, Vec<UpstreamParserField>>> {
    let bytes = std::fs::read(upstream_fields_disk_cache_path(training_root)).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&bytes)).ok()?;
    if value
        .get("key")?
        .as_str()?
        .parse::<u128>()
        .ok()? != cache_key
    {
        return None;
    }
    let fields = value.get("fields")?.clone();
    let parsed = serde_json::from_value::<HashMap<String, Vec<UpstreamParserField>>>(fields).ok()?;
    let mut cache = UPSTREAM_FIELDS_CACHE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("upstream fields cache lock poisoned");
    *cache = Some((cache_key, parsed.clone()));
    Some(parsed)
}

fn write_upstream_fields_disk_cache(
    training_root: &Path,
    cache_key: u128,
    fields: &HashMap<String, Vec<UpstreamParserField>>,
) {
    let path = upstream_fields_disk_cache_path(training_root);
    let value = serde_json::json!({ "key": cache_key.to_string(), "fields": fields });
    if let Err(error) = std::fs::write(&path, serde_json::to_vec(&value).unwrap_or_default()) {
        tracing::debug!(%error, path = %path.display(), "无法写入上游字段缓存");
    }
}

fn inspect_upstream_adapter_fields(
    training_root: &Path,
) -> Result<Option<HashMap<String, Vec<UpstreamParserField>>>, String> {
    let runtime_root = installed_training_runtime_root(training_root);
    let inspector = training_root.join("adapter_inspector.py");
    if !inspector.is_file() {
        return Ok(None);
    }
    let arguments = builtin_adapters()
        .into_iter()
        .filter_map(|adapter| {
            let trainer = runtime_root.join(adapter.trainer);
            trainer
                .is_file()
                .then(|| format!("{}={}", adapter.id, trainer.to_string_lossy()))
        })
        .collect::<Vec<_>>();
    if arguments.is_empty() {
        return Ok(None);
    }
    // The cache key covers both the inspector source and the adapter list
    // baked into this binary, so an upgrade that adds/renames an adapter
    // invalidates the persisted cache instead of serving stale fields.
    let mut arguments_hash = 0xcbf2_9ce4_8422_2325_u64;
    for argument in &arguments {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        argument.hash(&mut hasher);
        arguments_hash ^= hasher.finish();
    }
    let cache_key = ((arguments_hash as u128) << 64)
        | inspector
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|modified| {
                let seconds = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                (seconds as u128) << 32
                    | (modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.subsec_nanos() as u128)
                        .unwrap_or_default())
            })
            .unwrap_or(0);
    let cache = UPSTREAM_FIELDS_CACHE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("upstream fields cache lock poisoned");
    if let Some((cached_key, cached)) = cache.as_ref() {
        if *cached_key == cache_key {
            return Ok(Some(cached.clone()));
        }
    }
    drop(cache);
    if let Some(cached) = read_upstream_fields_disk_cache(training_root, cache_key) {
        return Ok(Some(cached));
    }
    let profiles = available_training_runtime_profiles(training_root);
    let runtime_root_for_threads = runtime_root.clone();
    let result = std::thread::scope(|scope| -> Result<Option<HashMap<String, Vec<UpstreamParserField>>>, String> {
        let found = std::sync::Mutex::new(None::<HashMap<String, Vec<UpstreamParserField>>>);
        let mut handles = Vec::new();
        for profile in profiles {
            if !profile.python.is_file() {
                continue;
            }
            let arguments = arguments.clone();
            let inspector = inspector.clone();
            let runtime_root = runtime_root_for_threads.clone();
            handles.push(scope.spawn(move || {
                let mut command = match training_runtime_python_command(&runtime_root, &profile) {
                    Ok(command) => command,
                    Err(error) => {
                        tracing::debug!(profile = %profile.id, %error, "无法准备上游参数检查器");
                        return None;
                    }
                };
                command.arg(&inspector).args(&arguments);
                let mut child = match command.stdout(std::process::Stdio::piped()).spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        tracing::debug!(profile = %profile.id, %error, "无法启动该训练环境的参数检查器，继续尝试其他环境");
                        return None;
                    }
                };
                let mut stdout = child.stdout.take();
                let reader = scope.spawn(move || {
                    use std::io::Read;
                    let mut buffer = Vec::new();
                    if let Some(stream) = stdout.as_mut() {
                        let _ = stream.read_to_end(&mut buffer);
                    }
                    buffer
                });
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break status,
                        Ok(None) => {
                            if std::time::Instant::now() >= deadline {
                                // Kill the inspector and give the stdout reader
                                // a moment to drain EOF.  The bundled inspector
                                // spawns no children, so the pipe closes right
                                // away; a hypothetical grandchild inheriting the
                                // pipe could keep the reader blocked, in which
                                // case thread::scope would wait for it below.
                                let _ = child.kill();
                                let _ = child.wait();
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                tracing::debug!(profile = %profile.id, "参数检查器超时，已终止");
                                return None;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(error) => {
                            tracing::debug!(profile = %profile.id, %error, "无法等待参数检查器");
                            return None;
                        }
                    }
                };
                if !status.success() {
                    tracing::debug!(profile = %profile.id, "参数检查器在该训练环境中不可用，继续尝试其他环境");
                    return None;
                }
                let buffer = reader.join().unwrap_or_default();
                parse_upstream_parser_catalog_export(&String::from_utf8_lossy(&buffer)).ok()
            }));
        }
        for handle in handles {
            if let Some(fields) = handle.join().ok().flatten() {
                *found.lock().expect("upstream fields mutex poisoned") = Some(fields);
                break;
            }
        }
        let found_value = found.lock().expect("upstream fields mutex poisoned").clone();
        if let Some(ref found_fields) = found_value {
            let mut cache = UPSTREAM_FIELDS_CACHE
                .get_or_init(|| std::sync::Mutex::new(None))
                .lock()
                .expect("upstream fields cache lock poisoned");
            *cache = Some((cache_key, found_fields.clone()));
            drop(cache);
            write_upstream_fields_disk_cache(training_root, cache_key, found_fields);
        }
        Ok(found_value)
    });
    result
}

fn augment_adapters_with_upstream_fields(
    adapters: Vec<crate::training::TrainingAdapter>,
    upstream_fields: Option<HashMap<String, Vec<UpstreamParserField>>>,
) -> Vec<crate::training::TrainingAdapter> {
    let Some(upstream_fields) = upstream_fields else {
        return adapters;
    };
    adapters
        .into_iter()
        .map(|adapter| match upstream_fields.get(adapter.id) {
            Some(fields) => augment_adapter_with_upstream_fields(adapter, fields.clone()),
            None => adapter,
        })
        .collect()
}

#[cfg(test)]
mod bundled_training_runtime_tests {
    use super::{
        augment_adapters_with_upstream_fields, builtin_adapters, bundled_training_runtime_source,
        decode_training_log_bytes, discover_python_venv_profiles_in_roots,
        ensure_training_support_scripts, install_bundled_training_runtime_source,
        installed_training_runtime_root, lora_svd_device_choice, parse_conda_environment_profiles,
        parse_upstream_parser_export, prepare_training_logging_dir, tail_training_log_lines,
        training_process_failure, training_runtime_profile_paths, training_runtime_python_command,
        validate_lora_svd_request, LoraSvdAnalysisCache, LoraSvdAnalysisFileRequest,
        LoraSvdAnalysisRequest, ResolvedTrainingRuntimeProfile, TrainingRuntimeInstallCoordinator,
    };
    use std::path::Path;

    #[test]
    fn lora_svd_requires_one_to_five_unique_safetensors_files() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("adapter.safetensors");
        std::fs::write(&file, b"fixture").unwrap();
        let mut valid = LoraSvdAnalysisRequest {
            runtime_profile_id: "windows".to_string(),
            files: vec![LoraSvdAnalysisFileRequest {
                path: file.to_string_lossy().to_string(),
                label: None,
            }],
            device: "auto".to_string(),
        };
        assert_eq!(validate_lora_svd_request(&mut valid).unwrap(), 7);

        let mut duplicate = LoraSvdAnalysisRequest {
            runtime_profile_id: "windows".to_string(),
            files: vec![
                LoraSvdAnalysisFileRequest {
                    path: file.to_string_lossy().to_string(),
                    label: None,
                },
                LoraSvdAnalysisFileRequest {
                    path: file.to_string_lossy().to_string(),
                    label: None,
                },
            ],
            device: "auto".to_string(),
        };
        assert!(validate_lora_svd_request(&mut duplicate).is_err());
    }

    #[test]
    fn lora_svd_prefers_cpu_for_small_or_busy_workloads() {
        assert_eq!(lora_svd_device_choice(1, false, &[]).0, "cpu");
        assert_eq!(
            lora_svd_device_choice(1024 * 1024 * 1024, true, &[]).0,
            "cpu"
        );
    }

    #[test]
    fn lora_svd_cache_keeps_results_only_for_the_active_session_window() {
        let cache = LoraSvdAnalysisCache::default();
        let (id, _) = cache.insert(serde_json::json!({ "reports": [] })).unwrap();
        assert!(cache.get(&id).unwrap().is_some());
    }

    #[test]
    fn startup_installs_the_lora_svd_inspector_for_selected_runtimes() {
        let root = tempfile::tempdir().unwrap();

        ensure_training_support_scripts(root.path()).unwrap();

        assert_eq!(
            std::fs::read(root.path().join("lora_svd_inspector.py")).unwrap(),
            super::TRAINING_LORA_SVD_INSPECTOR
        );
    }

    #[test]
    fn ships_a_pinned_kohya_v26_runtime_source_with_licenses_and_manifest() {
        let source = bundled_training_runtime_source();

        assert!(source.join("sd-scripts/sdxl_train_network.py").is_file());
        assert!(source.join("LICENSE.kohya_ss.md").is_file());
        assert!(source.join("sd-scripts/LICENSE.md").is_file());
        assert!(source.join("RUNTIME_MANIFEST.json").is_file());
    }

    #[test]
    fn support_script_installation_never_mutates_the_versioned_upstream_runtime() {
        let root = tempfile::tempdir().unwrap();
        let runtime_file =
            installed_training_runtime_root(root.path()).join("sd-scripts/train_network.py");
        std::fs::create_dir_all(runtime_file.parent().unwrap()).unwrap();
        std::fs::write(&runtime_file, "upstream-source\n").unwrap();

        ensure_training_support_scripts(root.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(runtime_file).unwrap(),
            "upstream-source\n"
        );
    }

    #[test]
    fn installer_copies_only_the_pinned_runtime_source_into_an_empty_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let destination = workspace.path().join("kohya-ss-v26.0.0");

        install_bundled_training_runtime_source(&destination).unwrap();

        assert!(destination
            .join("sd-scripts/sdxl_train_network.py")
            .is_file());
        assert!(destination.join("LICENSE.kohya_ss.md").is_file());
        assert!(destination.join("RUNTIME_MANIFEST.json").is_file());
        assert!(!destination.join("venv").exists());
    }

    #[test]
    fn runtime_profiles_use_platform_specific_isolated_python_paths() {
        let root = Path::new("C:/training/lora-scripts");

        assert_eq!(
            training_runtime_profile_paths(root, "windows")
                .unwrap()
                .python,
            root.join("venv/Scripts/python.exe")
        );
        assert_eq!(
            training_runtime_profile_paths(root, "wsl").unwrap().python,
            root.join("venv/bin/python")
        );
        assert!(training_runtime_profile_paths(root, "invalid").is_err());
    }

    #[test]
    fn conda_environment_discovery_exposes_selectable_profiles_without_mutating_them() {
        let profiles = parse_conda_environment_profiles(
            r#"{
                "envs": ["C:\\Users\\XieMo\\anaconda3", "C:\\Users\\XieMo\\anaconda3\\envs\\lora"],
                "envs_details": {
                    "C:\\Users\\XieMo\\anaconda3": {"name": "base"},
                    "C:\\Users\\XieMo\\anaconda3\\envs\\lora": {"name": "lora"}
                }
            }"#,
        )
        .unwrap();

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[1].id, "conda:lora");
        assert_eq!(profiles[1].label, "Conda · lora");
        assert_eq!(
            profiles[1].python,
            Path::new("C:/Users/XieMo/anaconda3/envs/lora/python.exe")
        );
        assert!(!profiles[1].managed);
    }

    #[test]
    fn external_conda_worker_does_not_require_a_managed_kohya_working_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let runtime_root = workspace.path().join("missing-kohya-runtime");
        let profile = ResolvedTrainingRuntimeProfile {
            id: "conda:lora".to_string(),
            label: "Conda · lora".to_string(),
            kind: "conda",
            python: Path::new("C:/Python/python.exe").to_path_buf(),
            managed: false,
        };

        let command = training_runtime_python_command(&runtime_root, &profile).unwrap();

        assert_eq!(command.get_current_dir(), None);
    }

    #[test]
    fn standard_python_virtualenvs_are_also_discoverable_as_external_profiles() {
        let workspace = tempfile::tempdir().unwrap();
        let environment = workspace.path().join("lora-lab");
        std::fs::create_dir_all(environment.join("Scripts")).unwrap();
        std::fs::write(environment.join("Scripts/python.exe"), b"").unwrap();

        let profiles = discover_python_venv_profiles_in_roots(&[workspace.path().to_path_buf()]);

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, "venv:lora-lab");
        assert_eq!(profiles[0].kind, "venv");
        assert!(!profiles[0].managed);
    }

    #[test]
    fn parser_export_keeps_unrecognized_flags_as_typed_adapter_fields() {
        let fields = parse_upstream_parser_export(
            r#"{"fields":[{"key":"new_upstream_switch","default":false,"choices":[],"kind":"boolean","required":false,"help":"New flag"}]}"#,
        )
        .unwrap();

        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "new_upstream_switch");
        assert_eq!(fields[0].kind, "boolean");
    }

    #[test]
    fn adapter_listing_keeps_the_static_kohya_catalog_when_parser_inspection_is_unavailable() {
        let adapters = augment_adapters_with_upstream_fields(builtin_adapters(), None);

        assert_eq!(adapters.len(), 27);
        assert!(adapters.iter().any(|adapter| adapter.id == "sdxl-lora"));
    }

    #[test]
    fn telemetry_launcher_makes_the_upstream_trainer_directory_importable() {
        let launcher = std::str::from_utf8(super::TRAINING_TELEMETRY_LAUNCHER).unwrap();

        assert!(launcher.contains("sys.path.insert(0, str(trainer.parent))"));
    }

    #[test]
    fn runtime_installer_allows_only_one_active_install_per_profile() {
        let coordinator = TrainingRuntimeInstallCoordinator::default();

        assert!(coordinator.begin("windows"));
        assert!(!coordinator.begin("windows"));
        coordinator.complete("windows", Ok(()));
        assert!(coordinator.begin("windows"));
    }

    #[test]
    fn training_log_tail_returns_only_the_requested_complete_lines() {
        assert_eq!(
            tail_training_log_lines("one\ntwo\nthree\n", 2),
            "two\nthree\n"
        );
        assert_eq!(tail_training_log_lines("one\ntwo", 10), "one\ntwo\n");
    }

    #[test]
    fn training_log_decoder_keeps_windows_legacy_output_readable() {
        assert_eq!(decode_training_log_bytes(b"one\n\xff\n"), "one\n�\n");
    }

    #[test]
    fn training_log_decoder_reads_windows_shift_jis_diagnostics() {
        assert_eq!(
            decode_training_log_bytes(
                b"\x83\x66\x81\x5b\x83\x5e\x82\xaa\x82\xa0\x82\xe8\x82\xdc\x82\xb9\x82\xf1"
            ),
            "データがありません",
        );
    }

    #[test]
    fn training_log_decoder_reads_gbk_encoded_japanese_from_chinese_windows() {
        assert_eq!(
            decode_training_log_bytes(
                b"\xbb\xad\xcf\xf1\xa4\xac\xa4\xa2\xa4\xea\xa4\xde\xa4\xbb\xa4\xf3\xa1\xa3"
            ),
            "画像がありません。",
        );
    }

    #[test]
    fn zero_exit_with_lora_scripts_no_data_error_is_not_a_completed_training() {
        let failure = training_process_failure(
            0,
            "ERROR    No data found. Please verify arguments (train_data_dir must be the parent of folders with images)",
        )
        .expect("an upstream no-data error must fail the task even when it exits with zero");

        assert_eq!(failure.code, "training_no_data");
        assert!(!failure.retryable);
    }

    #[test]
    fn tensorboard_uses_an_output_local_logging_dir_when_the_form_leaves_it_empty() {
        let mut parameters = serde_json::json!({
            "output_dir": "C:/training-output/odette",
            "log_with": "tensorboard",
            "logging_dir": ""
        });

        let logging_dir = prepare_training_logging_dir(&mut parameters)
            .expect("a TensorBoard run should receive a safe default logging directory");
        let expected = std::path::Path::new("C:/training-output/odette").join("logs");

        assert_eq!(logging_dir.as_deref(), Some(expected.as_path()));
        assert_eq!(
            parameters["logging_dir"],
            expected.to_string_lossy().as_ref()
        );
    }
}

fn find_vllm_launcher_root(paths: &AppPaths) -> Option<PathBuf> {
    let mut candidates = vec![paths.data_dir.clone()];
    candidates.extend(paths.static_dir.ancestors().map(Path::to_path_buf));
    if let Ok(executable) = std::env::current_exe() {
        candidates.extend(
            executable
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .map(Path::to_path_buf),
        );
    }
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.extend(current_dir.ancestors().map(Path::to_path_buf));
    }
    candidates.into_iter().find(|candidate| {
        candidate.join("start_vllm.sh").is_file() || candidate.join("start_vllm.bat").is_file()
    })
}

fn configured_local_vllm_port(endpoint: &str) -> Result<u16, &'static str> {
    let endpoint = Url::parse(endpoint).map_err(|_| "vLLM Base URL 格式无效")?;
    if endpoint.scheme() != "http"
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err("只能从本机 HTTP vLLM 服务加载模型");
    }
    let host = endpoint.host_str().ok_or("vLLM Base URL 缺少主机")?;
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !is_loopback {
        return Err("只能从本机 vLLM 服务加载模型");
    }
    if endpoint.path().trim_end_matches('/') != "/v1"
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err("vLLM Base URL 必须是本机地址并以 /v1 结尾");
    }
    endpoint
        .port()
        .ok_or("vLLM Base URL 必须包含端口，例如 http://127.0.0.1:8000/v1")
}

fn launch_vllm_process(project_root: &Path, port: u16) -> Result<(), String> {
    let (launcher, program) = if cfg!(windows) {
        (project_root.join("start_vllm.bat"), None)
    } else {
        (project_root.join("start_vllm.sh"), Some("bash"))
    };
    if !launcher.is_file() {
        return Err(format!("找不到 vLLM 启动脚本: {}", launcher.display()));
    }
    let logs = project_root.join("logs");
    std::fs::create_dir_all(&logs).map_err(|error| format!("无法创建 vLLM 日志目录: {error}"))?;
    let mut command = if let Some(program) = program {
        let mut command = Command::new(program);
        command.arg(&launcher);
        command
    } else {
        Command::new(&launcher)
    };
    command
        .current_dir(project_root)
        .env("VLLM_HOST", "127.0.0.1")
        .env("VLLM_PORT", port.to_string())
        .env("LOG_DIR", &logs)
        .env("VLLM_STATE_FILE", logs.join("vllm.state.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动 vLLM: {error}"))
}

fn unload_vllm_process(project_root: &Path) -> Result<&'static str, String> {
    let (launcher, program) = if cfg!(windows) {
        (project_root.join("stop_vllm.bat"), None)
    } else {
        (project_root.join("stop_vllm.sh"), Some("bash"))
    };
    if !launcher.is_file() {
        return Err(format!("找不到 vLLM 卸载脚本: {}", launcher.display()));
    }
    let mut command = if let Some(program) = program {
        let mut command = Command::new(program);
        command.arg(&launcher);
        command
    } else {
        Command::new(&launcher)
    };
    let output = command
        .current_dir(project_root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 vLLM 卸载脚本: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(if detail.is_empty() {
            format!("vLLM 卸载脚本退出失败（{}）", output.status)
        } else {
            format!("vLLM 卸载失败：{detail}")
        });
    }
    if stdout
        .lines()
        .any(|line| line.trim() == "VLLM_UNLOAD_STATE=stopped")
    {
        Ok("stopped")
    } else if stdout
        .lines()
        .any(|line| line.trim() == "VLLM_UNLOAD_STATE=not_running")
    {
        Ok("not_running")
    } else {
        Err("vLLM 卸载脚本返回了未知状态".to_string())
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
        .route("/api/vllm/load", axum::routing::post(vllm_load))
        .route("/api/vllm/unload", axum::routing::post(vllm_unload))
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
        .route("/api/tasks/{id}", get(task_detail).delete(task_delete))
        .route("/api/downloads/history", get(download_history))
        .route("/api/tasks/events", get(task_events))
        .route("/api/tasks/{id}/{action}", axum::routing::post(task_action))
        .route("/api/training/adapters", get(training_adapters))
        .route(
            "/api/training/runtime-profiles",
            get(training_runtime_profiles),
        )
        .route(
            "/api/training/runtime-profiles/{id}/diagnostics",
            get(training_runtime_diagnostics),
        )
        .route(
            "/api/training/runtime-profiles/{id}/install",
            axum::routing::post(install_training_runtime),
        )
        .route(
            "/api/vision-crop/runtime-profiles/{id}/health",
            get(vision_crop_runtime_health),
        )
        .route(
            "/api/vision-crop/runtime-profiles/{id}/install",
            axum::routing::post(install_vision_crop_runtime),
        )
        .route("/api/training/gpus", get(training_gpus))
        .route("/api/training/queue", get(training_queue))
        .route(
            "/api/training/datasets/gallery",
            get(training_gallery_dataset_preview),
        )
        .route(
            "/api/training/datasets/augmentations",
            get(training_gallery_augmentation_discovery),
        )
        .route("/api/training/paths", get(training_path_browser))
        .route(
            "/api/training/presets",
            get(list_training_presets).post(create_training_preset),
        )
        .route(
            "/api/training/presets/import",
            axum::routing::post(import_training_preset),
        )
        .route(
            "/api/training/presets/{id}",
            axum::routing::put(update_training_preset),
        )
        .route(
            "/api/training/presets/{id}/toml",
            axum::routing::put(update_training_preset_from_toml),
        )
        .route(
            "/api/training/presets/{id}/export",
            get(export_training_preset),
        )
        .route(
            "/api/training/preview",
            axum::routing::post(training_preview),
        )
        .route(
            "/api/training/lora-svd/analyses",
            axum::routing::post(create_lora_svd_analysis),
        )
        .route(
            "/api/training/lora-svd/analyses/{id}/modules/{module_id}",
            get(lora_svd_module),
        )
        .route(
            "/api/training/lora-svd/analyses/{id}/export",
            get(export_lora_svd_analysis),
        )
        .route("/api/training/tasks/{id}/logs", get(training_logs))
        .route(
            "/api/training/tasks/{id}/cleanup-preview",
            get(training_cleanup_preview),
        )
        .route(
            "/api/training/tasks/{id}",
            axum::routing::delete(delete_training_task),
        )
        .route("/api/training/tasks/{id}/metrics", get(training_metrics))
        .route(
            "/api/training/tasks/{id}/metrics/overview",
            get(training_metrics_overview),
        )
        .route("/api/training/tasks/{id}/events", get(training_events))
        .route(
            "/api/training/tasks/{id}/artifacts",
            get(training_artifacts),
        )
        .route(
            "/api/training/tasks/{id}/artifacts/{artifact_id}",
            get(training_artifact_file),
        )
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

#[derive(Debug, Serialize)]
struct TrainingRuntimeProfileResponse {
    id: String,
    label: String,
    kind: String,
    managed: bool,
    installed: bool,
    installing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    runtime_root: String,
    python_path: String,
}

#[derive(Debug, Serialize)]
struct TrainingRuntimeCheckResponse {
    id: &'static str,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct TrainingRuntimeDiagnosticsResponse {
    profile: TrainingRuntimeProfileResponse,
    checks: Vec<TrainingRuntimeCheckResponse>,
}

#[derive(Debug, Serialize)]
struct VisionCropRuntimeHealthResponse {
    runtime_profile_id: String,
    python_path: String,
    ready: bool,
    installing: bool,
    gpu_id: String,
    providers: Vec<String>,
    gpu_name: Option<String>,
    models_ready: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TrainingGpuResponse {
    #[serde(skip_serializing)]
    uuid: String,
    id: String,
    name: String,
    memory_total_mib: u64,
    memory_used_mib: u64,
    utilization_percent: u64,
    graphics_clock_mhz: Option<u64>,
    memory_clock_mhz: Option<u64>,
    power_draw_w: Option<f64>,
    power_limit_w: Option<f64>,
    temperature_c: Option<u64>,
    fan_speed_percent: Option<u64>,
    external_processes: Vec<TrainingGpuExternalProcessResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct TrainingGpuExternalProcessResponse {
    pid: u64,
    process_name: String,
    memory_used_mib: u64,
}

#[derive(Debug, Serialize)]
struct TrainingQueueResponse {
    entries: Vec<TrainingQueueEntryResponse>,
}

#[derive(Debug, Serialize)]
struct TrainingQueueEntryResponse {
    task_id: String,
    status: &'static str,
    adapter_id: String,
    runtime_profile_id: String,
    gpu_ids: Vec<String>,
    assigned_gpu_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_position: Option<u64>,
    blocking_task_ids: Vec<String>,
    blocked_gpu_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_wait_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait_reason: Option<String>,
}

async fn training_queue(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<TrainingQueueResponse>>, ApiError> {
    let snapshots = state
        .tasks
        .snapshot()
        .map_err(|error| ApiError::internal(format!("无法读取训练队列: {error}")))?;
    let task_by_id = snapshots
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<HashMap<_, _>>();
    let waits = state.training_leases.waiting_snapshot();
    let waits_by_task = waits
        .into_iter()
        .map(|wait| (wait.task_id.clone(), wait))
        .collect::<HashMap<_, _>>();
    let mut entries = snapshots
        .iter()
        .filter(|task| task.kind == "training")
        .filter_map(|task| {
            let training = task
                .payload
                .get("training")
                .cloned()
                .and_then(|value| serde_json::from_value::<TrainingRequest>(value).ok())?;
            let wait = waits_by_task.get(&task.id);
            let gpu_ids = if training.gpu_ids.is_empty() {
                detected_training_gpu_ids()
            } else {
                training.gpu_ids
            };
            let blocking_task_ids = wait
                .map(|wait| wait.blocker_task_ids.clone())
                .unwrap_or_default();
            let estimated_wait_seconds = blocking_task_ids
                .iter()
                .filter_map(|task_id| task_by_id.get(task_id.as_str()))
                .filter_map(|task| task.eta_seconds)
                .max();
            let wait_reason = (task.status == TaskStatus::Queued).then(|| {
                if blocking_task_ids.is_empty() {
                    "等待训练调度器分配 GPU".to_string()
                } else {
                    format!(
                        "等待 GPU {}：{}",
                        gpu_ids.join(", "),
                        blocking_task_ids.join("、")
                    )
                }
            });
            Some(TrainingQueueEntryResponse {
                task_id: task.id.clone(),
                status: task_status_response(task.status),
                adapter_id: training.adapter_id,
                runtime_profile_id: training.runtime_profile_id,
                assigned_gpu_ids: state.training_leases.assigned_gpus(&task.id, "physical"),
                gpu_ids: gpu_ids.clone(),
                queue_position: wait.map(|wait| wait.queue_position),
                blocking_task_ids: blocking_task_ids.clone(),
                blocked_gpu_ids: (!blocking_task_ids.is_empty())
                    .then_some(gpu_ids)
                    .unwrap_or_default(),
                estimated_wait_seconds,
                wait_reason,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.queue_position
            .unwrap_or(u64::MAX)
            .cmp(&right.queue_position.unwrap_or(u64::MAX))
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    Ok(Json(ApiSuccess {
        data: TrainingQueueResponse { entries },
        meta: None,
    }))
}

#[derive(Debug, Deserialize)]
struct TrainingGalleryDatasetQuery {
    root_id: String,
    #[serde(default)]
    relative_directory: String,
    repeats: u32,
    caption_extension: Option<String>,
}

#[derive(Debug, Serialize)]
struct TrainingGalleryDatasetResponse {
    root_id: String,
    root_name: String,
    relative_directory: String,
    image_dir: String,
    caption_extension: String,
    image_count: u64,
    caption_count: u64,
    repeats: u32,
    effective_image_count: u64,
}

#[derive(Debug, Clone)]
struct TrainingGalleryDatasetInspection {
    root_id: String,
    root_name: String,
    relative_directory: String,
    image_dir: PathBuf,
    caption_extension: String,
    image_count: u64,
    caption_count: u64,
    repeats: u32,
}

async fn training_gallery_dataset_preview(
    State(state): State<AppState>,
    Query(query): Query<TrainingGalleryDatasetQuery>,
) -> Result<Json<ApiSuccess<TrainingGalleryDatasetResponse>>, ApiError> {
    let inspection = inspect_training_gallery_dataset(
        &state,
        &TrainingGalleryDataset {
            root_id: query.root_id,
            relative_directory: query.relative_directory,
            repeats: query.repeats,
            caption_extension: query.caption_extension,
        },
    )?;
    let response = TrainingGalleryDatasetResponse {
        root_id: inspection.root_id,
        root_name: inspection.root_name,
        relative_directory: inspection.relative_directory,
        image_dir: inspection.image_dir.to_string_lossy().to_string(),
        caption_extension: inspection.caption_extension,
        image_count: inspection.image_count,
        caption_count: inspection.caption_count,
        repeats: inspection.repeats,
        effective_image_count: inspection
            .image_count
            .saturating_mul(inspection.repeats as u64),
    };
    Ok(Json(ApiSuccess {
        data: response,
        meta: None,
    }))
}

#[derive(Debug, Deserialize)]
struct TrainingAugmentationDiscoveryQuery {
    root_id: String,
    #[serde(default)]
    relative_directory: String,
}

#[derive(Debug, Serialize)]
struct TrainingAugmentationDiscoveryResponse {
    source: TrainingGalleryDatasetResponse,
    subsets: Vec<TrainingAugmentationSubsetResponse>,
}

#[derive(Debug, Serialize)]
struct TrainingAugmentationSubsetResponse {
    task_id: String,
    id: String,
    label: String,
    relative_directory: String,
    caption_extension: String,
    repeats: u32,
    image_count: u64,
    caption_count: u64,
}

async fn training_gallery_augmentation_discovery(
    State(state): State<AppState>,
    Query(query): Query<TrainingAugmentationDiscoveryQuery>,
) -> Result<Json<ApiSuccess<TrainingAugmentationDiscoveryResponse>>, ApiError> {
    let source_dataset = TrainingGalleryDataset {
        root_id: query.root_id.clone(),
        relative_directory: query.relative_directory.clone(),
        repeats: 1,
        caption_extension: Some(".txt".to_string()),
    };
    let source = inspect_training_gallery_dataset(&state, &source_dataset)?;
    let root = state
        .database
        .get_root(&query.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let verified = VerifiedMediaRoot::open(current_platform_path(&root)?).map_err(|error| {
        ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
    })?;
    let metadata_relative =
        PathBuf::from(&source.relative_directory).join(".augmentation-metadata");
    let metadata_directory = verified.resolve(&metadata_relative).map_err(|error| {
        ApiError::bad_request(
            "invalid_gallery_dataset",
            format!("增广元数据目录不可用: {error}"),
        )
    })?;
    let mut subsets = Vec::new();
    if metadata_directory.is_dir() {
        let mut entries = std::fs::read_dir(&metadata_directory)
            .map_err(|error| ApiError::internal(format!("无法读取增广元数据: {error}")))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
        for entry in entries.into_iter().take(100) {
            if !matches!(entry.file_type(), Ok(file_type) if !file_type.is_symlink() && file_type.is_dir())
            {
                continue;
            }
            let task_id = entry.file_name().to_string_lossy().to_string();
            if task_id.is_empty() || entry.path().join("INCOMPLETE.json").is_file() {
                continue;
            }
            let ready_path = entry.path().join("READY.json");
            if !matches!(std::fs::metadata(&ready_path), Ok(metadata) if metadata.is_file() && metadata.len() <= 2 * 1024 * 1024)
            {
                continue;
            }
            let ready = match std::fs::read(&ready_path)
                .ok()
                .and_then(|content| serde_json::from_slice::<Value>(&content).ok())
            {
                Some(ready) => ready,
                None => continue,
            };
            let Some(manifest_subsets) = ready
                .get("training_subsets")
                .and_then(|manifest| manifest.get("subsets"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            let expected_prefix = PathBuf::from(&source.relative_directory)
                .join(".augmentation")
                .join(&task_id)
                .join("ready/train");
            for subset in manifest_subsets {
                let Some(id) = subset.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !matches!(
                    id,
                    "horizontal_flip" | "portrait" | "upper_body" | "full_body_tight"
                ) {
                    continue;
                }
                let Some(relative_directory) =
                    subset.get("relative_directory").and_then(Value::as_str)
                else {
                    continue;
                };
                let relative_directory = match normalize_task_relative_directory(relative_directory)
                {
                    Ok(relative_directory) => relative_directory,
                    Err(_) => continue,
                };
                if !Path::new(&relative_directory).starts_with(&expected_prefix) {
                    continue;
                }
                let repeats = subset
                    .get("default_repeats")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(1)
                    .clamp(1, 10_000);
                let dataset = TrainingGalleryDataset {
                    root_id: query.root_id.clone(),
                    relative_directory: relative_directory.clone(),
                    repeats,
                    caption_extension: Some(source.caption_extension.clone()),
                };
                let inspection = match inspect_training_gallery_dataset(&state, &dataset) {
                    Ok(inspection)
                        if inspection.image_count > 0
                            && inspection.caption_count == inspection.image_count =>
                    {
                        inspection
                    }
                    _ => continue,
                };
                subsets.push(TrainingAugmentationSubsetResponse {
                    task_id: task_id.clone(),
                    id: id.to_string(),
                    label: subset
                        .get("label")
                        .and_then(Value::as_str)
                        .filter(|label| !label.trim().is_empty())
                        .unwrap_or(id)
                        .to_string(),
                    relative_directory,
                    caption_extension: inspection.caption_extension,
                    repeats: inspection.repeats,
                    image_count: inspection.image_count,
                    caption_count: inspection.caption_count,
                });
            }
        }
    }
    subsets.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(Json(ApiSuccess {
        data: TrainingAugmentationDiscoveryResponse {
            source: training_gallery_dataset_response(source),
            subsets,
        },
        meta: None,
    }))
}

fn training_gallery_dataset_response(
    inspection: TrainingGalleryDatasetInspection,
) -> TrainingGalleryDatasetResponse {
    TrainingGalleryDatasetResponse {
        root_id: inspection.root_id,
        root_name: inspection.root_name,
        relative_directory: inspection.relative_directory,
        image_dir: inspection.image_dir.to_string_lossy().to_string(),
        caption_extension: inspection.caption_extension,
        image_count: inspection.image_count,
        caption_count: inspection.caption_count,
        repeats: inspection.repeats,
        effective_image_count: inspection
            .image_count
            .saturating_mul(inspection.repeats as u64),
    }
}

#[derive(Debug, Deserialize)]
struct TrainingPathBrowserQuery {
    kind: String,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct TrainingPathBrowserResponse {
    current_path: String,
    parent_path: Option<String>,
    directories: Vec<TrainingPathEntry>,
    files: Vec<TrainingPathEntry>,
}

#[derive(Debug, Serialize)]
struct TrainingPathEntry {
    name: String,
    path: String,
}

async fn training_path_browser(
    State(state): State<AppState>,
    Query(query): Query<TrainingPathBrowserQuery>,
) -> Result<Json<ApiSuccess<TrainingPathBrowserResponse>>, ApiError> {
    if !matches!(query.kind.as_str(), "model" | "dataset" | "output") {
        return Err(ApiError::bad_request(
            "invalid_training_path_kind",
            "路径浏览类型必须是 model、dataset 或 output",
        ));
    }
    let requested = query
        .path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state.training_root.join("outputs"));
    if requested.to_string_lossy().len() > 4096 {
        return Err(ApiError::bad_request("invalid_training_path", "路径过长"));
    }
    let requested = if requested.is_file() {
        requested
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(requested)
    } else {
        requested
    };
    let current = std::fs::canonicalize(&requested).map_err(|error| {
        ApiError::bad_request(
            "training_path_unavailable",
            format!("路径不可访问: {error}"),
        )
    })?;
    if !current.is_dir() {
        return Err(ApiError::bad_request(
            "training_path_not_directory",
            "请浏览一个已存在的文件夹",
        ));
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&current)
        .map_err(|error| ApiError::internal(format!("无法读取路径: {error}")))?
        .filter_map(Result::ok)
        .take(500)
    {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = match entry.file_type() {
            Ok(file_type) if !file_type.is_symlink() => file_type,
            _ => continue,
        };
        let target = TrainingPathEntry {
            name,
            path: path.to_string_lossy().to_string(),
        };
        if file_type.is_dir() {
            directories.push(target);
        } else if file_type.is_file() && training_path_file_allowed(&query.kind, &path) {
            files.push(target);
        }
    }
    directories.sort_by_key(|entry| entry.name.to_ascii_lowercase());
    files.sort_by_key(|entry| entry.name.to_ascii_lowercase());
    Ok(Json(ApiSuccess {
        data: TrainingPathBrowserResponse {
            current_path: current.to_string_lossy().to_string(),
            parent_path: current
                .parent()
                .map(|parent| parent.to_string_lossy().to_string()),
            directories,
            files,
        },
        meta: None,
    }))
}

fn training_path_file_allowed(kind: &str, path: &Path) -> bool {
    match kind {
        "model" => matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase())
                .as_deref(),
            Some("safetensors" | "ckpt" | "pt" | "bin")
        ),
        "dataset" => {
            is_training_image_file(path)
                || path.extension().and_then(|extension| extension.to_str()) == Some("txt")
        }
        "output" => true,
        _ => false,
    }
}

fn inspect_training_gallery_dataset(
    state: &AppState,
    dataset: &TrainingGalleryDataset,
) -> Result<TrainingGalleryDatasetInspection, ApiError> {
    dataset
        .validate()
        .map_err(|message| ApiError::bad_request("invalid_gallery_dataset", message))?;
    let relative_directory = normalize_task_relative_directory(&dataset.relative_directory)?;
    let root = state
        .database
        .get_root(&dataset.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let verified = VerifiedMediaRoot::open(current_platform_path(&root)?).map_err(|error| {
        ApiError::bad_request("root_unavailable", format!("媒体根当前不可访问: {error}"))
    })?;
    let image_dir = if relative_directory.is_empty() {
        verified.path().to_path_buf()
    } else {
        verified
            .resolve(Path::new(&relative_directory))
            .map_err(|error| {
                ApiError::bad_request(
                    "invalid_gallery_dataset",
                    format!("图库目录不可用: {error}"),
                )
            })?
    };
    if !image_dir.is_dir() {
        return Err(ApiError::bad_request(
            "gallery_dataset_not_directory",
            "所选图库路径不是文件夹",
        ));
    }
    if has_incomplete_augmentation_marker(&verified, &image_dir) {
        return Err(ApiError::bad_request(
            "gallery_dataset_not_ready",
            "所选增广数据集尚未完整完成；请等待任务生成 READY.json 后再训练",
        ));
    }
    let caption_extension = dataset
        .caption_extension
        .clone()
        .unwrap_or_else(|| ".txt".to_string());
    let caption_suffix = caption_extension.trim_start_matches('.');
    let mut image_count = 0_u64;
    let mut caption_count = 0_u64;
    // The generated lora-scripts dataset TOML points directly at `image_dir`.
    // Its loader is non-recursive, so the preview must use the same rule and
    // never accept a parent folder whose images are only in child folders.
    for entry in WalkDir::new(&image_dir)
        .max_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || !is_training_image_file(entry.path()) {
            continue;
        }
        image_count = image_count.saturating_add(1);
        if entry.path().with_extension(caption_suffix).is_file() {
            caption_count = caption_count.saturating_add(1);
        }
    }
    Ok(TrainingGalleryDatasetInspection {
        root_id: root.id,
        root_name: root.name,
        relative_directory,
        image_dir,
        caption_extension,
        image_count,
        caption_count,
        repeats: dataset.repeats,
    })
}

fn has_incomplete_augmentation_marker(root: &VerifiedMediaRoot, image_dir: &Path) -> bool {
    let mut current = image_dir;
    loop {
        if current.join("INCOMPLETE.json").is_file() {
            return true;
        }
        if current
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".augmentation")
        {
            let Some(source_directory) = current.parent().and_then(Path::parent) else {
                return false;
            };
            let Some(task_id) = current.file_name() else {
                return false;
            };
            if source_directory
                .join(".augmentation-metadata")
                .join(task_id)
                .join("INCOMPLETE.json")
                .is_file()
            {
                return true;
            }
        }
        if current == root.path() {
            return false;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        current = parent;
    }
}

fn is_training_image_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "bmp")
    )
}

#[cfg(test)]
fn training_gallery_dataset_toml(
    inspection: &TrainingGalleryDatasetInspection,
    parameters: &Value,
) -> String {
    training_gallery_datasets_toml(std::slice::from_ref(inspection), parameters)
}

fn training_gallery_datasets_toml(
    inspections: &[TrainingGalleryDatasetInspection],
    parameters: &Value,
) -> String {
    let values = parameters.as_object();
    let number = |key: &str, default: u64| {
        values
            .and_then(|values| values.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(default)
    };
    let boolean = |key: &str, default: bool| {
        values
            .and_then(|values| values.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    };
    let resolution = values
        .and_then(|values| values.get("resolution"))
        .and_then(|value| match value {
            Value::String(value) => {
                let parts = value
                    .split(',')
                    .map(|part| part.trim().parse::<u64>().ok())
                    .collect::<Vec<_>>();
                match parts.as_slice() {
                    [Some(width), Some(height)] => Some([*width, *height]),
                    _ => None,
                }
            }
            Value::Array(values) if values.len() == 2 => {
                Some([values[0].as_u64()?, values[1].as_u64()?])
            }
            _ => None,
        })
        .unwrap_or([1024, 1024]);
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let caption_extension = inspections
        .first()
        .map(|inspection| inspection.caption_extension.as_str())
        .unwrap_or(".txt");
    let mut config = format!(
        "[general]\ncaption_extension = {}\nshuffle_caption = {}\nkeep_tokens = {}\n\n[[datasets]]\nbatch_size = {}\nenable_bucket = {}\nresolution = [{}, {}]\nmin_bucket_reso = {}\nmax_bucket_reso = {}\nbucket_reso_steps = {}\n",
        quote(caption_extension),
        boolean("shuffle_caption", false),
        number("keep_tokens", 0),
        number("train_batch_size", 1),
        boolean("enable_bucket", true),
        resolution[0],
        resolution[1],
        number("min_bucket_reso", 256),
        number("max_bucket_reso", 2048),
        number("bucket_reso_steps", 32),
    );
    for inspection in inspections {
        config.push_str(&format!(
            "\n[[datasets.subsets]]\nimage_dir = {}\nnum_repeats = {}\n",
            quote(&inspection.image_dir.to_string_lossy()),
            inspection.repeats,
        ));
    }
    config
}

/// Builds the text format consumed by lora-scripts' `--sample_prompts` flag.
/// Captions are read only at task start, then frozen in the task's output
/// directory so a later gallery edit cannot change an in-flight experiment.
fn training_sample_prompt_lines(
    settings: &crate::training::TrainingSampleSettings,
    dataset_dir: Option<&Path>,
    caption_extension: &str,
) -> Result<Vec<String>, String> {
    settings.validate()?;
    if !settings.enabled {
        return Ok(Vec::new());
    }
    let normalize = |value: &str| value.split_whitespace().collect::<Vec<_>>().join(" ");
    let prompts = match settings.prompt_source {
        crate::training::TrainingSamplePromptSource::Manual => settings
            .prompt
            .lines()
            .map(normalize)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>(),
        crate::training::TrainingSamplePromptSource::DatasetCaptions => {
            let directory = dataset_dir
                .ok_or_else(|| "从数据集抽取样图 Prompt 时需要可访问的数据集目录".to_string())?;
            let extension = caption_extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if extension.is_empty()
                || extension.len() > 31
                || !extension.chars().all(|value| value.is_ascii_alphanumeric())
            {
                return Err("样图 Caption 扩展名无效".to_string());
            }
            let mut captions = WalkDir::new(directory)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case(&extension))
                })
                .map(|entry| entry.into_path())
                .collect::<Vec<_>>();
            captions.sort();
            // True random sample: partially shuffle the (sorted) list with a
            // task-local seed, then take the first N entries.  Deterministic
            // ordering would always pick the same few files for every task.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15);
            let mut seed = nanos ^ (nanos << 21) ^ (nanos >> 7) ^ 0xD1B5_4A32_D192_ED03;
            let mut next_random = move || {
                seed ^= seed >> 12;
                seed ^= seed << 25;
                seed ^= seed >> 27;
                seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
            };
            let total = captions.len();
            let selected = (settings.dataset_caption_count as usize).min(total);
            for index in 0..selected {
                let swap = index + (next_random() % (total - index) as u64) as usize;
                captions.swap(index, swap);
            }
            captions
                .into_iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .map(|caption| normalize(&caption))
                .filter(|caption| !caption.is_empty())
                .take(settings.dataset_caption_count as usize)
                .collect::<Vec<_>>()
        }
    };
    if prompts.is_empty() {
        return Err(match settings.prompt_source {
            crate::training::TrainingSamplePromptSource::Manual => {
                "没有可用的样图 Prompt".to_string()
            }
            crate::training::TrainingSamplePromptSource::DatasetCaptions => {
                "所选数据集内没有可用的 Caption TXT，无法生成样图".to_string()
            }
        });
    }
    let negative_prompt = normalize(&settings.negative_prompt);
    Ok(prompts
        .into_iter()
        .map(|prompt| {
            let mut line = prompt;
            if !negative_prompt.is_empty() {
                line.push_str(" --n ");
                line.push_str(&negative_prompt);
            }
            line.push_str(&format!(
                " --w {} --h {} --s {}",
                settings.width, settings.height, settings.steps
            ));
            line
        })
        .collect())
}

/// Materializes the immutable prompt source for one run.  The patched bundled
/// lora-scripts runtime writes generated images to this same `samples`
/// directory, next to the exact prompts that produced them.
fn configure_training_samples(
    settings: &crate::training::TrainingSampleSettings,
    dataset_dir: Option<&Path>,
    caption_extension: &str,
    parameters: &mut Value,
) -> Result<PathBuf, String> {
    if !settings.enabled {
        return Err("样图生成未启用".to_string());
    }
    let lines = training_sample_prompt_lines(settings, dataset_dir, caption_extension)?;
    let values = parameters
        .as_object_mut()
        .ok_or_else(|| "训练参数必须是对象".to_string())?;
    let output_dir = values
        .get("output_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "启用样图前请填写 LoRA 输出文件夹".to_string())?;
    let sample_dir = PathBuf::from(output_dir).join("samples");
    std::fs::create_dir_all(&sample_dir)
        .map_err(|error| format!("无法创建样图输出目录 {}: {error}", sample_dir.display()))?;
    let prompt_file = sample_dir.join("sample_prompts.txt");
    std::fs::write(&prompt_file, format!("{}\n", lines.join("\n")))
        .map_err(|error| format!("无法写入样图 Prompt 文件: {error}"))?;
    values.insert(
        "sample_prompts".to_string(),
        Value::String(prompt_file.to_string_lossy().to_string()),
    );
    values.insert(
        "sample_every_n_epochs".to_string(),
        Value::from(settings.every_n_epochs),
    );
    values.remove("sample_every_n_steps");
    values.remove("sample_at_first");
    Ok(prompt_file)
}

#[derive(Debug, Clone, Deserialize)]
struct TrainingPresetInput {
    name: String,
    training: TrainingRequest,
}

#[derive(Debug, Deserialize)]
struct TrainingPresetImportRequest {
    name: String,
    #[serde(default = "default_training_adapter_id")]
    adapter_id: String,
    #[serde(default = "default_training_runtime_profile_id")]
    runtime_profile_id: String,
    #[serde(default)]
    gpu_ids: Vec<String>,
    toml: String,
}

#[derive(Debug, Serialize)]
struct TrainingPresetResponse {
    id: String,
    name: String,
    training: TrainingRequest,
    created_at: u64,
    updated_at: u64,
    version_count: usize,
}

#[derive(Debug, Serialize)]
struct TrainingPresetExportResponse {
    name: String,
    toml: String,
}

fn default_training_adapter_id() -> String {
    "sdxl-lora".to_string()
}

fn default_training_runtime_profile_id() -> String {
    "windows".to_string()
}

fn validate_training_preset_input(input: &TrainingPresetInput) -> Result<(), ApiError> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 120 {
        return Err(ApiError::bad_request(
            "invalid_training_preset_name",
            "预设名称不能为空且最长 120 个字符",
        ));
    }
    input
        .training
        .validate()
        .map_err(|message| ApiError::bad_request("invalid_training_preset", message))?;
    Ok(())
}

fn training_preset_response(preset: TrainingPresetRecord) -> TrainingPresetResponse {
    TrainingPresetResponse {
        id: preset.id,
        name: preset.name,
        training: preset.training,
        created_at: preset.created_at,
        updated_at: preset.updated_at,
        version_count: preset.versions.len(),
    }
}

async fn list_training_presets(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<Vec<TrainingPresetResponse>>>, ApiError> {
    let presets = state
        .training_presets
        .list()
        .map_err(ApiError::internal)?
        .into_iter()
        .map(training_preset_response)
        .collect();
    Ok(Json(ApiSuccess {
        data: presets,
        meta: None,
    }))
}

async fn create_training_preset(
    State(state): State<AppState>,
    Json(input): Json<TrainingPresetInput>,
) -> Result<(StatusCode, Json<ApiSuccess<TrainingPresetResponse>>), ApiError> {
    validate_training_preset_input(&input)?;
    let preset = state
        .training_presets
        .create(input.name.trim().to_string(), input.training)
        .map_err(ApiError::internal)?;
    Ok((
        StatusCode::CREATED,
        Json(ApiSuccess {
            data: training_preset_response(preset),
            meta: None,
        }),
    ))
}

async fn update_training_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<TrainingPresetInput>,
) -> Result<Json<ApiSuccess<TrainingPresetResponse>>, ApiError> {
    validate_training_preset_input(&input)?;
    let preset = state
        .training_presets
        .update(&id, input.name.trim().to_string(), input.training)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("training_preset_not_found", "训练预设不存在"))?;
    Ok(Json(ApiSuccess {
        data: training_preset_response(preset),
        meta: None,
    }))
}

async fn export_training_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingPresetExportResponse>>, ApiError> {
    let preset = state
        .training_presets
        .get(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("training_preset_not_found", "训练预设不存在"))?;
    let adapter = crate::training::adapter_by_id(&preset.training.adapter_id).ok_or_else(|| {
        ApiError::bad_request(
            "unsupported_training_adapter",
            "预设使用了不支持的训练模型适配器",
        )
    })?;
    let toml = serialize_toml(&adapter, &preset.training.parameters)
        .map_err(|message| ApiError::bad_request("invalid_training_preset", message))?;
    Ok(Json(ApiSuccess {
        data: TrainingPresetExportResponse {
            name: preset.name,
            toml,
        },
        meta: None,
    }))
}

async fn import_training_preset(
    State(state): State<AppState>,
    Json(request): Json<TrainingPresetImportRequest>,
) -> Result<(StatusCode, Json<ApiSuccess<TrainingPresetResponse>>), ApiError> {
    let input = training_preset_input_from_toml(request)?;
    create_training_preset(State(state), Json(input)).await
}

async fn update_training_preset_from_toml(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TrainingPresetImportRequest>,
) -> Result<Json<ApiSuccess<TrainingPresetResponse>>, ApiError> {
    let input = training_preset_input_from_toml(request)?;
    validate_training_preset_input(&input)?;
    let preset = state
        .training_presets
        .update(&id, input.name.trim().to_string(), input.training)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("training_preset_not_found", "训练预设不存在"))?;
    Ok(Json(ApiSuccess {
        data: training_preset_response(preset),
        meta: None,
    }))
}

fn training_preset_input_from_toml(
    request: TrainingPresetImportRequest,
) -> Result<TrainingPresetInput, ApiError> {
    let parsed = request.toml.parse::<toml::Value>().map_err(|error| {
        ApiError::bad_request("invalid_training_toml", format!("TOML 无效: {error}"))
    })?;
    let parameters = toml_training_parameters(&parsed)?;
    Ok(TrainingPresetInput {
        name: request.name,
        training: TrainingRequest {
            adapter_id: request.adapter_id,
            runtime_profile_id: request.runtime_profile_id,
            parameters,
            gpu_ids: request.gpu_ids,
            gallery_dataset: None,
            gallery_datasets: vec![],
            sample: None,
        },
    })
}

fn toml_training_parameters(value: &toml::Value) -> Result<Value, ApiError> {
    let table = value.as_table().ok_or_else(|| {
        ApiError::bad_request("invalid_training_toml", "训练 TOML 顶层必须是参数表")
    })?;
    let mut parameters = serde_json::Map::new();
    for (key, value) in table {
        let converted = toml_training_value(value).ok_or_else(|| {
            ApiError::bad_request(
                "invalid_training_toml",
                format!("参数 {key} 包含不支持的嵌套表"),
            )
        })?;
        parameters.insert(key.clone(), converted);
    }
    Ok(Value::Object(parameters))
}

fn toml_training_value(value: &toml::Value) -> Option<Value> {
    match value {
        toml::Value::String(value) => Some(Value::String(value.clone())),
        toml::Value::Integer(value) => Some(Value::from(*value)),
        toml::Value::Float(value) => serde_json::Number::from_f64(*value).map(Value::Number),
        toml::Value::Boolean(value) => Some(Value::Bool(*value)),
        toml::Value::Datetime(value) => Some(Value::String(value.to_string())),
        toml::Value::Array(values) => values
            .iter()
            .map(toml_training_value)
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        toml::Value::Table(_) => None,
    }
}

async fn training_adapters(
    State(state): State<AppState>,
) -> Json<ApiSuccess<Vec<crate::training::TrainingAdapter>>> {
    let training_root = state.training_root.clone();
    let exported =
        tokio::task::spawn_blocking(move || inspect_upstream_adapter_fields(&training_root))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
    Json(ApiSuccess {
        data: augment_adapters_with_upstream_fields(builtin_adapters(), exported),
        meta: None,
    })
}

async fn training_runtime_profiles(
    State(state): State<AppState>,
) -> Json<ApiSuccess<Vec<TrainingRuntimeProfileResponse>>> {
    let state_for_worker = state.clone();
    let training_root = state.training_root.clone();
    let profiles = tokio::task::spawn_blocking(move || {
        let runtime_root = installed_training_runtime_root(&training_root);
        available_training_runtime_profiles(&training_root)
            .into_iter()
            .map(|profile| {
                training_runtime_profile_response(&state_for_worker, &runtime_root, &profile)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    Json(ApiSuccess {
        data: profiles,
        meta: None,
    })
}

fn training_runtime_profile_response(
    state: &AppState,
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> TrainingRuntimeProfileResponse {
    let install_state = state.training_runtime_installs.state(&profile.id);
    TrainingRuntimeProfileResponse {
        id: profile.id.clone(),
        label: profile.label.clone(),
        kind: profile.kind.to_string(),
        managed: profile.managed,
        installed: runtime_root
            .join("sd-scripts/sdxl_train_network.py")
            .is_file()
            && runtime_root.join("RUNTIME_MANIFEST.json").is_file()
            && profile.python.is_file(),
        installing: install_state.active,
        last_error: install_state.error,
        runtime_root: runtime_root.to_string_lossy().to_string(),
        python_path: profile.python.to_string_lossy().to_string(),
    }
}

async fn training_runtime_diagnostics(
    State(state): State<AppState>,
    AxumPath(profile): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingRuntimeDiagnosticsResponse>>, ApiError> {
    let runtime_root = installed_training_runtime_root(&state.training_root);
    let state_for_response = state.clone();
    let profile_for_check = profile.clone();
    let training_root = state.training_root.clone();
    let diagnostics = tokio::task::spawn_blocking(move || {
        let resolved = resolve_training_runtime_profile(&training_root, &profile_for_check)?;
        let profile_response =
            training_runtime_profile_response(&state_for_response, &runtime_root, &resolved);
        let checks = collect_training_runtime_diagnostics(&runtime_root, &resolved);
        Ok::<_, String>(TrainingRuntimeDiagnosticsResponse {
            profile: profile_response,
            checks,
        })
    })
    .await
    .map_err(|error| ApiError::internal(format!("无法读取训练运行时诊断: {error}")))?
    .map_err(|error| ApiError::bad_request("invalid_training_runtime", error))?;
    Ok(Json(ApiSuccess {
        data: diagnostics,
        meta: None,
    }))
}

async fn install_training_runtime(
    State(state): State<AppState>,
    AxumPath(profile): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingRuntimeProfileResponse>>, ApiError> {
    let resolved = resolve_training_runtime_profile(&state.training_root, &profile)
        .map_err(|error| ApiError::bad_request("invalid_training_runtime", error))?;
    if !state.training_runtime_installs.begin(&profile) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "training_runtime_installing".to_string(),
            message: "该训练运行时正在安装".to_string(),
            retryable: true,
            fields: None,
        });
    }
    let root = state.training_root.clone();
    let coordinator = state.training_runtime_installs.clone();
    let profile_for_worker = profile.clone();
    let resolved_for_worker = resolved.clone();
    tokio::task::spawn_blocking(move || {
        let result = install_training_runtime_profile(&root, &resolved_for_worker);
        coordinator.complete(&profile_for_worker, result);
    });
    let runtime_root = installed_training_runtime_root(&state.training_root);
    Ok(Json(ApiSuccess {
        data: training_runtime_profile_response(&state, &runtime_root, &resolved),
        meta: None,
    }))
}

fn install_training_runtime_profile(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Result<(), String> {
    let runtime_root = installed_training_runtime_root(training_root);
    install_bundled_training_runtime_source(&runtime_root)?;
    if profile.managed {
        if !profile.python.is_file() {
            create_training_runtime_venv(&runtime_root, profile)?;
        }
    }
    // Clicking the explicit install/sync action is the only path that changes
    // an external Conda/venv environment.  This keeps ordinary diagnostics
    // read-only while allowing the user-selected `conda:lora` profile to be
    // brought to the pinned upstream dependency set.
    let scripts_root = runtime_root.join("sd-scripts");
    run_training_runtime_python(
        &scripts_root,
        profile,
        ["-m", "pip", "install", "--upgrade", "pip"],
        "升级训练环境 pip",
    )?;
    let requirements = runtime_argument_path(&scripts_root.join("requirements.txt"), profile)?;
    run_training_runtime_python(
        &scripts_root,
        profile,
        ["-m", "pip", "install", "-r", &requirements],
        "安装 kohya_ss v26.0.0 训练依赖",
    )?;
    let checks = collect_training_runtime_diagnostics(&runtime_root, profile);
    if checks.iter().any(|check| {
        matches!(
            check.id,
            "python" | "python-version" | "torch" | "accelerate" | "upstream-modules"
        ) && !check.ok
    }) {
        return Err("训练环境安装完成但健康检查未通过，请打开诊断查看详情".to_string());
    }
    Ok(())
}

async fn vision_crop_runtime_health(
    State(state): State<AppState>,
    AxumPath(profile): AxumPath<String>,
) -> Result<Json<ApiSuccess<VisionCropRuntimeHealthResponse>>, ApiError> {
    let root = state.training_root.clone();
    let installing = state.training_runtime_installs.state(&profile);
    let response = tokio::task::spawn_blocking(move || {
        let profile = resolve_training_runtime_profile(&root, &profile)?;
        vision_crop_health_response(&root, &profile, "0", installing)
    })
    .await
    .map_err(|error| ApiError::internal(format!("无法检查智能裁剪运行时: {error}")))?
    .map_err(|error| ApiError::bad_request("invalid_vision_crop_runtime", error))?;
    Ok(Json(ApiSuccess {
        data: response,
        meta: None,
    }))
}

async fn install_vision_crop_runtime(
    State(state): State<AppState>,
    AxumPath(profile): AxumPath<String>,
) -> Result<Json<ApiSuccess<VisionCropRuntimeHealthResponse>>, ApiError> {
    let resolved = resolve_training_runtime_profile(&state.training_root, &profile)
        .map_err(|error| ApiError::bad_request("invalid_vision_crop_runtime", error))?;
    if !state.training_runtime_installs.begin(&profile) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "vision_crop_runtime_installing".to_string(),
            message: "该 Python 运行时正在安装依赖".to_string(),
            retryable: true,
            fields: None,
        });
    }
    let root = state.training_root.clone();
    let coordinator = state.training_runtime_installs.clone();
    let profile_for_worker = profile.clone();
    let resolved_for_worker = resolved.clone();
    tokio::task::spawn_blocking(move || {
        let result = install_vision_crop_runtime_profile(&root, &resolved_for_worker);
        coordinator.complete(&profile_for_worker, result);
    });
    let response = VisionCropRuntimeHealthResponse {
        runtime_profile_id: resolved.id.clone(),
        python_path: resolved.python.to_string_lossy().to_string(),
        ready: false,
        installing: true,
        gpu_id: "0".to_string(),
        providers: Vec::new(),
        gpu_name: None,
        models_ready: false,
        message: "正在安装并预热动漫检测模型；完成后请重新检查运行时状态".to_string(),
        last_error: None,
    };
    Ok(Json(ApiSuccess {
        data: response,
        meta: None,
    }))
}

fn install_vision_crop_runtime_profile(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Result<(), String> {
    let runtime_root = installed_training_runtime_root(training_root);
    let mut command = training_runtime_python_command(&runtime_root, profile)?;
    command.args([
        "-m",
        "pip",
        "install",
        "--upgrade-strategy",
        "only-if-needed",
        "dghs-imgutils[gpu]==0.19.0",
        "rtmlib==0.0.16",
        // dghs-imgutils' optional tokenizer requirement is not used by its
        // ONNX detectors. Pin the Hub/tokenizer pair compatible with the
        // existing LoRA transformers runtime so clicking this button cannot
        // make `transformers` unimportable.
        "huggingface-hub==0.31.0",
        "tokenizers==0.19.1",
    ]);
    run_training_install_command(command, "安装动漫检测与姿态模型依赖")?;
    let warmup = run_anime_crop_worker(
        training_root,
        profile,
        "0",
        serde_json::json!({"action": "warmup"}),
    )?;
    if !warmup
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(worker_error_message(&warmup));
    }
    Ok(())
}

fn vision_crop_health_response(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    gpu_id: &str,
    install_state: TrainingRuntimeInstallState,
) -> Result<VisionCropRuntimeHealthResponse, String> {
    let value = run_anime_crop_worker(
        training_root,
        profile,
        gpu_id,
        serde_json::json!({"action": "health"}),
    )?;
    let ready = value.get("ready").and_then(Value::as_bool).unwrap_or(false);
    let providers = value
        .get("providers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let gpu_name = value
        .pointer("/gpu/name")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Ok(VisionCropRuntimeHealthResponse {
        runtime_profile_id: profile.id.clone(),
        python_path: profile.python.to_string_lossy().to_string(),
        ready,
        installing: install_state.active,
        gpu_id: gpu_id.to_string(),
        providers,
        gpu_name,
        models_ready: value
            .get("models_ready")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        message: if ready {
            "动漫检测模型运行时已就绪".to_string()
        } else {
            worker_error_message(&value)
        },
        last_error: install_state.error,
    })
}

fn run_anime_crop_worker(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    gpu_id: &str,
    payload: Value,
) -> Result<Value, String> {
    let output = run_anime_crop_worker_output(training_root, profile, gpu_id, payload)?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "动漫检测 worker 输出无效: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })
}

fn run_anime_crop_detection_worker(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    gpu_id: &str,
    payload: Value,
) -> Result<Vec<AnimeCropAnalysis>, String> {
    let output = run_anime_crop_worker_output(training_root, profile, gpu_id, payload)?;
    parse_anime_crop_detection_jsonl(&output.stdout).map_err(|error| {
        format!(
            "动漫检测 worker JSONL 输出无效: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })
}

fn run_anime_crop_worker_output(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    gpu_id: &str,
    mut payload: Value,
) -> Result<Output, String> {
    let runtime_root = installed_training_runtime_root(training_root);
    let worker = training_root.join("anime_crop_worker.py");
    if !worker.is_file() {
        return Err("动漫智能裁剪 worker 不存在，请重启应用以同步内置脚本".to_string());
    }
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "智能裁剪 worker 请求格式无效".to_string())?;
    object.insert("gpu_id".to_string(), Value::String(gpu_id.to_string()));
    let worker_argument = runtime_argument_path(&worker, profile)?;
    let mut command = training_runtime_python_command(&runtime_root, profile)?;
    command
        .arg(worker_argument)
        .env("CUDA_VISIBLE_DEVICES", gpu_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动动漫检测 worker: {error}"))?;
    let input = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(|| "无法写入动漫检测 worker".to_string())?
        .write_all(&input)
        .map_err(|error| format!("无法写入动漫检测 worker: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("等待动漫检测 worker 失败: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "动漫检测 worker 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

fn parse_anime_crop_detection_jsonl(output: &[u8]) -> Result<Vec<AnimeCropAnalysis>, String> {
    let source = std::str::from_utf8(output)
        .map_err(|error| format!("worker stdout 不是 UTF-8: {error}"))?;
    let mut analyses = Vec::new();
    let mut completed = None;
    for (line_number, line) in source.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .map_err(|error| format!("第 {} 行不是 JSON: {error}", line_number + 1))?;
        match record.get("type").and_then(Value::as_str) {
            Some("detection") => {
                let item = record
                    .get("item")
                    .cloned()
                    .ok_or_else(|| format!("第 {} 行缺少检测结果", line_number + 1))?;
                let analysis =
                    serde_json::from_value::<AnimeCropAnalysis>(item).map_err(|error| {
                        format!("第 {} 行检测结果格式无效: {error}", line_number + 1)
                    })?;
                analyses.push(analysis);
            }
            Some("complete") => {
                if completed.replace(record).is_some() {
                    return Err("worker 返回了多个完成记录".to_string());
                }
            }
            _ => return Err(format!("第 {} 行包含未知记录类型", line_number + 1)),
        }
    }
    let completed = completed.ok_or_else(|| "worker 未返回完成记录".to_string())?;
    if !completed
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(worker_error_message(&completed));
    }
    let expected = completed
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| "worker 完成记录缺少 count".to_string())?;
    if usize::try_from(expected).ok() != Some(analyses.len()) {
        return Err(format!(
            "worker 完成记录数量为 {expected}，实际收到 {} 个检测结果",
            analyses.len()
        ));
    }
    Ok(analyses)
}

#[cfg(test)]
mod anime_crop_worker_contract_tests {
    use super::parse_anime_crop_detection_jsonl;

    #[test]
    fn detection_jsonl_accepts_empty_detection_sets_without_losing_the_media_id() {
        let output = br#"{"type":"detection","item":{"media_id":"image-1","width":1200,"height":1600,"persons":[]}}
{"type":"complete","ready":true,"count":1}
"#;

        let analyses = parse_anime_crop_detection_jsonl(output).unwrap();

        assert_eq!(analyses.len(), 1);
        assert_eq!(analyses[0].media_id, "image-1");
        assert!(analyses[0].persons.is_empty());
    }

    #[test]
    fn detection_jsonl_rejects_corruption_or_a_missing_terminal_record() {
        assert!(parse_anime_crop_detection_jsonl(b"not-json\n").is_err());
        assert!(parse_anime_crop_detection_jsonl(
            br#"{"type":"detection","item":{"media_id":"image-1"}}\n"#
        )
        .is_err());
    }

    #[test]
    fn detection_jsonl_surfaces_missing_models_or_gpu_as_a_preflight_failure() {
        let model_missing =
            br#"{"type":"complete","ready":false,"missing":["dghs-imgutils==0.19.0"],"count":0}
"#;
        let gpu_missing = br#"{"type":"complete","ready":false,"missing":["ONNX Runtime CUDA provider / CUDA GPU"],"count":0}
"#;

        assert!(parse_anime_crop_detection_jsonl(model_missing)
            .unwrap_err()
            .contains("dghs-imgutils==0.19.0"));
        assert!(parse_anime_crop_detection_jsonl(gpu_missing)
            .unwrap_err()
            .contains("CUDA"));
    }
}

fn worker_error_message(value: &Value) -> String {
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return error.to_string();
    }
    if let Some(missing) = value.get("missing").and_then(Value::as_array) {
        let details = missing
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("、");
        if !details.is_empty() {
            return format!("缺少 {details}；请点击“安装并预热检测模型”");
        }
    }
    "动漫检测运行时未就绪；请点击“安装并预热检测模型”".to_string()
}

fn runtime_argument_path(
    path: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Result<String, String> {
    if profile.is_wsl() {
        windows_path_for_wsl(
            path,
            std::env::var("DANBOORU_TRAINING_WSL_DISTRO")
                .ok()
                .as_deref(),
        )
    } else {
        Ok(path.to_string_lossy().to_string())
    }
}

fn create_training_runtime_venv(
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Result<(), String> {
    let venv = runtime_root.join("venv");
    if !profile.is_wsl() {
        let python =
            std::env::var("DANBOORU_TRAINING_WINDOWS_PYTHON").unwrap_or_else(|_| "py".to_string());
        let mut command = Command::new(&python);
        if python.eq_ignore_ascii_case("py") || python.eq_ignore_ascii_case("py.exe") {
            command.arg("-3");
        }
        command.args(["-m", "venv"]).arg(&venv);
        run_training_install_command(command, "创建 Windows 隔离 Python 环境")
    } else {
        let wsl_venv = runtime_argument_path(&venv, profile)?;
        let mut command = Command::new("wsl.exe");
        if let Ok(distro) = std::env::var("DANBOORU_TRAINING_WSL_DISTRO") {
            if !distro.trim().is_empty() {
                command.args(["--distribution", &distro]);
            }
        }
        command.args(["--exec", "python3", "-m", "venv", &wsl_venv]);
        run_training_install_command(command, "创建 WSL 隔离 Python 环境")
    }
}

fn run_training_runtime_python<'a>(
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    args: impl IntoIterator<Item = &'a str>,
    phase: &str,
) -> Result<(), String> {
    let mut command = training_runtime_python_command(runtime_root, profile)?;
    command.args(args);
    run_training_install_command(command, phase)
}

fn training_runtime_python_command(
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Result<Command, String> {
    let mut command = if profile.is_wsl() {
        let mut command = Command::new("wsl.exe");
        if let Ok(distro) = std::env::var("DANBOORU_TRAINING_WSL_DISTRO") {
            if !distro.trim().is_empty() {
                command.args(["--distribution", &distro]);
            }
        }
        command.arg("--exec");
        command.arg(runtime_argument_path(&profile.python, profile)?);
        command
    } else {
        Command::new(&profile.python)
    };
    // External Conda environments can run the vision crop worker without a
    // managed kohya runtime.  Do not set a non-existent kohya directory as
    // the Windows process working directory (Windows then refuses to spawn
    // Python with ERROR_DIRECTORY).  Managed runtimes retain their existing
    // working directory once it has been installed.
    if !profile.is_wsl() && runtime_root.is_dir() {
        command.current_dir(runtime_root);
    }
    Ok(command)
}

fn run_training_install_command(mut command: Command, phase: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{phase}时无法启动命令: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{phase}失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn collect_training_runtime_diagnostics(
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
) -> Vec<TrainingRuntimeCheckResponse> {
    let source_ok = runtime_root
        .join("sd-scripts/sdxl_train_network.py")
        .is_file()
        && runtime_root.join("RUNTIME_MANIFEST.json").is_file();
    let mut checks = vec![TrainingRuntimeCheckResponse {
        id: "source",
        ok: source_ok,
        detail: if source_ok {
            "已安装锁定的 kohya_ss v26.0.0 源码".to_string()
        } else {
            "缺少内置 kohya_ss v26.0.0 源码".to_string()
        },
    }];
    if !profile.python.is_file() {
        checks.push(TrainingRuntimeCheckResponse {
            id: "python",
            ok: false,
            detail: format!("未找到 Python：{}", profile.python.display()),
        });
        return checks;
    }
    checks.push(TrainingRuntimeCheckResponse {
        id: "python",
        ok: true,
        detail: profile.python.to_string_lossy().to_string(),
    });
    let health = run_training_runtime_python_output(
        runtime_root,
        profile,
        ["-c", "import json, sys, torch, accelerate, diffusers, safetensors, transformers; print(json.dumps({'python': f'{sys.version_info.major}.{sys.version_info.minor}', 'torch': torch.__version__, 'cuda': torch.cuda.is_available(), 'accelerate': accelerate.__version__, 'diffusers': diffusers.__version__, 'transformers': transformers.__version__, 'hunyuan_image_text_encoder': hasattr(transformers, 'Qwen2_5_VLConfig')}))"],
    );
    match health {
        Ok(output) => {
            let value = serde_json::from_str::<Value>(&output).unwrap_or(Value::Null);
            let python = value
                .get("python")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let supported_python = matches!(python, "3.10" | "3.11");
            checks.push(TrainingRuntimeCheckResponse {
                id: "python-version",
                ok: supported_python,
                detail: if supported_python {
                    format!("Python {python}")
                } else {
                    format!("Python {python}；kohya_ss v26.0.0 需要 Python 3.10 或 3.11")
                },
            });
            let torch = value
                .get("torch")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let accelerate = value
                .get("accelerate")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            checks.push(TrainingRuntimeCheckResponse {
                id: "torch",
                ok: true,
                detail: format!("torch {torch}"),
            });
            checks.push(TrainingRuntimeCheckResponse {
                id: "accelerate",
                ok: true,
                detail: format!("accelerate {accelerate}"),
            });
            let hunyuan_ready = value
                .get("hunyuan_image_text_encoder")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            checks.push(TrainingRuntimeCheckResponse {
                id: "upstream-modules",
                ok: hunyuan_ready,
                detail: if hunyuan_ready {
                    format!(
                        "diffusers {}、transformers {}、safetensors 与 HunyuanImage 文本编码器可导入",
                        value.get("diffusers").and_then(Value::as_str).unwrap_or("unknown"),
                        value.get("transformers").and_then(Value::as_str).unwrap_or("unknown"),
                    )
                } else {
                    format!(
                        "transformers {} 缺少 Qwen2_5_VLConfig；HunyuanImage-2.1 入口不可用。点击“同步训练源码”后按锁定 requirements 更新此环境。",
                        value.get("transformers").and_then(Value::as_str).unwrap_or("unknown"),
                    )
                },
            });
            let cuda = value.get("cuda").and_then(Value::as_bool).unwrap_or(false);
            checks.push(TrainingRuntimeCheckResponse {
                id: "cuda",
                ok: cuda,
                detail: if cuda {
                    "CUDA 可用于训练".to_string()
                } else {
                    "PyTorch 未检测到 CUDA".to_string()
                },
            });
        }
        Err(error) => {
            checks.push(TrainingRuntimeCheckResponse {
                id: "torch",
                ok: false,
                detail: error.clone(),
            });
            checks.push(TrainingRuntimeCheckResponse {
                id: "accelerate",
                ok: false,
                detail: error,
            });
            checks.push(TrainingRuntimeCheckResponse {
                id: "upstream-modules",
                ok: false,
                detail: "无法导入 kohya_ss 所需的 diffusers / transformers / safetensors"
                    .to_string(),
            });
        }
    }
    checks
}

fn run_training_runtime_python_output<'a>(
    runtime_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<String, String> {
    let mut command = training_runtime_python_command(runtime_root, profile)?;
    command.args(args);
    let output = command
        .output()
        .map_err(|error| format!("无法运行训练环境诊断: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "训练环境诊断失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn training_gpus() -> Json<ApiSuccess<Vec<TrainingGpuResponse>>> {
    Json(ApiSuccess {
        data: training_gpu_inventory(),
        meta: None,
    })
}

#[derive(Debug, Deserialize)]
struct TrainingPreviewRequest {
    adapter_id: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct TrainingPreviewResponse {
    toml: String,
}

async fn training_preview(
    Json(request): Json<TrainingPreviewRequest>,
) -> Result<Json<ApiSuccess<TrainingPreviewResponse>>, ApiError> {
    let adapter = crate::training::adapter_by_id(&request.adapter_id).ok_or_else(|| {
        ApiError::bad_request("unsupported_training_adapter", "不支持的训练模型适配器")
    })?;
    let toml = serialize_toml(&adapter, &request.parameters)
        .map_err(|message| ApiError::bad_request("invalid_training_parameters", message))?;
    Ok(Json(ApiSuccess {
        data: TrainingPreviewResponse { toml },
        meta: None,
    }))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LoraSvdAnalysisFileRequest {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LoraSvdAnalysisRequest {
    runtime_profile_id: String,
    files: Vec<LoraSvdAnalysisFileRequest>,
    device: String,
}

fn validate_lora_svd_request(request: &mut LoraSvdAnalysisRequest) -> Result<u64, String> {
    if request.device != "auto" {
        return Err("当前仅支持 device=auto".to_string());
    }
    if !(1..=5).contains(&request.files.len()) {
        return Err("一次分析必须选择 1 到 5 个 LoRA 文件".to_string());
    }
    let mut seen = HashSet::new();
    let mut total_size = 0_u64;
    for file in &mut request.files {
        if file.path.trim().is_empty() {
            return Err("LoRA 文件路径不能为空".to_string());
        }
        let canonical = std::fs::canonicalize(&file.path)
            .map_err(|error| format!("无法访问 LoRA 文件 {}: {error}", file.path))?;
        if canonical
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("safetensors"))
        {
            return Err(format!("仅支持 .safetensors LoRA：{}", canonical.display()));
        }
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| format!("无法读取 LoRA 文件 {}: {error}", canonical.display()))?;
        if !metadata.is_file() {
            return Err(format!("LoRA 路径不是常规文件：{}", canonical.display()));
        }
        if !seen.insert(canonical.clone()) {
            return Err(format!("重复选择了同一 LoRA 文件：{}", canonical.display()));
        }
        total_size = total_size.saturating_add(metadata.len());
        file.path = canonical.to_string_lossy().to_string();
        file.label = file
            .label
            .take()
            .map(|label| label.trim().chars().take(160).collect::<String>())
            .filter(|label| !label.is_empty());
    }
    Ok(total_size)
}

fn lora_svd_device_choice(
    total_size: u64,
    training_active: bool,
    gpus: &[TrainingGpuResponse],
) -> (String, String) {
    const MIB: u64 = 1024 * 1024;
    if training_active {
        return (
            "cpu".to_string(),
            "检测到运行中的训练任务，避免争用显存".to_string(),
        );
    }
    if gpus.iter().any(|gpu| !gpu.external_processes.is_empty()) {
        return (
            "cpu".to_string(),
            "检测到外部 GPU 进程，避免影响其他本地模型".to_string(),
        );
    }
    if total_size < 512 * MIB {
        return (
            "cpu".to_string(),
            "适配器较小，CPU 分解避免 GPU 传输开销".to_string(),
        );
    }
    let candidate = gpus
        .iter()
        .filter_map(|gpu| {
            let free = gpu.memory_total_mib.saturating_sub(gpu.memory_used_mib);
            (free >= 4096).then_some((free, gpu.id.as_str()))
        })
        .max_by_key(|(free, _)| *free);
    match candidate {
        Some((_, id)) => (
            format!("cuda:{id}"),
            "大型适配器使用空闲 CUDA 设备".to_string(),
        ),
        None => (
            "cpu".to_string(),
            "没有具备至少 4 GiB 空闲显存的 GPU".to_string(),
        ),
    }
}

fn run_lora_svd_inspector(
    training_root: &Path,
    profile: &ResolvedTrainingRuntimeProfile,
    request: &LoraSvdAnalysisRequest,
    device: &str,
    device_reason: &str,
) -> Result<Value, String> {
    let runtime_root = installed_training_runtime_root(&training_root);
    let inspector = training_root.join("lora_svd_inspector.py");
    if !profile.python.is_file() {
        return Err(format!("训练 Python 不存在：{}", profile.python.display()));
    }
    if !inspector.is_file() {
        return Err(format!("LoRA SVD 分析器不存在：{}", inspector.display()));
    }
    let inspector_argument = runtime_argument_path(&inspector, profile)?;
    let mut payload = request.clone();
    for file in &mut payload.files {
        file.path = runtime_argument_path(Path::new(&file.path), profile)?;
    }
    let request_bytes = serde_json::to_vec(&payload)
        .map_err(|error| format!("无法序列化 SVD 分析请求: {error}"))?;
    let mut command = training_runtime_python_command(&runtime_root, profile)?;
    let mut child = command
        .arg(inspector_argument)
        .arg("--device")
        .arg(device)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 LoRA SVD 分析器: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "无法连接 LoRA SVD 分析器输入".to_string())?
        .write_all(&request_bytes)
        .map_err(|error| format!("无法写入 LoRA SVD 分析请求: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("LoRA SVD 分析器异常退出: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "LoRA SVD 分析失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut result = serde_json::from_slice::<Value>(&output.stdout)
        .map_err(|error| format!("无法解析 LoRA SVD 分析结果: {error}"))?;
    let root = result
        .as_object_mut()
        .ok_or_else(|| "LoRA SVD 分析结果不是对象".to_string())?;
    if let Some(execution) = root.get_mut("execution").and_then(Value::as_object_mut) {
        execution.insert(
            "selection_reason".to_string(),
            Value::String(device_reason.to_string()),
        );
    }
    Ok(result)
}

fn lora_svd_summary(mut payload: Value) -> Value {
    if let Some(reports) = payload.get_mut("reports").and_then(Value::as_array_mut) {
        for report in reports {
            for spectrum_key in ["global_singular_values", "global_cumulative_energy"] {
                if let Some(values) = report.get_mut(spectrum_key).and_then(Value::as_array_mut) {
                    let count = values.len();
                    values.truncate(LORA_SVD_SUMMARY_SPECTRUM_LIMIT);
                    if let Some(object) = report.as_object_mut() {
                        object.insert(format!("{spectrum_key}_count"), Value::from(count as u64));
                    }
                }
            }
            if let Some(modules) = report.get_mut("modules").and_then(Value::as_array_mut) {
                for module in modules {
                    if let Some(object) = module.as_object_mut() {
                        object.remove("singular_values");
                    }
                }
            }
        }
    }
    payload
}

fn cached_lora_svd_analysis(state: &AppState, id: &str) -> Result<CachedLoraSvdAnalysis, ApiError> {
    if id.len() != 36
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(ApiError::bad_request(
            "invalid_lora_svd_analysis",
            "SVD 分析标识无效",
        ));
    }
    state
        .lora_svd_analyses
        .get(id)
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::not_found(
                "lora_svd_analysis_expired",
                "SVD 分析结果已过期，请重新分析",
            )
        })
}

async fn create_lora_svd_analysis(
    State(state): State<AppState>,
    Json(mut request): Json<LoraSvdAnalysisRequest>,
) -> Result<Json<ApiSuccess<Value>>, ApiError> {
    let total_size = validate_lora_svd_request(&mut request)
        .map_err(|message| ApiError::bad_request("invalid_lora_svd_request", message))?;
    let profile =
        resolve_training_runtime_profile(&state.training_root, &request.runtime_profile_id)
            .map_err(|message| ApiError::bad_request("invalid_training_runtime", message))?;
    let training_active = state
        .tasks
        .snapshot()
        .map_err(|error| ApiError::internal(format!("无法读取训练任务状态: {error}")))?
        .iter()
        .any(|task| {
            task.kind == "training"
                && matches!(task.status, TaskStatus::Running | TaskStatus::Pausing)
        });
    let gpus = training_gpu_inventory();
    let (device, device_reason) = lora_svd_device_choice(total_size, training_active, &gpus);
    let training_root = state.training_root.clone();
    let payload = tokio::task::spawn_blocking(move || {
        run_lora_svd_inspector(&training_root, &profile, &request, &device, &device_reason)
    })
    .await
    .map_err(|error| ApiError::internal(format!("LoRA SVD 分析任务异常: {error}")))?
    .map_err(|message| ApiError::bad_request("lora_svd_analysis_failed", message))?;
    let (id, _) = state
        .lora_svd_analyses
        .insert(payload)
        .map_err(ApiError::internal)?;
    let cached = cached_lora_svd_analysis(&state, &id)?;
    Ok(Json(ApiSuccess {
        data: lora_svd_summary(cached.payload),
        meta: None,
    }))
}

async fn lora_svd_module(
    State(state): State<AppState>,
    AxumPath((id, module_id)): AxumPath<(String, String)>,
) -> Result<Json<ApiSuccess<Value>>, ApiError> {
    let analysis = cached_lora_svd_analysis(&state, &id)?;
    let module = analysis
        .payload
        .get("reports")
        .and_then(Value::as_array)
        .and_then(|reports| {
            reports.iter().find_map(|report| {
                report
                    .get("modules")
                    .and_then(Value::as_array)
                    .and_then(|modules| {
                        modules.iter().find(|module| {
                            module.get("id").and_then(Value::as_str) == Some(module_id.as_str())
                        })
                    })
            })
        })
        .cloned()
        .ok_or_else(|| ApiError::not_found("lora_svd_module_not_found", "SVD 模块不存在"))?;
    Ok(Json(ApiSuccess {
        data: module,
        meta: None,
    }))
}

async fn export_lora_svd_analysis(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<Value>>, ApiError> {
    let analysis = cached_lora_svd_analysis(&state, &id)?;
    Ok(Json(ApiSuccess {
        data: analysis.payload,
        meta: None,
    }))
}

/// serde_urlencoded 0.7 只通过 `series[]=` 方括号语法收集 Vec；对常见的
/// 单值 `series=loss` 会直接报 "expected a sequence"。这里与两种写法都兼容，
/// 避免监控页与第三方调用方因为参数写法不同而收到 400。
fn deserialize_series_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SeriesValue {
        One(String),
        Many(Vec<String>),
    }
    match SeriesValue::deserialize(deserializer)? {
        SeriesValue::One(value) => Ok(vec![value]),
        SeriesValue::Many(values) => Ok(values),
    }
}

#[derive(Debug, Deserialize)]
struct TrainingMetricsQuery {
    #[serde(default, deserialize_with = "deserialize_series_list")]
    series: Vec<String>,
    max_points: Option<usize>,
    from_step: Option<u64>,
    to_step: Option<u64>,
    from_timestamp: Option<u64>,
    to_timestamp: Option<u64>,
}

#[derive(Debug, Serialize)]
struct TrainingMetricsResponse {
    metrics: Vec<crate::training::TrainingMetric>,
    cursor: u64,
}

#[derive(Debug, Clone, Serialize)]
struct TrainingMetricSeriesSummary {
    series: String,
    count: u64,
    first: crate::training::TrainingMetric,
    latest: crate::training::TrainingMetric,
    minimum: crate::training::TrainingMetric,
    maximum: crate::training::TrainingMetric,
}

#[derive(Debug, Serialize)]
struct TrainingMetricsOverviewResponse {
    cursor: u64,
    series: Vec<TrainingMetricSeriesSummary>,
}

fn metric_matches_query(
    metric: &crate::training::TrainingMetric,
    wanted: &HashSet<String>,
    query: &TrainingMetricsQuery,
) -> bool {
    (wanted.is_empty() || wanted.contains(&metric.series))
        && query.from_step.is_none_or(|minimum| metric.step >= minimum)
        && query.to_step.is_none_or(|maximum| metric.step <= maximum)
        && query
            .from_timestamp
            .is_none_or(|minimum| metric.timestamp >= minimum)
        && query
            .to_timestamp
            .is_none_or(|maximum| metric.timestamp <= maximum)
}

#[derive(Debug)]
struct OnlineMetricSampler {
    total: usize,
    limit: usize,
    seen: usize,
    all: Vec<crate::training::TrainingMetric>,
    first: Option<crate::training::TrainingMetric>,
    last: Option<crate::training::TrainingMetric>,
    buckets: Vec<
        Option<(
            crate::training::TrainingMetric,
            crate::training::TrainingMetric,
        )>,
    >,
}

impl OnlineMetricSampler {
    fn new(total: usize, limit: usize) -> Self {
        let bucket_count = limit.saturating_sub(2) / 2;
        Self {
            total,
            limit,
            seen: 0,
            all: Vec::with_capacity(total.min(limit)),
            first: None,
            last: None,
            buckets: (0..bucket_count).map(|_| None).collect(),
        }
    }

    fn push(&mut self, metric: crate::training::TrainingMetric) {
        let index = self.seen;
        self.seen = self.seen.saturating_add(1);
        if self.total <= self.limit {
            self.all.push(metric);
            return;
        }
        if index == 0 {
            self.first = Some(metric);
            return;
        }
        if index.saturating_add(1) >= self.total {
            self.last = Some(metric);
            return;
        }
        if self.buckets.is_empty() {
            return;
        }
        let interior = self.total.saturating_sub(2).max(1);
        let bucket_index =
            ((index - 1) * self.buckets.len() / interior).min(self.buckets.len().saturating_sub(1));
        match &mut self.buckets[bucket_index] {
            Some((low, high)) => {
                if metric.value.total_cmp(&low.value).is_lt() {
                    *low = metric.clone();
                }
                if metric.value.total_cmp(&high.value).is_gt() {
                    *high = metric;
                }
            }
            slot => *slot = Some((metric.clone(), metric)),
        }
    }

    fn finish(self) -> Vec<crate::training::TrainingMetric> {
        if self.total <= self.limit {
            return self.all;
        }
        let mut selected = BTreeMap::<(u64, u64, u64), crate::training::TrainingMetric>::new();
        let mut insert = |metric: crate::training::TrainingMetric| {
            selected.insert(
                (metric.step, metric.timestamp, metric.value.to_bits()),
                metric,
            );
        };
        if let Some(first) = self.first {
            insert(first);
        }
        for (low, high) in self.buckets.into_iter().flatten() {
            insert(low);
            insert(high);
        }
        if let Some(last) = self.last {
            insert(last);
        }
        selected.into_values().take(self.limit).collect()
    }
}

fn scan_training_metrics(
    path: &Path,
    query: &TrainingMetricsQuery,
    wanted: &HashSet<String>,
    mut visit: impl FnMut(crate::training::TrainingMetric),
) -> Result<(), std::io::Error> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        for metric in parse_metric_line(&line).unwrap_or_default() {
            if metric_matches_query(&metric, wanted, query) {
                visit(metric);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TrainingLogsQuery {
    tail: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TrainingLogsResponse {
    text: String,
}

async fn training_logs(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TrainingLogsQuery>,
) -> Result<Json<ApiSuccess<TrainingLogsResponse>>, ApiError> {
    validate_training_task_id(&id)?;
    let path = state
        .training_root
        .join("runs")
        .join(&id)
        .join("console.log");
    let source = match std::fs::read(&path) {
        Ok(bytes) => decode_training_log_bytes(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(ApiError::internal(format!("无法读取训练日志: {error}"))),
    };
    Ok(Json(ApiSuccess {
        data: TrainingLogsResponse {
            text: tail_training_log_lines(&source, query.tail.unwrap_or(300).clamp(1, 2_000)),
        },
        meta: None,
    }))
}

fn decode_training_log_bytes(bytes: &[u8]) -> String {
    // Python packages and native extensions on Windows can mix UTF-8 and the
    // active CP936/GBK or CP932/Shift-JIS code page in one console log. Decode
    // line by line so valid UTF-8 (including Chinese paths) never gets
    // reinterpreted.
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .map(|line| match std::str::from_utf8(line) {
            Ok(text) => text.to_string(),
            Err(_) => decode_windows_legacy_training_line(line),
        })
        .collect()
}

fn decode_windows_legacy_training_line(line: &[u8]) -> String {
    let candidates = [GBK, SHIFT_JIS]
        .into_iter()
        .map(|encoding| {
            let (text, had_errors) = encoding.decode_without_bom_handling(line);
            (text.into_owned(), had_errors)
        })
        .collect::<Vec<_>>();
    let Some((text, had_errors)) = candidates
        .into_iter()
        .max_by_key(|(text, had_errors)| legacy_training_text_score(text, *had_errors))
    else {
        return String::from_utf8_lossy(line).into_owned();
    };

    if had_errors {
        String::from_utf8_lossy(line).into_owned()
    } else {
        text
    }
}

fn legacy_training_text_score(text: &str, had_errors: bool) -> i32 {
    let mut score = if had_errors { -1_000 } else { 0 };
    for character in text.chars() {
        score += match character {
            '\u{3040}'..='\u{30ff}' => 4,
            '\u{ff61}'..='\u{ff9f}' => -4,
            '\u{4e00}'..='\u{9fff}' => 1,
            '\u{e000}'..='\u{f8ff}' => -8,
            '\u{fffd}' => -100,
            value if value.is_control() && !matches!(value, '\n' | '\r' | '\t') => -10,
            _ => 0,
        };
    }
    score
}

fn validate_training_task_id(id: &str) -> Result<(), ApiError> {
    if id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ApiError::bad_request(
            "invalid_training_task",
            "训练任务标识无效",
        ));
    }
    Ok(())
}

fn tail_training_log_lines(source: &str, tail: usize) -> String {
    let mut lines = source.lines().rev().take(tail).collect::<Vec<_>>();
    lines.reverse();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

async fn training_metrics(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TrainingMetricsQuery>,
) -> Result<Json<ApiSuccess<TrainingMetricsResponse>>, ApiError> {
    validate_training_task_id(&id)?;
    let max_points = query.max_points.unwrap_or(1200).clamp(1, 5000);
    let path = state
        .training_root
        .join("runs")
        .join(&id)
        .join("metrics.jsonl");
    let cursor = std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let wanted = query.series.iter().cloned().collect::<HashSet<_>>();
    let mut counts = BTreeMap::<String, usize>::new();
    scan_training_metrics(&path, &query, &wanted, |metric| {
        *counts.entry(metric.series).or_default() += 1;
    })
    .map_err(|error| ApiError::internal(format!("无法读取训练指标: {error}")))?;
    let series_count = counts.len().max(1);
    let base_limit = max_points / series_count;
    let extra_slots = max_points % series_count;
    let mut samplers = counts
        .into_iter()
        .enumerate()
        .map(|(index, (series, count))| {
            let limit = base_limit + usize::from(index < extra_slots);
            (series, OnlineMetricSampler::new(count, limit))
        })
        .collect::<BTreeMap<_, _>>();
    scan_training_metrics(&path, &query, &wanted, |metric| {
        if let Some(sampler) = samplers.get_mut(&metric.series) {
            sampler.push(metric);
        }
    })
    .map_err(|error| ApiError::internal(format!("无法读取训练指标: {error}")))?;
    let mut metrics = samplers
        .into_values()
        .flat_map(OnlineMetricSampler::finish)
        .collect::<Vec<_>>();
    metrics.sort_by_key(|metric| (metric.timestamp, metric.step, metric.series.clone()));
    Ok(Json(ApiSuccess {
        data: TrainingMetricsResponse { metrics, cursor },
        meta: None,
    }))
}

async fn training_metrics_overview(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingMetricsOverviewResponse>>, ApiError> {
    validate_training_task_id(&id)?;
    let path = state
        .training_root
        .join("runs")
        .join(&id)
        .join("metrics.jsonl");
    let cursor = std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut summaries = BTreeMap::<String, TrainingMetricSeriesSummary>::new();
    let file = match std::fs::File::open(&path) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ApiError::internal(format!("无法读取训练指标: {error}"))),
    };
    for metric in file
        .into_iter()
        .flat_map(|file| std::io::BufReader::new(file).lines().filter_map(Result::ok))
        .flat_map(|line| parse_metric_line(&line).unwrap_or_default())
    {
        let entry =
            summaries
                .entry(metric.series.clone())
                .or_insert_with(|| TrainingMetricSeriesSummary {
                    series: metric.series.clone(),
                    count: 0,
                    first: metric.clone(),
                    latest: metric.clone(),
                    minimum: metric.clone(),
                    maximum: metric.clone(),
                });
        entry.count = entry.count.saturating_add(1);
        entry.latest = metric.clone();
        if metric.value.total_cmp(&entry.minimum.value).is_lt() {
            entry.minimum = metric.clone();
        }
        if metric.value.total_cmp(&entry.maximum.value).is_gt() {
            entry.maximum = metric;
        }
    }
    Ok(Json(ApiSuccess {
        data: TrainingMetricsOverviewResponse {
            cursor,
            series: summaries.into_values().collect(),
        },
        meta: None,
    }))
}

#[derive(Debug, Serialize)]
struct TrainingArtifactsResponse {
    artifacts: Vec<TrainingArtifactResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrainingArtifactManifest {
    version: u8,
    task_id: String,
    output_root: PathBuf,
    output_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct TrainingCleanupPathResponse {
    kind: &'static str,
    path: String,
    file_count: u64,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct TrainingCleanupPreviewResponse {
    deletable: Vec<TrainingCleanupPathResponse>,
    retained: Vec<TrainingCleanupPathResponse>,
}

#[derive(Debug, Serialize)]
struct TrainingCleanupResponse {
    task_id: String,
    deleted: Vec<TrainingCleanupPathResponse>,
    retained: Vec<TrainingCleanupPathResponse>,
}

#[derive(Debug, Clone)]
struct TrainingCleanupPlan {
    deletable: Vec<(PathBuf, &'static str)>,
    retained: Vec<TrainingCleanupPathResponse>,
}

fn terminal_training_task(state: &AppState, id: &str) -> Result<TaskSnapshot, ApiError> {
    validate_training_task_id(id)?;
    let task = state
        .tasks
        .get(id)
        .map_err(map_task_manager_error)?
        .ok_or_else(|| ApiError::not_found("training_task_not_found", "训练任务不存在"))?;
    if task.kind != "training" {
        return Err(ApiError::not_found(
            "training_task_not_found",
            "训练任务不存在",
        ));
    }
    if !matches!(
        task.status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "training_task_not_terminal".to_string(),
            message: "只能删除已完成、失败或已取消的训练运行".to_string(),
            retryable: false,
            fields: None,
        });
    }
    Ok(task)
}

fn training_cleanup_path(
    path: &Path,
    kind: &'static str,
    reason: Option<String>,
) -> TrainingCleanupPathResponse {
    let (file_count, bytes) = training_directory_usage(path);
    TrainingCleanupPathResponse {
        kind,
        path: path.to_string_lossy().to_string(),
        file_count,
        bytes,
        reason,
    }
}

fn training_directory_usage(path: &Path) -> (u64, u64) {
    if !path.is_dir() {
        return (0, 0);
    }
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .fold((0_u64, 0_u64), |(count, bytes), entry| {
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            (count.saturating_add(1), bytes.saturating_add(size))
        })
}

fn task_output_root(task: &TaskSnapshot) -> Option<PathBuf> {
    task.payload
        .get("training")
        .and_then(|value| value.get("parameters"))
        .and_then(|value| value.get("output_dir"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn owned_training_output_directory(run_dir: &Path, task: &TaskSnapshot) -> Option<PathBuf> {
    let source = std::fs::read(run_dir.join("artifact-manifest.json")).ok()?;
    let manifest = serde_json::from_slice::<TrainingArtifactManifest>(&source).ok()?;
    if manifest.version != 1 || manifest.task_id != task.id {
        return None;
    }
    let requested_root = std::fs::canonicalize(task_output_root(task)?).ok()?;
    if manifest.output_root != requested_root {
        return None;
    }
    let expected = manifest.output_root.join(&task.id);
    if manifest.output_directory != expected
        || manifest.output_directory.file_name()?.to_str()? != task.id
    {
        return None;
    }
    let metadata = std::fs::symlink_metadata(&manifest.output_directory).ok()?;
    (!metadata.file_type().is_symlink() && metadata.is_dir()).then_some(manifest.output_directory)
}

fn training_cleanup_plan(state: &AppState, task: &TaskSnapshot) -> TrainingCleanupPlan {
    let run_dir = state.training_root.join("runs").join(&task.id);
    let mut deletable = Vec::new();
    let mut retained = Vec::new();
    match std::fs::symlink_metadata(&run_dir) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            deletable.push((run_dir.clone(), "run_data"));
        }
        Ok(_) => retained.push(training_cleanup_path(
            &run_dir,
            "unverified_run_data",
            Some("训练运行目录不是可安全删除的普通目录；已保留".to_string()),
        )),
        Err(_) => {}
    }
    if let Some(output_dir) = owned_training_output_directory(&run_dir, task) {
        deletable.push((output_dir, "owned_output"));
    } else if let Some(output_root) = task_output_root(task) {
        retained.push(training_cleanup_path(
            &output_root,
            "unverified_output",
            Some("该历史运行没有可验证的专属产物清单；共享输出目录已保留".to_string()),
        ));
    }
    TrainingCleanupPlan {
        deletable,
        retained,
    }
}

async fn training_cleanup_preview(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingCleanupPreviewResponse>>, ApiError> {
    let task = terminal_training_task(&state, &id)?;
    let plan = training_cleanup_plan(&state, &task);
    Ok(Json(ApiSuccess {
        data: TrainingCleanupPreviewResponse {
            deletable: plan
                .deletable
                .iter()
                .map(|(path, kind)| training_cleanup_path(path, kind, None))
                .collect(),
            retained: plan.retained,
        },
        meta: None,
    }))
}

async fn delete_training_task(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingCleanupResponse>>, ApiError> {
    let task = terminal_training_task(&state, &id)?;
    if state.active_workers.lock().await.contains(&id) {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "training_task_still_stopping".to_string(),
            message: "训练进程仍在收尾，请稍后再删除".to_string(),
            retryable: true,
            fields: None,
        });
    }
    let plan = training_cleanup_plan(&state, &task);
    let deletable = plan.deletable.clone();
    let deleted = tokio::task::spawn_blocking(
        move || -> Result<Vec<TrainingCleanupPathResponse>, String> {
            let mut removed = Vec::new();
            // The owned output is removed first. If it cannot be removed, the run
            // record remains available for a safe retry rather than being hidden.
            for (path, kind) in deletable.iter().rev() {
                let response = training_cleanup_path(path, kind, None);
                if path.exists() {
                    std::fs::remove_dir_all(path)
                        .map_err(|error| format!("无法删除 {}: {error}", path.display()))?;
                }
                removed.push(response);
            }
            removed.reverse();
            Ok(removed)
        },
    )
    .await
    .map_err(|error| ApiError::internal(format!("训练清理任务异常: {error}")))?
    .map_err(|message| ApiError::internal(message))?;
    state
        .tasks
        .delete_terminal(&id)
        .map_err(map_task_manager_error)?;
    Ok(Json(ApiSuccess {
        data: TrainingCleanupResponse {
            task_id: id,
            deleted,
            retained: plan.retained,
        },
        meta: None,
    }))
}

#[derive(Debug, Clone, Serialize)]
struct TrainingArtifactResponse {
    id: String,
    kind: String,
    name: String,
    path: String,
    size_bytes: u64,
    modified_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    step: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    url: String,
}

async fn training_artifacts(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TrainingArtifactsResponse>>, ApiError> {
    let artifacts = collect_training_artifacts(&state, &id)?;
    Ok(Json(ApiSuccess {
        data: TrainingArtifactsResponse { artifacts },
        meta: None,
    }))
}

async fn training_artifact_file(
    State(state): State<AppState>,
    AxumPath((id, artifact_id)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    validate_training_task_id(&id)?;
    if artifact_id.len() != 64 || !artifact_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "invalid_training_artifact",
            "训练产物标识无效",
        ));
    }
    let artifact = collect_training_artifact_files(&state, &id)?
        .into_iter()
        .find(|artifact| artifact.response.id == artifact_id)
        .ok_or_else(|| ApiError::not_found("training_artifact_not_found", "训练产物不存在"))?;
    let mut request = Request::builder().method(method).uri("/");
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let request = request
        .body(Body::empty())
        .map_err(|error| ApiError::internal(format!("无法创建训练产物请求: {error}")))?;
    let mut response = ServeFile::new(artifact.path)
        .oneshot(request)
        .await
        .map_err(|never| match never {})?
        .map(Body::new);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=60"),
    );
    Ok(response)
}

#[derive(Debug, Clone)]
struct TrainingArtifactFile {
    response: TrainingArtifactResponse,
    path: PathBuf,
}

/// Read only a safetensors header to surface its optional Kohya `ss_steps`
/// metadata.  Tensor payload is intentionally never mapped or decoded here.
fn safetensors_artifact_step(path: &Path) -> Option<u64> {
    const MAX_HEADER_BYTES: u64 = 16 * 1024 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let mut length = [0_u8; 8];
    file.read_exact(&mut length).ok()?;
    let header_length = u64::from_le_bytes(length);
    if header_length == 0 || header_length > MAX_HEADER_BYTES {
        return None;
    }
    let mut header = vec![0_u8; header_length as usize];
    file.read_exact(&mut header).ok()?;
    let metadata = serde_json::from_slice::<Value>(&header)
        .ok()?
        .get("__metadata__")?
        .get("ss_steps")?
        .clone();
    metadata
        .as_u64()
        .or_else(|| metadata.as_str()?.trim().parse::<u64>().ok())
}

/// Reads the prompt text accompanying a sample artifact.  kohya stores the
/// prompt file next to the generated samples (`sample_prompts.txt` /
/// `.toml` / `.json`), so the monitor can show the prompt that produced a
/// given sample image.
fn sample_prompt_text_for_artifact(sample_path: &Path) -> Option<String> {
    let directory = sample_path.parent()?;
    for name in ["sample_prompts.txt", "sample_prompts.toml", "sample_prompts.json"] {
        let path = directory.join(name);
        if path.is_file() {
            let text = std::fs::read_to_string(&path).ok()?;
            let trimmed = text.trim();
            return if trimmed.is_empty() {
                None
            } else {
                Some(text)
            };
        }
    }
    None
}

fn collect_training_artifacts(
    state: &AppState,
    task_id: &str,
) -> Result<Vec<TrainingArtifactResponse>, ApiError> {
    Ok(collect_training_artifact_files(state, task_id)?
        .into_iter()
        .map(|artifact| artifact.response)
        .collect())
}

fn collect_training_artifact_files(
    state: &AppState,
    task_id: &str,
) -> Result<Vec<TrainingArtifactFile>, ApiError> {
    validate_training_task_id(task_id)?;
    let task = state
        .tasks
        .get(task_id)
        .map_err(|error| ApiError::internal(format!("无法读取训练任务: {error}")))?
        .filter(|task| task.kind == "training")
        .ok_or_else(|| ApiError::not_found("training_task_not_found", "训练任务不存在"))?;
    let mut directories = vec![state.training_root.join("runs").join(task_id)];
    if let Some(output_dir) = task
        .payload
        .get("training")
        .and_then(|value| value.get("parameters"))
        .and_then(|value| value.get("output_dir"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    {
        directories.push(PathBuf::from(output_dir));
    }
    let mut seen = HashSet::new();
    let mut artifacts = Vec::new();
    for directory in directories {
        if !directory.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&directory)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if artifacts.len() >= 500
                || !entry.file_type().is_file()
                || entry.file_type().is_symlink()
            {
                continue;
            }
            let path = entry.path().to_path_buf();
            let kind = training_artifact_kind(&path);
            if !matches!(kind.as_str(), "sample" | "lora") {
                continue;
            }
            let canonical = match std::fs::canonicalize(&path) {
                Ok(path) => path,
                Err(_) => continue,
            };
            if !seen.insert(canonical.clone()) {
                continue;
            }
            let metadata = match std::fs::metadata(&canonical) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => continue,
            };
            let canonical_text = canonical.to_string_lossy().to_string();
            let id = hex::encode(Sha256::digest(
                format!("{task_id}\0{canonical_text}").as_bytes(),
            ));
            let step = (kind == "lora")
                .then(|| safetensors_artifact_step(&canonical))
                .flatten();
            let prompt = (kind == "sample")
                .then(|| sample_prompt_text_for_artifact(&canonical))
                .flatten();
            let response = TrainingArtifactResponse {
                id: id.clone(),
                kind,
                name: canonical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("artifact")
                    .to_string(),
                path: canonical_text,
                size_bytes: metadata.len(),
                modified_at: metadata
                    .modified()
                    .ok()
                    .and_then(|timestamp| timestamp.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default(),
                step,
                prompt,
                url: format!("/api/training/tasks/{task_id}/artifacts/{id}"),
            };
            artifacts.push(TrainingArtifactFile {
                response,
                path: canonical,
            });
        }
    }
    artifacts.sort_by(|left, right| {
        left.response
            .kind
            .cmp(&right.response.kind)
            .then_with(|| {
                if left.response.kind == "lora" {
                    left.response
                        .step
                        .unwrap_or(u64::MAX)
                        .cmp(&right.response.step.unwrap_or(u64::MAX))
                        .then_with(|| left.response.modified_at.cmp(&right.response.modified_at))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| left.response.name.cmp(&right.response.name))
    });
    Ok(artifacts)
}

fn training_artifact_kind(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".safetensors") {
        "lora".to_string()
    } else if name.ends_with(".ckpt") || name.ends_with(".pt") {
        "checkpoint".to_string()
    } else if matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("png" | "jpg" | "jpeg" | "webp")
    ) {
        "sample".to_string()
    } else if name.contains("state") {
        "state".to_string()
    } else if matches!(
        name.as_str(),
        "config.toml" | "request.json" | "runtime.json" | "dataset.toml"
    ) {
        "config".to_string()
    } else if name.ends_with(".log") {
        "log".to_string()
    } else if name.ends_with(".jsonl") {
        "metrics".to_string()
    } else {
        "other".to_string()
    }
}

#[cfg(test)]
fn downsample_training_metrics(
    metrics: Vec<crate::training::TrainingMetric>,
    max_points: usize,
) -> Vec<crate::training::TrainingMetric> {
    if metrics.len() <= max_points {
        return metrics;
    }
    let mut per_series = BTreeMap::<String, Vec<crate::training::TrainingMetric>>::new();
    for metric in metrics {
        per_series
            .entry(metric.series.clone())
            .or_default()
            .push(metric);
    }
    let per_series_limit = (max_points / per_series.len().max(1)).max(3);
    let mut result = Vec::new();
    for mut series in per_series.into_values() {
        series.sort_by_key(|metric| (metric.step, metric.timestamp));
        if series.len() <= per_series_limit {
            result.extend(series);
            continue;
        }
        let bucket_count = ((per_series_limit.saturating_sub(2)) / 2).max(1);
        let bucket_size = (series.len() + bucket_count - 1) / bucket_count;
        let mut selected = BTreeMap::<(u64, u64), crate::training::TrainingMetric>::new();
        selected.insert((series[0].step, series[0].timestamp), series[0].clone());
        selected.insert(
            (
                series[series.len() - 1].step,
                series[series.len() - 1].timestamp,
            ),
            series[series.len() - 1].clone(),
        );
        for bucket in series.chunks(bucket_size) {
            if let Some(low) = bucket
                .iter()
                .min_by(|left, right| left.value.total_cmp(&right.value))
            {
                selected.insert((low.step, low.timestamp), low.clone());
            }
            if let Some(high) = bucket
                .iter()
                .max_by(|left, right| left.value.total_cmp(&right.value))
            {
                selected.insert((high.step, high.timestamp), high.clone());
            }
        }
        result.extend(selected.into_values());
    }
    result.sort_by_key(|metric| (metric.timestamp, metric.step, metric.series.clone()));
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetricFileLine {
    cursor: u64,
    line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetricFileDelta {
    lines: Vec<MetricFileLine>,
    next_cursor: u64,
}

/// Converts an appended byte range from metrics.jsonl into complete SSE lines.
/// `cursor` is the byte position immediately before `source`; an unterminated
/// final line is deliberately retained for the next read instead of being
/// emitted or skipped.
fn metric_file_delta(source: &[u8], cursor: u64) -> MetricFileDelta {
    let mut lines = Vec::new();
    let mut line_start = 0_usize;
    let mut next_cursor = cursor;
    for (index, byte) in source.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line_end = if index > line_start && source[index - 1] == b'\r' {
            index - 1
        } else {
            index
        };
        if let Ok(line) = std::str::from_utf8(&source[line_start..line_end]) {
            if !line.trim().is_empty() {
                lines.push(MetricFileLine {
                    cursor: cursor.saturating_add(index as u64 + 1),
                    line: line.to_string(),
                });
            }
        }
        next_cursor = cursor.saturating_add(index as u64 + 1);
        line_start = index + 1;
    }
    MetricFileDelta { lines, next_cursor }
}

#[derive(Debug, Deserialize)]
struct TrainingMetricEventsQuery {
    after: Option<u64>,
}

async fn training_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TrainingMetricEventsQuery>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    validate_training_task_id(&id)?;
    let path = state
        .training_root
        .join("runs")
        .join(id)
        .join("metrics.jsonl");
    let requested_cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .or(query.after)
        .unwrap_or(0);
    let stream = async_stream::stream! {
        let mut offset = requested_cursor;
        loop {
            let length = std::fs::metadata(&path).map(|metadata| metadata.len()).unwrap_or(0);
            if length < offset {
                offset = 0;
            }
            if length > offset {
                let mut source = Vec::new();
                if let Ok(mut file) = std::fs::File::open(&path) {
                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        let _ = file.read_to_end(&mut source);
                    }
                }
                let delta = metric_file_delta(&source, offset);
                for line in delta.lines {
                    yield Ok::<Event, Infallible>(Event::default()
                        .id(line.cursor.to_string())
                        .event("metrics")
                        .data(line.line));
                }
                offset = delta.next_cursor;
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
    let health = service.health().await;
    if health.available {
        state.vllm_loads.clear();
    }
    Ok(Json(ApiSuccess {
        data: health,
        meta: None,
    }))
}

async fn vllm_load(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<VllmLoadResponse>>, ApiError> {
    let settings = state.settings.read().await.clone();
    let port = configured_local_vllm_port(&settings.vllm_base_url)
        .map_err(|message| ApiError::bad_request("invalid_vllm_launch_endpoint", message))?;
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
    if tokio::time::timeout(Duration::from_secs(1), service.health())
        .await
        .is_ok_and(|health| health.available)
    {
        state.vllm_loads.clear();
        return Ok(Json(ApiSuccess {
            data: VllmLoadResponse {
                state: "ready",
                message: "vLLM 模型已可用".to_string(),
            },
            meta: None,
        }));
    }
    if !state.vllm_loads.begin() {
        return Ok(Json(ApiSuccess {
            data: VllmLoadResponse {
                state: "loading",
                message: "vLLM 模型正在加载，请稍候".to_string(),
            },
            meta: None,
        }));
    }
    let Some(project_root) = state.vllm_launcher_root.as_deref() else {
        state.vllm_loads.clear();
        return Err(ApiError::internal("找不到随应用提供的 vLLM 启动脚本"));
    };
    if let Err(error) = launch_vllm_process(project_root, port) {
        state.vllm_loads.clear();
        return Err(ApiError::internal(error));
    }
    Ok(Json(ApiSuccess {
        data: VllmLoadResponse {
            state: "started",
            message: "已开始加载 vLLM 模型；首次加载可能需要数分钟，可在此页查看状态。".to_string(),
        },
        meta: None,
    }))
}

async fn vllm_unload(
    State(state): State<AppState>,
) -> Result<Json<ApiSuccess<VllmUnloadResponse>>, ApiError> {
    let settings = state.settings.read().await.clone();
    configured_local_vllm_port(&settings.vllm_base_url)
        .map_err(|message| ApiError::bad_request("invalid_vllm_launch_endpoint", message))?;
    let Some(project_root) = state.vllm_launcher_root.as_deref() else {
        return Err(ApiError::internal("找不到随应用提供的 vLLM 卸载脚本"));
    };
    let unload_state = unload_vllm_process(project_root).map_err(ApiError::internal)?;
    state.vllm_loads.clear();
    let message = if unload_state == "stopped" {
        "vLLM 模型已卸载，显存已释放。".to_string()
    } else {
        "没有发现由本应用启动的 vLLM 模型。外部 vLLM 服务不会被停止。".to_string()
    };
    Ok(Json(ApiSuccess {
        data: VllmUnloadResponse {
            state: unload_state,
            message,
        },
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
    page: Option<usize>,
    score_min: Option<i64>,
    score_max: Option<i64>,
    min_resolution: Option<i64>,
    resolution_min: Option<i64>,
    resolution_max: Option<i64>,
    directory: Option<String>,
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
    page: usize,
    total_pages: usize,
    score_ranges: Vec<LibraryScoreRange>,
    resolution_ranges: Vec<LibraryResolutionRange>,
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
    let directory = query
        .directory
        .as_deref()
        .map(normalize_task_relative_directory)
        .transpose()?;
    if query
        .page
        .is_some_and(|page| !(1..=1_000_000).contains(&page))
    {
        return Err(ApiError::bad_request(
            "invalid_page_number",
            "页码必须在 1..=1000000",
        ));
    }
    if let (Some(score_min), Some(score_max)) = (query.score_min, query.score_max) {
        if score_min > score_max {
            return Err(ApiError::bad_request(
                "invalid_score_range",
                "评分区间的下限不能大于上限",
            ));
        }
    }
    let resolution_min = query.resolution_min.or(query.min_resolution);
    if resolution_min.is_some_and(|resolution| !(0..=1_000_000).contains(&resolution))
        || query
            .resolution_max
            .is_some_and(|resolution| !(0..=1_000_000).contains(&resolution))
    {
        return Err(ApiError::bad_request(
            "invalid_resolution_range",
            "分辨率区间必须在 0..=1000000",
        ));
    }
    if resolution_min
        .zip(query.resolution_max)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(ApiError::bad_request(
            "invalid_resolution_range",
            "分辨率区间的下限不能大于上限",
        ));
    }
    state
        .database
        .get_root(&query.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .ok_or_else(|| ApiError::not_found("root_not_found", "媒体根不存在"))?;
    let filters = LibraryMediaFilters {
        score_min: query.score_min,
        score_max: query.score_max,
        min_resolution: resolution_min,
        max_resolution: query.resolution_max,
        relative_directory: directory,
    };
    let numbered_page = state
        .database
        .list_library_media_by_page(
            &query.root_id,
            query.page.unwrap_or(1),
            query.limit,
            &query.query,
            &filters,
        )
        .map_err(|error| ApiError::internal(format!("无法读取图库: {error}")))?;
    let mut media_items = numbered_page.items;
    let next_cursor = if query.page.is_none()
        && query.score_min.is_none()
        && query.score_max.is_none()
        && query.min_resolution.is_none()
        && query.resolution_min.is_none()
        && query.resolution_max.is_none()
        && query.directory.is_none()
    {
        let legacy_page = state
            .database
            .list_library_media(
                &query.root_id,
                query.cursor.as_deref(),
                query.limit,
                &query.query,
            )
            .map_err(|error| ApiError::internal(format!("无法读取图库: {error}")))?;
        media_items = legacy_page.items;
        legacy_page.next_cursor
    } else {
        None
    };
    let items = media_items
        .into_iter()
        .map(|media| local_media_response(&state.database, media))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ApiSuccess {
        data: LibraryPageResponse {
            items,
            next_cursor,
            total: numbered_page.total,
            page: numbered_page.page,
            total_pages: numbered_page.total_pages,
            score_ranges: numbered_page.score_ranges,
            resolution_ranges: numbered_page.resolution_ranges,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    training: Option<TrainingTaskSummaryResponse>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct TrainingTaskSummaryResponse {
    adapter_id: String,
    runtime_profile_id: String,
    gpu_ids: Vec<String>,
    model_path: Option<String>,
    train_data_dir: Option<String>,
    output_dir: Option<String>,
    output_name: Option<String>,
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
    let has_item_records = counts.total > 0;
    let fallback_count = |key: &str| {
        task.result
            .as_ref()
            .and_then(|result| result.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let item_counts = TaskItemCountsResponse {
        total: if has_item_records {
            counts.total
        } else {
            task.total_items.unwrap_or(0)
        },
        queued: counts.queued,
        completed: if has_item_records {
            counts.completed
        } else {
            task.completed_items
        },
        skipped: if has_item_records {
            counts.skipped
        } else {
            fallback_count("skipped")
        },
        failed: if has_item_records {
            counts.failed
        } else {
            fallback_count("failed")
                .max(u64::from(task.error.is_some()))
        },
        retryable_failed: counts.retryable_failed,
        completed_bytes: counts.completed_bytes,
    };
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
            item_counts,
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
        "exact_dedup" | "near_dedup" | "integrity_scan" | "delete_by_tag" | "delete_selected" => {
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
        "dataset_augmentation" => {
            for key in [
                "source_images",
                "generated",
                "rejected",
                "retagging_pending",
                "retagged",
            ] {
                copy_count(key);
            }
            for key in [
                "output_relative_directory",
                "derived_relative_directory",
                "metadata_relative_directory",
                "training_relative_directory",
                "next_step",
            ] {
                if let Some(value) = object.get(key).and_then(Value::as_str) {
                    safe.insert(
                        key.to_string(),
                        Value::String(value.chars().take(4_096).collect()),
                    );
                }
            }
            for key in ["variant_counts", "rejection_reasons"] {
                if let Some(value) = sanitized_count_map(object.get(key), 8) {
                    safe.insert(key.to_string(), value);
                }
            }
            if let Some(smart_crop) = object.get("smart_crop").and_then(Value::as_object) {
                let mut crop = serde_json::Map::new();
                if let Some(enabled) = smart_crop.get("enabled").and_then(Value::as_bool) {
                    crop.insert("enabled".to_string(), Value::Bool(enabled));
                }
                for key in ["generated", "rejected"] {
                    if let Some(value) =
                        smart_crop.get(key).filter(|value| value.as_u64().is_some())
                    {
                        crop.insert(key.to_string(), value.clone());
                    }
                }
                if let Some(coverage) = smart_crop
                    .get("coverage_percent")
                    .and_then(Value::as_object)
                {
                    let mut safe_coverage = serde_json::Map::new();
                    for (variant, value) in coverage.iter().take(4) {
                        let Some(average) = value.get("average").filter(|value| value.is_number())
                        else {
                            continue;
                        };
                        safe_coverage.insert(
                            variant.chars().take(64).collect(),
                            serde_json::json!({ "average": average }),
                        );
                    }
                    if !safe_coverage.is_empty() {
                        crop.insert("coverage_percent".to_string(), Value::Object(safe_coverage));
                    }
                }
                if !crop.is_empty() {
                    safe.insert("smart_crop".to_string(), Value::Object(crop));
                }
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

fn sanitized_count_map(value: Option<&Value>, limit: usize) -> Option<Value> {
    let values = value?.as_object()?;
    let mut safe = serde_json::Map::new();
    for (key, value) in values.iter().take(limit) {
        if value.as_u64().is_some() {
            safe.insert(key.chars().take(512).collect(), value.clone());
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    training: Option<TrainingRequest>,
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
    expand_library_query_selection(&state, &mut request)?;
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
        "delete_selected",
        "tag_pipeline",
        "vllm_tag",
        "dataset_augmentation",
        "training",
    ];
    if !KINDS.contains(&request.kind.as_str()) {
        return Err(ApiError::bad_request("invalid_task_type", "未知任务类型"));
    }
    if request.kind == "training" {
        if request.root_id != "__training__" {
            return Err(ApiError::bad_request(
                "invalid_training_root",
                "训练任务不使用媒体根",
            ));
        }
        if request.source.is_some()
            || request.options.is_some()
            || request.relative_directory.is_some()
        {
            return Err(ApiError::bad_request(
                "invalid_training_fields",
                "训练任务包含媒体工具字段",
            ));
        }
        let training = request
            .training
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("missing_training_request", "缺少训练配置"))?;
        training
            .validate()
            .map_err(|message| ApiError::bad_request("invalid_training_request", message))?;
        let gallery_datasets = training.gallery_datasets();
        if !gallery_datasets.is_empty() {
            if training
                .parameters
                .get("dataset_config")
                .and_then(Value::as_str)
                .is_some_and(|path| !path.trim().is_empty())
            {
                return Err(ApiError::bad_request(
                    "gallery_dataset_config_conflict",
                    "使用图库数据源时由系统生成 dataset TOML，请清空手填数据集配置",
                ));
            }
            let mut caption_extension = None;
            for dataset in gallery_datasets {
                let inspection = inspect_training_gallery_dataset(state, dataset)?;
                if inspection.image_count == 0 {
                    return Err(ApiError::bad_request(
                        "empty_gallery_dataset",
                        "所选图库目录中没有可训练图片",
                    ));
                }
                if inspection.caption_count != inspection.image_count {
                    return Err(ApiError::bad_request(
                        "gallery_dataset_captions_incomplete",
                        format!(
                            "图库子集 {} 有 {} 张图片，但只有 {} 个 Caption；请先完成重新打标",
                            inspection.relative_directory,
                            inspection.image_count,
                            inspection.caption_count,
                        ),
                    ));
                }
                if let Some(previous) = caption_extension.replace(inspection.caption_extension) {
                    if caption_extension.as_deref() != Some(previous.as_str()) {
                        return Err(ApiError::bad_request(
                            "gallery_caption_extension_mismatch",
                            "多个图库子集必须使用相同的 Caption 扩展名",
                        ));
                    }
                }
            }
        }
        resolve_training_runtime_profile(&state.training_root, &training.runtime_profile_id)
            .map_err(|message| ApiError::bad_request("invalid_training_runtime", message))?;
        return Ok(());
    }
    if request.training.is_some() {
        return Err(ApiError::bad_request(
            "invalid_training_fields",
            "非训练任务不能包含训练配置",
        ));
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
            "resize"
                | "heic_convert"
                | "tag_pipeline"
                | "vllm_tag"
                | "dataset_augmentation"
                | "delete_selected"
        ) {
            let media_ids = validated_task_media_ids(request.options.as_ref())?;
            if matches!(
                request.kind.as_str(),
                "heic_convert" | "tag_pipeline" | "dataset_augmentation"
            ) {
                let options = request
                    .options
                    .as_ref()
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        ApiError::bad_request("invalid_task_options", "工具 options 必须是对象")
                    })?;
                let valid_keys: &[&str] = if request.kind == "tag_pipeline" {
                    &["media_ids", "artist_prefix"]
                } else if request.kind == "dataset_augmentation" {
                    &[
                        "media_ids",
                        "output_directory",
                        "augmentation_source_directory",
                        "min_megapixels",
                        "min_long_side",
                        "min_short_side",
                        "horizontal_flip",
                        "train_percent",
                        "validation_percent",
                        "test_percent",
                        "jpeg_quality",
                        "smart_crop",
                        "retagging",
                    ]
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
                if request.kind == "dataset_augmentation" {
                    parse_dataset_augmentation_config(request.options.as_ref())?;
                }
            }
            let mut unsupported_vllm_media = Vec::new();
            let mut unsupported_augmentation_media = Vec::new();
            let augmentation_config = (request.kind == "dataset_augmentation")
                .then(|| parse_dataset_augmentation_config(request.options.as_ref()))
                .transpose()?;
            let augmentation_output_directory = (request.kind == "dataset_augmentation")
                .then(|| dataset_augmentation_output_directory(request.options.as_ref()))
                .transpose()?;
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
                    unsupported_vllm_media.push(media_id.clone());
                }
                if augmentation_config.is_some() {
                    if !is_supported_dataset_augmentation_media(
                        &media.relative_path,
                        &media.mime_type,
                    ) {
                        unsupported_augmentation_media.push(media_id.clone());
                    }
                    let media_path = media.relative_path.replace('\\', "/");
                    let output_directory = augmentation_output_directory
                        .as_ref()
                        .expect("dataset augmentation output directory must be resolved");
                    let output_prefix =
                        format!("{}/", output_directory.to_string_lossy().replace('\\', "/"));
                    if is_augmentation_derived_path(&media.relative_path)
                        || media_path == output_directory.to_string_lossy().replace('\\', "/")
                        || media_path.starts_with(&output_prefix)
                    {
                        return Err(ApiError::bad_request(
                            "dataset_output_source_overlap",
                            "派生增广图片不能再次作为增广输入；请改选原始数据目录",
                        ));
                    }
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
            if !unsupported_augmentation_media.is_empty() {
                return Err(ApiError {
                    status: StatusCode::BAD_REQUEST,
                    code: "unsupported_dataset_augmentation_media".to_string(),
                    message: "数据集增广仅支持 PNG、JPEG、WebP 和 BMP 静态图片".to_string(),
                    retryable: false,
                    fields: Some(serde_json::json!({
                        "media_ids": {
                            "code": "unsupported_media_type",
                            "invalid_ids": unsupported_augmentation_media,
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
    if ids.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_media_ids",
            "media_ids 不能为空",
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

fn optional_library_selection_i64(
    options: &serde_json::Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<Option<i64>, ApiError> {
    options
        .get(key)
        .map(|value| {
            value.as_i64().ok_or_else(|| {
                ApiError::bad_request(
                    "invalid_library_selection_filter",
                    format!("{label} 必须是整数"),
                )
            })
        })
        .transpose()
}

fn expand_library_query_selection(
    state: &AppState,
    request: &mut CreateTaskRequest,
) -> Result<(), ApiError> {
    if !matches!(
        request.kind.as_str(),
        "resize"
            | "heic_convert"
            | "tag_pipeline"
            | "vllm_tag"
            | "dataset_augmentation"
            | "delete_selected"
    ) {
        return Ok(());
    }
    let Some(options) = request.options.as_mut().and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let Some(query_value) = options.get("library_query").cloned() else {
        return Ok(());
    };
    if options.contains_key("media_ids") || options.contains_key("relative_directory") {
        return Err(ApiError::bad_request(
            "ambiguous_media_selection",
            "library_query 不能与 media_ids 或 relative_directory 同时使用",
        ));
    }
    let query = query_value
        .as_str()
        .ok_or_else(|| {
            ApiError::bad_request("invalid_library_query", "library_query 必须是字符串")
        })?
        .trim();
    if query.len() > 4_096 {
        return Err(ApiError::bad_request(
            "invalid_library_query",
            "library_query 最长为 4096 字节",
        ));
    }
    let resolution_min =
        optional_library_selection_i64(options, "library_resolution_min", "分辨率下限")?.or(
            optional_library_selection_i64(options, "library_min_resolution", "最低分辨率")?,
        );
    let filters = LibraryMediaFilters {
        score_min: optional_library_selection_i64(options, "library_score_min", "评分下限")?,
        score_max: optional_library_selection_i64(options, "library_score_max", "评分上限")?,
        min_resolution: resolution_min,
        max_resolution: optional_library_selection_i64(
            options,
            "library_resolution_max",
            "分辨率上限",
        )?,
        relative_directory: None,
    };
    if filters
        .score_min
        .zip(filters.score_max)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(ApiError::bad_request(
            "invalid_library_selection_filter",
            "评分区间的下限不能大于上限",
        ));
    }
    if filters
        .min_resolution
        .zip(filters.max_resolution)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(ApiError::bad_request(
            "invalid_library_selection_filter",
            "分辨率区间的下限不能大于上限",
        ));
    }
    if filters
        .min_resolution
        .is_some_and(|resolution| !(0..=1_000_000).contains(&resolution))
        || filters
            .max_resolution
            .is_some_and(|resolution| !(0..=1_000_000).contains(&resolution))
    {
        return Err(ApiError::bad_request(
            "invalid_library_selection_filter",
            "分辨率区间必须在 0..=1000000",
        ));
    }
    let excluded = match options.get("excluded_media_ids") {
        None => HashSet::new(),
        Some(Value::Array(ids)) => {
            let mut excluded = HashSet::with_capacity(ids.len());
            for id in ids {
                let id = id
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 512)
                    .ok_or_else(|| {
                        ApiError::bad_request(
                            "invalid_excluded_media_ids",
                            "排除的 media ID 格式无效",
                        )
                    })?;
                excluded.insert(id.to_string());
            }
            excluded
        }
        Some(_) => {
            return Err(ApiError::bad_request(
                "invalid_excluded_media_ids",
                "excluded_media_ids 必须是数组",
            ));
        }
    };
    if state
        .database
        .get_root(&request.root_id)
        .map_err(|error| ApiError::internal(error.to_string()))?
        .is_none()
    {
        return Err(ApiError::not_found("root_not_found", "媒体根不存在"));
    }

    let mut media_ids = Vec::new();
    let mut cursor = None;
    loop {
        let page = state
            .database
            .list_library_media_filtered(&request.root_id, cursor.as_deref(), 200, query, &filters)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        for media in page.items {
            let supported = match request.kind.as_str() {
                "vllm_tag" => is_supported_vllm_media(&media.relative_path, &media.mime_type),
                "heic_convert" => is_supported_heic_media(&media.relative_path, &media.mime_type),
                "resize" => media.mime_type.starts_with("image/"),
                "dataset_augmentation" => {
                    is_supported_dataset_augmentation_media(&media.relative_path, &media.mime_type)
                }
                _ => true,
            };
            if supported && !excluded.contains(&media.id) {
                media_ids.push(media.id);
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    if media_ids.is_empty() {
        return Err(ApiError::bad_request(
            "empty_library_selection",
            "搜索结果内没有适用于该工具的已索引媒体",
        ));
    }
    options.remove("library_query");
    options.remove("excluded_media_ids");
    options.remove("library_score_min");
    options.remove("library_score_max");
    options.remove("library_min_resolution");
    options.remove("library_resolution_min");
    options.remove("library_resolution_max");
    options.insert(
        "media_ids".to_string(),
        Value::Array(media_ids.into_iter().map(Value::String).collect()),
    );
    Ok(())
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
        "dataset_augmentation" => {
            is_supported_dataset_augmentation_media(&item.relative_path, &item.mime_type)
                && !is_augmentation_derived_path(&item.relative_path)
        }
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
    if request.kind == "dataset_augmentation" {
        options.insert(
            "augmentation_source_directory".to_string(),
            Value::String(directory),
        );
    }
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetAugmentationOptions {
    media_ids: Vec<String>,
    #[serde(default, rename = "output_directory")]
    _output_directory: Option<String>,
    #[serde(default)]
    augmentation_source_directory: Option<String>,
    #[serde(default)]
    min_megapixels: Option<f64>,
    #[serde(default)]
    min_long_side: Option<u32>,
    #[serde(default)]
    min_short_side: Option<u32>,
    #[serde(default)]
    horizontal_flip: Option<bool>,
    #[serde(default)]
    train_percent: Option<u8>,
    #[serde(default)]
    validation_percent: Option<u8>,
    #[serde(default)]
    test_percent: Option<u8>,
    #[serde(default)]
    jpeg_quality: Option<u8>,
    #[serde(default)]
    smart_crop: Option<SmartCropOptions>,
    #[serde(default)]
    retagging: Option<DerivedRetaggingOptions>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SmartCropOptions {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    runtime_profile_id: Option<String>,
    #[serde(default)]
    gpu_id: Option<String>,
    #[serde(default)]
    quality_profile: Option<String>,
    #[serde(default)]
    portrait: Option<bool>,
    #[serde(default)]
    upper_body: Option<bool>,
    #[serde(default)]
    full_body_tight: Option<bool>,
    #[serde(default)]
    max_derived_per_family: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedRetaggingOptions {
    #[serde(default)]
    send_to_vllm: Option<bool>,
    #[serde(default)]
    preserve_artist_character_tags: Option<bool>,
}

fn parse_dataset_augmentation_config(
    options: Option<&Value>,
) -> Result<DatasetAugmentationConfig, ApiError> {
    let options = options.ok_or_else(|| {
        ApiError::bad_request("missing_dataset_augmentation_options", "缺少数据集增广选项")
    })?;
    let input: DatasetAugmentationOptions =
        serde_json::from_value(options.clone()).map_err(|_| {
            ApiError::bad_request(
                "invalid_dataset_augmentation_options",
                "数据集增广选项格式无效",
            )
        })?;
    if input.media_ids.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_media_ids",
            "media_ids 不能为空",
        ));
    }
    let mut config = DatasetAugmentationConfig::default();
    if let Some(value) = input.min_megapixels {
        config.min_megapixels = value;
    }
    if let Some(value) = input.min_long_side {
        config.min_long_side = value;
    }
    if let Some(value) = input.min_short_side {
        config.min_short_side = value;
    }
    if let Some(value) = input.horizontal_flip {
        config.horizontal_flip = value;
    }
    if let Some(value) = input.train_percent {
        config.train_percent = value;
    }
    if let Some(value) = input.validation_percent {
        config.validation_percent = value;
    }
    if let Some(value) = input.test_percent {
        config.test_percent = value;
    }
    if let Some(value) = input.jpeg_quality {
        config.jpeg_quality = value;
    }
    if let Some(smart_crop) = input.smart_crop {
        if let Some(value) = smart_crop.enabled {
            config.smart_crop.enabled = value;
        }
        if let Some(value) = smart_crop.runtime_profile_id {
            config.smart_crop.runtime_profile_id = value;
        }
        if let Some(value) = smart_crop.gpu_id {
            config.smart_crop.gpu_id = value;
        }
        if let Some(value) = smart_crop.quality_profile {
            config.smart_crop.quality_profile = value;
        }
        if let Some(value) = smart_crop.portrait {
            config.smart_crop.portrait = value;
        }
        if let Some(value) = smart_crop.upper_body {
            config.smart_crop.upper_body = value;
        }
        if let Some(value) = smart_crop.full_body_tight {
            config.smart_crop.full_body_tight = value;
        }
        if let Some(value) = smart_crop.max_derived_per_family {
            config.smart_crop.max_derived_per_family = value;
        }
    }
    if let Some(retagging) = input.retagging {
        if let Some(value) = retagging.send_to_vllm {
            config.retagging.send_to_vllm = value;
        }
        if let Some(value) = retagging.preserve_artist_character_tags {
            config.retagging.preserve_artist_character_tags = value;
        }
    }
    config.validate().map_err(|message| {
        ApiError::bad_request("invalid_dataset_augmentation_options", message)
    })?;
    Ok(config)
}

fn dataset_augmentation_source_directory(options: Option<&Value>) -> Result<PathBuf, ApiError> {
    let options = options.ok_or_else(|| {
        ApiError::bad_request("missing_dataset_augmentation_options", "缺少数据集增广选项")
    })?;
    let input: DatasetAugmentationOptions =
        serde_json::from_value(options.clone()).map_err(|_| {
            ApiError::bad_request(
                "invalid_dataset_augmentation_options",
                "数据集增广选项格式无效",
            )
        })?;
    input
        .augmentation_source_directory
        .as_deref()
        .map(normalize_task_relative_directory)
        .transpose()
        .map(|directory| directory.map(PathBuf::from).unwrap_or_default())
}

fn dataset_augmentation_output_directory(options: Option<&Value>) -> Result<PathBuf, ApiError> {
    Ok(dataset_augmentation_source_directory(options)?.join(".augmentation"))
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

fn is_supported_dataset_augmentation_media(relative_path: &str, mime_type: &str) -> bool {
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
        _ => false,
    }
}

fn is_augmentation_derived_path(relative_path: &str) -> bool {
    Path::new(relative_path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| component == ".augmentation")
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

async fn task_delete(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApiSuccess<TaskSummaryResponse>>, ApiError> {
    let task = state
        .tasks
        .delete_terminal(&id)
        .map_err(map_task_manager_error)?;
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
    let gpu_lease = state
        .tasks
        .get(&task_id)
        .ok()
        .flatten()
        .and_then(task_gpu_lease_request);
    if let Some((profile, gpu_ids)) = gpu_lease.as_ref() {
        state
            .training_leases
            .register_waiting(&task_id, profile, gpu_ids);
    }
    tokio::spawn(async move {
        loop {
            if let Some((profile, gpu_ids)) = gpu_lease.as_ref() {
                if !state
                    .training_leases
                    .try_acquire(&task_id, profile, gpu_ids)
                {
                    let blockers = state.training_leases.blockers(profile, gpu_ids);
                    tracing::debug!(task_id = %task_id, profile, ?gpu_ids, ?blockers, "GPU 任务正在等待已占用的 GPU");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            }
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
                    "exact_dedup" | "integrity_scan" | "near_dedup" | "delete_by_tag"
                    | "delete_selected" => run_tool_task(&state, &task).await,
                    "vllm_tag" => run_vllm_task(&state, &task).await,
                    "resize" => run_resize_task(&state, &task).await,
                    "heic_convert" => run_heic_task(&state, &task).await,
                    "tag_pipeline" => run_tag_pipeline_task(&state, &task).await,
                    "dataset_augmentation" => run_dataset_augmentation_task(&state, &task).await,
                    "training" => run_training_task(&state, &task).await,
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

            match state.tasks.get(&task_id) {
                Ok(Some(task))
                    if task.kind == "download"
                        && matches!(
                            task.status,
                            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                        ) =>
                {
                    match cleanup_terminal_download_part_files(&state, &task).await {
                        Ok(removed) if removed > 0 => {
                            tracing::info!(task_id = %task_id, removed, "已清理下载结束后遗留的临时文件");
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(task_id = %task_id, code = %error.code, message = %error.message, "下载结束后无法完整清理临时文件");
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(task_id = %task_id, %error, "无法读取下载任务的临时文件清理状态");
                }
            }

            drop(worker_slot);
            if gpu_lease.is_some() {
                state.training_leases.release(&task_id);
            }
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

fn task_gpu_lease_request(task: TaskSnapshot) -> Option<(String, Vec<String>)> {
    let request = serde_json::from_value::<CreateTaskRequest>(task.payload).ok()?;
    if task.kind == "training" {
        return request.training.map(|training| {
            let gpu_ids = if training.gpu_ids.is_empty() {
                detected_training_gpu_ids()
            } else {
                training.gpu_ids
            };
            (training.runtime_profile_id, gpu_ids)
        });
    }
    if task.kind == "dataset_augmentation" {
        let config = parse_dataset_augmentation_config(request.options.as_ref()).ok()?;
        if config.smart_crop.enabled {
            return Some((
                config.smart_crop.runtime_profile_id,
                vec![config.smart_crop.gpu_id],
            ));
        }
    }
    None
}

fn detected_training_gpu_ids() -> Vec<String> {
    let mut ids = training_gpu_inventory()
        .into_iter()
        .map(|gpu| gpu.id)
        .take(1)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        ids.push("0".to_string());
    }
    ids
}

fn training_gpu_inventory() -> Vec<TrainingGpuResponse> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,uuid,name,memory.total,memory.used,utilization.gpu,clocks.current.graphics,clocks.current.memory,power.draw,power.limit,temperature.gpu,fan.speed",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let mut gpus = output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_training_gpu_inventory(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    let processes = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=gpu_uuid,pid,process_name,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_training_gpu_processes(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    for gpu in &mut gpus {
        gpu.external_processes = processes.get(&gpu.uuid).cloned().unwrap_or_default();
    }
    gpus
}

fn parse_training_gpu_inventory(source: &str) -> Vec<TrainingGpuResponse> {
    let mut gpus = source
        .lines()
        .filter_map(|line| {
            let values = line
                .split(',')
                .map(|value| value.trim().trim_matches('"'))
                .collect::<Vec<_>>();
            let (id, uuid, name, telemetry) = match values.as_slice() {
                [id, uuid, name, telemetry @ ..] if telemetry.len() == 9 => (*id, *uuid, *name, telemetry),
                [id, name, telemetry @ ..] if telemetry.len() == 9 => (*id, *id, *name, telemetry),
                _ => return None,
            };
            let [
                memory_total_mib,
                memory_used_mib,
                utilization_percent,
                graphics_clock_mhz,
                memory_clock_mhz,
                power_draw_w,
                power_limit_w,
                temperature_c,
                fan_speed_percent,
            ] = telemetry
            else {
                return None;
            };
            if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            Some(TrainingGpuResponse {
                uuid: uuid.to_string(),
                id: (*id).to_string(),
                name: (*name).to_string(),
                memory_total_mib: memory_total_mib.parse().unwrap_or_default(),
                memory_used_mib: memory_used_mib.parse().unwrap_or_default(),
                utilization_percent: utilization_percent.parse().unwrap_or_default(),
                graphics_clock_mhz: graphics_clock_mhz.parse().ok(),
                memory_clock_mhz: memory_clock_mhz.parse().ok(),
                power_draw_w: power_draw_w.parse().ok(),
                power_limit_w: power_limit_w.parse().ok(),
                temperature_c: temperature_c.parse().ok(),
                fan_speed_percent: fan_speed_percent.parse().ok(),
                external_processes: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    gpus.sort_by(|left, right| left.id.cmp(&right.id));
    gpus.dedup_by(|left, right| left.id == right.id);
    gpus
}

fn parse_training_gpu_processes(
    source: &str,
) -> HashMap<String, Vec<TrainingGpuExternalProcessResponse>> {
    let mut processes = HashMap::<String, Vec<TrainingGpuExternalProcessResponse>>::new();
    for line in source.lines() {
        let values = line
            .split(',')
            .map(|value| value.trim().trim_matches('"'))
            .collect::<Vec<_>>();
        let [uuid, pid, process_name, memory_used_mib] = values.as_slice() else {
            continue;
        };
        let Ok(pid) = pid.parse() else { continue };
        let Ok(memory_used_mib) = memory_used_mib.parse() else {
            continue;
        };
        if uuid.is_empty() || process_name.is_empty() {
            continue;
        }
        processes.entry((*uuid).to_string()).or_default().push(
            TrainingGpuExternalProcessResponse {
                pid,
                process_name: (*process_name).to_string(),
                memory_used_mib,
            },
        );
    }
    for values in processes.values_mut() {
        values.sort_by(|left, right| right.memory_used_mib.cmp(&left.memory_used_mib));
    }
    processes
}

#[cfg(test)]
mod training_gpu_inventory_tests {
    use super::parse_training_gpu_inventory;

    #[test]
    fn parses_extended_nvidia_smi_telemetry_without_losing_basic_inventory_fields() {
        let gpus = parse_training_gpu_inventory(
            "0, NVIDIA GeForce RTX 5090, 32607, 12584, 72, 2840, 14001, 356.4, 575.0, 58, 42\n",
        );

        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.id, "0");
        assert_eq!(gpu.memory_total_mib, 32607);
        assert_eq!(gpu.utilization_percent, 72);
        assert_eq!(gpu.graphics_clock_mhz, Some(2840));
        assert_eq!(gpu.memory_clock_mhz, Some(14001));
        assert_eq!(gpu.power_draw_w, Some(356.4));
        assert_eq!(gpu.power_limit_w, Some(575.0));
        assert_eq!(gpu.temperature_c, Some(58));
        assert_eq!(gpu.fan_speed_percent, Some(42));
    }
}

fn windows_path_for_wsl(path: &Path, distro: Option<&str>) -> Result<String, String> {
    let mut command = Command::new("wsl.exe");
    if let Some(distro) = distro.filter(|value| !value.trim().is_empty()) {
        command.args(["--distribution", distro]);
    }
    let output = command
        .args(["--exec", "wslpath", "-a"])
        .arg(path)
        .output()
        .map_err(|error| format!("无法调用 WSL 路径转换: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "WSL 路径转换失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let converted = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if converted.is_empty() {
        Err("WSL 路径转换返回空值".to_string())
    } else {
        Ok(converted)
    }
}

fn prepare_training_logging_dir(parameters: &mut Value) -> Result<Option<PathBuf>, String> {
    let parameters = parameters
        .as_object_mut()
        .ok_or_else(|| "训练参数必须是对象".to_string())?;
    let log_with = parameters
        .get("log_with")
        .and_then(Value::as_str)
        .unwrap_or("tensorboard");
    if !matches!(log_with, "tensorboard" | "all") {
        return Ok(None);
    }
    if let Some(logging_dir) = parameters
        .get("logging_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    {
        return Ok(Some(PathBuf::from(logging_dir)));
    }
    let output_dir = parameters
        .get("output_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "启用 TensorBoard 时必须设置 LoRA 输出目录".to_string())?;
    let logging_dir = PathBuf::from(output_dir).join("logs");
    parameters.insert(
        "logging_dir".to_string(),
        Value::String(logging_dir.to_string_lossy().to_string()),
    );
    Ok(Some(logging_dir))
}

/// Converts the user-selected output root into a per-task directory. Keeping
/// every run in a UUID-named child directory gives cleanup an unambiguous
/// ownership boundary even when users reuse the same output root.
fn prepare_owned_training_output_dir(
    run_dir: &Path,
    task_id: &str,
    parameters: &mut Value,
) -> Result<PathBuf, String> {
    let output_root = parameters
        .get("output_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "必须设置 LoRA 输出根目录".to_string())?;
    std::fs::create_dir_all(&output_root)
        .map_err(|error| format!("无法创建 LoRA 输出根目录: {error}"))?;
    let output_root = std::fs::canonicalize(&output_root)
        .map_err(|error| format!("无法解析 LoRA 输出根目录: {error}"))?;
    if !output_root.is_dir() {
        return Err("LoRA 输出根目录不是目录".to_string());
    }
    let output_directory = output_root.join(task_id);
    match std::fs::symlink_metadata(&output_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("训练输出目录不能是符号链接".to_string());
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err("训练输出路径已被普通文件占用".to_string());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&output_directory)
                .map_err(|error| format!("无法创建训练专属输出目录: {error}"))?;
        }
        Err(error) => return Err(format!("无法检查训练输出目录: {error}")),
    }
    let manifest = TrainingArtifactManifest {
        version: 1,
        task_id: task_id.to_string(),
        output_root,
        output_directory: output_directory.clone(),
    };
    std::fs::write(
        run_dir.join("artifact-manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|error| format!("无法写入训练产物清单: {error}"))?;
    let parameters = parameters
        .as_object_mut()
        .ok_or_else(|| "训练参数必须是对象".to_string())?;
    parameters.insert(
        "output_dir".to_string(),
        Value::String(output_directory.to_string_lossy().to_string()),
    );
    Ok(output_directory)
}

async fn run_training_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_training_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let training = request.training.ok_or_else(|| TaskFailure {
        code: "missing_training_request".to_string(),
        message: "训练任务缺少训练配置".to_string(),
        retryable: false,
    })?;
    let adapter = training.validate().map_err(|message| TaskFailure {
        code: "invalid_training_request".to_string(),
        message,
        retryable: false,
    })?;
    let runtime_profile =
        resolve_training_runtime_profile(&state.training_root, &training.runtime_profile_id)
            .map_err(|message| TaskFailure {
                code: "training_runtime_unavailable".to_string(),
                message,
                retryable: true,
            })?;
    let run_dir = state.training_root.join("runs").join(&task.id);
    std::fs::create_dir_all(&run_dir).map_err(|error| TaskFailure {
        code: "training_run_directory_failed".to_string(),
        message: format!("无法创建训练目录: {error}"),
        retryable: true,
    })?;
    let mut effective_parameters = training.parameters.clone();
    let owned_output_dir =
        prepare_owned_training_output_dir(&run_dir, &task.id, &mut effective_parameters).map_err(
            |message| TaskFailure {
                code: "training_output_directory_failed".to_string(),
                message,
                retryable: true,
            },
        )?;
    let mut sample_dataset_dir = effective_parameters
        .get("train_data_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from);
    let mut sample_caption_extension = ".txt".to_string();
    let mut gallery_inspections = Vec::<TrainingGalleryDatasetInspection>::new();
    for dataset in training.gallery_datasets() {
        let inspection =
            inspect_training_gallery_dataset(state, dataset).map_err(api_task_failure)?;
        if inspection.image_count == 0 {
            return Err(TaskFailure {
                code: "empty_gallery_dataset".to_string(),
                message: "所选图库目录中没有可训练图片".to_string(),
                retryable: false,
            });
        }
        if inspection.caption_count != inspection.image_count {
            return Err(TaskFailure {
                code: "gallery_dataset_captions_incomplete".to_string(),
                message: format!(
                    "图库子集 {} 有 {} 张图片，但只有 {} 个 Caption；请先完成重新打标",
                    inspection.relative_directory, inspection.image_count, inspection.caption_count,
                ),
                retryable: false,
            });
        }
        if let Some(first) = gallery_inspections.first() {
            if first.caption_extension != inspection.caption_extension {
                return Err(TaskFailure {
                    code: "gallery_caption_extension_mismatch".to_string(),
                    message: "多个图库子集必须使用相同的 Caption 扩展名".to_string(),
                    retryable: false,
                });
            }
        }
        gallery_inspections.push(inspection);
    }
    if !gallery_inspections.is_empty() {
        let dataset_config_path = run_dir.join("dataset.toml");
        std::fs::write(
            &dataset_config_path,
            training_gallery_datasets_toml(&gallery_inspections, &effective_parameters),
        )
        .map_err(|error| TaskFailure {
            code: "training_dataset_config_write_failed".to_string(),
            message: format!("无法写入图库数据集配置: {error}"),
            retryable: true,
        })?;
        let parameters = effective_parameters
            .as_object_mut()
            .ok_or_else(|| TaskFailure {
                code: "invalid_training_request".to_string(),
                message: "训练参数必须是对象".to_string(),
                retryable: false,
            })?;
        parameters.insert(
            "train_data_dir".to_string(),
            Value::String(
                gallery_inspections[0]
                    .image_dir
                    .to_string_lossy()
                    .to_string(),
            ),
        );
        parameters.insert(
            "dataset_config".to_string(),
            Value::String(dataset_config_path.to_string_lossy().to_string()),
        );
        sample_dataset_dir = Some(gallery_inspections[0].image_dir.clone());
        sample_caption_extension = gallery_inspections[0].caption_extension.clone();
    }
    let resume_state = run_dir.join("resume_state");
    if resume_state.is_dir()
        && effective_parameters
            .get("resume")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        if let Some(parameters) = effective_parameters.as_object_mut() {
            parameters.insert(
                "resume".to_string(),
                Value::String(resume_state.to_string_lossy().to_string()),
            );
        }
    }
    match training.sample.as_ref().filter(|settings| settings.enabled) {
        Some(settings) => {
            configure_training_samples(
                settings,
                sample_dataset_dir.as_deref(),
                &sample_caption_extension,
                &mut effective_parameters,
            )
            .map_err(|message| TaskFailure {
                code: "training_sample_config_failed".to_string(),
                message,
                retryable: false,
            })?;
        }
        None => {
            // The UI deliberately requires an explicit sampling switch.  Do
            // not allow a hidden legacy TOML prompt path to generate images.
            if let Some(parameters) = effective_parameters.as_object_mut() {
                parameters.remove("sample_prompts");
                parameters.remove("sample_every_n_epochs");
                parameters.remove("sample_every_n_steps");
                parameters.remove("sample_at_first");
            }
        }
    }
    if let Some(logging_dir) =
        prepare_training_logging_dir(&mut effective_parameters).map_err(|message| TaskFailure {
            code: "training_logging_config_failed".to_string(),
            message,
            retryable: false,
        })?
    {
        std::fs::create_dir_all(&logging_dir).map_err(|error| TaskFailure {
            code: "training_logging_directory_failed".to_string(),
            message: format!("无法创建训练日志目录: {error}"),
            retryable: true,
        })?;
    }
    let toml = serialize_toml(&adapter, &effective_parameters).map_err(|message| TaskFailure {
        code: "training_config_failed".to_string(),
        message,
        retryable: false,
    })?;
    let config_path = run_dir.join("config.toml");
    std::fs::write(&config_path, toml).map_err(|error| TaskFailure {
        code: "training_config_write_failed".to_string(),
        message: format!("无法写入训练配置: {error}"),
        retryable: true,
    })?;
    let runtime_root = installed_training_runtime_root(&state.training_root);
    let trainer = runtime_root.join(adapter.trainer);
    if !trainer.is_file() {
        return Err(TaskFailure {
            code: "training_runtime_not_installed".to_string(),
            message: format!("内置训练运行时未安装：{}", runtime_root.display()),
            retryable: true,
        });
    }
    let python = runtime_profile.python.clone();
    if !python.is_file() {
        return Err(TaskFailure {
            code: "training_python_not_installed".to_string(),
            message: format!("训练 Python 不存在：{}", python.display()),
            retryable: true,
        });
    }
    let launcher = state.training_root.join("telemetry_launcher.py");
    if !launcher.is_file() {
        return Err(TaskFailure {
            code: "training_telemetry_not_installed".to_string(),
            message: format!("训练遥测桥接器不存在：{}", launcher.display()),
            retryable: true,
        });
    }
    let mut safe_snapshot = training.clone();
    safe_snapshot.parameters = effective_parameters.clone();
    if let Some(parameters) = safe_snapshot.parameters.as_object_mut() {
        for (key, value) in parameters.iter_mut() {
            if key.to_ascii_lowercase().contains("key")
                || key.to_ascii_lowercase().contains("token")
            {
                *value = Value::String("***".to_string());
            }
        }
    }
    std::fs::write(
        run_dir.join("request.json"),
        serde_json::to_vec_pretty(&safe_snapshot).unwrap_or_default(),
    )
    .map_err(|error| TaskFailure {
        code: "training_snapshot_write_failed".to_string(),
        message: format!("无法写入训练快照: {error}"),
        retryable: true,
    })?;
    std::fs::write(
        run_dir.join("runtime.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "adapter_id": adapter.id,
            "adapter_version": adapter.version,
            "runtime_profile": training.runtime_profile_id,
            "runtime_profile_kind": runtime_profile.kind,
            "runtime_profile_managed": runtime_profile.managed,
            "python": python.to_string_lossy(),
            "runtime_root": runtime_root,
            "telemetry": "jsonl-v1",
        }))
        .unwrap_or_default(),
    )
    .map_err(|error| TaskFailure {
        code: "training_snapshot_write_failed".to_string(),
        message: format!("无法写入运行时摘要: {error}"),
        retryable: true,
    })?;
    let log_path = run_dir.join("console.log");
    let allocated_gpu_ids = state
        .training_leases
        .assigned_gpus(&task.id, &training.runtime_profile_id);
    let gpu_ids = if allocated_gpu_ids.is_empty() {
        training.gpu_ids.clone()
    } else {
        allocated_gpu_ids
    }
    .join(",");
    let wandb_api_key = training
        .parameters
        .get("wandb_api_key")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let run_dir_for_process = run_dir.clone();
    let run_dir_for_progress = run_dir.clone();
    let state_for_progress = state.clone();
    let config_path_for_process = config_path.clone();
    let launcher_for_process = launcher.clone();
    let trainer_for_process = trainer.clone();
    let metrics_path = run_dir.join("metrics.jsonl");
    let control_path = run_dir.join("control.json");
    let task_manager = state.tasks.clone();
    let task_id = task.id.clone();
    let gpu_ids_for_process = gpu_ids.clone();
    let training_max_steps = effective_parameters
        .get("max_train_steps")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .to_string();
    let wsl_distro = std::env::var("DANBOORU_TRAINING_WSL_DISTRO").ok();
    let is_wsl = runtime_profile.is_wsl();
    let wsl_paths = if is_wsl {
        Some((
            windows_path_for_wsl(&python, wsl_distro.as_deref()).map_err(|message| {
                TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                }
            })?,
            windows_path_for_wsl(&runtime_root, wsl_distro.as_deref()).map_err(|message| {
                TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                }
            })?,
            windows_path_for_wsl(&run_dir_for_process, wsl_distro.as_deref()).map_err(
                |message| TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                },
            )?,
            windows_path_for_wsl(&metrics_path, wsl_distro.as_deref()).map_err(|message| {
                TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                }
            })?,
            windows_path_for_wsl(&control_path, wsl_distro.as_deref()).map_err(|message| {
                TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                }
            })?,
            windows_path_for_wsl(&launcher_for_process, wsl_distro.as_deref()).map_err(
                |message| TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                },
            )?,
            windows_path_for_wsl(&trainer_for_process, wsl_distro.as_deref()).map_err(
                |message| TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                },
            )?,
            windows_path_for_wsl(&config_path_for_process, wsl_distro.as_deref()).map_err(
                |message| TaskFailure {
                    code: "training_wsl_path_failed".to_string(),
                    message,
                    retryable: true,
                },
            )?,
        ))
    } else {
        None
    };
    let log_path_for_process = log_path.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<(i32, bool), String> {
        let mut command;
        if let Some((python, _runtime, run_dir, metrics, control, launcher, trainer, config)) =
            wsl_paths
        {
            command = Command::new("wsl.exe");
            if let Some(distro) = wsl_distro
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                command.args(["--distribution", distro]);
            }
            command.args(["--exec", "env"]);
            for (key, value) in [
                ("PYTHONUNBUFFERED", "1".to_string()),
                ("ACCELERATE_DISABLE_RICH", "1".to_string()),
                ("DANBOORU_TRAINING_RUN_DIR", run_dir),
                ("DANBOORU_TRAINING_METRICS_FILE", metrics),
                ("DANBOORU_TRAINING_CONTROL_FILE", control),
                ("CUDA_VISIBLE_DEVICES", gpu_ids_for_process.clone()),
                ("DANBOORU_TRAINING_MAX_STEPS", training_max_steps.clone()),
            ] {
                command.arg(format!("{key}={value}"));
            }
            if let Some(api_key) = wandb_api_key.as_deref() {
                command.arg(format!("WANDB_API_KEY={api_key}"));
            }
            command
                .arg(python)
                .args([
                    "-m",
                    "accelerate.commands.launch",
                    "--num_cpu_threads_per_process",
                    "2",
                    "--quiet",
                ])
                .arg(launcher)
                .arg(trainer)
                .arg("--config_file")
                .arg(config);
        } else {
            command = Command::new(&python);
            command
                .current_dir(&runtime_root)
                .env("PYTHONUNBUFFERED", "1")
                .env("ACCELERATE_DISABLE_RICH", "1")
                .env("DANBOORU_TRAINING_RUN_DIR", &run_dir_for_process)
                .env("DANBOORU_TRAINING_METRICS_FILE", &metrics_path)
                .env("DANBOORU_TRAINING_CONTROL_FILE", &control_path)
                .env("CUDA_VISIBLE_DEVICES", &gpu_ids_for_process)
                .env("DANBOORU_TRAINING_MAX_STEPS", &training_max_steps)
                .args([
                    "-m",
                    "accelerate.commands.launch",
                    "--num_cpu_threads_per_process",
                    "2",
                    "--quiet",
                ])
                .arg(&launcher_for_process)
                .arg(&trainer_for_process)
                .arg("--config_file")
                .arg(&config_path_for_process);
            if let Some(api_key) = wandb_api_key.as_deref() {
                command.env("WANDB_API_KEY", api_key);
            }
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("无法启动 Accelerate: {error}"))?;
        std::fs::write(&log_path_for_process, b"")
            .map_err(|error| format!("无法初始化训练日志: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法捕获训练标准输出".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "无法捕获训练错误输出".to_string())?;
        let stdout_log_path = log_path_for_process.clone();
        let stdout_reader =
            std::thread::spawn(move || stream_training_output(stdout, &stdout_log_path, "stdout"));
        let stderr_log_path = log_path_for_process.clone();
        let stderr_reader =
            std::thread::spawn(move || stream_training_output(stderr, &stderr_log_path, "stderr"));
        let mut stop_requested: Option<(String, Instant)> = None;
        let started = Instant::now();
        let mut last_progress_report = Instant::now();
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("无法等待训练进程: {error}"))?
            {
                break status;
            }
            if last_progress_report.elapsed() >= Duration::from_secs(1) {
                last_progress_report = Instant::now();
                if let Some((completed, total, _)) =
                    read_training_metrics_progress(&run_dir_for_progress)
                {
                    let _ = report_task_progress(
                        &state_for_progress,
                        &task_id,
                        completed,
                        total,
                        0,
                        0,
                        started,
                    );
                }
            }
            let requested =
                task_manager
                    .get(&task_id)
                    .ok()
                    .flatten()
                    .and_then(|task| match task.status {
                        TaskStatus::Pausing => Some("pause"),
                        TaskStatus::Cancelling => Some("cancel"),
                        _ => None,
                    });
            if let Some(action) = requested {
                if stop_requested.is_none() {
                    std::fs::write(&control_path, format!("{{\"action\":\"{action}\"}}"))
                        .map_err(|error| format!("无法请求训练停止: {error}"))?;
                    stop_requested = Some((action.to_string(), Instant::now()));
                }
                if stop_requested
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(30))
                {
                    terminate_training_process_tree(&mut child)?;
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        Ok((status.code().unwrap_or(-1), stop_requested.is_some()))
    })
    .await
    .map_err(|error| TaskFailure {
        code: "training_worker_failed".to_string(),
        message: error.to_string(),
        retryable: true,
    })?
    .map_err(|message| TaskFailure {
        code: "training_launch_failed".to_string(),
        message,
        retryable: true,
    })?;
    if outcome.1 {
        return Ok(WorkerOutcome::Stopped);
    }
    let console = std::fs::read(&log_path)
        .map(|bytes| decode_training_log_bytes(&bytes))
        .unwrap_or_default();
    if let Some(failure) = training_process_failure(outcome.0, &console) {
        return Err(failure);
    }
    Ok(WorkerOutcome::Complete(serde_json::json!({
        "adapter_id": adapter.id,
        "run_directory": run_dir,
        "output_directory": owned_output_dir,
        "config_path": config_path,
        "gpu_ids": gpu_ids,
    })))
}

fn training_process_failure(exit_code: i32, console: &str) -> Option<TaskFailure> {
    if exit_code != 0 {
        return Some(TaskFailure {
            code: "training_failed".to_string(),
            message: format!("训练进程以退出码 {exit_code} 结束；请查看控制台日志"),
            retryable: true,
        });
    }
    let normalized = console.to_ascii_lowercase();
    if normalized.contains("no data found") {
        return Some(TaskFailure {
            code: "training_no_data".to_string(),
            message: "训练器没有找到任何训练图片。lora-scripts 要求“训练集目录”指向包含训练子文件夹的父目录；请改选正确的父目录，或从图库导入数据集后重新创建任务。".to_string(),
            retryable: false,
        });
    }
    None
}

fn stream_training_output<R: Read>(stream: R, path: &Path, channel: &str) -> Result<(), String> {
    let mut reader = std::io::BufReader::new(stream);
    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("无法打开训练日志: {error}"))?;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("无法读取训练{channel}输出: {error}"))?;
        if read == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        output
            .write_all(&line)
            .map_err(|error| format!("无法写入训练日志: {error}"))?;
        let _ = output.flush();
    }
    Ok(())
}

fn terminate_training_process_tree(child: &mut std::process::Child) -> Result<(), String> {
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status()
            .map_err(|error| format!("无法终止训练进程树: {error}"))?;
        if !status.success() {
            return Err("训练进程树终止失败".to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        child
            .kill()
            .map_err(|error| format!("无法终止训练进程: {error}"))
    }
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
    let Some(_root_write) = acquire_task_root_write(state, &task.id, verified_root.path()).await?
    else {
        return Ok(WorkerOutcome::Stopped);
    };
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
            tag_prefixes: Vec::new(),
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
        .unwrap_or(100);
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
    let Some(_root_write) = acquire_task_root_write(state, &task.id, &root_path).await? else {
        return Ok(WorkerOutcome::Stopped);
    };
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

async fn run_dataset_augmentation_task(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<WorkerOutcome, TaskFailure> {
    let request: CreateTaskRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| TaskFailure {
            code: "invalid_task_payload".to_string(),
            message: error.to_string(),
            retryable: false,
        })?;
    let mut config =
        parse_dataset_augmentation_config(request.options.as_ref()).map_err(api_task_failure)?;
    let source_directory = dataset_augmentation_source_directory(request.options.as_ref())
        .map_err(api_task_failure)?;
    config.output_directory = source_directory.join(".augmentation");
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
    let Some(_root_write) = acquire_task_root_write(state, &task.id, &root_path).await? else {
        return Ok(WorkerOutcome::Stopped);
    };

    let mut sources = Vec::with_capacity(media_ids.len());
    let mut source_media = HashMap::with_capacity(media_ids.len());
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
        if !is_supported_dataset_augmentation_media(&media.relative_path, &media.mime_type) {
            return Err(TaskFailure {
                code: "unsupported_dataset_augmentation_media".to_string(),
                message: format!("媒体 {media_id} 不是支持的静态图片"),
                retryable: false,
            });
        }
        let fallback_caption = media
            .post_id
            .map(|post_id| {
                state
                    .database
                    .get_post_library_metadata(post_id)
                    .map(|metadata| {
                        metadata
                            .map(|metadata| {
                                metadata
                                    .tags
                                    .into_iter()
                                    .map(|tag| tag.name)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default()
                    })
            })
            .transpose()
            .map_err(database_task_failure)?
            .unwrap_or_default();
        sources.push(DatasetAugmentationSource {
            media_id: media.id.clone(),
            relative_path: PathBuf::from(&media.relative_path),
            sha256: media.sha256.clone(),
            fallback_caption,
        });
        source_media.insert(media.id.clone(), media);
    }

    let analyses = if config.smart_crop.enabled {
        let root_for_worker = root_path.clone();
        let sources_for_worker = sources.clone();
        let training_root = state.training_root.clone();
        let smart_crop = config.smart_crop.clone();
        tokio::task::spawn_blocking(move || {
            run_dataset_vision_detection(
                &training_root,
                &root_for_worker,
                &sources_for_worker,
                &smart_crop,
            )
        })
        .await
        .map_err(join_task_failure)?
        .map_err(|message| TaskFailure {
            code: "vision_crop_preflight_failed".to_string(),
            message,
            retryable: true,
        })?
    } else {
        HashMap::new()
    };

    let retagging_config = config.retagging.clone();
    let smart_crop_enabled = config.smart_crop.enabled;
    let source_lookup = sources
        .iter()
        .map(|source| (source.media_id.clone(), source.clone()))
        .collect::<HashMap<_, _>>();

    let workspace_root = root_path.clone();
    let workspace_task_id = task.id.clone();
    let mut workspace = tokio::task::spawn_blocking(move || {
        let root = VerifiedMediaRoot::open(workspace_root)?;
        DatasetAugmentationWorkspace::create(root, &workspace_task_id, config)
    })
    .await
    .map_err(join_task_failure)?
    .map_err(tool_task_failure)?;

    let total = sources.len() as u64;
    let started = Instant::now();
    let mut generated_samples = Vec::new();
    let mut rejections = Vec::new();
    for (index, source) in sources.into_iter().enumerate() {
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        let analysis = analyses.get(&source.media_id).cloned();
        let (next_workspace, item_result) = tokio::task::spawn_blocking(move || {
            let mut workspace = workspace;
            let item_result = workspace.process_with_analysis(&source, analysis.as_ref());
            (workspace, item_result)
        })
        .await
        .map_err(join_task_failure)?;
        workspace = next_workspace;
        match item_result.map_err(tool_task_failure)? {
            DatasetAugmentationItemResult::Generated(samples) => {
                for sample in &samples {
                    let source =
                        source_media
                            .get(&sample.source_media_id)
                            .ok_or_else(|| TaskFailure {
                                code: "dataset_source_missing".to_string(),
                                message: "数据集增广源媒体记录丢失".to_string(),
                                retryable: false,
                            })?;
                    let output_path = root_path.join(&sample.output_relative_path);
                    let metadata =
                        std::fs::metadata(&output_path).map_err(|error| TaskFailure {
                            code: "dataset_output_missing".to_string(),
                            message: format!("派生样本不存在: {error}"),
                            retryable: false,
                        })?;
                    state
                        .database
                        .upsert_media_file(&MediaFileInput {
                            id: uuid::Uuid::new_v4().to_string(),
                            root_id: request.root_id.clone(),
                            post_id: source.post_id,
                            relative_path: sample
                                .output_relative_path
                                .to_string_lossy()
                                .replace('\\', "/"),
                            variant: format!("dataset_augmentation_{}", sample.variant),
                            mime_type: dataset_augmentation_mime_type(&sample.output_relative_path)
                                .to_string(),
                            byte_size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                            sha256: None,
                            md5: None,
                            width: Some(i64::from(sample.width)),
                            height: Some(i64::from(sample.height)),
                            duration: None,
                        })
                        .map_err(database_task_failure)?;
                }
                generated_samples.extend(samples);
            }
            DatasetAugmentationItemResult::Rejected(rejection) => rejections.push(rejection),
        }
        if !report_download_progress(state, &task.id, index as u64 + 1, total, 0, started)? {
            return Ok(WorkerOutcome::Stopped);
        }
    }
    let mut retagging_successes = Vec::new();
    let mut retagging_failures = Vec::new();
    if retagging_config.send_to_vllm {
        let retagging_samples = generated_samples
            .iter()
            .filter(|sample| sample.requires_retagging)
            .cloned()
            .collect::<Vec<_>>();
        if !retagging_samples.is_empty() {
            let (successes, failures) = retag_dataset_augmentation_samples(
                state,
                &root_path,
                &source_lookup,
                &source_media,
                &retagging_samples,
                retagging_config.preserve_artist_character_tags,
            )
            .await?;
            let successful_samples = successes
                .iter()
                .filter_map(|success| {
                    retagging_samples
                        .iter()
                        .find(|sample| sample.sample_id == success.media_id)
                        .cloned()
                })
                .collect::<Vec<_>>();
            if !successful_samples.is_empty() {
                let (next_workspace, promoted) = tokio::task::spawn_blocking(move || {
                    let mut workspace = workspace;
                    let promoted = workspace.promote_retagged_samples(&successful_samples);
                    (workspace, promoted)
                })
                .await
                .map_err(join_task_failure)?;
                workspace = next_workspace;
                promoted.map_err(tool_task_failure)?;
            }
            retagging_successes = successes;
            retagging_failures = failures;
        }
    }
    let summary = workspace.finish().map_err(tool_task_failure)?;
    let mut variant_counts = BTreeMap::<String, usize>::new();
    let mut crop_coverage = BTreeMap::<String, (usize, f64)>::new();
    for sample in &generated_samples {
        *variant_counts.entry(sample.variant.clone()).or_default() += 1;
        if matches!(
            sample.variant.as_str(),
            "portrait" | "upper_body" | "full_body_tight"
        ) {
            if let Some(source) = source_media.get(&sample.source_media_id) {
                let source_width = source.width.unwrap_or_default().max(1) as f64;
                let source_height = source.height.unwrap_or_default().max(1) as f64;
                let retained_percent = (f64::from(sample.width) * f64::from(sample.height))
                    / (source_width * source_height)
                    * 100.0;
                let entry = crop_coverage.entry(sample.variant.clone()).or_default();
                entry.0 += 1;
                entry.1 += retained_percent;
            }
        }
    }
    let crop_coverage = crop_coverage
        .into_iter()
        .map(|(variant, (count, total_percent))| {
            (
                variant,
                serde_json::json!({
                    "count": count,
                    "average": (total_percent / count.max(1) as f64 * 10.0).round() / 10.0,
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    let smart_crop_rejected = summary
        .rejection_reasons
        .iter()
        .filter(|(reason, _)| reason.starts_with("智能裁剪拒绝："))
        .map(|(_, count)| count)
        .sum::<usize>();
    Ok(WorkerOutcome::Complete(serde_json::json!({
        "output_relative_directory": summary.output_relative_directory,
        "derived_relative_directory": summary.derived_relative_directory,
        "metadata_relative_directory": summary.metadata_relative_directory,
        "training_relative_directory": summary.training_relative_directory,
        "source_images": source_media.len(),
        "generated": summary.generated,
        "rejected": summary.rejected,
        "retagging_pending": summary.retagging_pending,
        "retagged": summary.retagged,
        "training_subsets_relative_path": summary.metadata_relative_directory.join("metadata/training-subsets.json"),
        "retagging": {
            "requested": retagging_config.send_to_vllm,
            "preserve_artist_character_tags": retagging_config.preserve_artist_character_tags,
            "successes": retagging_successes,
            "failures": retagging_failures,
        },
        "samples": generated_samples,
        "rejections": rejections,
        "variant_counts": variant_counts,
        "smart_crop": {
            "enabled": smart_crop_enabled,
            "generated": generated_samples.iter().filter(|sample| matches!(sample.variant.as_str(), "portrait" | "upper_body" | "full_body_tight")).count(),
            "rejected": smart_crop_rejected,
            "coverage_percent": crop_coverage,
        },
        "rejection_reasons": summary.rejection_reasons,
        "next_step": if summary.retagging_pending == 0 { "原图保留在所选源目录；已二次打标的派生子集位于 .augmentation，任务元数据位于 .augmentation-metadata。训练工作台会自动发现可用派生子集，并可分别设置 repeat。" } else { "原图保留在源目录且不会复制。派生图没有沿用原标签，待重新打标的状态和原因在 .augmentation-metadata 中；只有完成二次打标的派生图会被训练工作台自动加入。" },
    })))
}

async fn retag_dataset_augmentation_samples(
    state: &AppState,
    root_path: &Path,
    sources: &HashMap<String, DatasetAugmentationSource>,
    source_media: &HashMap<String, MediaFileRecord>,
    samples: &[crate::services::dataset_augmentation::DatasetAugmentationSample],
    preserve_artist_character_tags: bool,
) -> Result<(Vec<VllmTagSuccess>, Vec<VllmRetryItem>), TaskFailure> {
    let root = VerifiedMediaRoot::open(root_path).map_err(tool_task_failure)?;
    let settings = state.settings.read().await.clone();
    let mut items = Vec::with_capacity(samples.len());
    for sample in samples {
        let image_path = root
            .resolve_existing_file(&sample.output_relative_path)
            .map_err(tool_task_failure)?;
        let tag_prefixes = if preserve_artist_character_tags {
            let media = source_media
                .get(&sample.source_media_id)
                .ok_or_else(|| TaskFailure {
                    code: "dataset_source_missing".to_string(),
                    message: "无法查找派生图的原始媒体记录".to_string(),
                    retryable: false,
                })?;
            let source = sources
                .get(&sample.source_media_id)
                .ok_or_else(|| TaskFailure {
                    code: "dataset_source_missing".to_string(),
                    message: "无法查找派生图的原始标签".to_string(),
                    retryable: false,
                })?;
            augmentation_identity_tag_prefixes(state, &root, media, source)?
        } else {
            Vec::new()
        };
        items.push(VllmBatchItem {
            media_id: sample.sample_id.clone(),
            image_path,
            existing_tags: None,
            sidecar_quarantine_path: None,
            tag_prefixes,
        });
    }
    let output = VllmOutputOptions {
        language: settings.vllm_language,
        max_tags: settings.vllm_max_tags,
        max_length: settings.vllm_max_length,
        verify_danbooru: settings.vllm_verify_danbooru,
        reference_existing: false,
    };
    let verify_danbooru =
        output.verify_danbooru && output.language == crate::services::vllm::VllmLanguage::Danbooru;
    let config = VllmServiceConfig {
        endpoint: settings.vllm_base_url,
        allowed_hosts: settings.vllm_allowed_hosts,
        model: settings.vllm_model,
        system_prompt: settings.vllm_system_prompt,
        // A derived sample starts with no sidecar. Always create a clean,
        // comma-separated caption rather than inheriting the user's global
        // append setting.
        tag_mode: TagWriteMode::Overwrite,
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
    let mut service = VllmService::new(config, api_key)
        .map_err(vllm_task_failure)?
        .with_output_options(output)
        .map_err(vllm_task_failure)?;
    if verify_danbooru {
        service = service.with_danbooru_client(state.danbooru.read().await.clone());
    }
    let result = service.tag_batch(items).await.map_err(vllm_task_failure)?;
    Ok((result.successes, result.retry_manifest.items))
}

fn augmentation_identity_tag_prefixes(
    state: &AppState,
    root: &VerifiedMediaRoot,
    media: &MediaFileRecord,
    source: &DatasetAugmentationSource,
) -> Result<Vec<String>, TaskFailure> {
    let mut prefixes = Vec::new();
    if let Some(post_id) = media.post_id {
        if let Some(metadata) = state
            .database
            .get_post_library_metadata(post_id)
            .map_err(database_task_failure)?
        {
            for tag in metadata.tags {
                match tag.category {
                    1 => prefixes.push(format!("artist:{}", tag.name)),
                    4 => prefixes.push(tag.name),
                    _ => {}
                }
            }
        }
    }
    // Locally supplied captions may carry explicit identity prefixes even
    // when this root has no indexed Danbooru post metadata.
    if prefixes.is_empty() {
        let sidecar = root
            .resolve(&source.relative_path)
            .map_err(tool_task_failure)?
            .with_extension("txt");
        let caption =
            std::fs::read_to_string(sidecar).unwrap_or_else(|_| source.fallback_caption.clone());
        prefixes.extend(caption.split(',').filter_map(|raw| {
            let tag = raw.trim();
            (tag.starts_with("artist:") || tag.starts_with('@') || tag.starts_with("character:"))
                .then(|| tag.to_string())
        }));
    }
    prefixes.sort();
    prefixes.dedup();
    Ok(prefixes)
}

const MIN_VISION_CROP_FREE_VRAM_MIB: u64 = 4_096;

fn run_dataset_vision_detection(
    training_root: &Path,
    root_path: &Path,
    sources: &[DatasetAugmentationSource],
    smart_crop: &SmartCropConfig,
) -> Result<HashMap<String, AnimeCropAnalysis>, String> {
    let gpu = training_gpu_inventory()
        .into_iter()
        .find(|gpu| gpu.id == smart_crop.gpu_id)
        .ok_or_else(|| format!("未发现 GPU {}，无法启动智能裁剪", smart_crop.gpu_id))?;
    let free_mib = gpu.memory_total_mib.saturating_sub(gpu.memory_used_mib);
    if free_mib < MIN_VISION_CROP_FREE_VRAM_MIB {
        let processes = gpu
            .external_processes
            .iter()
            .map(|process| format!("{} ({} MiB)", process.process_name, process.memory_used_mib))
            .collect::<Vec<_>>()
            .join("、");
        return Err(format!(
            "GPU {} 仅剩 {free_mib} MiB 显存，智能裁剪至少需要 {MIN_VISION_CROP_FREE_VRAM_MIB} MiB{}。请释放外部 vLLM/推理进程的显存后重试。",
            gpu.id,
            if processes.is_empty() { String::new() } else { format!("（当前进程：{processes}）") }
        ));
    }
    let profile = resolve_training_runtime_profile(training_root, &smart_crop.runtime_profile_id)?;
    // The single detect worker below is also the model/inference preflight. It
    // runs before a workspace is created or a media file is written, so a
    // missing model, CUDA failure, or bad provider rejects the task cleanly
    // without spinning up a second Python process that would load the same
    // ONNX models twice.
    let verified_root = VerifiedMediaRoot::open(root_path).map_err(|error| error.to_string())?;
    let mut items = Vec::with_capacity(sources.len());
    for source in sources {
        let path = verified_root
            .resolve_existing_file(&source.relative_path)
            .map_err(|error| {
                format!(
                    "无法验证智能裁剪源图片 {}: {error}",
                    source.relative_path.display()
                )
            })?;
        items.push(serde_json::json!({
            "media_id": source.media_id,
            "path": path.to_string_lossy(),
        }));
    }
    let analyses = run_anime_crop_detection_worker(
        training_root,
        &profile,
        &smart_crop.gpu_id,
        serde_json::json!({"action": "detect", "items": items}),
    )?;
    if analyses.len() != sources.len() {
        return Err("动漫检测 worker 返回数量与受检图片数量不一致".to_string());
    }
    let expected_media_ids = sources
        .iter()
        .map(|source| source.media_id.as_str())
        .collect::<HashSet<_>>();
    let actual_media_ids = analyses
        .iter()
        .map(|analysis| analysis.media_id.as_str())
        .collect::<HashSet<_>>();
    if expected_media_ids.len() != sources.len()
        || actual_media_ids.len() != analyses.len()
        || actual_media_ids != expected_media_ids
    {
        return Err("动漫检测 worker 返回的媒体标识与受检清单不一致".to_string());
    }
    Ok(analyses
        .into_iter()
        .map(|analysis| (analysis.media_id.clone(), analysis))
        .collect())
}

fn dataset_augmentation_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
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
        let Some(_root_write) = acquire_task_root_write(state, &task.id, &root_path).await? else {
            return Ok(WorkerOutcome::Stopped);
        };
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
        let Some(_root_write) = acquire_task_root_write(state, &task.id, &root_path).await? else {
            return Ok(WorkerOutcome::Stopped);
        };
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
    let Some(_root_write) = acquire_task_root_write(state, &task.id, &root_path).await? else {
        return Ok(WorkerOutcome::Stopped);
    };
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
    const INDEX_WRITE_BATCH_SIZE: usize = 256;
    let mut indexed = 0_u64;
    let started = Instant::now();
    for batch in candidates.chunks(INDEX_WRITE_BATCH_SIZE) {
        if worker_was_stopped(state, &task.id) {
            return Ok(WorkerOutcome::Stopped);
        }
        let mut local_posts = Vec::new();
        let mut media_files = Vec::with_capacity(batch.len());
        for candidate in batch {
            if let Some(post_id) = candidate.post_id {
                let tag_string = candidate.tags.join(" ");
                local_posts.push((
                    PostRecordInput {
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
                    candidate
                        .tags
                        .iter()
                        .map(|tag| PostTagInput::new(tag, 0))
                        .collect::<Vec<_>>(),
                ));
            }
            let mut id_hash = Sha256::new();
            id_hash.update(request.root_id.as_bytes());
            id_hash.update([0]);
            id_hash.update(candidate.relative_path.as_bytes());
            media_files.push(MediaFileInput {
                id: format!("indexed-{}", hex::encode(id_hash.finalize())),
                root_id: request.root_id.clone(),
                post_id: candidate.post_id.map(|id| id as i64),
                relative_path: candidate.relative_path.clone(),
                variant: "original".to_string(),
                mime_type: media_mime_type(&candidate.extension).to_string(),
                byte_size: i64::try_from(candidate.byte_size).unwrap_or(i64::MAX),
                sha256: None,
                md5: None,
                width: candidate.width.map(i64::from),
                height: candidate.height.map(i64::from),
                duration: None,
            });
        }
        state
            .database
            .upsert_indexed_media_batch(&local_posts, &media_files)
            .map_err(database_task_failure)?;
        indexed = indexed.saturating_add(batch.len() as u64);
        if !report_download_progress(state, &task.id, indexed, total, 0, started)? {
            return Ok(WorkerOutcome::Stopped);
        }
        tokio::task::yield_now().await;
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
        "exact_dedup" | "integrity_scan" | "delete_by_tag" | "delete_selected" => {
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
            "delete_selected" => {
                let media = selected_media.as_deref().ok_or_else(|| {
                    crate::services::image_processor::ToolError::InvalidManifest(
                        "删除所选任务缺少 media_ids".to_string(),
                    )
                })?;
                plan_delete_selected(&root, media)
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
    let Some(_root_write) = acquire_task_root_write(state, task_id, &root_path).await? else {
        return Ok(WorkerOutcome::Stopped);
    };

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
    let Some(_root_write) = acquire_task_root_write(state, &task.id, verified_root.path()).await?
    else {
        return Ok(WorkerOutcome::Stopped);
    };
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
    let blur_sensitive = state.settings.read().await.blur_sensitive_media;
    let sensitive_rating_filter = blur_sensitive
        .then(|| " -rating:questionable -rating:explicit".to_string());
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
    let starting_bytes = bytes;
    if !report_download_run_progress(
        state,
        &task.id,
        downloaded,
        target,
        bytes,
        starting_bytes,
        started,
    )? {
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
                    let task_starting_bytes = starting_bytes;
                    let root_id = request.root_id.clone();
                    let destination = destination.clone();
                    let template = template.clone();
                    workers.spawn(async move {
                        download_post_id(
                            worker_state,
                            worker_client,
                            task_id,
                            task_started,
                            task_starting_bytes,
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
                        if !report_download_run_progress(
                            state,
                            &task.id,
                            downloaded,
                            target,
                            bytes,
                            starting_bytes,
                            started,
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
            if let Some(filter) = sensitive_rating_filter.as_ref() {
                active_query.push_str(filter);
            }
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
                        if let Some(sensitive) = sensitive_rating_filter.as_ref() {
                            active_query.push_str(sensitive);
                        }
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
                        (!blur_sensitive || matches!(post.rating.as_str(), "g" | "s"))
                            && tracked_post_statuses
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
                        let task_starting_bytes = starting_bytes;
                        let root_id = request.root_id.clone();
                        let destination = destination.clone();
                        let template = template.clone();
                        workers.spawn(async move {
                            download_tracked_known_post(
                                worker_state,
                                worker_client,
                                task_id,
                                task_started,
                                task_starting_bytes,
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
                            if !report_download_run_progress(
                                state,
                                &task.id,
                                downloaded,
                                target,
                                bytes,
                                starting_bytes,
                                started,
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
        && !report_download_run_progress(
            state,
            &task.id,
            downloaded,
            downloaded,
            bytes,
            starting_bytes,
            started,
        )?
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
    task_starting_bytes: u64,
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
            task_starting_bytes,
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
    task_starting_bytes: u64,
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
        task_starting_bytes,
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
    task_starting_bytes: u64,
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
        Some(task_starting_bytes),
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

async fn acquire_task_root_write(
    state: &AppState,
    task_id: &str,
    root_path: &Path,
) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, TaskFailure> {
    const STOP_POLL_INTERVAL: Duration = Duration::from_millis(25);

    if worker_was_stopped(state, task_id) {
        return Ok(None);
    }
    let acquire = state.root_writes.acquire(root_path);
    tokio::pin!(acquire);
    loop {
        tokio::select! {
            result = &mut acquire => return result.map(Some).map_err(root_write_task_failure),
            _ = tokio::time::sleep(STOP_POLL_INTERVAL) => {
                if worker_was_stopped(state, task_id) {
                    return Ok(None);
                }
            }
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
    report_task_progress(state, task_id, completed, total, bytes, 0, started)
}

fn report_download_run_progress(
    state: &AppState,
    task_id: &str,
    completed: u64,
    total: u64,
    bytes: u64,
    starting_bytes: u64,
    started: Instant,
) -> Result<bool, TaskFailure> {
    report_task_progress(
        state,
        task_id,
        completed,
        total,
        bytes,
        starting_bytes,
        started,
    )
}

/// Reads the tail of the training `metrics.jsonl` stream and derives the
/// current optimizer step plus the configured maximum, so the task centre can
/// show a real training progress bar instead of a permanent 0%.
fn read_training_metrics_progress(run_dir: &std::path::Path) -> Option<(u64, u64, Option<u64>)> {
    use std::io::{Read, Seek, SeekFrom};
    let path = run_dir.join("metrics.jsonl");
    let mut file = std::fs::File::open(&path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len == 0 {
        return None;
    }
    let tail_len = file_len.min(16 * 1024);
    file.seek(SeekFrom::End(-(tail_len as i64))).ok()?;
    let mut bytes = Vec::with_capacity(tail_len as usize);
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut step: u64 = 0;
    let mut max_steps: Option<u64> = None;
    let mut eta_seconds: Option<u64> = None;
    for line in text.lines() {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(metrics) = value.get("metrics").and_then(Value::as_object) else {
            continue;
        };
        if let Some(parsed) = value.get("step").and_then(Value::as_u64) {
            step = step.max(parsed);
        }
        if let Some(parsed) = metrics.get("train.max_steps").and_then(Value::as_u64) {
            max_steps = Some(max_steps.map_or(parsed, |current| current.max(parsed)));
        }
        if let Some(parsed) = metrics.get("train.eta_seconds").and_then(Value::as_u64) {
            eta_seconds = Some(parsed);
        }
    }
    let total = max_steps?;
    if total == 0 {
        return None;
    }
    Some((step.min(total), total, eta_seconds))
}

fn report_task_progress(
    state: &AppState,
    task_id: &str,
    completed: u64,
    total: u64,
    bytes: u64,
    starting_bytes: u64,
    started: Instant,
) -> Result<bool, TaskFailure> {
    let speed = task_run_speed(starting_bytes, bytes, started.elapsed());
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

fn task_run_speed(starting_bytes: u64, current_bytes: u64, elapsed: Duration) -> u64 {
    task_average_speed(current_bytes.saturating_sub(starting_bytes), elapsed)
}

fn report_download_chunk_progress(
    state: &AppState,
    task_id: &str,
    baseline_bytes: u64,
    task_starting_bytes: u64,
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
    let speed = task_run_speed(task_starting_bytes, bytes, task_started.elapsed());
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
    task_starting_bytes: Option<u64>,
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
    let relative_directory = destination_dir
        .strip_prefix(verified_root.path())
        .map_err(|_| TaskFailure {
            code: "download_outside_root".to_string(),
            message: "下载子文件夹无法归入媒体根".to_string(),
            retryable: false,
        })?
        .to_string_lossy()
        .replace('\\', "/");
    if skip_existing
        && state
            .database
            .was_post_downloaded_in_directory(root_id, &relative_directory, post.id as i64)
            .map_err(database_task_failure)?
    {
        return Ok(PostDownloadOutcome::Skipped);
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
                if media_relative_directory(&media.relative_path) == relative_directory {
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
                            task_starting_bytes.unwrap_or(0),
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
                id: downloaded_media_id(
                    root_id,
                    post.id,
                    media_variant_name(variant),
                    &relative_path,
                ),
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
    let created_sidecars = if destination.keep_sidecar_txt {
        match write_download_sidecars(&verified_root, &downloaded_files, &post.tag_string) {
            Ok(sidecars) => sidecars,
            Err(failure) => {
                if let Err(rollback_error) =
                    rollback_new_download_files(verified_root.path(), &downloaded_files)
                {
                    return Err(TaskFailure {
                        code: "download_sidecar_rollback_incomplete".to_string(),
                        message: format!(
                            "写入同名 TXT 标签文件失败，且新媒体回滚不完整（{rollback_error}）: {}",
                            failure.message
                        ),
                        retryable: false,
                    });
                }
                return Err(failure);
            }
        }
    } else {
        Vec::new()
    };
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
                root_id,
                &relative_directory,
            )
    } else {
        state.database.register_downloaded_post(
            &post_record_input(post),
            &tags,
            &media_records,
            root_id,
            &relative_directory,
            None,
        )
    };
    if let Err(database_error) = database_result {
        let sidecar_rollback = rollback_created_download_sidecars(&created_sidecars);
        let media_rollback = rollback_new_download_files(verified_root.path(), &downloaded_files);
        if let Err(rollback_error) = sidecar_rollback.and(media_rollback) {
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

fn write_download_sidecars(
    root: &VerifiedMediaRoot,
    downloads: &[DownloadedMediaRegistration],
    tag_string: &str,
) -> Result<Vec<PathBuf>, TaskFailure> {
    let content = tag_string
        .split_whitespace()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if content.is_empty() {
        return Ok(Vec::new());
    }

    let mut created = Vec::new();
    let mut targets = HashSet::new();
    let write_result = (|| -> Result<(), TaskFailure> {
        for download in downloads.iter().filter(|item| item.newly_created) {
            let sidecar_relative =
                PathBuf::from(&download.record.relative_path).with_extension("txt");
            if !targets.insert(sidecar_relative.clone()) {
                continue;
            }
            let sidecar = root
                .resolve(&sidecar_relative)
                .map_err(|error| TaskFailure {
                    code: "unsafe_sidecar_path".to_string(),
                    message: format!("无法使用同名 TXT 标签文件路径: {error}"),
                    retryable: false,
                })?;
            match std::fs::symlink_metadata(&sidecar) {
                Ok(metadata)
                    if metadata.file_type().is_file()
                        && !metadata_is_link_or_reparse_point(&metadata) =>
                {
                    continue;
                }
                Ok(_) => {
                    return Err(TaskFailure {
                        code: "unsafe_sidecar_file".to_string(),
                        message: format!(
                            "拒绝覆盖不安全的同名 TXT 标签文件: {}",
                            sidecar_relative.display()
                        ),
                        retryable: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(TaskFailure {
                        code: "sidecar_metadata_failed".to_string(),
                        message: format!(
                            "无法检查同名 TXT 标签文件 {}: {error}",
                            sidecar_relative.display()
                        ),
                        retryable: true,
                    });
                }
            }
            let parent = sidecar.parent().ok_or_else(|| TaskFailure {
                code: "sidecar_parent_missing".to_string(),
                message: format!(
                    "同名 TXT 标签文件缺少父目录: {}",
                    sidecar_relative.display()
                ),
                retryable: false,
            })?;
            let mut temporary =
                tempfile::NamedTempFile::new_in(parent).map_err(|error| TaskFailure {
                    code: "sidecar_create_failed".to_string(),
                    message: format!("无法创建同名 TXT 标签临时文件: {error}"),
                    retryable: true,
                })?;
            temporary
                .write_all(content.as_bytes())
                .map_err(|error| TaskFailure {
                    code: "sidecar_write_failed".to_string(),
                    message: format!("无法写入同名 TXT 标签文件: {error}"),
                    retryable: true,
                })?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| TaskFailure {
                    code: "sidecar_write_failed".to_string(),
                    message: format!("无法同步同名 TXT 标签文件: {error}"),
                    retryable: true,
                })?;
            match temporary.persist_noclobber(&sidecar) {
                Ok(_) => created.push(sidecar),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(TaskFailure {
                        code: "sidecar_write_failed".to_string(),
                        message: format!("无法保存同名 TXT 标签文件: {}", error.error),
                        retryable: true,
                    });
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = write_result {
        if let Err(rollback_error) = rollback_created_download_sidecars(&created) {
            return Err(TaskFailure {
                code: "sidecar_rollback_incomplete".to_string(),
                message: format!("{}；TXT 回滚不完整（{rollback_error}）", error.message),
                retryable: false,
            });
        }
        return Err(error);
    }
    Ok(created)
}

fn rollback_created_download_sidecars(sidecars: &[PathBuf]) -> Result<(), String> {
    let mut failures = Vec::new();
    for sidecar in sidecars.iter().rev() {
        match std::fs::symlink_metadata(sidecar) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata_is_link_or_reparse_point(&metadata) =>
            {
                if let Err(error) = std::fs::remove_file(sidecar) {
                    failures.push(format!("{}: {error}", sidecar.display()));
                }
            }
            Ok(_) => failures.push(format!(
                "{}: 拒绝删除不安全的 TXT 标签文件",
                sidecar.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{}: {error}", sidecar.display())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn media_relative_directory(relative_path: &str) -> String {
    relative_path
        .replace('\\', "/")
        .rsplit_once('/')
        .map_or_else(String::new, |(directory, _)| directory.to_string())
}

fn downloaded_media_id(root_id: &str, post_id: u64, variant: &str, relative_path: &str) -> String {
    let directory = media_relative_directory(relative_path);
    if directory.is_empty() {
        return format!("{root_id}:{post_id}:{variant}");
    }
    let digest = Sha256::digest(relative_path.as_bytes());
    let path_key = URL_SAFE_NO_PAD.encode(&digest[..12]);
    format!("{root_id}:{post_id}:{variant}:{path_key}")
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

fn cleanup_download_part_files(
    root: &VerifiedMediaRoot,
    output_dir: &Path,
) -> Result<u64, std::io::Error> {
    let output_dir = std::fs::canonicalize(output_dir)?;
    if !output_dir.starts_with(root.path()) || !output_dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "下载临时文件目录越过媒体根",
        ));
    }

    let mut removed = 0_u64;
    for entry in std::fs::read_dir(&output_dir)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "part")
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !metadata.file_type().is_file() || metadata_is_link_or_reparse_point(&metadata) {
            tracing::warn!(path = %entry.path().display(), "拒绝清理不安全的下载临时文件路径");
            continue;
        }
        std::fs::remove_file(entry.path())?;
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

async fn cleanup_terminal_download_part_files(
    state: &AppState,
    task: &TaskSnapshot,
) -> Result<u64, TaskFailure> {
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
        .map_err(api_task_failure)?;
    let verified_root = VerifiedMediaRoot::open(&root_path).map_err(tool_task_failure)?;
    let _root_write = state
        .root_writes
        .acquire(verified_root.path())
        .await
        .map_err(root_write_task_failure)?;
    let output_dir = match request.relative_directory.as_deref() {
        Some(relative_directory) => verified_root
            .resolve(Path::new(relative_directory))
            .map_err(tool_task_failure)?,
        None => verified_root.path().to_path_buf(),
    };
    cleanup_download_part_files(&verified_root, &output_dir).map_err(|error| TaskFailure {
        code: "download_part_cleanup_failed".to_string(),
        message: error.to_string(),
        retryable: true,
    })
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
        event_type: match event.event.as_str() {
            "created" => "created",
            "deleted" => "deleted",
            _ => "updated",
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
    let training = training_task_summary(&task);
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
        training,
        created_at: format_unix_timestamp(task.created_at),
        updated_at: format_unix_timestamp(task.updated_at),
    }
}

fn training_task_summary(task: &TaskSnapshot) -> Option<TrainingTaskSummaryResponse> {
    if task.kind != "training" {
        return None;
    }
    let training = task.payload.get("training")?;
    let parameter = |key: &str| {
        training
            .get("parameters")?
            .get(key)?
            .as_str()
            .map(str::to_string)
    };
    Some(TrainingTaskSummaryResponse {
        adapter_id: training.get("adapter_id")?.as_str()?.to_string(),
        runtime_profile_id: training.get("runtime_profile_id")?.as_str()?.to_string(),
        gpu_ids: training
            .get("gpu_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        model_path: parameter("pretrained_model_name_or_path"),
        train_data_dir: parameter("train_data_dir"),
        output_dir: parameter("output_dir"),
        output_name: parameter("output_name"),
    })
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
        "delete_selected" => "删除所选媒体",
        "tag_pipeline" => "标签处理",
        "vllm_tag" => "vLLM 视觉打标",
        "dataset_augmentation" => "数据集增广",
        "training" => "LoRA 训练",
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
        apply_tool_manifest, cleanup_download_part_files, configure_training_samples,
        download_post, download_post_to_destination, downsample_training_metrics,
        has_incomplete_augmentation_marker, is_static_image_post, isolated_mode_enabled,
        meets_minimum_resolution, migrate_legacy_database_from, normalize_task_relative_directory,
        parse_media_variant, prepare_owned_training_output_dir,
        purge_registered_quarantine_file_with, run_resize_task, safetensors_artifact_step,
        segmented_batch_anchor_query, segmented_batch_verification_query, sort_posts_for_download,
        spawn_task_worker, split_batch_verification_groups, task_average_speed, task_run_speed,
        test_router, training_gallery_dataset_toml, training_gallery_datasets_toml,
        training_sample_prompt_lines, validate_batch_download_filter, validate_task_request,
        BatchDownloadFilter, CreateTaskRequest, DownloadDestination, DownloadSource,
        MediaPolicyRequest, OnlineMetricSampler, PostDownloadOutcome,
        TrainingGalleryDatasetInspection,
    };
    use crate::models::DownloadConfig;
    use crate::secrets::SecretKind;
    use crate::services::danbooru::{
        DanbooruClient, DanbooruClientConfig, MediaAsset, MediaAssetVariant, Post,
    };
    use crate::services::image_processor::{plan_delete_by_tag, VerifiedMediaRoot};
    use crate::tasks::{TaskFailure, TaskStatus};
    use crate::training::{TrainingSamplePromptSource, TrainingSampleSettings};
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
    fn training_gallery_detects_an_incomplete_augmentation_ancestor() {
        let temporary = tempfile::tempdir().unwrap();
        let images = temporary
            .path()
            .join("odette/.augmentation/task-1/ready/train/portrait/images");
        std::fs::create_dir_all(&images).unwrap();
        std::fs::create_dir_all(
            temporary
                .path()
                .join("odette/.augmentation-metadata/task-1"),
        )
        .unwrap();
        std::fs::write(
            temporary
                .path()
                .join("odette/.augmentation-metadata/task-1/INCOMPLETE.json"),
            "{}",
        )
        .unwrap();

        let root = VerifiedMediaRoot::open(temporary.path()).unwrap();
        assert!(has_incomplete_augmentation_marker(&root, &images));

        std::fs::remove_file(
            temporary
                .path()
                .join("odette/.augmentation-metadata/task-1/INCOMPLETE.json"),
        )
        .unwrap();
        assert!(!has_incomplete_augmentation_marker(&root, &images));
    }

    #[test]
    fn dataset_smart_crop_options_default_to_lora_gpu_and_accept_all_compositions() {
        let options = serde_json::json!({
            "media_ids": ["media-1"],
            "smart_crop": {
                "portrait": true,
                "upper_body": true,
                "full_body_tight": true
            }
        });
        let config = super::parse_dataset_augmentation_config(Some(&options)).unwrap();
        assert!(config.smart_crop.enabled);
        assert_eq!(config.smart_crop.runtime_profile_id, "conda:lora");
        assert_eq!(config.smart_crop.gpu_id, "0");
        assert_eq!(config.smart_crop.quality_profile, "anime-quality");
        assert!(config.smart_crop.portrait);
        assert!(config.smart_crop.upper_body);
        assert!(config.smart_crop.full_body_tight);
    }

    #[test]
    fn dataset_task_accepts_the_smart_crop_contract_without_extra_file_controls() {
        let (_application, state, directory) = test_router();
        let media_root = directory.path().join("dataset-smart-crop-root");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-dataset-smart-crop",
                "Library",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .upsert_media_file(&crate::database::MediaFileInput {
                id: "media-dataset-smart-crop".into(),
                root_id: "root-dataset-smart-crop".into(),
                post_id: None,
                relative_path: "selected.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 1,
                sha256: None,
                md5: None,
                width: Some(1600),
                height: Some(2000),
                duration: None,
            })
            .unwrap();
        let request = CreateTaskRequest {
            kind: "dataset_augmentation".to_string(),
            root_id: "root-dataset-smart-crop".to_string(),
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
                "media_ids": ["media-dataset-smart-crop"],
                "smart_crop": {
                    "enabled": true,
                    "runtime_profile_id": "conda:lora",
                    "gpu_id": "0",
                    "quality_profile": "anime-quality",
                    "portrait": true,
                    "upper_body": true,
                    "full_body_tight": true,
                    "max_derived_per_family": 3
                }
            })),
            training: None,
        };
        assert!(validate_task_request(&state, &request).is_ok());
    }

    #[test]
    fn metric_delta_only_emits_complete_lines_after_the_requested_cursor() {
        let source = b"{\"step\":1,\"timestamp\":1,\"metrics\":{\"loss\":1.0}}\n{\"step\":2";

        let delta = super::metric_file_delta(source, 0);

        assert_eq!(delta.lines.len(), 1);
        assert_eq!(delta.lines[0].cursor, 48);
        assert_eq!(
            delta.lines[0].line,
            "{\"step\":1,\"timestamp\":1,\"metrics\":{\"loss\":1.0}}"
        );
        assert_eq!(delta.next_cursor, 48);
    }

    #[test]
    fn isolated_runtime_mode_requires_an_explicit_one_value() {
        assert!(isolated_mode_enabled(Some(std::ffi::OsStr::new("1"))));
        assert!(!isolated_mode_enabled(None));
        assert!(!isolated_mode_enabled(Some(std::ffi::OsStr::new("true"))));
    }

    #[test]
    fn finished_download_cleanup_removes_only_regular_part_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = VerifiedMediaRoot::open(directory.path()).unwrap();
        let stale_part = directory.path().join("stale.jpg.part");
        let completed_file = directory.path().join("completed.jpg");
        let unrelated_file = directory.path().join("notes.part.txt");
        std::fs::write(&stale_part, b"incomplete").unwrap();
        std::fs::write(&completed_file, b"completed").unwrap();
        std::fs::write(&unrelated_file, b"keep").unwrap();

        let removed = cleanup_download_part_files(&root, directory.path()).unwrap();

        assert_eq!(removed, 1);
        assert!(!stale_part.exists());
        assert!(completed_file.exists());
        assert!(unrelated_file.exists());
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
            training: None,
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
    fn training_metric_downsampling_preserves_each_series_endpoints_and_extrema() {
        let source = [1.0, 9.0, 2.0, 8.0, 3.0, 7.0]
            .into_iter()
            .enumerate()
            .map(|(step, value)| crate::training::TrainingMetric {
                step: step as u64,
                timestamp: step as u64,
                series: "loss".to_string(),
                value,
            })
            .collect();
        let sampled = downsample_training_metrics(source, 4);
        let values = sampled
            .iter()
            .map(|metric| metric.value)
            .collect::<Vec<_>>();

        assert!(values.contains(&1.0));
        assert!(values.contains(&9.0));
        assert!(values.contains(&7.0));
        assert!(sampled.iter().any(|metric| metric.step == 0));
        assert!(sampled.iter().any(|metric| metric.step == 5));
    }

    #[test]
    fn online_metric_sampler_keeps_a_long_selected_series_within_the_browser_limit() {
        let total = 12_000;
        let mut sampler = OnlineMetricSampler::new(total, 5_000);
        for step in 0..total {
            let value = match step {
                2_000 => -12.0,
                9_000 => 18.0,
                _ => step as f64 / total as f64,
            };
            sampler.push(crate::training::TrainingMetric {
                step: step as u64,
                timestamp: step as u64,
                series: "loss".to_string(),
                value,
            });
        }
        let sampled = sampler.finish();

        assert!(sampled.len() <= 5_000);
        assert!(sampled.iter().any(|metric| metric.step == 0));
        assert!(sampled
            .iter()
            .any(|metric| metric.step == (total - 1) as u64));
        assert!(sampled.iter().any(|metric| metric.value == -12.0));
        assert!(sampled.iter().any(|metric| metric.value == 18.0));
    }

    #[test]
    fn new_training_output_root_becomes_a_task_owned_directory_with_a_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let run_dir = directory.path().join("runs").join("task-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let output_root = directory.path().join("outputs");
        let mut parameters = serde_json::json!({ "output_dir": output_root });

        let output =
            prepare_owned_training_output_dir(&run_dir, "task-1", &mut parameters).unwrap();

        assert_eq!(
            output.file_name().and_then(|name| name.to_str()),
            Some("task-1")
        );
        assert!(output.is_dir());
        assert_eq!(parameters["output_dir"], output.to_string_lossy().as_ref());
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(run_dir.join("artifact-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["task_id"], "task-1");
        assert_eq!(
            manifest["output_directory"],
            output.to_string_lossy().as_ref()
        );
    }

    #[tokio::test]
    async fn training_queue_reports_waiting_order_and_gpu_blocker() {
        let (application, state, _directory) = test_router();
        let first = state
            .tasks
            .create(
                "training",
                serde_json::json!({
                    "type": "training",
                    "training": {
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": ["0"],
                        "parameters": {}
                    }
                }),
            )
            .unwrap();
        let second = state
            .tasks
            .create(
                "training",
                serde_json::json!({
                    "type": "training",
                    "training": {
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": ["0"],
                        "parameters": {}
                    }
                }),
            )
            .unwrap();
        state
            .training_leases
            .register_waiting(&first.id, "windows", &["0".to_string()]);
        state
            .training_leases
            .register_waiting(&second.id, "windows", &["0".to_string()]);

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/training/queue")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        let entries = payload["data"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["queue_position"], 1);
        assert_eq!(entries[1]["queue_position"], 2);
        assert_eq!(entries[1]["blocking_task_ids"][0], first.id);
    }

    #[tokio::test]
    async fn gallery_dataset_preview_counts_images_captions_and_repeat_without_copying_media() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("gallery-media").join("odette");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("one.jpg"), b"image").unwrap();
        std::fs::write(media.join("one.txt"), b"caption").unwrap();
        std::fs::write(media.join("two.png"), b"image").unwrap();
        state
            .database
            .create_root(
                "gallery-root",
                "图库",
                Some(media.parent().unwrap().to_str().unwrap()),
                Some(media.parent().unwrap().to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/training/datasets/gallery?root_id=gallery-root&relative_directory=odette&repeats=3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["data"]["image_count"], 2);
        assert_eq!(payload["data"]["caption_count"], 1);
        assert_eq!(payload["data"]["repeats"], 3);
        assert_eq!(payload["data"]["effective_image_count"], 6);
    }

    #[tokio::test]
    async fn gallery_dataset_preview_does_not_count_images_hidden_in_nested_folders() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("gallery-media");
        std::fs::create_dir_all(media.join("odette")).unwrap();
        std::fs::write(media.join("odette").join("one.jpg"), b"image").unwrap();
        state
            .database
            .create_root(
                "gallery-root-nested",
                "图库",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/training/datasets/gallery?root_id=gallery-root-nested&relative_directory=&repeats=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(payload["data"]["image_count"], 0);
    }

    #[tokio::test]
    async fn training_gallery_discovers_ready_augmentation_subsets_without_an_original_copy() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("gallery-media");
        let source = media.join("odette");
        let derived = source.join(".augmentation/task-portrait/ready/train/portrait/images");
        let metadata = source.join(".augmentation-metadata/task-portrait");
        std::fs::create_dir_all(&derived).unwrap();
        std::fs::create_dir_all(metadata.join("metadata")).unwrap();
        std::fs::write(source.join("source.png"), b"image").unwrap();
        std::fs::write(source.join("source.txt"), b"caption").unwrap();
        std::fs::write(derived.join("crop.png"), b"image").unwrap();
        std::fs::write(derived.join("crop.txt"), b"new caption").unwrap();
        std::fs::write(
            metadata.join("READY.json"),
            serde_json::json!({
                "training_subsets": {
                    "subsets": [{
                        "id": "portrait",
                        "label": "肖像裁剪",
                        "relative_directory": "odette/.augmentation/task-portrait/ready/train/portrait/images",
                        "requires_retagging": true,
                        "training_ready_count": 1,
                        "default_repeats": 1
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();
        state
            .database
            .create_root(
                "gallery-root-augmentations",
                "图库",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/training/datasets/augmentations?root_id=gallery-root-augmentations&relative_directory=odette")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["data"]["source"]["relative_directory"], "odette");
        assert_eq!(payload["data"]["subsets"].as_array().unwrap().len(), 1);
        assert_eq!(payload["data"]["subsets"][0]["id"], "portrait");
        assert_eq!(
            payload["data"]["subsets"][0]["relative_directory"],
            "odette/.augmentation/task-portrait/ready/train/portrait/images"
        );
    }

    #[test]
    fn gallery_dataset_config_references_the_original_library_and_repeat_count() {
        let inspection = TrainingGalleryDatasetInspection {
            root_id: "root".to_string(),
            root_name: "图库".to_string(),
            relative_directory: "odette".to_string(),
            image_dir: std::path::PathBuf::from("D:/gallery/odette"),
            caption_extension: ".txt".to_string(),
            image_count: 4,
            caption_count: 4,
            repeats: 3,
        };
        let config = training_gallery_dataset_toml(
            &inspection,
            &serde_json::json!({
                "resolution": "1024,1024",
                "enable_bucket": true,
                "train_batch_size": 2
            }),
        );

        assert!(config.contains("image_dir = \"D:/gallery/odette\""));
        assert!(config.contains("num_repeats = 3"));
        assert!(config.contains("batch_size = 2"));
        assert!(config.contains("resolution = [1024, 1024]"));
        assert!(config.contains("bucket_reso_steps = 32"));
    }

    #[test]
    fn gallery_dataset_config_keeps_each_subset_and_repeat_independent() {
        let first = TrainingGalleryDatasetInspection {
            root_id: "root-a".to_string(),
            root_name: "原图".to_string(),
            relative_directory: "original/images".to_string(),
            image_dir: std::path::PathBuf::from("D:/dataset/original/images"),
            caption_extension: ".txt".to_string(),
            image_count: 20,
            caption_count: 20,
            repeats: 2,
        };
        let second = TrainingGalleryDatasetInspection {
            root_id: "root-a".to_string(),
            root_name: "肖像裁剪".to_string(),
            relative_directory: "derived/portrait/images".to_string(),
            image_dir: std::path::PathBuf::from("D:/dataset/derived/portrait/images"),
            caption_extension: ".txt".to_string(),
            image_count: 8,
            caption_count: 8,
            repeats: 5,
        };

        let config = training_gallery_datasets_toml(
            &[first, second],
            &serde_json::json!({"resolution": [1024, 1024]}),
        );

        assert!(config.contains("image_dir = \"D:/dataset/original/images\"\nnum_repeats = 2"));
        assert!(
            config.contains("image_dir = \"D:/dataset/derived/portrait/images\"\nnum_repeats = 5")
        );
    }

    #[tokio::test]
    async fn training_presets_are_versioned_on_the_server_and_export_lora_toml() {
        let (application, _state, _directory) = test_router();
        let request = serde_json::json!({
            "name": "Odette baseline",
            "training": {
                "adapter_id": "sdxl-lora",
                "runtime_profile_id": "windows",
                "gpu_ids": ["0"],
                "parameters": {
                    "pretrained_model_name_or_path": "D:/models/base.safetensors",
                    "train_data_dir": "D:/datasets/odette",
                    "output_dir": "D:/outputs",
                    "output_name": "odette"
                }
            }
        });
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/training/presets")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let id = created["data"]["id"].as_str().unwrap();
        assert_eq!(created["data"]["version_count"], 1);

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/training/presets/{id}/toml"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({
                        "name": "Odette tuned",
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": ["0"],
                        "toml": "pretrained_model_name_or_path = \"D:/models/base.safetensors\"\ntrain_data_dir = \"D:/datasets/odette\"\noutput_dir = \"D:/outputs\"\noutput_name = \"odette-v2\"\n"
                    }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let updated: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated["data"]["version_count"], 2);

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/presets/{id}/export"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let exported: Value = serde_json::from_slice(&body).unwrap();
        assert!(exported["data"]["toml"]
            .as_str()
            .unwrap()
            .contains("output_name = \"odette-v2\""));
    }

    #[tokio::test]
    async fn training_artifacts_index_run_files_and_lora_outputs_without_exposing_arbitrary_paths()
    {
        let (application, state, directory) = test_router();
        let output_dir = directory.path().join("lora-output");
        std::fs::create_dir_all(&output_dir).unwrap();
        let task = state
            .tasks
            .create(
                "training",
                serde_json::json!({
                    "type": "training",
                    "training": {
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": ["0"],
                        "parameters": { "output_dir": output_dir }
                    }
                }),
            )
            .unwrap();
        let run_dir = state.training_root.join("runs").join(&task.id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("config.toml"), b"output_name = 'odette'").unwrap();
        std::fs::write(run_dir.join("sample-000001.png"), b"sample").unwrap();
        std::fs::write(output_dir.join("odette.safetensors"), b"lora").unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/tasks/{}/artifacts", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        let artifacts = payload["data"]["artifacts"].as_array().unwrap();
        assert!(artifacts
            .iter()
            .any(|artifact| artifact["kind"] == "sample"));
        assert!(!artifacts
            .iter()
            .any(|artifact| artifact["kind"] == "config"));
        let lora = artifacts
            .iter()
            .find(|artifact| artifact["kind"] == "lora")
            .unwrap();
        let artifact_id = lora["id"].as_str().unwrap();

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/training/tasks/{}/artifacts/{artifact_id}",
                        task.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn training_metrics_query_accepts_single_and_absent_series_values() {
        use axum::extract::FromRequestParts;
        use axum::http::Request;
        use axum::body::Body;
        let cases: [(&str, Option<(Vec<String>, Option<usize>)>); 3] = [
            ("/metrics?series=loss&max_points=1200", Some((vec!["loss".to_string()], Some(1200)))),
            ("/metrics?series=loss/current&max_points=2000", Some((vec!["loss/current".to_string()], Some(2000)))),
            ("/metrics", Some((vec![], None))),
        ];
        for (uri, expected) in cases {
            let request = Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let (mut parts, _) = request.into_parts();
            let parsed = axum::extract::Query::<crate::routes::api::TrainingMetricsQuery>::from_request_parts(&mut parts, &()).await;
            match (parsed, expected) {
                (Ok(parsed), Some((series, points))) => {
                    assert_eq!(parsed.series, series, "uri {uri}");
                    assert_eq!(parsed.max_points, points, "uri {uri}");
                }
                (Err(error), Some(_)) => panic!("Query rejection for {uri}: {error:?}"),
                (Ok(_), None) => panic!("expected rejection for {uri}"),
                (Err(_), None) => {}
            }
        }
    }

    #[test]
    fn safetensors_artifact_step_reads_ss_steps_without_loading_tensor_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("step-0042.safetensors");
        let header = serde_json::json!({
            "__metadata__": { "ss_steps": "42" },
            "lora_unet_block.lora_down.weight": { "dtype": "F32", "shape": [1, 1], "data_offsets": [0, 4] }
        });
        let encoded = serde_json::to_vec(&header).unwrap();
        let mut content = (encoded.len() as u64).to_le_bytes().to_vec();
        content.extend_from_slice(&encoded);
        content.extend_from_slice(&[0_u8; 4]);
        std::fs::write(&path, content).unwrap();

        assert_eq!(safetensors_artifact_step(&path), Some(42));
    }

    #[tokio::test]
    async fn training_metric_snapshot_includes_a_cursor_and_series_overview() {
        let (application, state, _directory) = test_router();
        let task = state
            .tasks
            .create(
                "training",
                serde_json::json!({
                    "type": "training",
                    "training": {
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": [],
                        "parameters": {}
                    }
                }),
            )
            .unwrap();
        let run_dir = state.training_root.join("runs").join(&task.id);
        std::fs::create_dir_all(&run_dir).unwrap();
        let source =
            "{\"step\":1,\"timestamp\":1700000000000,\"metrics\":{\"loss/current\":0.5}}\n";
        std::fs::write(run_dir.join("metrics.jsonl"), source).unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/tasks/{}/metrics", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(payload["data"]["cursor"], source.len() as u64);
        assert_eq!(payload["data"]["metrics"].as_array().unwrap().len(), 1);

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/tasks/{}/metrics/overview", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(payload["data"]["cursor"], source.len() as u64);
        assert_eq!(payload["data"]["series"][0]["series"], "loss/current");
        assert_eq!(payload["data"]["series"][0]["count"], 1);
    }

    #[tokio::test]
    async fn terminal_training_cleanup_removes_owned_outputs_run_data_and_task_record() {
        let (application, state, directory) = test_router();
        let task = state
            .tasks
            .create(
                "training",
                serde_json::json!({
                    "type": "training",
                    "training": {
                        "adapter_id": "sdxl-lora",
                        "runtime_profile_id": "windows",
                        "gpu_ids": [],
                        "parameters": { "output_dir": directory.path().join("outputs") }
                    }
                }),
            )
            .unwrap();
        state.tasks.start(&task.id).unwrap();
        state
            .tasks
            .complete(&task.id, serde_json::json!({}))
            .unwrap();
        let run_dir = state.training_root.join("runs").join(&task.id);
        let output_root_path = directory.path().join("outputs");
        std::fs::create_dir_all(&output_root_path).unwrap();
        let output_root = std::fs::canonicalize(&output_root_path).unwrap();
        let output_dir = output_root.join(&task.id);
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(output_dir.join("model.safetensors"), b"weights").unwrap();
        std::fs::write(run_dir.join("metrics.jsonl"), b"metrics\n").unwrap();
        std::fs::write(
            run_dir.join("artifact-manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "task_id": task.id,
                "output_root": output_root,
                "output_directory": output_dir,
            }))
            .unwrap(),
        )
        .unwrap();

        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/tasks/{}/cleanup-preview", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let preview: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(preview["data"]["deletable"][0]["kind"], "run_data");
        assert!(
            preview["data"]["deletable"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "owned_output"),
            "{preview}"
        );

        let response = application
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/training/tasks/{}", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!run_dir.exists());
        assert!(!output_dir.exists());
        assert!(state.tasks.get(&task.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn training_cleanup_rejects_active_runs_and_preserves_legacy_shared_outputs() {
        let (application, state, directory) = test_router();
        let output_root = directory.path().join("shared-outputs");
        std::fs::create_dir_all(&output_root).unwrap();
        let shared_weight = output_root.join("unrelated.safetensors");
        std::fs::write(&shared_weight, b"keep").unwrap();
        let task = state.tasks.create(
            "training",
            serde_json::json!({
                "type": "training",
                "training": { "adapter_id": "sdxl-lora", "runtime_profile_id": "windows", "gpu_ids": [], "parameters": { "output_dir": output_root } }
            }),
        ).unwrap();
        let active_response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/training/tasks/{}", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(active_response.status(), StatusCode::CONFLICT);

        state.tasks.start(&task.id).unwrap();
        state
            .tasks
            .complete(&task.id, serde_json::json!({}))
            .unwrap();
        let run_dir = state.training_root.join("runs").join(&task.id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("metrics.jsonl"), b"legacy\n").unwrap();
        let preview_response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/tasks/{}/cleanup-preview", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let preview: Value = serde_json::from_slice(
            &to_bytes(preview_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(preview["data"]["retained"][0]["kind"], "unverified_output");

        let delete_response = application
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/training/tasks/{}", task.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::OK);
        assert!(!run_dir.exists());
        assert!(shared_weight.exists());
    }

    #[tokio::test]
    async fn training_path_browser_lists_only_relevant_model_files() {
        let (application, _state, directory) = test_router();
        let files = directory.path().join("model-browser");
        std::fs::create_dir_all(files.join("nested")).unwrap();
        std::fs::write(files.join("base.safetensors"), b"model").unwrap();
        std::fs::write(files.join("notes.txt"), b"notes").unwrap();
        let encoded = url::form_urlencoded::byte_serialize(files.to_string_lossy().as_bytes())
            .collect::<String>();
        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/api/training/paths?kind=model&path={encoded}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["data"]["directories"][0]["name"], "nested");
        assert_eq!(payload["data"]["files"][0]["name"], "base.safetensors");
    }

    #[tokio::test]
    async fn training_path_browser_opens_a_ready_output_directory_by_default() {
        let (application, _state, _directory) = test_router();
        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/training/paths?kind=output")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            payload["data"]["current_path"]
                .as_str()
                .unwrap()
                .ends_with("training\\outputs")
                || payload["data"]["current_path"]
                    .as_str()
                    .unwrap()
                    .ends_with("training/outputs")
        );
    }

#[test]
    fn dataset_caption_samples_shuffle_captions_and_keep_negative_prompt_steps_and_resolution_separate() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("beta.txt"), "second caption\n").unwrap();
        std::fs::write(directory.path().join("alpha.txt"), "first caption\n").unwrap();
        std::fs::write(directory.path().join("gamma.txt"), "third caption\n").unwrap();
        let settings = TrainingSampleSettings {
            enabled: true,
            prompt_source: TrainingSamplePromptSource::DatasetCaptions,
            prompt: String::new(),
            negative_prompt: "low quality, blurry".to_string(),
            dataset_caption_count: 2,
            steps: 30,
            width: 1024,
            height: 768,
            every_n_epochs: 1,
        };

        let lines =
            training_sample_prompt_lines(&settings, Some(directory.path()), ".txt").unwrap();

        assert_eq!(lines.len(), 2);
        let suffix = "--n low quality, blurry --w 1024 --h 768 --s 30";
        let prefixes = lines
            .iter()
            .map(|line| line.split(" --n ").next().unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(prefixes.iter().all(|prefix| matches!(
            *prefix,
            "first caption" | "second caption" | "third caption"
        )), "unexpected sample selection: {lines:?}");
        assert_ne!(prefixes[0], prefixes[1], "sample must not repeat files: {lines:?}");
        assert!(lines.iter().all(|line| line.ends_with(suffix)));
    }

    #[test]
    fn enabled_samples_write_the_prompt_file_under_the_lora_output_samples_directory() {
        let directory = tempfile::tempdir().unwrap();
        let output_dir = directory.path().join("lora-output");
        let settings = TrainingSampleSettings {
            enabled: true,
            prompt_source: TrainingSamplePromptSource::Manual,
            prompt: "portrait of odette".to_string(),
            negative_prompt: "low quality".to_string(),
            dataset_caption_count: 4,
            steps: 30,
            width: 1024,
            height: 1024,
            every_n_epochs: 1,
        };
        let mut parameters = serde_json::json!({
            "output_dir": output_dir.to_string_lossy(),
            "sample_every_n_steps": 20,
        });

        let prompt_file =
            configure_training_samples(&settings, None, ".txt", &mut parameters).unwrap();

        assert_eq!(
            prompt_file,
            output_dir.join("samples").join("sample_prompts.txt")
        );
        assert!(prompt_file.is_file());
        assert_eq!(
            std::fs::read_to_string(&prompt_file).unwrap(),
            "portrait of odette --n low quality --w 1024 --h 1024 --s 30\n"
        );
        assert_eq!(
            parameters["sample_prompts"],
            prompt_file.to_string_lossy().to_string()
        );
        assert_eq!(parameters["sample_every_n_epochs"], 1);
        assert!(parameters.get("sample_every_n_steps").is_none());
    }

    #[test]
    fn training_gpu_process_parser_groups_external_memory_by_gpu_uuid() {
        let processes = super::parse_training_gpu_processes(
            "GPU-aaa, 123, python.exe, 4096\nGPU-aaa, 456, blender.exe, 512\nGPU-bbb, 789, train.py, 2048\n",
        );

        assert_eq!(processes["GPU-aaa"].len(), 2);
        assert_eq!(processes["GPU-aaa"][0].process_name, "python.exe");
        assert_eq!(processes["GPU-bbb"][0].memory_used_mib, 2048);
    }

    #[test]
    fn resumed_download_speed_excludes_bytes_completed_before_this_run() {
        assert_eq!(
            task_run_speed(3_000_000_000_000, 3_001_000_000_000, Duration::from_secs(1),),
            1_000_000_000
        );
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
        tokio::task::yield_now().await;
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
        tokio::task::yield_now().await;
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
        tokio::task::yield_now().await;
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
        tokio::task::yield_now().await;
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
        tokio::task::yield_now().await;
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
    async fn vllm_load_endpoint_rejects_a_non_local_model_endpoint() {
        let (application, state, _directory) = test_router();
        state.settings.write().await.vllm_base_url = "https://vision.example.com/v1".to_string();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/vllm/load")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["code"], "invalid_vllm_launch_endpoint");
    }

    #[tokio::test]
    async fn vllm_unload_endpoint_rejects_a_non_local_model_endpoint() {
        let (application, state, _directory) = test_router();
        state.settings.write().await.vllm_base_url = "https://vision.example.com/v1".to_string();

        let response = application
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/vllm/unload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["code"], "invalid_vllm_launch_endpoint");
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
    async fn pausing_a_task_waiting_for_a_root_lock_releases_its_worker_slot() {
        let (_application, mut state, directory) = test_router();
        state.worker_slots = Arc::new(tokio::sync::Semaphore::new(1));
        for (id, name) in [("root-pause-a", "pause-a"), ("root-pause-b", "pause-b")] {
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
            .acquire(&directory.path().join("pause-a"))
            .await
            .unwrap();
        let lock_b = state
            .root_writes
            .acquire(&directory.path().join("pause-b"))
            .await
            .unwrap();
        let first = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({"type":"index_library","root_id":"root-pause-a"}),
            )
            .unwrap();
        let second = state
            .tasks
            .create(
                "index_library",
                serde_json::json!({"type":"index_library","root_id":"root-pause-b"}),
            )
            .unwrap();
        spawn_task_worker(state.clone(), first.id.clone()).await;
        spawn_task_worker(state.clone(), second.id.clone()).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state.tasks.get(&first.id).unwrap().unwrap().status == TaskStatus::Running
                    && state.tasks.get(&second.id).unwrap().unwrap().status == TaskStatus::Queued
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first task should hold the only worker slot while it waits for the root lock");

        state.tasks.pause(&first.id).unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state.tasks.get(&first.id).unwrap().unwrap().status == TaskStatus::Paused
                    && state.tasks.get(&second.id).unwrap().unwrap().status == TaskStatus::Running
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a paused lock waiter must release its worker slot for the next queued task");
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
    async fn creating_vllm_task_expands_every_library_page_and_honors_exclusions() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("vllm-library-query-items");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "root-vllm-library-query",
                "vLLM library query",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for index in 0..=200 {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: format!("media-{index:03}"),
                    root_id: "root-vllm-library-query".into(),
                    post_id: None,
                    relative_path: format!("{index:03}.jpg"),
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
                            "root_id": "root-vllm-library-query",
                            "options": {
                                "library_query": "",
                                "excluded_media_ids": ["media-042"]
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap();
        let items = state.database.list_task_items(task_id).unwrap();
        assert_eq!(items.len(), 200);
        assert!(items
            .iter()
            .all(|item| item.payload["media_id"] != "media-042"));
        assert!(items
            .iter()
            .any(|item| item.payload["media_id"] == "media-200"));
    }

    #[tokio::test]
    async fn creating_a_library_task_keeps_its_score_and_resolution_filters() {
        let (application, state, directory) = test_router();
        let media_root = directory.path().join("filtered-library-task-items");
        std::fs::create_dir_all(&media_root).unwrap();
        state
            .database
            .create_root(
                "filtered-library-root",
                "Filtered library query",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        for (id, score) in [(201, 3), (202, 7), (203, 7)] {
            state
                .database
                .upsert_post_with_tags(
                    &crate::database::PostRecordInput {
                        id,
                        md5: None,
                        rating: "g".into(),
                        score,
                        fav_count: 0,
                        width: 0,
                        height: 0,
                        file_ext: Some("jpg".into()),
                        file_size: None,
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
        }
        for (id, post_id, width, height) in [
            ("filtered-media-1", 201, 512, 512),
            ("filtered-media-2", 202, 1024, 512),
            ("filtered-media-3", 203, 2048, 1024),
        ] {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "filtered-library-root".into(),
                    post_id: Some(post_id),
                    relative_path: format!("{id}.jpg"),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 7,
                    sha256: None,
                    md5: None,
                    width: Some(width),
                    height: Some(height),
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
                            "root_id": "filtered-library-root",
                            "options": {
                                "library_query": "",
                                "library_score_min": 0,
                                "library_score_max": 9,
                                "library_resolution_min": 512,
                                "library_resolution_max": 1023,
                                "excluded_media_ids": [],
                                "max_size": 1216
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let task_id = json["data"]["id"].as_str().unwrap();
        let items = state.database.list_task_items(task_id).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .all(|item| item.payload["media_id"] != "filtered-media-3"));
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
            training: None,
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
            training: None,
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
            training: None,
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
            training: None,
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
            training: None,
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
            training: None,
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
                    assert_eq!(
                        status,
                        TaskStatus::Completed,
                        "task={:?}, items={:?}",
                        state.tasks.get(task_id).unwrap().unwrap().error,
                        state.database.list_task_items(task_id).unwrap()
                    );
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
    async fn library_items_filter_a_dynamic_score_interval_and_jump_to_a_page() {
        let (application, state, directory) = test_router();
        let media = directory.path().join("score-page-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "score-page-root",
                "Score page library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
            .unwrap();
        for (id, score) in [(101, 3), (102, 7), (103, 48)] {
            state
                .database
                .upsert_post_with_tags(
                    &crate::database::PostRecordInput {
                        id,
                        md5: None,
                        rating: "g".into(),
                        score,
                        fav_count: 0,
                        width: 0,
                        height: 0,
                        file_ext: Some("jpg".into()),
                        file_size: None,
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
        }
        for (id, post_id, width, height) in [
            ("score-media-1", 101, 512, 512),
            ("score-media-2", 102, 1024, 512),
            ("score-media-3", 103, 2048, 1024),
        ] {
            state
                .database
                .upsert_media_file(&crate::database::MediaFileInput {
                    id: id.into(),
                    root_id: "score-page-root".into(),
                    post_id: Some(post_id),
                    relative_path: format!("{id}.jpg"),
                    variant: "original".into(),
                    mime_type: "image/jpeg".into(),
                    byte_size: 4,
                    sha256: None,
                    md5: None,
                    width: Some(width),
                    height: Some(height),
                    duration: None,
                })
                .unwrap();
        }

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/api/library/items?root_id=score-page-root&page=2&limit=1&score_min=0&score_max=9&min_resolution=512")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["data"]["items"][0]["id"], "score-media-2");
        assert_eq!(json["data"]["total"], 2);
        assert_eq!(json["data"]["page"], 2);
        assert_eq!(json["data"]["total_pages"], 2);
        assert_eq!(json["data"]["score_ranges"][0]["score_min"], 0);
        assert_eq!(json["data"]["score_ranges"][0]["score_max"], 9);
        assert_eq!(json["data"]["score_ranges"][0]["count"], 2);
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
    async fn download_writes_same_name_txt_with_danbooru_tags_when_enabled() {
        let (_application, state, directory) = test_router();
        let media = directory.path().join("download-sidecar-media");
        std::fs::create_dir_all(&media).unwrap();
        state
            .database
            .create_root(
                "root-download-sidecar",
                "Library",
                Some(media.to_str().unwrap()),
                Some(media.to_str().unwrap()),
            )
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

        let outcome = download_post(
            &state,
            &client,
            None,
            "root-download-sidecar",
            &media,
            "{id}.{ext}",
            crate::config::UgoiraPolicy::default(),
            true,
            &post,
        )
        .await
        .unwrap();
        server.abort();

        assert!(matches!(outcome, PostDownloadOutcome::Downloaded(_)));
        assert_eq!(
            std::fs::read_to_string(media.join("42.txt")).unwrap(),
            "cat, solo"
        );
    }

    #[tokio::test]
    async fn historical_download_in_the_same_directory_is_skipped_after_its_file_is_deleted() {
        let (_application, state, directory) = test_router();
        let media_root = directory.path().join("historical-directory-downloads");
        let output_directory = media_root.join("characters/alice");
        std::fs::create_dir_all(&output_directory).unwrap();
        state
            .database
            .create_root(
                "root-history",
                "Library",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
            .unwrap();
        state
            .database
            .record_downloaded_post_in_directory(
                "root-history",
                "characters/alice",
                42,
                Some("old-task"),
            )
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

        let outcome = download_post_to_destination(
            &state,
            &client,
            None,
            None,
            None,
            "root-history",
            &DownloadDestination {
                root_dir: media_root.clone(),
                output_dir: output_directory.clone(),
                keep_sidecar_txt: true,
                static_images_only: false,
            },
            "{id}.{ext}",
            crate::config::UgoiraPolicy::default(),
            true,
            &post,
        )
        .await
        .unwrap();
        server.abort();

        assert!(matches!(outcome, PostDownloadOutcome::Skipped));
        assert!(!output_directory.join("42.jpg").exists());
    }

    #[tokio::test]
    async fn download_in_a_different_directory_is_not_skipped_by_another_directorys_media() {
        let (_application, state, directory) = test_router();
        let media_root = directory.path().join("directory-scoped-downloads");
        let first_directory = media_root.join("characters/alice");
        let second_directory = media_root.join("characters/bob");
        std::fs::create_dir_all(&first_directory).unwrap();
        std::fs::create_dir_all(&second_directory).unwrap();
        state
            .database
            .create_root(
                "root-directory-scope",
                "Library",
                Some(media_root.to_str().unwrap()),
                Some(media_root.to_str().unwrap()),
            )
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
        let first_destination = DownloadDestination {
            root_dir: media_root.clone(),
            output_dir: first_directory.clone(),
            keep_sidecar_txt: true,
            static_images_only: false,
        };
        let second_destination = DownloadDestination {
            root_dir: media_root.clone(),
            output_dir: second_directory.clone(),
            keep_sidecar_txt: true,
            static_images_only: false,
        };

        assert!(matches!(
            download_post_to_destination(
                &state,
                &client,
                None,
                None,
                None,
                "root-directory-scope",
                &first_destination,
                "{id}.{ext}",
                crate::config::UgoiraPolicy::default(),
                true,
                &post,
            )
            .await
            .unwrap(),
            PostDownloadOutcome::Downloaded(_)
        ));
        let second = download_post_to_destination(
            &state,
            &client,
            None,
            None,
            None,
            "root-directory-scope",
            &second_destination,
            "{id}.{ext}",
            crate::config::UgoiraPolicy::default(),
            true,
            &post,
        )
        .await
        .unwrap();
        server.abort();

        assert!(matches!(second, PostDownloadOutcome::Downloaded(_)));
        assert!(second_directory.join("42.jpg").exists());
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
