#![allow(dead_code)]

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use utoipa::{OpenApi, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiSuccess<T> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, Value>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<HashMap<String, Value>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiFailure {
    pub error: ApiErrorBody,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Ok,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseHealthState {
    Ok,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthStatus {
    pub status: HealthState,
    pub version: String,
    pub database: DatabaseHealthState,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VllmHealthStatus {
    pub available: bool,
    pub models: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UgoiraPolicy {
    WebmAndZip,
    WebmOnly,
    ZipOnly,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VllmTagMode {
    Overwrite,
    Append,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub enum VllmLanguage {
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "danbooru")]
    Danbooru,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppConfig {
    pub danbooru_username: String,
    pub danbooru_api_key_configured: bool,
    pub vllm_api_key_configured: bool,
    pub vllm_base_url: String,
    pub vllm_allowed_hosts: Vec<String>,
    pub vllm_model: String,
    pub vllm_system_prompt: String,
    pub vllm_tag_mode: VllmTagMode,
    pub vllm_language: VllmLanguage,
    pub vllm_max_tags: usize,
    pub vllm_max_length: usize,
    pub vllm_verify_danbooru: bool,
    pub vllm_reference_existing: bool,
    pub vllm_concurrency: usize,
    pub proxy_url: Option<String>,
    pub download_concurrency: usize,
    pub filename_template: String,
    pub ugoira_policy: UgoiraPolicy,
    pub blur_sensitive_media: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateConfigRequest {
    pub danbooru_username: String,
    pub vllm_base_url: String,
    pub vllm_allowed_hosts: Vec<String>,
    pub vllm_model: String,
    pub vllm_system_prompt: String,
    pub vllm_tag_mode: VllmTagMode,
    pub vllm_language: VllmLanguage,
    pub vllm_max_tags: usize,
    pub vllm_max_length: usize,
    pub vllm_verify_danbooru: bool,
    pub vllm_reference_existing: bool,
    pub vllm_concurrency: usize,
    pub proxy_url: Option<String>,
    pub download_concurrency: usize,
    pub filename_template: String,
    pub ugoira_policy: UgoiraPolicy,
    pub blur_sensitive_media: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretRequest {
    pub secret: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretStorage {
    System,
    Session,
    None,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SecretResponse {
    pub configured: bool,
    pub storage: SecretStorage,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaRoot {
    pub id: String,
    pub name: String,
    pub windows_path: Option<String>,
    pub linux_path: Option<String>,
    pub indexed: bool,
    pub media_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SaveMediaRootRequest {
    pub name: String,
    pub windows_path: Option<String>,
    pub linux_path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RootRemoval {
    pub id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaDirectoryList {
    pub directories: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateMediaDirectoryRequest {
    pub relative_path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaDirectory {
    pub relative_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentRating {
    G,
    S,
    Q,
    E,
    Unknown,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LocalMedia {
    pub id: String,
    pub root_id: String,
    pub post_id: Option<u64>,
    pub filename: String,
    pub relative_path: String,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub size_bytes: u64,
    pub rating: Option<ContentRating>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LibraryPage {
    pub items: Vec<LocalMedia>,
    pub next_cursor: Option<String>,
    pub total: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QuarantineEntry {
    pub id: String,
    pub root_id: String,
    pub original_relative_path: String,
    pub quarantine_relative_path: String,
    pub size_bytes: u64,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PurgeResponse {
    pub purged: usize,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Download,
    IndexLibrary,
    IntegrityScan,
    ExactDedup,
    NearDedup,
    Resize,
    HeicConvert,
    DeleteByTag,
    TagPipeline,
    VllmTag,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Pausing,
    Paused,
    Cancelling,
    AwaitingConfirmation,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskFailure {
    pub item_id: Option<String>,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskProgress {
    pub completed: u64,
    pub total: u64,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskPreviewCandidate {
    pub relative_path: String,
    pub companion_paths: Option<Vec<String>>,
    pub reason: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NearDuplicatePair {
    pub left: String,
    pub right: String,
    pub distance: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskPreview {
    pub candidates: Option<Vec<TaskPreviewCandidate>>,
    pub pairs: Option<Vec<NearDuplicatePair>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskSummary {
    pub id: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub revision: u64,
    pub title: String,
    pub progress: TaskProgress,
    pub failures: Vec<TaskFailure>,
    pub preview: Option<TaskPreview>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskSnapshot {
    pub tasks: Vec<TaskSummary>,
    pub last_event_id: u64,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventType {
    Created,
    Updated,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskEvent {
    pub sequence: u64,
    pub task_id: String,
    pub revision: u64,
    pub event_type: TaskEventType,
    pub task: TaskSummary,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskItemStatus {
    Queued,
    Completed,
    Skipped,
    Failed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskItem {
    pub item_id: String,
    pub post_id: Option<u64>,
    pub status: TaskItemStatus,
    pub attempts: u64,
    pub result: Option<Value>,
    pub error: Option<TaskFailure>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskItemCounts {
    pub total: u64,
    pub queued: u64,
    pub completed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub retryable_failed: u64,
    pub completed_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TaskDetails {
    pub task: TaskSummary,
    pub result: Option<Value>,
    pub item_counts: TaskItemCounts,
    pub items: Vec<TaskItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DownloadSource {
    Query { query: String },
    PostIds { post_ids: Vec<u64> },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaPolicy {
    pub original: bool,
    pub ugoira: UgoiraPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadTaskType {
    Download,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadTaskRequest {
    #[serde(rename = "type")]
    pub task_type: DownloadTaskType,
    pub source: DownloadSource,
    pub root_id: String,
    pub relative_directory: Option<String>,
    pub limit: u64,
    pub concurrency: u16,
    pub filename_template: String,
    pub skip_existing: bool,
    pub keep_sidecar_txt: Option<bool>,
    pub static_images_only: Option<bool>,
    pub prioritize_score: Option<bool>,
    pub prioritize_resolution: Option<bool>,
    pub batch_filter: Option<BatchDownloadFilter>,
    pub media_policy: MediaPolicy,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchDownloadFilter {
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub minimum_score: i64,
    pub minimum_resolution: u32,
}

macro_rules! literal_task_type {
    ($name:ident, $variant:ident) => {
        #[derive(Debug, Clone, Copy, Serialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $variant,
        }
    };
}

literal_task_type!(IndexLibraryTaskType, IndexLibrary);
literal_task_type!(IntegrityScanTaskType, IntegrityScan);
literal_task_type!(ExactDedupTaskType, ExactDedup);
literal_task_type!(NearDedupTaskType, NearDedup);
literal_task_type!(ResizeTaskType, Resize);
literal_task_type!(HeicConvertTaskType, HeicConvert);
literal_task_type!(DeleteByTagTaskType, DeleteByTag);
literal_task_type!(TagPipelineTaskType, TagPipeline);
literal_task_type!(VllmTagTaskType, VllmTag);

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaIdsTaskOptions {
    pub media_ids: Option<Vec<String>>,
    pub relative_directory: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreflightTaskOptions {
    pub preflight: Option<bool>,
    pub relative_directory: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResizeTaskOptions {
    pub media_ids: Option<Vec<String>>,
    pub relative_directory: Option<String>,
    pub max_size: Option<u32>,
    pub quality: Option<u8>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NearDedupTaskOptions {
    pub preflight: Option<bool>,
    pub distance: Option<u32>,
    pub relative_directory: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteByTagTaskOptions {
    pub preflight: Option<bool>,
    pub tag: String,
    pub relative_directory: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtistPrefix {
    Artist,
    At,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TagPipelineTaskOptions {
    pub media_ids: Option<Vec<String>>,
    pub relative_directory: Option<String>,
    pub artist_prefix: Option<ArtistPrefix>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IndexLibraryTaskRequest {
    #[serde(rename = "type")]
    pub task_type: IndexLibraryTaskType,
    pub root_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IntegrityScanTaskRequest {
    #[serde(rename = "type")]
    pub task_type: IntegrityScanTaskType,
    pub root_id: String,
    pub options: Option<PreflightTaskOptions>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExactDedupTaskRequest {
    #[serde(rename = "type")]
    pub task_type: ExactDedupTaskType,
    pub root_id: String,
    pub options: Option<PreflightTaskOptions>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NearDedupTaskRequest {
    #[serde(rename = "type")]
    pub task_type: NearDedupTaskType,
    pub root_id: String,
    pub options: Option<NearDedupTaskOptions>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResizeTaskRequest {
    #[serde(rename = "type")]
    pub task_type: ResizeTaskType,
    pub root_id: String,
    pub options: ResizeTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HeicConvertTaskRequest {
    #[serde(rename = "type")]
    pub task_type: HeicConvertTaskType,
    pub root_id: String,
    pub options: MediaIdsTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteByTagTaskRequest {
    #[serde(rename = "type")]
    pub task_type: DeleteByTagTaskType,
    pub root_id: String,
    pub options: DeleteByTagTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TagPipelineTaskRequest {
    #[serde(rename = "type")]
    pub task_type: TagPipelineTaskType,
    pub root_id: String,
    pub options: TagPipelineTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VllmTagTaskRequest {
    #[serde(rename = "type")]
    pub task_type: VllmTagTaskType,
    pub root_id: String,
    pub options: MediaIdsTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(discriminator(
    property_name = "type",
    mapping(
        ("download" = "#/components/schemas/DownloadTaskRequest"),
        ("index_library" = "#/components/schemas/IndexLibraryTaskRequest"),
        ("integrity_scan" = "#/components/schemas/IntegrityScanTaskRequest"),
        ("exact_dedup" = "#/components/schemas/ExactDedupTaskRequest"),
        ("near_dedup" = "#/components/schemas/NearDedupTaskRequest"),
        ("resize" = "#/components/schemas/ResizeTaskRequest"),
        ("heic_convert" = "#/components/schemas/HeicConvertTaskRequest"),
        ("delete_by_tag" = "#/components/schemas/DeleteByTagTaskRequest"),
        ("tag_pipeline" = "#/components/schemas/TagPipelineTaskRequest"),
        ("vllm_tag" = "#/components/schemas/VllmTagTaskRequest")
    )
))]
pub enum CreateTaskRequest {
    Download(DownloadTaskRequest),
    IndexLibrary(IndexLibraryTaskRequest),
    IntegrityScan(IntegrityScanTaskRequest),
    ExactDedup(ExactDedupTaskRequest),
    NearDedup(NearDedupTaskRequest),
    Resize(ResizeTaskRequest),
    HeicConvert(HeicConvertTaskRequest),
    DeleteByTag(DeleteByTagTaskRequest),
    TagPipeline(TagPipelineTaskRequest),
    VllmTag(VllmTagTaskRequest),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadHistoryRecord {
    pub id: String,
    pub task_id: String,
    pub status: TaskStatus,
    pub source_label: String,
    pub root_name: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub duration_seconds: Option<u64>,
    pub total_items: u64,
    pub completed_items: u64,
    pub skipped_items: u64,
    pub failed_items: u64,
    pub bytes_processed: u64,
    pub error_message: Option<String>,
    pub can_repeat: bool,
    pub repeat_request: Option<CreateTaskRequest>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DownloadHistoryPage {
    pub items: Vec<DownloadHistoryRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DanbooruTags {
    pub general: Vec<String>,
    pub artist: Vec<String>,
    pub copyright: Vec<String>,
    pub character: Vec<String>,
    pub meta: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DanbooruPost {
    pub id: u64,
    pub rating: ContentRating,
    pub score: i64,
    pub fav_count: u64,
    pub image_width: u32,
    pub image_height: u32,
    pub file_ext: String,
    pub file_size: u64,
    pub duration: Option<f64>,
    pub source: Option<String>,
    pub is_video: bool,
    pub is_ugoira: bool,
    pub restricted: bool,
    pub downloaded: bool,
    pub tags: DanbooruTags,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DanbooruPostsPage {
    pub posts: Vec<DanbooruPost>,
    pub page: u64,
    pub next_page: Option<String>,
    pub previous_page: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DanbooruCount {
    pub count: u64,
    pub exact: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TagCategory {
    General,
    Artist,
    Copyright,
    Character,
    Meta,
    Query,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TagSuggestion {
    pub value: String,
    pub label: String,
    pub category: TagCategory,
    pub post_count: Option<u64>,
}

mod paths {
    use super::*;

    #[utoipa::path(get, path = "/api/health", responses((status = 200, body = ApiSuccess<HealthStatus>), (status = 503, body = ApiFailure)))]
    pub async fn health() {}

    #[utoipa::path(get, path = "/api/vllm/health", responses((status = 200, body = ApiSuccess<VllmHealthStatus>), (status = 400, body = ApiFailure)))]
    pub async fn vllm_health() {}

    #[utoipa::path(get, path = "/api/config", responses((status = 200, body = ApiSuccess<AppConfig>)))]
    pub async fn get_config() {}

    #[utoipa::path(put, path = "/api/config", request_body = UpdateConfigRequest, responses((status = 200, body = ApiSuccess<AppConfig>), (status = 400, body = ApiFailure)))]
    pub async fn update_config() {}

    #[utoipa::path(put, path = "/api/secrets/{kind}", request_body = SecretRequest, params(("kind" = String, Path)), responses((status = 200, body = ApiSuccess<SecretResponse>), (status = 404, body = ApiFailure)))]
    pub async fn put_secret() {}

    #[utoipa::path(delete, path = "/api/secrets/{kind}", params(("kind" = String, Path)), responses((status = 200, body = ApiSuccess<SecretResponse>), (status = 404, body = ApiFailure)))]
    pub async fn delete_secret() {}

    #[utoipa::path(get, path = "/api/library/roots", responses((status = 200, body = ApiSuccess<Vec<MediaRoot>>)))]
    pub async fn list_roots() {}

    #[utoipa::path(post, path = "/api/library/roots", request_body = SaveMediaRootRequest, responses((status = 201, body = ApiSuccess<MediaRoot>), (status = 400, body = ApiFailure)))]
    pub async fn create_root() {}

    #[utoipa::path(put, path = "/api/library/roots/{id}", request_body = SaveMediaRootRequest, params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<MediaRoot>), (status = 404, body = ApiFailure)))]
    pub async fn update_root() {}

    #[utoipa::path(delete, path = "/api/library/roots/{id}", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<RootRemoval>), (status = 404, body = ApiFailure), (status = 409, body = ApiFailure)))]
    pub async fn delete_root() {}

    #[utoipa::path(get, path = "/api/library/roots/{id}/directories", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<MediaDirectoryList>), (status = 404, body = ApiFailure)))]
    pub async fn list_root_directories() {}

    #[utoipa::path(post, path = "/api/library/roots/{id}/directories", request_body = CreateMediaDirectoryRequest, params(("id" = String, Path)), responses((status = 201, body = ApiSuccess<MediaDirectory>), (status = 400, body = ApiFailure), (status = 404, body = ApiFailure)))]
    pub async fn create_root_directory() {}

    #[utoipa::path(get, path = "/api/library/items", params(("root_id" = String, Query), ("q" = Option<String>, Query), ("cursor" = Option<String>, Query), ("limit" = Option<usize>, Query)), responses((status = 200, body = ApiSuccess<LibraryPage>), (status = 400, body = ApiFailure)))]
    pub async fn list_library_items() {}

    #[utoipa::path(get, path = "/api/library/items/{id}", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<LocalMedia>), (status = 404, body = ApiFailure)))]
    pub async fn library_item() {}

    #[utoipa::path(get, path = "/api/library/media/{id}/{variant}", params(("id" = String, Path), ("variant" = String, Path)), responses((status = 200, content_type = "application/octet-stream"), (status = 206, content_type = "application/octet-stream"), (status = 404, body = ApiFailure)))]
    pub async fn library_media() {}

    #[utoipa::path(get, path = "/api/library/quarantine", params(("root_id" = String, Query)), responses((status = 200, body = ApiSuccess<Vec<QuarantineEntry>>)))]
    pub async fn list_quarantine() {}

    #[utoipa::path(delete, path = "/api/library/quarantine", params(("root_id" = String, Query)), responses((status = 200, body = ApiSuccess<PurgeResponse>), (status = 400, body = ApiFailure)))]
    pub async fn purge_quarantine() {}

    #[utoipa::path(post, path = "/api/library/quarantine/{id}/restore", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<QuarantineEntry>), (status = 409, body = ApiFailure)))]
    pub async fn restore_quarantine() {}

    #[utoipa::path(get, path = "/api/tasks", responses((status = 200, body = ApiSuccess<TaskSnapshot>)))]
    pub async fn list_tasks() {}

    #[utoipa::path(post, path = "/api/tasks", request_body = CreateTaskRequest, responses((status = 201, body = ApiSuccess<TaskSummary>), (status = 400, body = ApiFailure)))]
    pub async fn create_task() {}

    #[utoipa::path(get, path = "/api/tasks/{id}", params(("id" = String, Path), ("item_status" = Option<TaskItemStatus>, Query), ("item_cursor" = Option<String>, Query), ("item_limit" = Option<usize>, Query)), responses((status = 200, body = ApiSuccess<TaskDetails>), (status = 404, body = ApiFailure)))]
    pub async fn task_detail() {}

    #[utoipa::path(post, path = "/api/tasks/{id}/{action}", params(("id" = String, Path), ("action" = String, Path)), responses((status = 200, body = ApiSuccess<TaskSummary>), (status = 409, body = ApiFailure)))]
    pub async fn task_action() {}

    #[utoipa::path(get, path = "/api/tasks/events", params(("after" = Option<u64>, Query), ("Last-Event-ID" = Option<u64>, Header)), responses((status = 200, body = String, content_type = "text/event-stream")))]
    pub async fn task_events() {}

    #[utoipa::path(get, path = "/api/downloads/history", params(("cursor" = Option<String>, Query), ("limit" = Option<usize>, Query)), responses((status = 200, body = ApiSuccess<DownloadHistoryPage>), (status = 400, body = ApiFailure)))]
    pub async fn download_history() {}

    #[utoipa::path(get, path = "/api/danbooru/posts", params(("q" = String, Query), ("page" = Option<String>, Query), ("limit" = Option<u16>, Query)), responses((status = 200, body = ApiSuccess<DanbooruPostsPage>), (status = 502, body = ApiFailure)))]
    pub async fn danbooru_posts() {}

    #[utoipa::path(get, path = "/api/danbooru/posts/{id}", params(("id" = u64, Path)), responses((status = 200, body = ApiSuccess<DanbooruPost>), (status = 404, body = ApiFailure)))]
    pub async fn danbooru_post() {}

    #[utoipa::path(get, path = "/api/danbooru/posts/{id}/media/{variant}", params(("id" = u64, Path), ("variant" = String, Path)), responses((status = 200, content_type = "application/octet-stream"), (status = 206, content_type = "application/octet-stream"), (status = 404, body = ApiFailure)))]
    pub async fn danbooru_media() {}

    #[utoipa::path(get, path = "/api/danbooru/autocomplete", params(("q" = String, Query)), responses((status = 200, body = ApiSuccess<Vec<TagSuggestion>>), (status = 502, body = ApiFailure)))]
    pub async fn danbooru_autocomplete() {}

    #[utoipa::path(get, path = "/api/danbooru/count", params(("q" = String, Query)), responses((status = 200, body = ApiSuccess<DanbooruCount>), (status = 502, body = ApiFailure)))]
    pub async fn danbooru_count() {}
}

#[derive(OpenApi)]
#[openapi(
    info(title = "DanbooruDownload Tool Pro API", version = "2.0.0"),
    paths(
        paths::health,
        paths::vllm_health,
        paths::get_config,
        paths::update_config,
        paths::put_secret,
        paths::delete_secret,
        paths::list_roots,
        paths::create_root,
        paths::update_root,
        paths::delete_root,
        paths::list_root_directories,
        paths::create_root_directory,
        paths::list_library_items,
        paths::library_item,
        paths::library_media,
        paths::list_quarantine,
        paths::purge_quarantine,
        paths::restore_quarantine,
        paths::list_tasks,
        paths::create_task,
        paths::task_detail,
        paths::task_action,
        paths::task_events,
        paths::download_history,
        paths::danbooru_posts,
        paths::danbooru_post,
        paths::danbooru_media,
        paths::danbooru_autocomplete,
        paths::danbooru_count,
    ),
    components(schemas(
        AppConfig,
        UpdateConfigRequest,
        SecretResponse,
        MediaRoot,
        SaveMediaRootRequest,
        RootRemoval,
        MediaDirectoryList,
        CreateMediaDirectoryRequest,
        MediaDirectory,
        LocalMedia,
        LibraryPage,
        QuarantineEntry,
        TaskSummary,
        TaskSnapshot,
        TaskEvent,
        TaskItem,
        TaskItemCounts,
        TaskDetails,
        CreateTaskRequest,
        DownloadHistoryRecord,
        DownloadHistoryPage,
        DanbooruPost,
        DanbooruPostsPage,
        DanbooruCount,
        TagSuggestion,
        HealthStatus,
        VllmHealthStatus,
        ApiFailure,
    ))
)]
pub struct ApiDoc;

pub fn document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

pub fn export_document(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec_pretty(&document())?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_contract_covers_every_public_router_operation() {
        let document = document();
        assert_eq!(document.paths.paths.len(), 22);
        let operation_count = document
            .paths
            .paths
            .values()
            .map(|item| {
                [
                    item.get.as_ref(),
                    item.put.as_ref(),
                    item.post.as_ref(),
                    item.delete.as_ref(),
                    item.options.as_ref(),
                    item.head.as_ref(),
                    item.patch.as_ref(),
                    item.trace.as_ref(),
                ]
                .into_iter()
                .flatten()
                .count()
            })
            .sum::<usize>();
        assert_eq!(operation_count, 29);
    }

    #[test]
    fn editable_config_schema_cannot_accept_secret_status_or_values() {
        let json = serde_json::to_value(document()).unwrap();
        let properties = &json["components"]["schemas"]["UpdateConfigRequest"]["properties"];
        assert!(properties.get("danbooru_api_key_configured").is_none());
        assert!(properties.get("vllm_api_key_configured").is_none());
        assert!(properties.get("api_key").is_none());
        assert!(properties.get("secret").is_none());
        assert!(properties.get("legacy_media_path_suggestion").is_none());
    }

    #[test]
    fn task_creation_contract_is_a_discriminated_union_with_literal_types() {
        let json = serde_json::to_value(document()).unwrap();
        let schemas = &json["components"]["schemas"];
        assert_eq!(
            schemas["CreateTaskRequest"]["discriminator"]["propertyName"],
            "type"
        );
        assert_eq!(
            schemas["DownloadTaskType"]["enum"],
            serde_json::json!(["download"])
        );
        assert_eq!(
            schemas["IndexLibraryTaskType"]["enum"],
            serde_json::json!(["index_library"])
        );
        assert_eq!(
            schemas["VllmTagTaskType"]["enum"],
            serde_json::json!(["vllm_tag"])
        );
    }

    #[test]
    fn every_tool_task_has_a_distinct_typed_contract() {
        let json = serde_json::to_value(document()).unwrap();
        let schemas = &json["components"]["schemas"];
        let mapping = &schemas["CreateTaskRequest"]["discriminator"]["mapping"];

        assert_eq!(mapping["resize"], "#/components/schemas/ResizeTaskRequest");
        assert_eq!(
            mapping["vllm_tag"],
            "#/components/schemas/VllmTagTaskRequest"
        );
        assert_eq!(
            schemas["ResizeTaskRequest"]["properties"]["options"]["$ref"],
            "#/components/schemas/ResizeTaskOptions"
        );
        assert_eq!(
            schemas["VllmTagTaskRequest"]["properties"]["options"]["$ref"],
            "#/components/schemas/MediaIdsTaskOptions"
        );
        assert!(schemas.get("RootTaskRequest").is_none());
    }

    #[test]
    fn health_contract_uses_literal_ok_states() {
        let json = serde_json::to_value(document()).unwrap();
        let schemas = &json["components"]["schemas"];
        assert_eq!(schemas["HealthState"]["enum"], serde_json::json!(["ok"]));
        assert_eq!(
            schemas["DatabaseHealthState"]["enum"],
            serde_json::json!(["ok"])
        );
    }
}
