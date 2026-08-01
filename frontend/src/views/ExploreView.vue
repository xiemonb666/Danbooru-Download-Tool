<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ChevronLeft, ChevronRight, Download, Eye, History, ImageOff, Search, X } from '@lucide/vue'
import DownloadDestinationPicker from '../components/DownloadDestinationPicker.vue'
import PostCard from '../components/PostCard.vue'
import {
  countDanbooruPosts,
  createTask,
  danbooruMediaUrl,
  getDanbooruPost,
  getDanbooruPosts,
  getMediaRoots,
  type DanbooruPost,
  type DanbooruPostsPage,
  type MediaRoot,
} from '../api'
import { useTagAutocomplete } from '../composables/useTagAutocomplete'
import { useConfigStore } from '../stores/config'
import { useTasksStore } from '../stores/tasks'
import { useToastStore } from '../stores/toast'
import {
  composeBatchDownloadQuery,
  composeDanbooruQuery,
  splitBatchTags,
  type DanbooruQuickFilters,
} from '../utils/danbooruQuery'
import {
  loadBatchDownloadHistory,
  saveBatchDownloadHistory,
  type BatchDownloadSettings,
} from '../utils/batchDownloadHistory'
import { contentRatingName, requiresContentReveal } from '../utils/contentRating'
import { cachePosts, getCachedPost, prunePostCache } from '../utils/postCache'

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const tasks = useTasksStore()
const toast = useToastStore()
const autocomplete = useTagAutocomplete()
const batchAutocomplete = useTagAutocomplete()

const inputQuery = ref('')
const result = ref<DanbooruPostsPage | null>(null)
const resultCount = ref<{ count: number; exact: boolean } | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
const selected = ref<Set<number>>(new Set())
const detailPost = ref<DanbooruPost | null>(null)
const detailRevealed = ref(false)
const detailPreviewVariant = ref<'sample' | 'preview' | 'large' | 'original'>('sample')
const detailPreviewUnavailable = ref(false)
const originalPreviewOpen = ref(false)
const originalPreviewVariant = ref<'original' | 'sample'>('original')
const detailPanel = ref<HTMLElement | null>(null)
const originalPreviewPanel = ref<HTMLElement | null>(null)
const roots = ref<MediaRoot[]>([])
const targetRootId = ref('')
const targetDirectory = ref('')
const creatingTask = ref(false)
const activeSubview = ref<'browse' | 'batch'>('browse')
const queryDownloadLimit = ref(100)
const queryDownloadInclude = ref('')
const queryDownloadExclude = ref('')
const queryDownloadMinimumScore = ref(0)
const queryDownloadMinimumResolution = ref(0)
const queryDownloadPrioritizeScore = ref(false)
const queryDownloadPrioritizeResolution = ref(false)
const queryDownloadKeepSidecarTxt = ref(true)
const queryDownloadStaticImagesOnly = ref(false)
const batchHistory = ref<BatchDownloadSettings[]>(loadBatchDownloadHistory())
const suggestionsOpen = ref(false)
const batchSuggestionsOpen = ref(false)
let controller: AbortController | null = null
let detailController: AbortController | null = null
let detailOpener: HTMLElement | null = null
let originalPreviewOpener: HTMLElement | null = null
let cacheCleanupTimer: ReturnType<typeof setInterval> | null = null

function routeText(key: string, fallback = ''): string {
  const value = route.query[key]
  return typeof value === 'string' ? value : fallback
}

function routeMinimumMegapixels(): DanbooruQuickFilters['minimumMegapixels'] {
  const value = routeText('resolution')
  return value === '1' || value === '2' || value === '4' || value === '8' ? value : ''
}

const filters = ref<DanbooruQuickFilters>({
  rating: (routeText('rating') || '') as DanbooruQuickFilters['rating'],
  order: (routeText('order') || 'id') as DanbooruQuickFilters['order'],
  format: (routeText('format') || '') as DanbooruQuickFilters['format'],
  minimumMegapixels: routeMinimumMegapixels(),
})

const pageToken = computed(() => routeText('page', '1'))
const page = computed(() => /^\d+$/.test(pageToken.value) ? Math.max(1, Number.parseInt(pageToken.value, 10)) : null)
const pageLabel = computed(() => page.value === null ? `游标 ${pageToken.value}` : `第 ${page.value} 页`)
const activeQuery = computed(() => routeText('q'))
const selectedCount = computed(() => selected.value.size)
const hasPrevious = computed(() => Boolean(result.value?.previous_page) || (page.value !== null && page.value > 1))
const hasNext = computed(() => Boolean(result.value?.next_page) || (result.value?.posts.length ?? 0) === 40)
const detailObscured = computed(() => detailPost.value !== null
  && config.config.blur_sensitive_media
  && requiresContentReveal(detailPost.value.rating)
  && !detailRevealed.value)
const queryDownloadQuery = computed(() => composeBatchDownloadQuery({
  tags: queryDownloadInclude.value,
  excludedTags: queryDownloadExclude.value,
  minimumScore: queryDownloadMinimumScore.value,
  minimumResolution: queryDownloadMinimumResolution.value,
  prioritizeScore: queryDownloadPrioritizeScore.value,
  prioritizeResolution: queryDownloadPrioritizeResolution.value,
}))

function normalizedBatchMinimumScore(): number {
  return Number.isFinite(queryDownloadMinimumScore.value)
    ? Math.trunc(queryDownloadMinimumScore.value)
    : 0
}

function normalizedBatchMinimumResolution(value = queryDownloadMinimumResolution.value): number {
  if (!Number.isFinite(value)) return 0
  return Math.min(8192, Math.max(0, Math.floor(Math.trunc(value) / 512) * 512))
}

function normalizedBatchLimit(): number {
  return Number.isFinite(queryDownloadLimit.value)
    ? Math.max(1, Math.trunc(queryDownloadLimit.value))
    : 1
}

function currentBatchSettings(): Omit<BatchDownloadSettings, 'savedAt'> {
  return {
    includeTags: queryDownloadInclude.value.trim(),
    excludeTags: queryDownloadExclude.value.trim(),
    minimumScore: normalizedBatchMinimumScore(),
    minimumResolution: normalizedBatchMinimumResolution(),
    limit: normalizedBatchLimit(),
    prioritizeScore: queryDownloadPrioritizeScore.value,
    prioritizeResolution: queryDownloadPrioritizeResolution.value,
    keepSidecarTxt: queryDownloadKeepSidecarTxt.value,
    staticImagesOnly: queryDownloadStaticImagesOnly.value,
    rootId: targetRootId.value,
    directory: targetDirectory.value,
  }
}

function applyBatchSettings(settings: BatchDownloadSettings): void {
  queryDownloadInclude.value = settings.includeTags
  queryDownloadExclude.value = settings.excludeTags
  queryDownloadMinimumScore.value = settings.minimumScore
  queryDownloadMinimumResolution.value = normalizedBatchMinimumResolution(settings.minimumResolution ?? 0)
  queryDownloadLimit.value = settings.limit
  queryDownloadPrioritizeScore.value = settings.prioritizeScore
  queryDownloadPrioritizeResolution.value = settings.prioritizeResolution
  queryDownloadKeepSidecarTxt.value = settings.keepSidecarTxt ?? true
  queryDownloadStaticImagesOnly.value = settings.staticImagesOnly ?? false
  if (roots.value.some((root) => root.id === settings.rootId)) {
    targetRootId.value = settings.rootId
    targetDirectory.value = settings.directory
  } else if (settings.rootId) {
    toast.warning('历史下载位置不可用', '标签和筛选参数已加载，请重新选择媒体库。')
  }
}

function loadLastBatchSettings(): void {
  const latest = batchHistory.value[0]
  if (!latest) return
  applyBatchSettings(latest)
  toast.success('已加载上次批量设置')
}

function historySummary(settings: BatchDownloadSettings): string {
  const priorities = [
    settings.prioritizeResolution ? '高分辨率' : '',
    settings.prioritizeScore ? '评分' : '',
  ].filter(Boolean).join(' + ')
  const resolution = settings.minimumResolution ?? 0
  return `最低评分 ${settings.minimumScore}${resolution > 0 ? ` · 最短边 ≥ ${resolution}px` : ''}${settings.staticImagesOnly ? ' · 静态图' : ''} · ${settings.limit} 张${priorities ? ` · ${priorities}优先` : ''}`
}

watch([targetRootId, targetDirectory], ([rootId, directory]) => {
  if (rootId) localStorage.setItem('danbooru-download-root', rootId)
  else localStorage.removeItem('danbooru-download-root')
  if (directory) localStorage.setItem('danbooru-download-directory', directory)
  else localStorage.removeItem('danbooru-download-directory')
})

function syncAutocompleteQuery(value: string): void {
  const token = value.match(/[^\s()]+$/)?.[0] ?? ''
  autocomplete.query.value = token.replace(/^-/, '')
  suggestionsOpen.value = token.length >= 2
}

watch(inputQuery, syncAutocompleteQuery)

function syncBatchAutocompleteQuery(value: string): void {
  const token = value.match(/[^\s,()]+$/)?.[0] ?? ''
  const query = token.replace(/^-/, '')
  if (query.includes(':')) {
    batchAutocomplete.query.value = ''
    batchSuggestionsOpen.value = false
    return
  }
  batchAutocomplete.query.value = query
  batchSuggestionsOpen.value = query.length >= 2
}

watch(queryDownloadInclude, syncBatchAutocompleteQuery)

watch(() => route.fullPath, async () => {
  suggestionsOpen.value = false
  batchSuggestionsOpen.value = false
  inputQuery.value = activeQuery.value
  filters.value = {
    rating: (routeText('rating') || '') as DanbooruQuickFilters['rating'],
    order: (routeText('order') || 'id') as DanbooruQuickFilters['order'],
    format: (routeText('format') || '') as DanbooruQuickFilters['format'],
    minimumMegapixels: routeMinimumMegapixels(),
  }
  controller?.abort()
  const requestController = new AbortController()
  controller = requestController
  loading.value = true
  error.value = null
  resultCount.value = null
  const query = composeDanbooruQuery(activeQuery.value, filters.value)
  void countDanbooruPosts(query, requestController.signal)
    .then((count) => {
      if (controller === requestController && !requestController.signal.aborted) {
        resultCount.value = count
      }
    })
    .catch(() => undefined)
  try {
    result.value = await getDanbooruPosts({
      query,
      page: pageToken.value,
      limit: 40,
    }, requestController.signal)
    cachePosts(result.value.posts)
  } catch (reason: unknown) {
    if (!(reason instanceof DOMException && reason.name === 'AbortError')) {
      error.value = reason instanceof Error ? reason.message : 'Danbooru 查询失败'
      result.value = null
    }
  } finally {
    if (controller === requestController) loading.value = false
  }
}, { immediate: true })

function updateRoute(nextPage: number | string = 1): void {
  const query: Record<string, string> = { page: String(nextPage) }
  if (inputQuery.value.trim()) query.q = inputQuery.value.trim()
  if (filters.value.rating) query.rating = filters.value.rating
  if (filters.value.order && filters.value.order !== 'id') query.order = filters.value.order
  if (filters.value.format) query.format = filters.value.format
  if (filters.value.minimumMegapixels) query.resolution = filters.value.minimumMegapixels
  void router.push({ path: '/explore', query })
  suggestionsOpen.value = false
}

function chooseSuggestion(value: string): void {
  const negative = inputQuery.value.match(/-?[^\s()]+$/)?.[0]?.startsWith('-') ?? false
  inputQuery.value = inputQuery.value.replace(/-?[^\s()]+$/, `${negative ? '-' : ''}${value}`)
  suggestionsOpen.value = false
}

function chooseBatchSuggestion(value: string): void {
  queryDownloadInclude.value = queryDownloadInclude.value.replace(/[^\s,()]+$/, value)
  batchSuggestionsOpen.value = false
}

function togglePost(post: DanbooruPost): void {
  const next = new Set(selected.value)
  if (next.has(post.id)) next.delete(post.id)
  else next.add(post.id)
  selected.value = next
}

async function openDetail(post: DanbooruPost): Promise<void> {
  detailOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null
  detailPost.value = getCachedPost(post.id) ?? post
  detailRevealed.value = false
  detailPreviewVariant.value = 'sample'
  detailPreviewUnavailable.value = false
  originalPreviewOpen.value = false
  originalPreviewVariant.value = 'original'
  originalPreviewOpener = null
  await nextTick()
  detailPanel.value?.focus()
  detailController?.abort()
  const requestController = new AbortController()
  detailController = requestController
  try {
    const refreshed = await getDanbooruPost(post.id, requestController.signal)
    if (detailController === requestController && detailPost.value?.id === post.id) {
      detailPost.value = refreshed
      cachePosts([refreshed])
    }
  } catch (reason: unknown) {
    if (!(reason instanceof DOMException && reason.name === 'AbortError')) {
      toast.warning('详情更新失败', '已显示搜索结果中的帖子信息。')
    }
  }
}

async function closeDetail(): Promise<void> {
  const opener = detailOpener
  detailController?.abort()
  detailController = null
  originalPreviewOpen.value = false
  originalPreviewOpener = null
  detailPost.value = null
  detailOpener = null
  await nextTick()
  opener?.focus()
}

function onDetailKeydown(event: KeyboardEvent): void {
  if (event.key !== 'Escape') return
  event.preventDefault()
  void closeDetail()
}

async function openOriginalPreview(): Promise<void> {
  if (detailObscured.value) return
  originalPreviewOpener = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null
  originalPreviewVariant.value = 'original'
  originalPreviewOpen.value = true
  await nextTick()
  originalPreviewPanel.value?.focus()
}

async function closeOriginalPreview(): Promise<void> {
  const opener = originalPreviewOpener
  originalPreviewOpen.value = false
  originalPreviewOpener = null
  await nextTick()
  opener?.focus()
}

function onOriginalPreviewKeydown(event: KeyboardEvent): void {
  if (event.key !== 'Escape') return
  event.preventDefault()
  void closeOriginalPreview()
}

function useDetailFallbackPreview(): void {
  if (detailPreviewVariant.value === 'sample') detailPreviewVariant.value = 'preview'
  else if (detailPreviewVariant.value === 'preview') detailPreviewVariant.value = 'large'
  else if (detailPreviewVariant.value === 'large') detailPreviewVariant.value = 'original'
  else detailPreviewUnavailable.value = true
}

function useOriginalFallbackPreview(): void {
  if (originalPreviewVariant.value === 'original') originalPreviewVariant.value = 'sample'
}

async function downloadSelected(): Promise<void> {
  if (!targetRootId.value) {
    toast.warning('需要下载位置', '请先在设置中添加并选择媒体库。')
    void router.push('/settings')
    return
  }
  creatingTask.value = true
  try {
    await config.load()
    await createTask({
      type: 'download',
      source: { type: 'post_ids', post_ids: [...selected.value] },
      root_id: targetRootId.value,
      relative_directory: targetDirectory.value || null,
      limit: selected.value.size,
      concurrency: config.config.download_concurrency,
      filename_template: config.config.filename_template,
      skip_existing: true,
      media_policy: { original: true, ugoira: config.config.ugoira_policy },
    })
    selected.value = new Set()
    await tasks.loadSnapshot()
    toast.success('下载任务已加入队列')
    void router.push('/tasks')
  } catch (reason: unknown) {
    toast.error('无法创建下载任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creatingTask.value = false
  }
}

async function downloadQuery(): Promise<void> {
  if (!queryDownloadInclude.value.trim()) {
    toast.warning('请输入包含标签', '批量下载至少需要一个普通标签，避免 Danbooru 拒绝空查询排序。')
    return
  }
  if (!targetRootId.value) {
    toast.warning('需要下载位置', '请先在设置中添加并选择媒体库。')
    void router.push('/settings')
    return
  }
  creatingTask.value = true
  try {
    await config.load()
    const settings = currentBatchSettings()
    await createTask({
      type: 'download',
      source: { type: 'query', query: queryDownloadQuery.value },
      batch_filter: {
        include_tags: splitBatchTags(settings.includeTags),
        exclude_tags: splitBatchTags(settings.excludeTags)
          .map((tag) => tag.replace(/^-+/, ''))
          .filter(Boolean),
        minimum_score: settings.minimumScore,
        minimum_resolution: settings.minimumResolution ?? 0,
      },
      root_id: settings.rootId,
      relative_directory: settings.directory || null,
      limit: settings.limit,
      concurrency: config.config.download_concurrency,
      filename_template: config.config.filename_template,
      skip_existing: true,
      prioritize_score: queryDownloadPrioritizeScore.value,
      prioritize_resolution: queryDownloadPrioritizeResolution.value,
      keep_sidecar_txt: settings.keepSidecarTxt,
      static_images_only: settings.staticImagesOnly,
      media_policy: { original: true, ugoira: config.config.ugoira_policy },
    })
    batchHistory.value = saveBatchDownloadHistory(settings)
    await tasks.loadSnapshot()
    toast.success('标签批量下载已加入队列')
    void router.push('/tasks')
  } catch (reason: unknown) {
    toast.error('无法创建下载任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creatingTask.value = false
  }
}

function showBatchDownload(): void {
  if (!queryDownloadInclude.value.trim()) queryDownloadInclude.value = activeQuery.value
  activeSubview.value = 'batch'
  suggestionsOpen.value = false
  batchSuggestionsOpen.value = false
}

function showBrowse(): void {
  activeSubview.value = 'browse'
  suggestionsOpen.value = false
  batchSuggestionsOpen.value = false
}

function formatBytes(value: number): string {
  if (value < 1024 * 1024) return `${Math.max(1, Math.round(value / 1024))} KB`
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

function ratingName(rating: DanbooruPost['rating']): string {
  return contentRatingName(rating)
}

onMounted(async () => {
  prunePostCache()
  cacheCleanupTimer = setInterval(prunePostCache, 15 * 60 * 1_000)
  try {
    roots.value = await getMediaRoots()
    const rememberedRoot = localStorage.getItem('danbooru-download-root')
    targetRootId.value = roots.value.some((root) => root.id === rememberedRoot)
      ? rememberedRoot ?? ''
      : roots.value[0]?.id ?? ''
    targetDirectory.value = targetRootId.value
      ? localStorage.getItem('danbooru-download-directory') ?? ''
      : ''
  } catch {
    roots.value = []
  }
})

onBeforeUnmount(() => {
  controller?.abort()
  detailController?.abort()
  autocomplete.dispose()
  batchAutocomplete.dispose()
  if (cacheCleanupTimer !== null) clearInterval(cacheCleanupTimer)
})
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Danbooru</p>
        <h1 class="page-title">探索与下载</h1>
        <p class="page-description">浏览预览并选择帖子，或使用标签条件创建大批量下载任务。</p>
      </div>
    </header>

    <nav class="subview-tabs" role="tablist" aria-label="探索与下载子界面">
      <button
        id="explore-browse-tab"
        type="button"
        role="tab"
        :aria-selected="activeSubview === 'browse'"
        aria-controls="explore-browse-panel"
        :class="{ active: activeSubview === 'browse' }"
        @click="showBrowse"
      >
        浏览与选择
      </button>
      <button
        id="explore-batch-tab"
        type="button"
        role="tab"
        :aria-selected="activeSubview === 'batch'"
        aria-controls="explore-batch-panel"
        :class="{ active: activeSubview === 'batch' }"
        @click="showBatchDownload"
      >
        标签批量下载
      </button>
    </nav>

    <div
      v-if="activeSubview === 'browse'"
      id="explore-browse-panel"
      role="tabpanel"
      aria-labelledby="explore-browse-tab"
    >
    <div class="search-panel">
      <form class="search-box" role="search" @submit.prevent="updateRoute(1)">
        <Search :size="20" />
        <div style="position: relative; min-width: 0; flex: 1">
          <label class="sr-only" for="danbooru-query">Danbooru 查询</label>
          <input
            id="danbooru-query"
            v-model="inputQuery"
            class="search-input"
            autocomplete="off"
            placeholder="例如：1girl solo score:>=20 order:score"
            @focus="suggestionsOpen = autocomplete.suggestions.value.length > 0"
            @blur="suggestionsOpen = false"
            @keydown.esc="suggestionsOpen = false"
          >
          <div v-if="suggestionsOpen && autocomplete.suggestions.value.length" class="autocomplete" role="listbox">
            <button
              v-for="suggestion in autocomplete.suggestions.value.slice(0, 8)"
              :key="suggestion.value"
              type="button"
              role="option"
              @mousedown.prevent="chooseSuggestion(suggestion.value)"
            >
              <span class="autocomplete-name">{{ suggestion.label || suggestion.value }}</span>
              <span class="autocomplete-meta">{{ suggestion.category }}<template v-if="suggestion.post_count"> · {{ suggestion.post_count.toLocaleString() }}</template></span>
            </button>
          </div>
        </div>
        <button type="submit" class="button button-primary search-submit" aria-label="执行查询">
          <Search :size="16" />
          <span>查询</span>
        </button>
      </form>

      <div class="filter-row" aria-label="快捷筛选">
        <select v-model="filters.rating" class="select filter-select" aria-label="内容分级" @change="updateRoute(1)">
          <option value="">全部分级</option>
          <option value="g">General</option>
          <option value="s">Sensitive</option>
          <option value="q">Questionable</option>
          <option value="e">Explicit</option>
        </select>
        <select v-model="filters.order" class="select filter-select" aria-label="排序" @change="updateRoute(1)">
          <option value="id">最新发布</option>
          <option value="score">评分最高</option>
          <option value="favcount">收藏最多</option>
          <option value="random">随机</option>
        </select>
        <select v-model="filters.format" class="select filter-select" aria-label="文件格式" @change="updateRoute(1)">
          <option value="">全部格式</option>
          <option value="jpg">JPG</option>
          <option value="png">PNG</option>
          <option value="webp">WebP</option>
          <option value="gif">GIF</option>
          <option value="avif">AVIF</option>
          <option value="mp4">MP4</option>
          <option value="webm">WebM</option>
        </select>
        <select v-model="filters.minimumMegapixels" class="select filter-select" aria-label="最低分辨率" @change="updateRoute(1)">
          <option value="">任意分辨率</option>
          <option value="1">≥ 1 MP</option>
          <option value="2">≥ 2 MP（约 1080p）</option>
          <option value="4">≥ 4 MP（约 1440p）</option>
          <option value="8">≥ 8 MP（约 4K）</option>
        </select>
        <button v-if="filters.rating || filters.format || filters.minimumMegapixels || (filters.order && filters.order !== 'id')" type="button" class="button button-small button-quiet" @click="filters = { rating: '', order: 'id', format: '', minimumMegapixels: '' }; updateRoute(1)">
          <X :size="14" /> 清除筛选
        </button>
      </div>
    </div>

    <div class="result-summary">
      <p v-if="loading">正在加载 40 项结果</p>
      <p v-else-if="resultCount">{{ resultCount.exact ? '' : '约 ' }}{{ resultCount.count.toLocaleString() }} 项结果 · {{ pageLabel }}</p>
      <p v-else>{{ result?.posts.length ?? 0 }} 项结果 · {{ pageLabel }}</p>
      <p>{{ config.config.blur_sensitive_media ? 'Questionable、Explicit 与未知分级默认模糊' : '分级内容按原图显示' }}</p>
    </div>

    <div v-if="loading" class="loading-grid" aria-label="正在加载">
      <div v-for="index in 12" :key="index" class="skeleton" />
    </div>

    <div v-else-if="error" class="empty-state">
      <div>
        <ImageOff :size="30" />
        <strong>无法加载查询结果</strong>
        <p>{{ error }}</p>
        <button type="button" class="button button-small" style="margin-top: 16px" @click="updateRoute(pageToken)">重试</button>
      </div>
    </div>

    <div v-else-if="result?.posts.length" class="post-grid">
      <PostCard
        v-for="post in result.posts"
        :key="post.id"
        :post="post"
        :selected="selected.has(post.id)"
        :blur-sensitive="config.config.blur_sensitive_media"
        @select="togglePost"
        @open="openDetail"
      />
    </div>

    <div v-else class="empty-state">
      <div>
        <Search :size="30" />
        <strong>没有找到匹配的帖子</strong>
        <p>尝试减少标签、移除筛选，或直接输入 Danbooru 的原生 metatag 查询。</p>
      </div>
    </div>

    <nav v-if="!loading && result?.posts.length" class="pagination" aria-label="查询分页">
      <button type="button" class="button button-small" :disabled="!hasPrevious" @click="updateRoute(result?.previous_page ?? Math.max(1, (page ?? 1) - 1))">
        <ChevronLeft :size="15" /> 上一页
      </button>
      <span class="pagination-info">{{ pageLabel }}</span>
      <button type="button" class="button button-small" :disabled="!hasNext" @click="updateRoute(result?.next_page ?? (page ?? 1) + 1)">
        下一页 <ChevronRight :size="15" />
      </button>
    </nav>

    <Transition name="toast">
      <div v-if="selectedCount" class="selection-bar" role="region" aria-label="批量下载栏">
        <strong>已选择 {{ selectedCount }} 项</strong>
        <span>下载原始媒体，已存在项目自动跳过</span>
        <div class="selection-actions">
          <DownloadDestinationPicker v-model:root-id="targetRootId" v-model:directory="targetDirectory" :roots="roots" compact />
          <button type="button" class="button button-quiet" @click="selected = new Set()">清除</button>
          <button type="button" class="button button-primary" :disabled="creatingTask" @click="downloadSelected">
            <Download :size="16" /> {{ creatingTask ? '加入中' : '下载所选' }}
          </button>
        </div>
      </div>
    </Transition>

    <template v-if="detailPost">
      <button type="button" class="detail-scrim" aria-label="关闭详情" @click="closeDetail" />
      <aside
        ref="detailPanel"
        class="detail-panel"
        :class="{ 'is-landscape': detailPost.image_width > detailPost.image_height }"
        role="dialog"
        aria-modal="true"
        aria-labelledby="post-detail-title"
        tabindex="-1"
        @keydown="onDetailKeydown"
      >
        <header class="detail-header">
          <h2 id="post-detail-title">帖子 #{{ detailPost.id }}</h2>
          <span class="status-pill">{{ ratingName(detailPost.rating) }}</span>
          <button type="button" class="button icon-button button-quiet" aria-label="关闭详情" @click="closeDetail"><X :size="19" /></button>
        </header>
        <div class="detail-content">
          <div
            class="detail-media"
            :style="{ aspectRatio: `${Math.max(detailPost.image_width, 1)} / ${Math.max(detailPost.image_height, 1)}` }"
          >
            <video
              v-if="detailPost.is_video || detailPost.is_ugoira"
              controls
              playsinline
              preload="metadata"
              :width="Math.max(detailPost.image_width, 1)"
              :height="Math.max(detailPost.image_height, 1)"
              :poster="danbooruMediaUrl(detailPost.id, 'sample')"
              :src="danbooruMediaUrl(detailPost.id, detailPost.is_ugoira ? 'ugoira_webm' : 'original')"
              :class="{ 'media-obscured': detailObscured }"
            />
            <button
              v-else-if="!detailPreviewUnavailable"
              type="button"
              class="detail-media-button"
              :disabled="detailObscured"
              :aria-label="`放大查看帖子 ${detailPost.id} 原图`"
              @click="openOriginalPreview"
            >
              <img
                :src="danbooruMediaUrl(detailPost.id, detailPreviewVariant)"
                :alt="`帖子 ${detailPost.id} 预览`"
                :width="Math.max(detailPost.image_width, 1)"
                :height="Math.max(detailPost.image_height, 1)"
                :class="{ 'media-obscured': detailObscured }"
                @error="useDetailFallbackPreview"
              >
            </button>
            <span v-else class="preview-unavailable"><ImageOff :size="22" />暂无可访问的详情预览</span>
            <button v-if="detailObscured && !detailPreviewUnavailable" type="button" class="reveal-button" :aria-label="`显示帖子 ${detailPost.id} 的受限内容`" @click="detailRevealed = true"><Eye :size="16" /> 显示敏感内容</button>
          </div>

          <div class="detail-stats">
            <div class="detail-stat"><small>评分</small><strong>{{ detailPost.score }}</strong></div>
            <div class="detail-stat"><small>收藏</small><strong>{{ detailPost.fav_count }}</strong></div>
            <div class="detail-stat"><small>文件</small><strong>{{ formatBytes(detailPost.file_size) }}</strong></div>
            <div class="detail-stat"><small>尺寸</small><strong>{{ detailPost.image_width }} × {{ detailPost.image_height }}</strong></div>
            <div class="detail-stat"><small>格式</small><strong>{{ detailPost.file_ext.toUpperCase() }}</strong></div>
            <div class="detail-stat"><small>状态</small><strong>{{ detailPost.restricted ? '受限' : detailPost.downloaded ? '已下载' : '可下载' }}</strong></div>
            <div v-if="detailPost.duration !== null && detailPost.duration !== undefined" class="detail-stat"><small>时长</small><strong>{{ detailPost.duration }} 秒</strong></div>
          </div>

          <div v-if="detailPost.source" class="tag-section"><h3>来源</h3><code style="font-size: 11px; color: var(--text-secondary); word-break: break-all">{{ detailPost.source }}</code></div>

          <div v-for="group in (['artist', 'character', 'copyright', 'general', 'meta'] as const)" :key="group" class="tag-section">
            <template v-if="detailPost.tags[group].length">
              <h3>{{ group }}</h3>
              <div class="tag-list">
                <button v-for="tag in detailPost.tags[group]" :key="tag" type="button" class="tag" :class="group" @click="inputQuery = `${inputQuery} ${tag}`.trim(); updateRoute(1)">{{ tag }}</button>
              </div>
            </template>
          </div>

          <button type="button" class="button button-primary" style="width: 100%; margin-top: 20px" @click="togglePost(detailPost)">
            <Download :size="16" /> {{ selected.has(detailPost.id) ? '已加入批选' : '加入批量下载' }}
          </button>
        </div>
      </aside>
      <div
        v-if="originalPreviewOpen"
        ref="originalPreviewPanel"
        class="original-preview"
        role="dialog"
        aria-modal="true"
        :aria-label="`原图预览 #${detailPost.id}`"
        tabindex="-1"
        @click.self="closeOriginalPreview"
        @keydown="onOriginalPreviewKeydown"
      >
        <button type="button" class="button icon-button original-preview-close" aria-label="关闭原图预览" @click="closeOriginalPreview"><X :size="20" /></button>
        <img
          :src="danbooruMediaUrl(detailPost.id, originalPreviewVariant)"
          :alt="`帖子 ${detailPost.id} 原图`"
          :width="Math.max(detailPost.image_width, 1)"
          :height="Math.max(detailPost.image_height, 1)"
          @error="useOriginalFallbackPreview"
        >
      </div>
    </template>
    </div>

    <section
      v-else
      id="explore-batch-panel"
      class="batch-download-layout"
      role="tabpanel"
      aria-labelledby="explore-batch-tab"
    >
      <form class="surface batch-download-form" @submit.prevent="downloadQuery">
        <header class="surface-header">
          <div>
            <h2 class="section-title">标签批量下载</h2>
            <p class="section-copy">按包含标签、排除标签、最低评分和所选优先级持续翻页，直到达到成功新增数量。</p>
          </div>
          <button
            type="button"
            class="button button-small button-quiet"
            :disabled="!batchHistory.length"
            @click="loadLastBatchSettings"
          >
            <History :size="14" /> 加载上次设置
          </button>
        </header>
        <div class="surface-body form-grid">
          <div class="field span-full">
            <label class="field-label" for="batch-tags">包含标签</label>
            <div class="tag-autocomplete-field">
              <textarea
                id="batch-tags"
                v-model="queryDownloadInclude"
                class="textarea"
                required
                maxlength="4096"
                autocomplete="off"
                aria-autocomplete="list"
                placeholder="例如：carlotta_(wuthering_waves) solo"
                @focus="batchSuggestionsOpen = batchAutocomplete.suggestions.value.length > 0"
                @blur="batchSuggestionsOpen = false"
                @keydown.esc="batchSuggestionsOpen = false"
              ></textarea>
              <div v-if="batchSuggestionsOpen && batchAutocomplete.suggestions.value.length" class="autocomplete" role="listbox" aria-label="标签匹配建议">
                <button
                  v-for="suggestion in batchAutocomplete.suggestions.value.slice(0, 8)"
                  :key="suggestion.value"
                  type="button"
                  role="option"
                  @mousedown.prevent="chooseBatchSuggestion(suggestion.value)"
                >
                  <span class="autocomplete-name">{{ suggestion.label || suggestion.value }}</span>
                  <span class="autocomplete-meta">{{ suggestion.category }}<template v-if="suggestion.post_count"> · {{ suggestion.post_count.toLocaleString() }}</template></span>
                </button>
              </div>
            </div>
            <span class="field-help">至少填写一个普通 Danbooru 标签；多个标签使用空格分隔。</span>
          </div>
          <div class="field span-full">
            <label class="field-label" for="batch-excluded-tags">排除标签</label>
            <textarea id="batch-excluded-tags" v-model="queryDownloadExclude" class="textarea" maxlength="4096" placeholder="例如：1boy, comic, watermark"></textarea>
            <span class="field-help">支持空格、逗号或换行分隔，系统自动添加负标签前缀。</span>
          </div>
          <div class="field">
            <label class="field-label" for="batch-minimum-score">最低评分</label>
            <input id="batch-minimum-score" v-model.number="queryDownloadMinimumScore" class="input" type="number" min="-1000000" max="1000000">
          </div>
          <div class="field">
            <label class="field-label" for="batch-minimum-resolution">最低分辨率</label>
            <input id="batch-minimum-resolution" v-model.number="queryDownloadMinimumResolution" class="input" type="range" min="0" max="8192" step="512">
            <span class="field-help">{{ queryDownloadMinimumResolution === 0 ? '不限' : `最短边至少 ${queryDownloadMinimumResolution}px` }}；范围 0–8K，每档 512px。</span>
          </div>
          <div class="field">
            <label class="field-label" for="batch-limit">最大下载数量</label>
            <input id="batch-limit" v-model.number="queryDownloadLimit" class="input" type="number" min="1" required>
            <span class="field-help">填写大于 0 的数量，按成功新增文件计数。</span>
          </div>
          <DownloadDestinationPicker v-model:root-id="targetRootId" v-model:directory="targetDirectory" :roots="roots" class="span-full" />
          <div class="field">
            <label class="field-label" for="batch-score-priority">评分优先排序</label>
            <label class="batch-check" for="batch-score-priority">
              <input id="batch-score-priority" v-model="queryDownloadPrioritizeScore" type="checkbox">
              <span>{{ queryDownloadPrioritizeResolution ? '下载器在每批候选中，同分辨率图片按评分优先' : '下载器在每批候选中优先选择高评分帖子' }}</span>
            </label>
          </div>
          <div class="field">
            <label class="field-label" for="batch-resolution-priority">高分辨率优先</label>
            <label class="batch-check" for="batch-resolution-priority">
              <input id="batch-resolution-priority" v-model="queryDownloadPrioritizeResolution" type="checkbox">
              <span>下载器在每批候选中优先选择高分辨率图片</span>
            </label>
          </div>
          <div class="field span-full">
            <label class="field-label" for="batch-static-images-only">仅下载静态图片</label>
            <label class="batch-check" for="batch-static-images-only">
              <input id="batch-static-images-only" v-model="queryDownloadStaticImagesOnly" type="checkbox">
              <span>跳过 GIF、MP4、WebM 和 Ugoira；仅保留 JPG、PNG、WebP、AVIF。</span>
            </label>
          </div>
          <div class="field span-full">
            <label class="field-label" for="batch-keep-sidecar">保留同名 TXT 标签文件</label>
            <label class="batch-check" for="batch-keep-sidecar">
              <input id="batch-keep-sidecar" v-model="queryDownloadKeepSidecarTxt" type="checkbox">
              <span>默认保留已写好的同名 <code>.txt</code>；关闭后，仅在新图片成功入库时清理对应 TXT。</span>
            </label>
          </div>
          <div class="field span-full batch-query-preview">
            <span class="field-label">最终 Danbooru 查询</span>
            <code>{{ queryDownloadQuery }}</code>
          </div>
          <div class="notice span-full">已存在内容会跳过并继续翻页补足；使用设置中的 {{ config.config.download_concurrency }} 路并发和文件名模板 <code>{{ config.config.filename_template }}</code>。</div>
          <div class="inline span-full">
            <button type="submit" class="button button-primary" :disabled="creatingTask || !queryDownloadInclude.trim() || !targetRootId">
              <Download :size="16" /> {{ creatingTask ? '正在加入队列' : '开始批量下载' }}
            </button>
          </div>
        </div>
      </form>

      <aside class="surface batch-download-help">
        <header class="surface-header"><div><h2 class="section-title">查询与下载策略</h2><p class="section-copy">筛选转换为 Danbooru 原生 metatag；优先级由下载器在每批候选中本地执行，避免远程查询超时。</p></div></header>
        <div class="surface-body stack">
          <div><span class="field-label">最低评分前缀</span><code>score:&gt;={{ Math.trunc(queryDownloadMinimumScore || 0) }}</code></div>
          <div><span class="field-label">分辨率优先</span><code>{{ queryDownloadPrioritizeResolution ? '下载器本地排序' : '未启用' }}</code></div>
          <div><span class="field-label">评分优先</span><code>{{ queryDownloadPrioritizeScore ? (queryDownloadPrioritizeResolution ? '同分辨率时按评分排序' : '下载器本地排序') : '未启用' }}</code></div>
          <div><span class="field-label">多标签兼容</span><p class="section-copy">先按原查询下载；若 Danbooru 返回标签超额，自动改用稀有包含标签取候选，再分组由服务器验证其余标签。</p></div>
          <div><span class="field-label">媒体策略</span><p class="section-copy">下载原始图片或视频；Ugoira 使用设置中的 {{ config.config.ugoira_policy }} 策略。</p></div>
          <div class="batch-history" aria-label="批量参数历史">
            <span class="field-label">批量参数历史</span>
            <p v-if="!batchHistory.length" class="section-copy">完成一次创建后会保留最近 10 组参数，方便重复下载。</p>
            <div v-else class="batch-history-list">
              <button
                v-for="entry in batchHistory"
                :key="entry.savedAt"
                type="button"
                class="batch-history-entry"
                :aria-label="`使用历史设置 ${entry.includeTags}`"
                @click="applyBatchSettings(entry)"
              >
                <strong>{{ entry.includeTags }}</strong>
                <small v-if="entry.excludeTags">排除：{{ entry.excludeTags }}</small>
                <small>{{ historySummary(entry) }}</small>
              </button>
            </div>
          </div>
        </div>
      </aside>
    </section>
  </div>
</template>
