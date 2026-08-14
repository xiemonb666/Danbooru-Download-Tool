import type { components } from './generated'

type ApiSchema<Name extends keyof components['schemas']> = components['schemas'][Name]

export interface ApiSuccess<T> {
  data: T
  meta?: ApiMeta
}

export interface ApiMeta {
  request_id?: string
  [key: string]: unknown
}

type ApiFailure = ApiSchema<'ApiFailure'>

export class ApiError extends Error {
  readonly name = 'ApiError'

  constructor(
    message: string,
    readonly code: string,
    readonly retryable: boolean,
    readonly status: number,
    readonly requestId: string,
    readonly fields?: Record<string, unknown>,
  ) {
    super(message)
  }
}

export interface RequestOptions {
  body?: unknown
  signal?: AbortSignal
}

function isApiSuccess<T>(value: unknown): value is ApiSuccess<T> {
  return typeof value === 'object' && value !== null && 'data' in value
}

function isApiFailure(value: unknown): value is ApiFailure {
  if (typeof value !== 'object' || value === null || !('error' in value) || !('request_id' in value)) return false
  const error = Reflect.get(value, 'error')
  return typeof error === 'object' && error !== null && typeof Reflect.get(error, 'code') === 'string'
}

async function requestEnvelope<T>(method: string, path: string, options: RequestOptions = {}): Promise<ApiSuccess<T>> {
  const headers = new Headers({ Accept: 'application/json' })
  if (options.body !== undefined) headers.set('Content-Type', 'application/json')
  const response = await fetch(`/api${path}`, {
    method,
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
    signal: options.signal,
  })
  const payload: unknown = await response.json()
  if (!response.ok && isApiFailure(payload)) {
    throw new ApiError(
      payload.error.message,
      payload.error.code,
      payload.error.retryable,
      response.status,
      payload.request_id,
      payload.error.fields ?? undefined,
    )
  }
  if (!isApiSuccess<T>(payload)) throw new Error('Invalid API response')
  return payload
}

async function request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
  const envelope = await requestEnvelope<T>(method, path, options)
  return envelope.data
}

export const apiClient = {
  get<T>(path: string, options?: Pick<RequestOptions, 'signal'>): Promise<T> {
    return request<T>('GET', path, options)
  },
  post<T>(path: string, body?: unknown): Promise<T> {
    return request<T>('POST', path, { body })
  },
  put<T>(path: string, body?: unknown): Promise<T> {
    return request<T>('PUT', path, { body })
  },
  delete<T>(path: string): Promise<T> {
    return request<T>('DELETE', path)
  },
  getEnvelope<T>(path: string, options?: Pick<RequestOptions, 'signal'>): Promise<ApiSuccess<T>> {
    return requestEnvelope<T>('GET', path, options)
  },
}

export type UgoiraPolicy = ApiSchema<'UgoiraPolicy'>

export type AppConfig = ApiSchema<'AppConfig'>

export type UpdateConfigRequest = ApiSchema<'UpdateConfigRequest'>

export function getConfig(): Promise<AppConfig> {
  return apiClient.get<AppConfig>('/config')
}

export function updateConfig(config: UpdateConfigRequest): Promise<AppConfig> {
  return apiClient.put<AppConfig>('/config', config)
}

export type TaskKind = ApiSchema<'TaskKind'>

export type TaskStatus = ApiSchema<'TaskStatus'>

export type TaskFailure = ApiSchema<'TaskFailure'>

export type TaskProgress = ApiSchema<'TaskProgress'>

export type TaskPreviewCandidate = ApiSchema<'TaskPreviewCandidate'>

export type TaskPreview = ApiSchema<'TaskPreview'>

export type TaskSummary = ApiSchema<'TaskSummary'>

export type TaskSnapshot = ApiSchema<'TaskSnapshot'>

export type TaskEvent = ApiSchema<'TaskEvent'>

export type TaskItemStatus = ApiSchema<'TaskItemStatus'>

export type TaskItem = ApiSchema<'TaskItem'>

export type TaskItemCounts = ApiSchema<'TaskItemCounts'>

export type TaskDetails = ApiSchema<'TaskDetails'>

export interface TaskDetailsParams {
  itemStatus?: TaskItemStatus
  itemCursor?: string
  itemLimit?: number
}

export function getTasks(): Promise<TaskSnapshot> {
  return apiClient.get<TaskSnapshot>('/tasks')
}

export function getTaskDetails(id: string, params: TaskDetailsParams = {}, signal?: AbortSignal): Promise<TaskDetails> {
  const query = new URLSearchParams()
  if (params.itemStatus) query.set('item_status', params.itemStatus)
  if (params.itemCursor) query.set('item_cursor', params.itemCursor)
  query.set('item_limit', String(params.itemLimit ?? 50))
  return apiClient.get<TaskDetails>(`/tasks/${encodeURIComponent(id)}?${query}`, { signal })
}

export function createTask(request: CreateTaskRequest): Promise<TaskSummary> {
  return apiClient.post<TaskSummary>('/tasks', request)
}

export function taskAction(id: string, action: 'pause' | 'resume' | 'cancel' | 'retry' | 'confirm', decision?: string): Promise<TaskSummary> {
  return apiClient.post<TaskSummary>(
    `/tasks/${encodeURIComponent(id)}/${action}`,
    decision ? { decision } : undefined,
  )
}

export function deleteTask(id: string): Promise<TaskSummary> {
  return apiClient.delete<TaskSummary>(`/tasks/${encodeURIComponent(id)}`)
}

export type DownloadTaskRequest = ApiSchema<'DownloadTaskRequest'>

export type CreateTaskRequest = ApiSchema<'CreateTaskRequest'>

export type TrainingTaskRequest = Extract<CreateTaskRequest, { type: 'training' }>

export type RootTaskRequest = Exclude<CreateTaskRequest, DownloadTaskRequest | TrainingTaskRequest>

export type DownloadHistoryRecord = ApiSchema<'DownloadHistoryRecord'>

export type DownloadHistoryPage = ApiSchema<'DownloadHistoryPage'>

export interface DownloadHistoryParams {
  cursor?: string
  limit?: number
}

export function getDownloadHistory(params: DownloadHistoryParams = {}): Promise<DownloadHistoryPage> {
  const query = new URLSearchParams({ limit: String(params.limit ?? 50) })
  if (params.cursor) query.set('cursor', params.cursor)
  return apiClient.get<DownloadHistoryPage>(`/downloads/history?${query}`)
}

export type ContentRating = ApiSchema<'ContentRating'>
export type DanbooruRating = Exclude<ContentRating, 'unknown'>

export type DanbooruTags = ApiSchema<'DanbooruTags'>

export type DanbooruPost = ApiSchema<'DanbooruPost'>

export function danbooruMediaUrl(postId: number, variant: 'preview' | 'sample' | 'large' | 'original' | 'ugoira_webm' | 'ugoira_zip'): string {
  return `/api/danbooru/posts/${postId}/media/${variant}`
}

export type TagSuggestion = ApiSchema<'TagSuggestion'>

export function autocompleteTags(query: string, signal?: AbortSignal): Promise<TagSuggestion[]> {
  const params = new URLSearchParams({ q: query })
  return apiClient.get<TagSuggestion[]>(`/danbooru/autocomplete?${params}`, { signal })
}

export type HealthStatus = ApiSchema<'HealthStatus'>

export function getHealth(signal?: AbortSignal): Promise<HealthStatus> {
  return apiClient.get<HealthStatus>('/health', { signal })
}

export type VllmHealthStatus = ApiSchema<'VllmHealthStatus'>

export function getVllmHealth(signal?: AbortSignal): Promise<VllmHealthStatus> {
  return apiClient.get<VllmHealthStatus>('/vllm/health', { signal })
}

export type VllmLoadStatus = ApiSchema<'VllmLoadStatus'>

export function loadVllmModel(): Promise<VllmLoadStatus> {
  return apiClient.post<VllmLoadStatus>('/vllm/load')
}

export type VllmUnloadStatus = ApiSchema<'VllmUnloadStatus'>

export function unloadVllmModel(): Promise<VllmUnloadStatus> {
  return apiClient.post<VllmUnloadStatus>('/vllm/unload')
}

export type DanbooruPostsPage = ApiSchema<'DanbooruPostsPage'>

export interface DanbooruPostsParams {
  query: string
  page?: string
  limit?: number
}

function normalizeDanbooruRating(value: unknown): ContentRating {
  return value === 'g' || value === 's' || value === 'q' || value === 'e' ? value : 'unknown'
}

function normalizeDanbooruPost(post: DanbooruPost): DanbooruPost {
  return { ...post, rating: normalizeDanbooruRating(post.rating) }
}

export async function getDanbooruPosts(params: DanbooruPostsParams, signal?: AbortSignal): Promise<DanbooruPostsPage> {
  const query = new URLSearchParams({ q: params.query, limit: String(params.limit ?? 40) })
  if (params.page) query.set('page', params.page)
  const page = await apiClient.get<DanbooruPostsPage>(`/danbooru/posts?${query}`, { signal })
  return { ...page, posts: page.posts.map(normalizeDanbooruPost) }
}

export async function getDanbooruPost(id: number, signal?: AbortSignal): Promise<DanbooruPost> {
  const post = await apiClient.get<DanbooruPost>(`/danbooru/posts/${id}`, { signal })
  return normalizeDanbooruPost(post)
}

export type DanbooruCount = ApiSchema<'DanbooruCount'>

export function countDanbooruPosts(query: string, signal?: AbortSignal): Promise<DanbooruCount> {
  const params = new URLSearchParams({ q: query })
  return apiClient.get<DanbooruCount>(`/danbooru/count?${params}`, { signal })
}

export type MediaRoot = ApiSchema<'MediaRoot'>

export type SaveMediaRootRequest = ApiSchema<'SaveMediaRootRequest'>

export function getMediaRoots(): Promise<MediaRoot[]> {
  return apiClient.get<MediaRoot[]>('/library/roots')
}

export function createMediaRoot(request: SaveMediaRootRequest): Promise<MediaRoot> {
  return apiClient.post<MediaRoot>('/library/roots', request)
}

export function updateMediaRoot(id: string, request: SaveMediaRootRequest): Promise<MediaRoot> {
  return apiClient.put<MediaRoot>(`/library/roots/${encodeURIComponent(id)}`, request)
}

export type RootRemoval = ApiSchema<'RootRemoval'>

export function deleteMediaRoot(id: string): Promise<RootRemoval> {
  return apiClient.delete<RootRemoval>(`/library/roots/${encodeURIComponent(id)}`)
}

export type MediaDirectoryList = ApiSchema<'MediaDirectoryList'>

export type MediaDirectory = ApiSchema<'MediaDirectory'>

export function getMediaDirectories(rootId: string): Promise<MediaDirectoryList> {
  return apiClient.get<MediaDirectoryList>(`/library/roots/${encodeURIComponent(rootId)}/directories`)
}

export function createMediaDirectory(rootId: string, relativePath: string): Promise<MediaDirectory> {
  return apiClient.post<MediaDirectory>(`/library/roots/${encodeURIComponent(rootId)}/directories`, {
    relative_path: relativePath,
  })
}

export type LocalMedia = ApiSchema<'LocalMedia'>

export type LibraryPage = ApiSchema<'LibraryPage'>
export type LibraryFacets = ApiSchema<'LibraryFacets'>

export interface LibraryParams {
  rootId: string
  query?: string
  cursor?: string
  before?: boolean
  page?: number
  scoreMin?: number
  scoreMax?: number
  minResolution?: number
  resolutionMin?: number
  resolutionMax?: number
  postCreatedFrom?: number
  postCreatedTo?: number
  directory?: string
  limit?: number
}

export function getLibrary(params: LibraryParams, signal?: AbortSignal): Promise<LibraryPage> {
  const query = new URLSearchParams({ root_id: params.rootId, limit: String(params.limit ?? 60) })
  if (params.query) query.set('q', params.query)
  if (params.cursor) query.set('cursor', params.cursor)
  if (params.before) query.set('before', 'true')
  if (params.page !== undefined) query.set('page', String(params.page))
  if (params.scoreMin !== undefined) query.set('score_min', String(params.scoreMin))
  if (params.scoreMax !== undefined) query.set('score_max', String(params.scoreMax))
  if (params.minResolution !== undefined) query.set('min_resolution', String(params.minResolution))
  if (params.resolutionMin !== undefined) query.set('resolution_min', String(params.resolutionMin))
  if (params.resolutionMax !== undefined) query.set('resolution_max', String(params.resolutionMax))
  if (params.postCreatedFrom !== undefined) query.set('post_created_from', String(params.postCreatedFrom))
  if (params.postCreatedTo !== undefined) query.set('post_created_to', String(params.postCreatedTo))
  if (params.directory !== undefined) query.set('directory', params.directory)
  return apiClient.get<LibraryPage>(`/library/items?${query}`, { signal })
}

export function getLibraryFacets(params: Omit<LibraryParams, 'cursor' | 'before' | 'page' | 'limit'>, signal?: AbortSignal): Promise<LibraryFacets> {
  const query = new URLSearchParams({ root_id: params.rootId })
  if (params.query) query.set('q', params.query)
  if (params.scoreMin !== undefined) query.set('score_min', String(params.scoreMin))
  if (params.scoreMax !== undefined) query.set('score_max', String(params.scoreMax))
  if (params.minResolution !== undefined) query.set('min_resolution', String(params.minResolution))
  if (params.resolutionMin !== undefined) query.set('resolution_min', String(params.resolutionMin))
  if (params.resolutionMax !== undefined) query.set('resolution_max', String(params.resolutionMax))
  if (params.postCreatedFrom !== undefined) query.set('post_created_from', String(params.postCreatedFrom))
  if (params.postCreatedTo !== undefined) query.set('post_created_to', String(params.postCreatedTo))
  if (params.directory !== undefined) query.set('directory', params.directory)
  return apiClient.get<LibraryFacets>(`/library/facets?${query}`, { signal })
}

export function getLibraryItem(id: string, signal?: AbortSignal): Promise<LocalMedia> {
  return apiClient.get<LocalMedia>(`/library/items/${encodeURIComponent(id)}`, { signal })
}

export function libraryMediaUrl(id: string, variant: 'thumbnail' | 'file' = 'file'): string {
  return `/api/library/media/${encodeURIComponent(id)}/${variant}`
}

export type QuarantineEntry = ApiSchema<'QuarantineEntry'>

export function getQuarantine(rootId: string): Promise<QuarantineEntry[]> {
  const query = new URLSearchParams({ root_id: rootId })
  return apiClient.get<QuarantineEntry[]>(`/library/quarantine?${query}`)
}

export function restoreQuarantine(id: string): Promise<QuarantineEntry> {
  return apiClient.post<QuarantineEntry>(`/library/quarantine/${encodeURIComponent(id)}/restore`)
}

export type PurgeResponse = ApiSchema<'PurgeResponse'>

export function purgeQuarantine(rootId: string): Promise<PurgeResponse> {
  const query = new URLSearchParams({ root_id: rootId })
  return apiClient.delete<PurgeResponse>(`/library/quarantine?${query}`)
}

export type SecretKind = 'danbooru' | 'vllm'

export type SecretResponse = ApiSchema<'SecretResponse'>

export function saveSecret(kind: SecretKind, secret: string): Promise<SecretResponse> {
  return apiClient.put<SecretResponse>(`/secrets/${kind}`, { secret })
}

export function deleteSecret(kind: SecretKind): Promise<SecretResponse> {
  return apiClient.delete<SecretResponse>(`/secrets/${kind}`)
}

export interface TrainingSubgroup {
  id: string
  label: string
}

export interface TrainingGroup {
  id: string
  label: string
  description: string
  subgroups: TrainingSubgroup[]
}

export interface TrainingField {
  key: string
  label: string
  group: string
  subgroup: string
  kind: 'path' | 'text' | 'number' | 'boolean' | 'select' | 'list' | 'json' | 'secret'
  default: unknown
  choices: string[]
  required: boolean
  advanced: boolean
  help: string
  description: string
  when_to_adjust: string
}

export interface TrainingAdapter {
  id: string
  version: string
  label: string
  family: string
  family_label: string
  training_type: string
  training_type_label: string
  trainer: string
  fields: TrainingField[]
  groups: TrainingGroup[]
}

export interface TrainingRuntimeProfile {
  id: string
  label: string
  kind: 'windows' | 'wsl' | 'conda' | 'venv'
  /** Discovered environments are changed only after the user explicitly presses install/sync. */
  managed: boolean
  installed: boolean
  installing?: boolean
  last_error?: string | null
  runtime_root: string
  python_path: string
}

export interface TrainingRuntimeCheck {
  id: string
  ok: boolean
  detail: string
}

export interface TrainingRuntimeDiagnostics {
  profile: TrainingRuntimeProfile
  checks: TrainingRuntimeCheck[]
}

export type VisionCropRuntimeHealth = ApiSchema<'VisionCropRuntimeHealth'>

export interface TrainingGpu {
  id: string
  name: string
  memory_total_mib: number
  memory_used_mib: number
  utilization_percent: number
  graphics_clock_mhz?: number | null
  memory_clock_mhz?: number | null
  power_draw_w?: number | null
  power_limit_w?: number | null
  temperature_c?: number | null
  fan_speed_percent?: number | null
  external_processes?: TrainingGpuExternalProcess[]
}

export interface TrainingGpuExternalProcess {
  pid: number
  process_name: string
  memory_used_mib: number
}

export interface TrainingPreview {
  toml: string
}

export interface LoraSvdAnalysisFile {
  path: string
  label?: string
}

export interface LoraSvdAnalysisRequest {
  runtime_profile_id: string
  files: LoraSvdAnalysisFile[]
  device: 'auto'
}

export interface LoraSvdAnalysisResult {
  id: string
  reports: LoraSvdModelReport[]
  comparison?: LoraSvdComparison | null
  execution: LoraSvdExecution
  expires_at: number
}

export interface LoraSvdExecution {
  device: string
  reason: string
  selection_reason?: string
  duration_ms: number
  fallback?: boolean
}

export interface LoraSvdThresholdRanks {
  energy_95: number
  energy_99: number
  energy_999: number
}

export interface LoraSvdModuleSummary {
  id: string
  component: string
  rank: number
  alpha: number
  scale: number
  numerical_rank: number
  stable_rank: number
  tail_energy_20: number
  effective_rank: LoraSvdThresholdRanks
  energy: number
  flag?: 'compression_headroom' | 'compressible' | 'well_utilized' | 'saturation_signal' | null
}

export interface LoraSvdModelReport {
  id: string
  label: string
  path: string
  file_size_bytes: number
  sha256: string
  modified_at: number
  step?: number | null
  architecture: string
  format: string
  /** False for LoHa/LoKr/other structures where standard LoRA QR-SVD is invalid. */
  svd_applicable?: boolean
  coverage: { analyzed_modules: number; candidate_modules: number; unsupported_modules: number }
  rank_distribution: { minimum: number; maximum: number; modal: number; uniform: boolean }
  effective_rank: LoraSvdThresholdRanks
  current_rank_energy: number
  tail_energy_20: number
  verdict: 'high_compression_headroom' | 'compressible' | 'well_utilized' | 'saturation_signal' | 'partial_evidence'
  verdict_message: string
  metadata: Record<string, string>
  excluded: Array<{ id: string; reason: string }>
  modules: LoraSvdModuleSummary[]
  global_singular_values_count?: number
  global_singular_values: number[]
  global_cumulative_energy_count?: number
  global_cumulative_energy: number[]
}

export interface LoraSvdComparison {
  comparable: boolean
  reason: string
  checkpoints: Array<{ id: string; label: string; step?: number | null; effective_rank: LoraSvdThresholdRanks; rank_utilization: number; tail_energy_20: number }>
}

export interface TrainingMetric {
  step: number
  timestamp: number
  series: string
  value: number
}

export type TrainingMetricSnapshot = ApiSchema<'TrainingMetricsResponse'>
export type TrainingMetricSeriesSummary = ApiSchema<'TrainingMetricSeriesSummary'>
export type TrainingMetricsOverview = ApiSchema<'TrainingMetricsOverviewResponse'>
export type TrainingCleanupPath = ApiSchema<'TrainingCleanupPath'>
export type TrainingCleanupPreview = ApiSchema<'TrainingCleanupPreviewResponse'>
export type TrainingCleanupResult = ApiSchema<'TrainingCleanupResponse'>

export interface TrainingMetricQuery {
  series?: string[]
  maxPoints?: number
  fromStep?: number
  toStep?: number
  fromTimestamp?: number
  toTimestamp?: number
}

export interface TrainingLogs {
  text: string
  cursor: number
  truncated: boolean
}

export interface TrainingGalleryDataset {
  root_id: string
  relative_directory: string
  repeats: number
  caption_extension?: string | null
}

export interface TrainingGalleryDatasetPreview {
  root_id: string
  root_name: string
  relative_directory: string
  image_dir: string
  caption_extension: string
  image_count: number
  caption_count: number
  repeats: number
  effective_image_count: number
}

export interface TrainingAugmentationSubset {
  task_id: string
  id: 'horizontal_flip' | 'portrait' | 'upper_body' | 'cowboy_shot' | 'full_body_tight' | 'lower_body' | 'feet'
  label: string
  relative_directory: string
  caption_extension: string
  repeats: number
  image_count: number
  caption_count: number
}

export interface TrainingAugmentationDiscovery {
  source: TrainingGalleryDatasetPreview
  subsets: TrainingAugmentationSubset[]
}

export type TrainingSamplePromptSource = 'manual' | 'dataset_captions'

export interface TrainingSampleSettings {
  enabled: boolean
  prompt_source: TrainingSamplePromptSource
  prompt: string
  negative_prompt: string
  dataset_caption_count: number
  steps: number
  width: number
  height: number
  every_n_epochs: number
}

export interface TrainingQueueEntry {
  task_id: string
  status: TaskStatus
  adapter_id: string
  runtime_profile_id: string
  gpu_ids: string[]
  assigned_gpu_ids: string[]
  queue_position?: number | null
  blocking_task_ids: string[]
  blocked_gpu_ids: string[]
  estimated_wait_seconds?: number | null
  wait_reason?: string | null
}

export interface TrainingQueue {
  entries: TrainingQueueEntry[]
}

export interface TrainingPresetInput {
  name: string
  training: {
    adapter_id: string
    runtime_profile_id: string
    gpu_ids: string[]
    parameters: Record<string, unknown>
    gallery_dataset?: TrainingGalleryDataset | null
    gallery_datasets?: TrainingGalleryDataset[]
    sample?: TrainingSampleSettings | null
  }
}

export interface TrainingPreset extends TrainingPresetInput {
  id: string
  created_at: number
  updated_at: number
  version_count: number
}

export interface TrainingArtifact {
  id: string
  kind: 'lora' | 'checkpoint' | 'sample' | 'state' | 'config' | 'log' | 'metrics' | 'other'
  name: string
  path: string
  size_bytes: number
  modified_at: number
  step?: number | null
  prompt?: string | null
  url: string
}

export interface TrainingPathEntry {
  name: string
  path: string
}

export interface TrainingPathBrowser {
  current_path: string
  parent_path?: string | null
  directories: TrainingPathEntry[]
  files: TrainingPathEntry[]
}

export interface TrainingTaskCreateRequest {
  type: 'training'
  root_id: '__training__'
  training: TrainingPresetInput['training']
}

export function getTrainingAdapters(): Promise<TrainingAdapter[]> {
  return apiClient.get<TrainingAdapter[]>('/training/adapters')
}

export function getTrainingRuntimeProfiles(): Promise<TrainingRuntimeProfile[]> {
  return apiClient.get<TrainingRuntimeProfile[]>('/training/runtime-profiles')
}

export function getTrainingRuntimeDiagnostics(profileId: string): Promise<TrainingRuntimeDiagnostics> {
  return apiClient.get<TrainingRuntimeDiagnostics>(`/training/runtime-profiles/${encodeURIComponent(profileId)}/diagnostics`)
}

export function installTrainingRuntime(profileId: string): Promise<TaskSummary> {
  return apiClient.post<TaskSummary>(`/training/runtime-profiles/${encodeURIComponent(profileId)}/install`)
}

export function uploadBackground(name: string, data: string): Promise<AppConfig> {
  return apiClient.put<AppConfig>('/settings/background', { name, data })
}

export function deleteBackground(): Promise<AppConfig> {
  return apiClient.delete<AppConfig>('/settings/background')
}

export function getVisionCropRuntimeHealth(profileId: string): Promise<VisionCropRuntimeHealth> {
  return apiClient.get<VisionCropRuntimeHealth>(`/vision-crop/runtime-profiles/${encodeURIComponent(profileId)}/health`)
}

export function installVisionCropRuntime(profileId: string): Promise<VisionCropRuntimeHealth> {
  return apiClient.post<VisionCropRuntimeHealth>(`/vision-crop/runtime-profiles/${encodeURIComponent(profileId)}/install`)
}

export function getTrainingGpus(): Promise<TrainingGpu[]> {
  return apiClient.get<TrainingGpu[]>('/training/gpus')
}

export function getTrainingQueue(): Promise<TrainingQueue> {
  return apiClient.get<TrainingQueue>('/training/queue')
}

export function previewTrainingGalleryDataset(dataset: TrainingGalleryDataset): Promise<TrainingGalleryDatasetPreview> {
  const query = new URLSearchParams({
    root_id: dataset.root_id,
    relative_directory: dataset.relative_directory,
    repeats: String(dataset.repeats),
  })
  if (dataset.caption_extension) query.set('caption_extension', dataset.caption_extension)
  return apiClient.get<TrainingGalleryDatasetPreview>(`/training/datasets/gallery?${query}`)
}

export function discoverTrainingGalleryAugmentations(rootId: string, relativeDirectory: string): Promise<TrainingAugmentationDiscovery> {
  const query = new URLSearchParams({ root_id: rootId, relative_directory: relativeDirectory })
  return apiClient.get<TrainingAugmentationDiscovery>(`/training/datasets/augmentations?${query}`)
}

export function browseTrainingPath(kind: 'model' | 'dataset' | 'output', path: string): Promise<TrainingPathBrowser> {
  const query = new URLSearchParams({ kind, path })
  return apiClient.get<TrainingPathBrowser>(`/training/paths?${query}`)
}

export function getTrainingPresets(): Promise<TrainingPreset[]> {
  return apiClient.get<TrainingPreset[]>('/training/presets')
}

export function createTrainingPreset(input: TrainingPresetInput): Promise<TrainingPreset> {
  return apiClient.post<TrainingPreset>('/training/presets', input)
}

export function updateTrainingPreset(id: string, input: TrainingPresetInput): Promise<TrainingPreset> {
  return apiClient.put<TrainingPreset>(`/training/presets/${encodeURIComponent(id)}`, input)
}

export function exportTrainingPreset(id: string): Promise<{ name: string; toml: string }> {
  return apiClient.get<{ name: string; toml: string }>(`/training/presets/${encodeURIComponent(id)}/export`)
}

export function importTrainingPreset(input: { name: string; adapter_id?: string; runtime_profile_id?: string; gpu_ids?: string[]; toml: string }): Promise<TrainingPreset> {
  return apiClient.post<TrainingPreset>('/training/presets/import', input)
}

export function updateTrainingPresetToml(id: string, input: { name: string; adapter_id?: string; runtime_profile_id?: string; gpu_ids?: string[]; toml: string }): Promise<TrainingPreset> {
  return apiClient.put<TrainingPreset>(`/training/presets/${encodeURIComponent(id)}/toml`, input)
}

export function getTrainingArtifacts(id: string): Promise<{ artifacts: TrainingArtifact[] }> {
  return apiClient.get<{ artifacts: TrainingArtifact[] }>(`/training/tasks/${encodeURIComponent(id)}/artifacts`)
}

export function getTrainingLogs(taskId: string, options: { tail?: number; after?: number; limit?: number } = {}): Promise<TrainingLogs> {
  const query = new URLSearchParams()
  if (options.after !== undefined) query.set('after', String(Math.max(0, options.after)))
  else query.set('tail', String(Math.max(1, Math.min(2000, options.tail ?? 300))))
  if (options.limit !== undefined) query.set('limit', String(Math.max(1, Math.min(1024 * 1024, options.limit))))
  return apiClient.get<TrainingLogs>(`/training/tasks/${encodeURIComponent(taskId)}/logs?${query}`)
}

export interface TrainingPreflightCheck {
  id: string
  ok: boolean
  severity: string
  message: string
  recovery?: string
}

export interface TrainingParameterSuggestion {
  field: string
  value: unknown
  reason: string
}

export interface TrainingPreflight {
  ready: boolean
  checks: TrainingPreflightCheck[]
  suggestions: TrainingParameterSuggestion[]
  effective_steps: number
  estimated_vram_mib: number
}

export function preflightTraining(request: TrainingPresetInput['training']): Promise<TrainingPreflight> {
  return apiClient.post<TrainingPreflight>('/training/preflight', request)
}

export function previewTraining(adapterId: string, parameters: Record<string, unknown>): Promise<TrainingPreview> {
  return apiClient.post<TrainingPreview>('/training/preview', { adapter_id: adapterId, parameters })
}

export function analyzeLoraSvd(request: LoraSvdAnalysisRequest): Promise<LoraSvdAnalysisResult> {
  return apiClient.post<LoraSvdAnalysisResult>('/training/lora-svd/analyses', request)
}

export function loraSvdModuleUrl(analysisId: string, moduleId: string): string {
  return `/api/training/lora-svd/analyses/${encodeURIComponent(analysisId)}/modules/${encodeURIComponent(moduleId)}`
}

export function loraSvdExportUrl(analysisId: string): string {
  return `/api/training/lora-svd/analyses/${encodeURIComponent(analysisId)}/export`
}

export function createTrainingTask(request: TrainingTaskCreateRequest): Promise<TaskSummary> {
  return apiClient.post<TaskSummary>('/tasks', request)
}

export function getTrainingMetrics(id: string, query: TrainingMetricQuery = {}, signal?: AbortSignal): Promise<TrainingMetricSnapshot> {
  const params = new URLSearchParams()
  for (const value of query.series ?? []) params.append('series', value)
  if (query.maxPoints != null) params.set('max_points', String(query.maxPoints))
  if (query.fromStep != null) params.set('from_step', String(query.fromStep))
  if (query.toStep != null) params.set('to_step', String(query.toStep))
  if (query.fromTimestamp != null) params.set('from_timestamp', String(query.fromTimestamp))
  if (query.toTimestamp != null) params.set('to_timestamp', String(query.toTimestamp))
  return apiClient.get<TrainingMetricSnapshot>(`/training/tasks/${encodeURIComponent(id)}/metrics?${params}`, { signal })
}

export function getTrainingMetricsOverview(id: string, signal?: AbortSignal): Promise<TrainingMetricsOverview> {
  return apiClient.get<TrainingMetricsOverview>(`/training/tasks/${encodeURIComponent(id)}/metrics/overview`, { signal })
}

export function getTrainingCleanupPreview(id: string): Promise<TrainingCleanupPreview> {
  return apiClient.get<TrainingCleanupPreview>(`/training/tasks/${encodeURIComponent(id)}/cleanup-preview`)
}

export function deleteTrainingTask(id: string): Promise<TrainingCleanupResult> {
  return apiClient.delete<TrainingCleanupResult>(`/training/tasks/${encodeURIComponent(id)}`)
}

export function trainingMetricEventsUrl(id: string, after?: number): string {
  const suffix = after == null ? '' : `?after=${encodeURIComponent(String(after))}`
  return `/api/training/tasks/${encodeURIComponent(id)}/events${suffix}`
}
