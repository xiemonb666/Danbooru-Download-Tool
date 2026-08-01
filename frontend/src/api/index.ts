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

export function taskAction(id: string, action: 'pause' | 'resume' | 'cancel' | 'retry' | 'confirm'): Promise<TaskSummary> {
  return apiClient.post<TaskSummary>(`/tasks/${encodeURIComponent(id)}/${action}`)
}

export type DownloadTaskRequest = ApiSchema<'DownloadTaskRequest'>

export type CreateTaskRequest = ApiSchema<'CreateTaskRequest'>

export type RootTaskRequest = Exclude<CreateTaskRequest, DownloadTaskRequest>

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

export interface LibraryParams {
  rootId: string
  query?: string
  cursor?: string
  limit?: number
}

export function getLibrary(params: LibraryParams, signal?: AbortSignal): Promise<LibraryPage> {
  const query = new URLSearchParams({ root_id: params.rootId, limit: String(params.limit ?? 60) })
  if (params.query) query.set('q', params.query)
  if (params.cursor) query.set('cursor', params.cursor)
  return apiClient.get<LibraryPage>(`/library/items?${query}`, { signal })
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
