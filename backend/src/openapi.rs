#![allow(dead_code)]

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
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

#[derive(Debug, Serialize, ToSchema)]
pub struct VllmLoadStatus {
    pub state: String,
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
    pub page: u64,
    pub total_pages: u64,
    pub score_ranges: Vec<LibraryScoreRange>,
    pub resolution_ranges: Vec<LibraryResolutionRange>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LibraryScoreRange {
    pub score_min: i64,
    pub score_max: i64,
    pub count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LibraryResolutionRange {
    pub resolution_min: i64,
    pub resolution_max: i64,
    pub count: u64,
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
    DeleteSelected,
    TagPipeline,
    VllmTag,
    DatasetAugmentation,
    Training,
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
    pub training: Option<TrainingTaskSummary>,
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
    Deleted,
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
literal_task_type!(DeleteSelectedTaskType, DeleteSelected);
literal_task_type!(TagPipelineTaskType, TagPipeline);
literal_task_type!(VllmTagTaskType, VllmTag);
literal_task_type!(DatasetAugmentationTaskType, DatasetAugmentation);
literal_task_type!(TrainingTaskType, Training);

#[derive(Debug, Serialize, ToSchema)]
pub struct MediaIdsTaskOptions {
    pub media_ids: Option<Vec<String>>,
    pub relative_directory: Option<String>,
    pub library_query: Option<String>,
    pub excluded_media_ids: Option<Vec<String>>,
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
    pub library_query: Option<String>,
    pub excluded_media_ids: Option<Vec<String>>,
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
    pub library_query: Option<String>,
    pub excluded_media_ids: Option<Vec<String>>,
    pub artist_prefix: Option<ArtistPrefix>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DatasetAugmentationTaskOptions {
    pub media_ids: Option<Vec<String>>,
    pub relative_directory: Option<String>,
    pub library_query: Option<String>,
    pub excluded_media_ids: Option<Vec<String>>,
    pub output_directory: Option<String>,
    pub min_megapixels: Option<f64>,
    pub min_long_side: Option<u32>,
    pub min_short_side: Option<u32>,
    pub horizontal_flip: Option<bool>,
    pub train_percent: Option<u8>,
    pub validation_percent: Option<u8>,
    pub test_percent: Option<u8>,
    pub jpeg_quality: Option<u8>,
    pub smart_crop: Option<SmartCropTaskOptions>,
    pub retagging: Option<DerivedRetaggingTaskOptions>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SmartCropTaskOptions {
    pub enabled: Option<bool>,
    pub runtime_profile_id: Option<String>,
    pub gpu_id: Option<String>,
    pub quality_profile: Option<String>,
    pub portrait: Option<bool>,
    pub upper_body: Option<bool>,
    pub full_body_tight: Option<bool>,
    pub max_derived_per_family: Option<u8>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DerivedRetaggingTaskOptions {
    pub send_to_vllm: Option<bool>,
    pub preserve_artist_character_tags: Option<bool>,
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
pub struct DeleteSelectedTaskRequest {
    #[serde(rename = "type")]
    pub task_type: DeleteSelectedTaskType,
    pub root_id: String,
    pub options: MediaIdsTaskOptions,
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
pub struct DatasetAugmentationTaskRequest {
    #[serde(rename = "type")]
    pub task_type: DatasetAugmentationTaskType,
    pub root_id: String,
    pub options: DatasetAugmentationTaskOptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingGalleryDataset {
    pub root_id: String,
    pub relative_directory: String,
    pub repeats: u32,
    pub caption_extension: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrainingSamplePromptSource {
    Manual,
    DatasetCaptions,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingSampleSettings {
    pub enabled: bool,
    pub prompt_source: TrainingSamplePromptSource,
    pub prompt: String,
    pub negative_prompt: String,
    pub dataset_caption_count: u32,
    pub steps: u32,
    pub width: u32,
    pub height: u32,
    pub every_n_epochs: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingRunRequest {
    pub adapter_id: String,
    pub runtime_profile_id: String,
    pub parameters: Value,
    pub gpu_ids: Vec<String>,
    pub gallery_dataset: Option<TrainingGalleryDataset>,
    pub gallery_datasets: Vec<TrainingGalleryDataset>,
    pub sample: Option<TrainingSampleSettings>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingTaskRequest {
    #[serde(rename = "type")]
    pub task_type: TrainingTaskType,
    pub root_id: String,
    pub training: TrainingRunRequest,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingTaskSummary {
    pub adapter_id: String,
    pub runtime_profile_id: String,
    pub gpu_ids: Vec<String>,
    pub model_path: Option<String>,
    pub train_data_dir: Option<String>,
    pub output_dir: Option<String>,
    pub output_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingAdapterField {
    pub key: String,
    pub label: String,
    pub group: String,
    pub kind: String,
    pub default: Value,
    pub choices: Vec<String>,
    pub required: bool,
    pub advanced: bool,
    pub help: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingAdapterGroup {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingAdapterResponse {
    pub id: String,
    pub version: String,
    pub label: String,
    pub family: String,
    pub family_label: String,
    pub training_type: String,
    pub training_type_label: String,
    pub trainer: String,
    pub fields: Vec<TrainingAdapterField>,
    pub groups: Vec<TrainingAdapterGroup>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingRuntimeProfile {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub managed: bool,
    pub installed: bool,
    pub installing: bool,
    pub last_error: Option<String>,
    pub runtime_root: String,
    pub python_path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingRuntimeCheck {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingRuntimeDiagnostics {
    pub profile: TrainingRuntimeProfile,
    pub checks: Vec<TrainingRuntimeCheck>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VisionCropRuntimeHealth {
    pub runtime_profile_id: String,
    pub python_path: String,
    pub ready: bool,
    pub installing: bool,
    pub gpu_id: String,
    pub providers: Vec<String>,
    pub gpu_name: Option<String>,
    pub models_ready: bool,
    pub message: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingGpuExternalProcess {
    pub pid: u64,
    pub process_name: String,
    pub memory_used_mib: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingGpu {
    pub id: String,
    pub name: String,
    pub memory_total_mib: u64,
    pub memory_used_mib: u64,
    pub utilization_percent: u64,
    pub graphics_clock_mhz: Option<u64>,
    pub memory_clock_mhz: Option<u64>,
    pub power_draw_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temperature_c: Option<u64>,
    pub fan_speed_percent: Option<u64>,
    pub external_processes: Vec<TrainingGpuExternalProcess>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingMetric {
    pub step: u64,
    pub timestamp: u64,
    pub series: String,
    pub value: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingMetricsResponse {
    pub metrics: Vec<TrainingMetric>,
    pub cursor: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingMetricSeriesSummary {
    pub series: String,
    pub count: u64,
    pub first: TrainingMetric,
    pub latest: TrainingMetric,
    pub minimum: TrainingMetric,
    pub maximum: TrainingMetric,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingMetricsOverviewResponse {
    pub cursor: u64,
    pub series: Vec<TrainingMetricSeriesSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingCleanupPath {
    pub kind: String,
    pub path: String,
    pub file_count: u64,
    pub bytes: u64,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingCleanupPreviewResponse {
    pub deletable: Vec<TrainingCleanupPath>,
    pub retained: Vec<TrainingCleanupPath>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingCleanupResponse {
    pub task_id: String,
    pub deleted: Vec<TrainingCleanupPath>,
    pub retained: Vec<TrainingCleanupPath>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingLogsResponse {
    pub text: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingQueueEntry {
    pub task_id: String,
    pub status: String,
    pub adapter_id: String,
    pub runtime_profile_id: String,
    pub gpu_ids: Vec<String>,
    pub assigned_gpu_ids: Vec<String>,
    pub queue_position: Option<u64>,
    pub blocking_task_ids: Vec<String>,
    pub blocked_gpu_ids: Vec<String>,
    pub estimated_wait_seconds: Option<u64>,
    pub wait_reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingQueueResponse {
    pub entries: Vec<TrainingQueueEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingGalleryDatasetResponse {
    pub root_id: String,
    pub root_name: String,
    pub relative_directory: String,
    pub image_dir: String,
    pub caption_extension: String,
    pub image_count: u64,
    pub caption_count: u64,
    pub repeats: u32,
    pub effective_image_count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPathEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPathBrowserResponse {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub directories: Vec<TrainingPathEntry>,
    pub files: Vec<TrainingPathEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPresetInput {
    pub name: String,
    pub training: TrainingRunRequest,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPresetResponse {
    pub id: String,
    pub name: String,
    pub training: TrainingRunRequest,
    pub created_at: u64,
    pub updated_at: u64,
    pub version_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPresetImportRequest {
    pub name: String,
    pub adapter_id: Option<String>,
    pub runtime_profile_id: Option<String>,
    pub gpu_ids: Option<Vec<String>>,
    pub toml: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPresetExportResponse {
    pub name: String,
    pub toml: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingArtifact {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: u64,
    pub step: Option<u64>,
    pub url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingArtifactsResponse {
    pub artifacts: Vec<TrainingArtifact>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPreviewRequest {
    pub adapter_id: String,
    pub parameters: Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrainingPreviewResponse {
    pub toml: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdAnalysisFileRequest {
    pub path: String,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdAnalysisRequest {
    pub runtime_profile_id: String,
    pub files: Vec<LoraSvdAnalysisFileRequest>,
    /// Currently only `auto` is accepted.
    pub device: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdThresholdRanks {
    pub energy_95: u64,
    pub energy_99: u64,
    pub energy_999: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdCoverage {
    pub analyzed_modules: u64,
    pub candidate_modules: u64,
    pub unsupported_modules: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdRankDistribution {
    pub minimum: u64,
    pub maximum: u64,
    pub modal: u64,
    pub uniform: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdExcludedModule {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdModule {
    pub id: String,
    pub component: String,
    pub rank: u64,
    pub alpha: f64,
    pub scale: f64,
    pub numerical_rank: u64,
    pub stable_rank: f64,
    pub tail_energy_20: f64,
    pub effective_rank: LoraSvdThresholdRanks,
    pub energy: f64,
    pub flag: Option<String>,
    /// Returned from the module detail/export endpoints; omitted from the initial summary.
    pub singular_values: Option<Vec<f64>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdModelReport {
    pub id: String,
    pub label: String,
    pub path: String,
    pub file_size_bytes: u64,
    pub sha256: String,
    pub modified_at: u64,
    pub step: Option<u64>,
    pub architecture: String,
    pub format: String,
    /// Whether a standard LoRA factor-pair QR-SVD is mathematically applicable.
    pub svd_applicable: bool,
    pub coverage: LoraSvdCoverage,
    pub rank_distribution: LoraSvdRankDistribution,
    pub effective_rank: LoraSvdThresholdRanks,
    pub current_rank_energy: f64,
    pub tail_energy_20: f64,
    pub verdict: String,
    pub verdict_message: String,
    pub metadata: BTreeMap<String, String>,
    pub excluded: Vec<LoraSvdExcludedModule>,
    pub modules: Vec<LoraSvdModule>,
    /// Full point count before the initial response is reduced for interactive rendering.
    pub global_singular_values_count: Option<u64>,
    pub global_singular_values: Vec<f64>,
    /// Full point count before the initial response is reduced for interactive rendering.
    pub global_cumulative_energy_count: Option<u64>,
    pub global_cumulative_energy: Vec<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdExecution {
    pub device: String,
    pub reason: String,
    pub selection_reason: Option<String>,
    pub duration_ms: u64,
    pub fallback: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdComparisonCheckpoint {
    pub id: String,
    pub label: String,
    pub step: Option<u64>,
    pub effective_rank: LoraSvdThresholdRanks,
    pub rank_utilization: f64,
    pub tail_energy_20: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdComparison {
    pub comparable: bool,
    pub reason: String,
    pub checkpoints: Vec<LoraSvdComparisonCheckpoint>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoraSvdAnalysisResult {
    pub id: String,
    pub reports: Vec<LoraSvdModelReport>,
    pub comparison: Option<LoraSvdComparison>,
    pub execution: LoraSvdExecution,
    pub expires_at: u64,
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
        ("delete_selected" = "#/components/schemas/DeleteSelectedTaskRequest"),
        ("tag_pipeline" = "#/components/schemas/TagPipelineTaskRequest"),
        ("vllm_tag" = "#/components/schemas/VllmTagTaskRequest"),
        ("dataset_augmentation" = "#/components/schemas/DatasetAugmentationTaskRequest"),
        ("training" = "#/components/schemas/TrainingTaskRequest")
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
    DeleteSelected(DeleteSelectedTaskRequest),
    TagPipeline(TagPipelineTaskRequest),
    VllmTag(VllmTagTaskRequest),
    DatasetAugmentation(DatasetAugmentationTaskRequest),
    Training(TrainingTaskRequest),
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

    #[utoipa::path(post, path = "/api/vllm/load", responses((status = 200, body = ApiSuccess<VllmLoadStatus>), (status = 400, body = ApiFailure), (status = 500, body = ApiFailure)))]
    pub async fn vllm_load() {}

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

    #[utoipa::path(get, path = "/api/library/items", params(("root_id" = String, Query), ("q" = Option<String>, Query), ("cursor" = Option<String>, Query), ("page" = Option<usize>, Query), ("score_min" = Option<i64>, Query), ("score_max" = Option<i64>, Query), ("min_resolution" = Option<i64>, Query), ("resolution_min" = Option<i64>, Query), ("resolution_max" = Option<i64>, Query), ("directory" = Option<String>, Query), ("limit" = Option<usize>, Query)), responses((status = 200, body = ApiSuccess<LibraryPage>), (status = 400, body = ApiFailure)))]
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

    #[utoipa::path(get, path = "/api/training/adapters", responses((status = 200, body = ApiSuccess<Vec<TrainingAdapterResponse>>)))]
    pub async fn training_adapters() {}

    #[utoipa::path(get, path = "/api/training/runtime-profiles", responses((status = 200, body = ApiSuccess<Vec<TrainingRuntimeProfile>>)))]
    pub async fn training_runtime_profiles() {}

    #[utoipa::path(get, path = "/api/training/runtime-profiles/{id}/diagnostics", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingRuntimeDiagnostics>), (status = 400, body = ApiFailure)))]
    pub async fn training_runtime_diagnostics() {}

    #[utoipa::path(post, path = "/api/training/runtime-profiles/{id}/install", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingRuntimeProfile>), (status = 409, body = ApiFailure)))]
    pub async fn install_training_runtime() {}

    #[utoipa::path(get, path = "/api/vision-crop/runtime-profiles/{id}/health", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<VisionCropRuntimeHealth>), (status = 400, body = ApiFailure)))]
    pub async fn vision_crop_runtime_health() {}

    #[utoipa::path(post, path = "/api/vision-crop/runtime-profiles/{id}/install", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<VisionCropRuntimeHealth>), (status = 409, body = ApiFailure)))]
    pub async fn install_vision_crop_runtime() {}

    #[utoipa::path(get, path = "/api/training/gpus", responses((status = 200, body = ApiSuccess<Vec<TrainingGpu>>)))]
    pub async fn training_gpus() {}

    #[utoipa::path(get, path = "/api/training/queue", responses((status = 200, body = ApiSuccess<TrainingQueueResponse>)))]
    pub async fn training_queue() {}

    #[utoipa::path(get, path = "/api/training/datasets/gallery", params(("root_id" = String, Query), ("relative_directory" = Option<String>, Query), ("repeats" = u32, Query), ("caption_extension" = Option<String>, Query)), responses((status = 200, body = ApiSuccess<TrainingGalleryDatasetResponse>), (status = 400, body = ApiFailure)))]
    pub async fn training_gallery_dataset() {}

    #[utoipa::path(get, path = "/api/training/paths", params(("kind" = String, Query), ("path" = Option<String>, Query)), responses((status = 200, body = ApiSuccess<TrainingPathBrowserResponse>), (status = 400, body = ApiFailure)))]
    pub async fn training_path_browser() {}

    #[utoipa::path(get, path = "/api/training/presets", responses((status = 200, body = ApiSuccess<Vec<TrainingPresetResponse>>)))]
    pub async fn list_training_presets() {}

    #[utoipa::path(post, path = "/api/training/presets", request_body = TrainingPresetInput, responses((status = 201, body = ApiSuccess<TrainingPresetResponse>), (status = 400, body = ApiFailure)))]
    pub async fn create_training_preset() {}

    #[utoipa::path(post, path = "/api/training/presets/import", request_body = TrainingPresetImportRequest, responses((status = 201, body = ApiSuccess<TrainingPresetResponse>), (status = 400, body = ApiFailure)))]
    pub async fn import_training_preset() {}

    #[utoipa::path(put, path = "/api/training/presets/{id}", params(("id" = String, Path)), request_body = TrainingPresetInput, responses((status = 200, body = ApiSuccess<TrainingPresetResponse>), (status = 404, body = ApiFailure)))]
    pub async fn update_training_preset() {}

    #[utoipa::path(put, path = "/api/training/presets/{id}/toml", params(("id" = String, Path)), request_body = TrainingPresetImportRequest, responses((status = 200, body = ApiSuccess<TrainingPresetResponse>), (status = 400, body = ApiFailure), (status = 404, body = ApiFailure)))]
    pub async fn update_training_preset_from_toml() {}

    #[utoipa::path(get, path = "/api/training/presets/{id}/export", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingPresetExportResponse>), (status = 404, body = ApiFailure)))]
    pub async fn export_training_preset() {}

    #[utoipa::path(post, path = "/api/training/preview", request_body = TrainingPreviewRequest, responses((status = 200, body = ApiSuccess<TrainingPreviewResponse>), (status = 400, body = ApiFailure)))]
    pub async fn training_preview() {}

    #[utoipa::path(post, path = "/api/training/lora-svd/analyses", request_body = LoraSvdAnalysisRequest, responses((status = 200, body = ApiSuccess<LoraSvdAnalysisResult>), (status = 400, body = ApiFailure)))]
    pub async fn create_lora_svd_analysis() {}

    #[utoipa::path(get, path = "/api/training/lora-svd/analyses/{id}/modules/{module_id}", params(("id" = String, Path), ("module_id" = String, Path)), responses((status = 200, body = ApiSuccess<LoraSvdModule>), (status = 404, body = ApiFailure)))]
    pub async fn lora_svd_module() {}

    #[utoipa::path(get, path = "/api/training/lora-svd/analyses/{id}/export", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<LoraSvdAnalysisResult>), (status = 404, body = ApiFailure)))]
    pub async fn export_lora_svd_analysis() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/logs", params(("id" = String, Path), ("tail" = Option<usize>, Query)), responses((status = 200, body = ApiSuccess<TrainingLogsResponse>), (status = 400, body = ApiFailure)))]
    pub async fn training_logs() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/metrics", params(("id" = String, Path), ("series" = Vec<String>, Query), ("max_points" = Option<usize>, Query), ("from_step" = Option<u64>, Query), ("to_step" = Option<u64>, Query), ("from_timestamp" = Option<u64>, Query), ("to_timestamp" = Option<u64>, Query)), responses((status = 200, body = ApiSuccess<TrainingMetricsResponse>)))]
    pub async fn training_metrics() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/metrics/overview", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingMetricsOverviewResponse>)))]
    pub async fn training_metrics_overview() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/cleanup-preview", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingCleanupPreviewResponse>), (status = 409, body = ApiFailure)))]
    pub async fn training_cleanup_preview() {}

    #[utoipa::path(delete, path = "/api/training/tasks/{id}", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingCleanupResponse>), (status = 409, body = ApiFailure)))]
    pub async fn delete_training_task() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/events", params(("id" = String, Path), ("after" = Option<u64>, Query), ("Last-Event-ID" = Option<u64>, Header)), responses((status = 200, body = String, content_type = "text/event-stream")))]
    pub async fn training_events() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/artifacts", params(("id" = String, Path)), responses((status = 200, body = ApiSuccess<TrainingArtifactsResponse>), (status = 404, body = ApiFailure)))]
    pub async fn training_artifacts() {}

    #[utoipa::path(get, path = "/api/training/tasks/{id}/artifacts/{artifact_id}", params(("id" = String, Path), ("artifact_id" = String, Path)), responses((status = 200, content_type = "application/octet-stream"), (status = 404, body = ApiFailure)))]
    pub async fn training_artifact_file() {}

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
        paths::vllm_load,
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
        paths::training_adapters,
        paths::training_runtime_profiles,
        paths::training_runtime_diagnostics,
        paths::install_training_runtime,
        paths::vision_crop_runtime_health,
        paths::install_vision_crop_runtime,
        paths::training_gpus,
        paths::training_queue,
        paths::training_gallery_dataset,
        paths::training_path_browser,
        paths::list_training_presets,
        paths::create_training_preset,
        paths::import_training_preset,
        paths::update_training_preset,
        paths::update_training_preset_from_toml,
        paths::export_training_preset,
        paths::training_preview,
        paths::create_lora_svd_analysis,
        paths::lora_svd_module,
        paths::export_lora_svd_analysis,
        paths::training_logs,
        paths::training_metrics,
        paths::training_metrics_overview,
        paths::training_cleanup_preview,
        paths::delete_training_task,
        paths::training_events,
        paths::training_artifacts,
        paths::training_artifact_file,
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
        LibraryScoreRange,
        LibraryResolutionRange,
        QuarantineEntry,
        TaskSummary,
        TaskSnapshot,
        TaskEvent,
        TaskItem,
        TaskItemCounts,
        TaskDetails,
        CreateTaskRequest,
        DatasetAugmentationTaskRequest,
        DatasetAugmentationTaskOptions,
        SmartCropTaskOptions,
        DerivedRetaggingTaskOptions,
        TrainingGalleryDataset,
        TrainingSamplePromptSource,
        TrainingSampleSettings,
        TrainingRunRequest,
        TrainingTaskRequest,
        TrainingTaskSummary,
        TrainingAdapterField,
        TrainingAdapterGroup,
        TrainingAdapterResponse,
        TrainingRuntimeProfile,
        TrainingRuntimeCheck,
        TrainingRuntimeDiagnostics,
        VisionCropRuntimeHealth,
        TrainingGpuExternalProcess,
        TrainingGpu,
        TrainingMetric,
        TrainingMetricsResponse,
        TrainingMetricSeriesSummary,
        TrainingMetricsOverviewResponse,
        TrainingCleanupPath,
        TrainingCleanupPreviewResponse,
        TrainingCleanupResponse,
        TrainingLogsResponse,
        TrainingQueueEntry,
        TrainingQueueResponse,
        TrainingGalleryDatasetResponse,
        TrainingPathEntry,
        TrainingPathBrowserResponse,
        TrainingPresetInput,
        TrainingPresetResponse,
        TrainingPresetImportRequest,
        TrainingPresetExportResponse,
        TrainingArtifact,
        TrainingArtifactsResponse,
        TrainingPreviewRequest,
        TrainingPreviewResponse,
        LoraSvdAnalysisFileRequest,
        LoraSvdAnalysisRequest,
        LoraSvdThresholdRanks,
        LoraSvdCoverage,
        LoraSvdRankDistribution,
        LoraSvdExcludedModule,
        LoraSvdModule,
        LoraSvdModelReport,
        LoraSvdExecution,
        LoraSvdComparisonCheckpoint,
        LoraSvdComparison,
        LoraSvdAnalysisResult,
        DownloadHistoryRecord,
        DownloadHistoryPage,
        DanbooruPost,
        DanbooruPostsPage,
        DanbooruCount,
        TagSuggestion,
        HealthStatus,
        VllmHealthStatus,
        VllmLoadStatus,
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
        // Training runtime discovery, diagnostics, installation and live logs
        // are public API operations and must stay represented in the contract.
        assert_eq!(document.paths.paths.len(), 50);
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
        assert_eq!(operation_count, 58);
        for path in [
            "/api/training/runtime-profiles",
            "/api/training/runtime-profiles/{id}/diagnostics",
            "/api/training/runtime-profiles/{id}/install",
            "/api/vision-crop/runtime-profiles/{id}/health",
            "/api/vision-crop/runtime-profiles/{id}/install",
            "/api/training/queue",
            "/api/training/datasets/gallery",
            "/api/training/paths",
            "/api/training/presets",
            "/api/training/presets/{id}/toml",
            "/api/training/lora-svd/analyses",
            "/api/training/lora-svd/analyses/{id}/modules/{module_id}",
            "/api/training/lora-svd/analyses/{id}/export",
            "/api/training/tasks/{id}/logs",
            "/api/training/tasks/{id}/artifacts",
            "/api/training/tasks/{id}/metrics/overview",
            "/api/training/tasks/{id}/cleanup-preview",
            "/api/training/tasks/{id}",
        ] {
            assert!(
                document.paths.paths.contains_key(path),
                "缺少 OpenAPI 路径 {path}"
            );
        }
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
            mapping["dataset_augmentation"],
            "#/components/schemas/DatasetAugmentationTaskRequest"
        );
        assert_eq!(
            schemas["ResizeTaskRequest"]["properties"]["options"]["$ref"],
            "#/components/schemas/ResizeTaskOptions"
        );
        assert_eq!(
            schemas["VllmTagTaskRequest"]["properties"]["options"]["$ref"],
            "#/components/schemas/MediaIdsTaskOptions"
        );
        assert_eq!(
            schemas["DatasetAugmentationTaskRequest"]["properties"]["options"]["$ref"],
            "#/components/schemas/DatasetAugmentationTaskOptions"
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
