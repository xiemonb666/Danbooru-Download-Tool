<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave, useRoute, useRouter } from 'vue-router'
import { Bot, ChevronLeft, ChevronRight, Eye, FileImage, FolderPlus, Images, Play, RefreshCw, Search, Tags, Trash2, X } from '@lucide/vue'
import { createTask, getLibrary, getLibraryFacets, getLibraryItem, getMediaDirectories, getMediaRoots, libraryMediaUrl, type CreateTaskRequest, type LibraryFacets, type LibraryPage, type LocalMedia, type MediaRoot } from '../api'
import { useConfigStore } from '../stores/config'
import { useTasksStore } from '../stores/tasks'
import { useToastStore } from '../stores/toast'
import { requiresContentReveal } from '../utils/contentRating'
import { scrollToPageTop } from '../utils/pageScroll'
import { loadLibraryViewState, resolveLibraryViewState, saveLibraryViewState, type LibraryViewState } from '../utils/libraryViewState'
import { formatPostCreatedAt, localDateEndEpochSeconds, localDateStartEpochSeconds } from '../utils/postDateRange'
import ConfirmDialog from '../components/ConfirmDialog.vue'

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const tasks = useTasksStore()
const toast = useToastStore()
const restoredView = resolveLibraryViewState(loadLibraryViewState(), route.query)
const roots = ref<MediaRoot[]>([])
const activeRootId = ref('')
const folderDirectories = ref<string[]>([])
const activeDirectory = ref(restoredView.state.directory)
const queryInput = ref(restoredView.state.query)
const loadedQuery = ref(restoredView.state.query)
const page = ref<LibraryPage | null>(null)
const facets = ref<LibraryFacets | null>(null)
const cursor = ref(restoredView.state.cursor)
const cursorBefore = ref(restoredView.state.before)
const cursorDepth = ref(restoredView.state.cursorDepth)
const scoreRangeInput = ref(restoredView.state.scoreRange)
const resolutionRangeInput = ref(restoredView.state.resolutionRange)
const postCreatedFromDate = ref(restoredView.state.postCreatedFromDate)
const postCreatedToDate = ref(restoredView.state.postCreatedToDate)
const loading = ref(false)
const error = ref<string | null>(null)
const detail = ref<LocalMedia | null>(null)
const detailRevealed = ref(false)
const detailPanel = ref<HTMLElement | null>(null)
const indexing = ref(false)
const reindexConfirmationOpen = ref(false)
const selected = ref<Set<string>>(new Set())
const selectedMedia = ref<Map<string, LocalMedia>>(new Map())
const allMatchingSelected = ref(false)
const allMatchingTotal = ref(0)
const selectedQuery = ref('')
const selectedScoreMin = ref<number | undefined>()
const selectedScoreMax = ref<number | undefined>()
const selectedResolutionMin = ref<number | undefined>()
const selectedResolutionMax = ref<number | undefined>()
const selectedPostCreatedFrom = ref<number | undefined>()
const selectedPostCreatedTo = ref<number | undefined>()
const excludedMediaIds = ref<Set<string>>(new Set())
const revealedMedia = ref<Set<string>>(new Set())
const fullSizePreviews = ref<Set<string>>(new Set())
const creatingBatchTask = ref(false)
const resizeMaxSize = ref(1216)
const originalPreviewOpen = ref(false)
const originalPreviewPanel = ref<HTMLElement | null>(null)
let controller: AbortController | null = null
let detailController: AbortController | null = null
let detailOpener: HTMLElement | null = null
let originalPreviewOpener: HTMLElement | null = null
let savedBeforeRouteLeave = false

const activeRoot = computed(() => roots.value.find((root) => root.id === activeRootId.value) ?? null)
const scoreRanges = computed(() => page.value?.score_ranges?.length
  ? page.value.score_ranges
  : (facets.value?.score_ranges ?? []))
const resolutionRanges = computed(() => page.value?.resolution_ranges?.length
  ? page.value.resolution_ranges
  : (facets.value?.resolution_ranges ?? []))
const selectedCount = computed(() => allMatchingSelected.value
  ? Math.max(0, allMatchingTotal.value - excludedMediaIds.value.size)
  : selected.value.size)
const someMatchingSelected = computed(() => selectedCount.value > 0 && !allMatchingSelected.value)
const heicSelectionEligible = computed(() => !allMatchingSelected.value && selected.value.size > 0
  && selectedMedia.value.size === selected.value.size
  && Array.from(selectedMedia.value.values()).every(isHeicMedia))
const vllmSelectionEligible = computed(() => !allMatchingSelected.value && selected.value.size > 0
  && selectedMedia.value.size === selected.value.size
  && Array.from(selectedMedia.value.values()).every(isVllmMedia))
const detailObscured = computed(() => detail.value !== null
  && config.config.blur_sensitive_media
  && requiresContentReveal(detail.value.rating)
  && !detailRevealed.value)
const postDateRangeInvalid = computed(() => {
  const from = localDateStartEpochSeconds(postCreatedFromDate.value)
  const to = localDateEndEpochSeconds(postCreatedToDate.value)
  return from !== undefined && to !== undefined && from > to
})

async function loadRoots(): Promise<void> {
  roots.value = await getMediaRoots()
  const requested = restoredView.state.rootId
  activeRootId.value = roots.value.some((root) => root.id === requested) ? requested : (roots.value[0]?.id ?? '')
  if (requested && activeRootId.value !== requested) activeDirectory.value = ''
  void loadDirectories()
}

async function loadDirectories(): Promise<void> {
  folderDirectories.value = []
  if (!activeRootId.value) return
  try {
    folderDirectories.value = (await getMediaDirectories(activeRootId.value)).directories
  } catch (reason: unknown) {
    toast.warning('无法读取图库文件夹', reason instanceof Error ? reason.message : '可在刷新图库后重试')
  }
}

function isPlainRootBrowse(): boolean {
  return activeDirectory.value === ''
    && queryInput.value.trim() === ''
    && scoreRangeInput.value === ''
    && resolutionRangeInput.value === ''
    && postCreatedFromDate.value === ''
    && postCreatedToDate.value === ''
}

function enterFirstDirectoryIfEmptyRoot(): boolean {
  if (!isPlainRootBrowse() || page.value?.total !== 0 || !folderDirectories.value.length) return false
  activeDirectory.value = folderDirectories.value[0]
  resetToFirstPage()
  void router.replace({
    path: '/library',
    query: { root: activeRootId.value, directory: activeDirectory.value },
  })
  void loadPage()
  return true
}

watch(folderDirectories, () => {
  enterFirstDirectoryIfEmptyRoot()
})

async function loadPage(): Promise<void> {
  if (!activeRootId.value) {
    page.value = null
    return
  }
  if (postDateRangeInvalid.value) {
    error.value = null
    page.value = null
    return
  }
  controller?.abort()
  const requestController = new AbortController()
  controller = requestController
  loading.value = true
  error.value = null
  try {
    const query = queryInput.value.trim()
    const [scoreMin, scoreMax] = parseScoreRange(scoreRangeInput.value)
    const [resolutionMin, resolutionMax] = parseNumericRange(resolutionRangeInput.value)
    const postCreatedFrom = localDateStartEpochSeconds(postCreatedFromDate.value)
    const postCreatedTo = localDateEndEpochSeconds(postCreatedToDate.value)
    const parameters = {
      rootId: activeRootId.value,
      query,
      ...(cursor.value ? { cursor: cursor.value, before: cursorBefore.value } : {}),
      ...(scoreMin === undefined ? {} : { scoreMin }),
      ...(scoreMax === undefined ? {} : { scoreMax }),
      ...(resolutionMin === undefined ? {} : { resolutionMin }),
      ...(resolutionMax === undefined ? {} : { resolutionMax }),
      ...(postCreatedFrom === undefined ? {} : { postCreatedFrom }),
      ...(postCreatedTo === undefined ? {} : { postCreatedTo }),
      directory: activeDirectory.value,
      limit: 60,
    }
    const response = await getLibrary(parameters, requestController.signal)
    if (controller === requestController) {
      page.value = response
      loadedQuery.value = query
      void router.replace({
        path: '/library',
        query: {
          root: activeRootId.value,
          directory: activeDirectory.value,
          ...(query ? { q: query } : {}),
          ...(scoreRangeInput.value ? { score: scoreRangeInput.value } : {}),
          ...(resolutionRangeInput.value ? { resolution: resolutionRangeInput.value } : {}),
          ...(postCreatedFromDate.value ? { post_created_from: postCreatedFromDate.value } : {}),
          ...(postCreatedToDate.value ? { post_created_to: postCreatedToDate.value } : {}),
          ...(cursor.value ? { cursor: cursor.value, before: cursorBefore.value ? '1' : '0' } : {}),
          ...(cursor.value && cursorDepth.value > 1 ? { cursor_depth: String(cursorDepth.value) } : {}),
        },
      })
      if (enterFirstDirectoryIfEmptyRoot()) return
    }
  } catch (reason: unknown) {
    if (controller === requestController && !(reason instanceof DOMException && reason.name === 'AbortError')) {
      error.value = reason instanceof Error ? reason.message : '图库加载失败'
      page.value = null
    }
  } finally {
    if (controller === requestController) loading.value = false
  }
}

async function loadFacets(): Promise<void> {
  if (!activeRootId.value || postDateRangeInvalid.value) return
  const [scoreMin, scoreMax] = parseScoreRange(scoreRangeInput.value)
  const [resolutionMin, resolutionMax] = parseNumericRange(resolutionRangeInput.value)
  const postCreatedFrom = localDateStartEpochSeconds(postCreatedFromDate.value)
  const postCreatedTo = localDateEndEpochSeconds(postCreatedToDate.value)
  try {
    facets.value = await getLibraryFacets({
      rootId: activeRootId.value,
      query: queryInput.value.trim(),
      ...(scoreMin === undefined ? {} : { scoreMin }),
      ...(scoreMax === undefined ? {} : { scoreMax }),
      ...(resolutionMin === undefined ? {} : { resolutionMin }),
      ...(resolutionMax === undefined ? {} : { resolutionMax }),
      ...(postCreatedFrom === undefined ? {} : { postCreatedFrom }),
      ...(postCreatedTo === undefined ? {} : { postCreatedTo }),
      directory: activeDirectory.value,
    })
  } catch {
    facets.value = null
  }
}

function selectRoot(id: string): void {
  activeRootId.value = id
  activeDirectory.value = ''
  clearSelection()
  resetToFirstPage()
  void router.replace({ path: '/library', query: { root: id, directory: '' } })
  void loadDirectories()
  void loadPage()
  void loadFacets()
}

function selectDirectory(): void {
  clearSelection()
  resetToFirstPage()
  void router.replace({
    path: '/library',
    query: { root: activeRootId.value, directory: activeDirectory.value },
  })
  void loadPage()
  void loadFacets()
}

function parseScoreRange(value: string): [number | undefined, number | undefined] {
  return parseNumericRange(value)
}

function parseNumericRange(value: string): [number | undefined, number | undefined] {
  const [minimum, maximum] = (value ?? '').split(':').map(Number)
  if (!Number.isInteger(minimum) || !Number.isInteger(maximum) || minimum > maximum) {
    return [undefined, undefined]
  }
  return [minimum, maximum]
}

function resetToFirstPage(): void {
  cursor.value = ''
  cursorBefore.value = false
  cursorDepth.value = 1
}

function clearSelection(): void {
  selected.value = new Set()
  selectedMedia.value = new Map()
  allMatchingSelected.value = false
  allMatchingTotal.value = 0
  selectedQuery.value = ''
  selectedScoreMin.value = undefined
  selectedScoreMax.value = undefined
  selectedResolutionMin.value = undefined
  selectedResolutionMax.value = undefined
  selectedPostCreatedFrom.value = undefined
  selectedPostCreatedTo.value = undefined
  excludedMediaIds.value = new Set()
}

function isMediaSelected(mediaId: string): boolean {
  return allMatchingSelected.value
    ? !excludedMediaIds.value.has(mediaId)
    : selected.value.has(mediaId)
}

function toggleMedia(media: LocalMedia): void {
  if (allMatchingSelected.value) {
    const nextExcluded = new Set(excludedMediaIds.value)
    if (nextExcluded.has(media.id)) nextExcluded.delete(media.id)
    else nextExcluded.add(media.id)
    excludedMediaIds.value = nextExcluded
    return
  }
  const next = new Set(selected.value)
  const nextMedia = new Map(selectedMedia.value)
  if (next.has(media.id)) {
    next.delete(media.id)
    nextMedia.delete(media.id)
  } else {
    next.add(media.id)
    nextMedia.set(media.id, media)
  }
  selected.value = next
  selectedMedia.value = nextMedia
}

function toggleAllMatching(): void {
  if (allMatchingSelected.value) {
    clearSelection()
    return
  }
  allMatchingSelected.value = true
  allMatchingTotal.value = page.value?.total ?? 0
  selectedQuery.value = loadedQuery.value
  const [scoreMin, scoreMax] = parseScoreRange(scoreRangeInput.value)
  selectedScoreMin.value = scoreMin
  selectedScoreMax.value = scoreMax
  const [resolutionMin, resolutionMax] = parseNumericRange(resolutionRangeInput.value)
  selectedResolutionMin.value = resolutionMin
  selectedResolutionMax.value = resolutionMax
  selectedPostCreatedFrom.value = localDateStartEpochSeconds(postCreatedFromDate.value)
  selectedPostCreatedTo.value = localDateEndEpochSeconds(postCreatedToDate.value)
  selected.value = new Set()
  selectedMedia.value = new Map()
  excludedMediaIds.value = new Set()
}

function invertSelection(): void {
  const items = page.value?.items ?? []
  if (allMatchingSelected.value) {
    const nextExcluded = new Set(excludedMediaIds.value)
    for (const media of items) {
      if (nextExcluded.has(media.id)) nextExcluded.delete(media.id)
      else nextExcluded.add(media.id)
    }
    excludedMediaIds.value = nextExcluded
    return
  }
  const next = new Set(selected.value)
  const nextMedia = new Map(selectedMedia.value)
  for (const media of items) {
    if (next.has(media.id)) {
      next.delete(media.id)
      nextMedia.delete(media.id)
    } else {
      next.add(media.id)
      nextMedia.set(media.id, media)
    }
  }
  selected.value = next
  selectedMedia.value = nextMedia
}

async function createBatchTask(type: 'resize' | 'heic_convert' | 'tag_pipeline' | 'vllm_tag' | 'delete_selected'): Promise<void> {
  if (!activeRootId.value || !selectedCount.value) return
  if (type === 'heic_convert' && !heicSelectionEligible.value) return
  if (type === 'vllm_tag' && !vllmSelectionEligible.value) return
  creatingBatchTask.value = true
  const mediaIds = Array.from(selected.value).sort((left, right) => left.localeCompare(right))
  const selection = allMatchingSelected.value
    ? {
        library_query: selectedQuery.value,
        library_relative_directory: activeDirectory.value,
        ...(selectedScoreMin.value === undefined ? {} : { library_score_min: selectedScoreMin.value }),
        ...(selectedScoreMax.value === undefined ? {} : { library_score_max: selectedScoreMax.value }),
        ...(selectedResolutionMin.value === undefined ? {} : { library_resolution_min: selectedResolutionMin.value }),
        ...(selectedResolutionMax.value === undefined ? {} : { library_resolution_max: selectedResolutionMax.value }),
        ...(selectedPostCreatedFrom.value === undefined ? {} : { library_post_created_from: selectedPostCreatedFrom.value }),
        ...(selectedPostCreatedTo.value === undefined ? {} : { library_post_created_to: selectedPostCreatedTo.value }),
        excluded_media_ids: Array.from(excludedMediaIds.value).sort((left, right) => left.localeCompare(right)),
      }
    : { media_ids: mediaIds }
  const request: CreateTaskRequest = type === 'resize'
    ? { type, root_id: activeRootId.value, options: { ...selection, max_size: resizeMaxSize.value } }
    : { type, root_id: activeRootId.value, options: selection }
  try {
    await createTask(request)
    await tasks.loadSnapshot()
    clearSelection()
    const successMessage = {
      resize: '安全缩放任务已加入队列',
      heic_convert: 'HEIC 转换预检已加入队列',
      tag_pipeline: '标签处理预检已加入队列',
      vllm_tag: '视觉模型打标任务已加入队列',
      delete_selected: '删除任务已加入队列',
    }[type]
    if (type === 'delete_selected') {
      toast.success(successMessage, '请通过任务概览审阅确认；确认后媒体及同名标签文件会移入隔离区，可在隔离区恢复。')
    } else {
      toast.success(successMessage)
    }
  } catch (reason: unknown) {
    toast.error('无法创建批处理任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creatingBatchTask.value = false
  }
}

function search(): void {
  clearSelection()
  resetToFirstPage()
  void loadPage()
  void loadFacets()
}

function applyFilters(): void {
  if (postDateRangeInvalid.value) {
    toast.warning('发布时间范围无效', '起始日期不能晚于结束日期。')
    return
  }
  clearSelection()
  resetToFirstPage()
  void loadPage()
  void loadFacets()
}

function nextPage(): void {
  if (!page.value?.next_cursor) return
  cursor.value = page.value.next_cursor
  cursorBefore.value = false
  cursorDepth.value += 1
  scrollToPageTop()
  void loadPage()
}

function previousPage(): void {
  if (!page.value?.previous_cursor) return
  cursor.value = page.value.previous_cursor
  cursorBefore.value = true
  cursorDepth.value = Math.max(1, cursorDepth.value - 1)
  scrollToPageTop()
  void loadPage()
}

async function refreshLibrary(): Promise<void> {
  if (!activeRootId.value) return
  indexing.value = true
  try {
    await createTask({ type: 'index_library', root_id: activeRootId.value })
    await tasks.loadSnapshot()
    toast.success('图库刷新已加入队列', '会读取文件夹内容以同步新增图片，不会移动或删除现有媒体。')
  } catch (reason: unknown) {
    toast.error('无法刷新图库', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    indexing.value = false
  }
}

async function rebuildLibrary(): Promise<void> {
  if (!activeRootId.value) return
  reindexConfirmationOpen.value = false
  indexing.value = true
  try {
    await createTask({ type: 'reindex_library', root_id: activeRootId.value })
    await tasks.loadSnapshot()
    toast.success('图库重建已加入维护队列', '元数据数据库会先备份；原媒体不会移动或删除。')
  } catch (reason: unknown) {
    toast.error('无法创建图库重建任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    indexing.value = false
  }
}

function formatBytes(value: number): string {
  if (value < 1024 * 1024) return `${Math.max(1, Math.round(value / 1024))} KB`
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

function isVideo(media: LocalMedia): boolean {
  return media.mime_type.startsWith('video/')
}

function isHeicMedia(media: LocalMedia): boolean {
  const mime = media.mime_type.toLowerCase()
  const path = (media.relative_path || media.filename).toLowerCase()
  const hasHeicExtension = path.endsWith('.heic') || path.endsWith('.heif')
  const hasAcceptedMime = mime === 'image/heic' || mime === 'image/heif' || mime === 'application/octet-stream'
  return hasHeicExtension && hasAcceptedMime
}

function isVllmMedia(media: LocalMedia): boolean {
  const extension = (media.relative_path || media.filename).toLowerCase().match(/\.([^.\\/]+)$/)?.[1] ?? ''
  const mime = media.mime_type.toLowerCase()
  const extensions = new Set(['png', 'jpg', 'jpeg', 'bmp', 'webp', 'gif'])
  const mimeTypes = new Set(['image/png', 'image/jpeg', 'image/jpg', 'image/bmp', 'image/webp', 'image/gif'])
  return extensions.has(extension) && mimeTypes.has(mime)
}

function isObscured(media: LocalMedia): boolean {
  return config.config.blur_sensitive_media
    && requiresContentReveal(media.rating)
    && !revealedMedia.value.has(media.id)
}

function revealMedia(id: string): void {
  revealedMedia.value = new Set(revealedMedia.value).add(id)
}

function libraryPreviewUrl(media: LocalMedia): string {
  return libraryMediaUrl(media.id, fullSizePreviews.value.has(media.id) ? 'file' : 'thumbnail')
}

function useOriginalLibraryPreview(id: string): void {
  if (fullSizePreviews.value.has(id)) return
  fullSizePreviews.value = new Set(fullSizePreviews.value).add(id)
}

async function openDetail(media: LocalMedia): Promise<void> {
  detailOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null
  detail.value = media
  detailRevealed.value = false
  originalPreviewOpen.value = false
  originalPreviewOpener = null
  detailController?.abort()
  const requestController = new AbortController()
  detailController = requestController
  await nextTick()
  detailPanel.value?.focus()
  try {
    const refreshed = await getLibraryItem(media.id, requestController.signal)
    if (detailController === requestController && detail.value?.id === media.id) detail.value = refreshed
  } catch (reason: unknown) {
    if (!(reason instanceof DOMException && reason.name === 'AbortError')
      && detailController === requestController
      && detail.value?.id === media.id) {
      toast.error('无法刷新媒体详情', reason instanceof Error ? reason.message : '未知错误')
    }
  } finally {
    if (detailController === requestController) detailController = null
  }
}

async function closeDetail(): Promise<void> {
  const opener = detailOpener
  detailController?.abort()
  detailController = null
  originalPreviewOpen.value = false
  originalPreviewOpener = null
  detail.value = null
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
  originalPreviewOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null
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

function appendTagToSearch(tag: string): void {
  queryInput.value = `${queryInput.value} ${tag}`.trim()
  search()
  void closeDetail()
}

watch(activeRootId, () => {
  detailController?.abort()
  detailController = null
  detail.value = null
  originalPreviewOpen.value = false
  originalPreviewOpener = null
  fullSizePreviews.value = new Set()
  clearSelection()
})

function restoreSavedSelection(): void {
  if (!restoredView.restoreSelection) return
  const state = restoredView.state
  selected.value = new Set(state.selectedIds)
  selectedMedia.value = new Map(state.selectedMedia
    .filter((media) => state.selectedIds.includes(media.id))
    .map((media) => [media.id, media as unknown as LocalMedia]))
  allMatchingSelected.value = state.allMatchingSelected
  allMatchingTotal.value = state.allMatchingTotal
  selectedQuery.value = state.selectedQuery
  selectedScoreMin.value = state.selectedScoreMin
  selectedScoreMax.value = state.selectedScoreMax
  selectedResolutionMin.value = state.selectedResolutionMin
  selectedResolutionMax.value = state.selectedResolutionMax
  selectedPostCreatedFrom.value = state.selectedPostCreatedFrom
  selectedPostCreatedTo.value = state.selectedPostCreatedTo
  excludedMediaIds.value = new Set(state.excludedMediaIds)
}

function currentViewState(): LibraryViewState {
  return {
    version: 1,
    rootId: activeRootId.value,
    directory: activeDirectory.value,
    query: loadedQuery.value,
    scoreRange: scoreRangeInput.value,
    resolutionRange: resolutionRangeInput.value,
    postCreatedFromDate: postCreatedFromDate.value,
    postCreatedToDate: postCreatedToDate.value,
    cursor: cursor.value,
    before: cursorBefore.value,
    cursorDepth: cursorDepth.value,
    scrollY: Math.max(0, window.scrollY),
    selectedIds: Array.from(selected.value),
    selectedMedia: Array.from(selectedMedia.value.values()),
    allMatchingSelected: allMatchingSelected.value,
    allMatchingTotal: allMatchingTotal.value,
    selectedQuery: selectedQuery.value,
    selectedScoreMin: selectedScoreMin.value,
    selectedScoreMax: selectedScoreMax.value,
    selectedResolutionMin: selectedResolutionMin.value,
    selectedResolutionMax: selectedResolutionMax.value,
    selectedPostCreatedFrom: selectedPostCreatedFrom.value,
    selectedPostCreatedTo: selectedPostCreatedTo.value,
    excludedMediaIds: Array.from(excludedMediaIds.value),
  }
}

function persistViewState(): void {
  saveLibraryViewState(currentViewState())
}

onBeforeRouteLeave(() => {
  persistViewState()
  savedBeforeRouteLeave = true
})

onMounted(async () => {
  try {
    await loadRoots()
    await loadPage()
    await loadFacets()
    restoreSavedSelection()
    if (restoredView.restoreScroll && restoredView.state.scrollY > 0) {
      await nextTick()
      window.scrollTo({ top: restoredView.state.scrollY, behavior: 'auto' })
    }
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : '无法读取媒体库'
  }
})

onBeforeUnmount(() => {
  if (!savedBeforeRouteLeave) persistViewState()
  controller?.abort()
  detailController?.abort()
})
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Local library</p>
        <h1 class="page-title">本地图库</h1>
        <p class="page-description">基于本地数据库进行精确多标签查询。每页最多 60 项，媒体通过受控 ID 访问。</p>
      </div>
      <span class="inline">
        <button v-if="activeRoot" type="button" class="button" :disabled="indexing" @click="refreshLibrary">
          <RefreshCw :size="16" /> {{ indexing ? '正在创建' : '刷新图库' }}
        </button>
        <button v-if="activeRoot" type="button" class="button" :disabled="indexing" @click="reindexConfirmationOpen = true">重建索引</button>
        <RouterLink to="/settings" class="button"><FolderPlus :size="16" /> 管理根目录</RouterLink>
      </span>
    </header>

    <div v-if="roots.length" class="root-tabs" aria-label="媒体库">
      <button v-for="root in roots" :key="root.id" type="button" class="root-tab" :class="{ active: root.id === activeRootId }" @click="selectRoot(root.id)">
        {{ root.name }} · {{ root.media_count.toLocaleString() }}
      </button>
    </div>

    <div v-if="!roots.length && !loading" class="empty-state" style="margin-top: 20px">
      <div>
        <FolderPlus :size="30" />
        <strong>尚未添加下载位置</strong>
        <p>为了保护现有文件，应用不会自动注册或扫描目录。请先在设置中确认 Windows 与 Linux 路径映射。</p>
        <RouterLink to="/settings" class="button button-primary" style="margin-top: 16px">打开设置</RouterLink>
      </div>
    </div>

    <template v-else-if="activeRoot">
      <form class="search-box" style="margin-top: 20px" role="search" @submit.prevent="search">
        <Search :size="19" />
        <input v-model="queryInput" class="search-input" placeholder="精确标签，例如：1girl landscape（所有标签都必须匹配）">
        <button type="submit" class="button button-primary search-submit"><Search :size="15" /> 搜索</button>
      </form>

      <div class="library-filter-grid" aria-label="图库筛选">
        <label class="inline" for="library-directory">
          <span class="field-help">文件夹</span>
          <select id="library-directory" v-model="activeDirectory" class="input" aria-label="图库文件夹" @change="selectDirectory">
            <option value="">根目录（仅本层图片）</option>
            <option v-for="directory in folderDirectories" :key="directory" :value="directory">{{ directory }}</option>
          </select>
        </label>
        <label class="inline" for="library-score-range">
          <span class="field-help">评分区间</span>
          <select id="library-score-range" v-model="scoreRangeInput" class="input" aria-label="评分区间" @change="applyFilters">
            <option value="">全部评分</option>
            <option v-for="range in scoreRanges" :key="`${range.score_min}:${range.score_max}`" :value="`${range.score_min}:${range.score_max}`">
              {{ range.score_min }}–{{ range.score_max }}（{{ range.count.toLocaleString() }} 项）
            </option>
          </select>
        </label>
        <label class="inline" for="library-resolution-range">
          <span class="field-help">分辨率区间（短边）</span>
          <select id="library-resolution-range" v-model="resolutionRangeInput" class="input" aria-label="分辨率区间" @change="applyFilters">
            <option value="">全部分辨率</option>
            <option v-for="range in resolutionRanges" :key="`${range.resolution_min}:${range.resolution_max}`" :value="`${range.resolution_min}:${range.resolution_max}`">
              {{ range.resolution_min }}–{{ range.resolution_max }}px（{{ range.count.toLocaleString() }} 项）
            </option>
          </select>
        </label>
        <label class="inline" for="library-post-created-from">
          <span class="field-help">帖子发布日期起</span>
          <input
            id="library-post-created-from"
            v-model="postCreatedFromDate"
            class="input"
            type="date"
            aria-label="帖子发布日期起"
            :max="postCreatedToDate || undefined"
            @input="applyFilters"
          >
        </label>
        <label class="inline" for="library-post-created-to">
          <span class="field-help">帖子发布日期止</span>
          <input
            id="library-post-created-to"
            v-model="postCreatedToDate"
            class="input"
            type="date"
            aria-label="帖子发布日期止"
            :min="postCreatedFromDate || undefined"
            @input="applyFilters"
          >
        </label>
        <span v-if="postDateRangeInvalid" class="field-error" role="alert">起始日期不能晚于结束日期</span>
      </div>

      <div class="result-summary">
        <label v-if="page?.items.length" class="page-selection">
          <input
            type="checkbox"
            aria-label="全选搜索结果"
            :checked="allMatchingSelected"
            :indeterminate="someMatchingSelected"
            @change="toggleAllMatching"
          >
          <span>{{ loading ? '正在读取图库' : `${page?.total.toLocaleString() ?? 0} 项本地媒体` }}</span>
        </label>
        <p v-else>{{ loading ? '正在读取图库' : `${page?.total.toLocaleString() ?? 0} 项本地媒体` }}</p>
        <p>每页最多 60 项</p>
      </div>

      <div v-if="loading" class="loading-grid"><div v-for="index in 12" :key="index" class="skeleton" /></div>
      <div v-else-if="error" class="empty-state">
        <div><Images :size="30" /><strong>无法加载图库</strong><p>{{ error }}</p><button type="button" class="button" style="margin-top: 16px" @click="loadPage">重试</button></div>
      </div>
      <div v-else-if="page?.items.length" class="library-grid">
        <article v-for="media in page.items" :key="media.id" class="library-card" :class="{ 'is-selected': isMediaSelected(media.id) }">
          <label class="library-select">
            <input
              type="checkbox"
              :aria-label="`选择 ${media.filename}`"
              :checked="isMediaSelected(media.id)"
              @change="toggleMedia(media)"
            >
          </label>
          <button type="button" :aria-label="`查看 ${media.filename}`" @click="openDetail(media)">
            <img :src="libraryPreviewUrl(media)" :alt="media.filename" loading="lazy" decoding="async" :class="{ 'media-obscured': isObscured(media) }" @error="useOriginalLibraryPreview(media.id)">
            <span v-if="isVideo(media)" class="media-kind"><Play :size="14" fill="currentColor" /></span>
          </button>
          <button v-if="isObscured(media)" type="button" class="reveal-button" :aria-label="`显示 ${media.filename}`" @click="revealMedia(media.id)">
            <Eye :size="16" /> 显示内容
          </button>
          <footer class="library-card-footer">
            <span>{{ media.filename }}</span>
            <span>{{ formatBytes(media.size_bytes) }}</span>
            <time v-if="media.post_created_at" :datetime="media.post_created_at">帖子发布于 {{ formatPostCreatedAt(media.post_created_at) }}</time>
          </footer>
        </article>
      </div>
      <div v-else class="empty-state"><div><Images :size="30" /><strong>没有匹配的媒体</strong><p>尝试减少标签或清空查询。</p></div></div>

      <nav v-if="page?.items.length" class="pagination" :class="{ 'pagination-below-selection': selectedCount }" aria-label="图库分页">
        <button type="button" class="button button-small" :disabled="!page.previous_cursor" @click="previousPage"><ChevronLeft :size="15" /> 上一批</button>
        <span class="pagination-info">第 {{ cursorDepth }} 批 · 共 {{ page.total.toLocaleString() }} 项</span>
        <button type="button" class="button button-small" :disabled="!page.next_cursor" @click="nextPage">下一批 <ChevronRight :size="15" /></button>
      </nav>

      <Transition name="toast">
        <div v-if="selectedCount" class="selection-bar" role="region" aria-label="图库批量处理栏">
          <strong>已选择 {{ selectedCount }} 项</strong>
          <span class="selection-copy">任务只接收受控媒体 ID</span>
          <div class="selection-actions">
            <button type="button" class="button button-quiet" :disabled="creatingBatchTask" @click="invertSelection">反选</button>
            <button type="button" class="button button-quiet" :disabled="creatingBatchTask" @click="clearSelection">清除</button>
            <label class="inline" for="library-resize-max-size">
              <span class="field-help">最长边</span>
              <input id="library-resize-max-size" v-model.number="resizeMaxSize" class="input" type="number" min="1" max="8192" aria-label="缩放最长边像素">
            </label>
            <button type="button" class="button" :disabled="creatingBatchTask" aria-label="安全缩放所选" @click="createBatchTask('resize')">
              <FileImage :size="16" /> 安全缩放
            </button>
            <button type="button" class="button" :disabled="creatingBatchTask" aria-label="标签处理所选" @click="createBatchTask('tag_pipeline')">
              <Tags :size="16" /> 标签处理
            </button>
            <button type="button" class="button button-danger" :disabled="creatingBatchTask" aria-label="删除所选" @click="createBatchTask('delete_selected')">
              <Trash2 :size="16" /> 删除所选
            </button>
            <button
              type="button"
              class="button"
              :disabled="creatingBatchTask || !heicSelectionEligible"
              :aria-label="heicSelectionEligible ? 'HEIC 转换所选' : 'HEIC 转换不可用：所选媒体必须全部为 HEIC 或 HEIF 图片'"
              :aria-describedby="heicSelectionEligible ? undefined : 'heic-selection-hint'"
              @click="createBatchTask('heic_convert')"
            >
              <FileImage :size="16" /> HEIC 转换
            </button>
            <button
              type="button"
              class="button button-primary"
              :disabled="creatingBatchTask || !vllmSelectionEligible"
              :aria-label="vllmSelectionEligible ? '视觉模型打标所选' : '视觉模型打标不可用：所选媒体必须全部为支持的静态图片'"
              :aria-describedby="vllmSelectionEligible ? undefined : 'vllm-selection-hint'"
              @click="createBatchTask('vllm_tag')"
            >
              <Bot :size="16" /> 视觉打标
            </button>
          </div>
          <span v-if="!heicSelectionEligible" id="heic-selection-hint" class="selection-hint" role="status">HEIC 转换仅支持所选内容全部为 HEIC/HEIF 图片。</span>
          <span v-if="!vllmSelectionEligible" id="vllm-selection-hint" class="selection-hint" role="status">视觉模型打标仅支持 PNG、JPG/JPEG、BMP、WebP 或 GIF 图片。</span>
        </div>
      </Transition>
    </template>

    <template v-if="detail">
      <button type="button" class="detail-scrim" aria-label="关闭详情" @click="closeDetail" />
      <aside ref="detailPanel" class="detail-panel" role="dialog" aria-modal="true" aria-labelledby="library-detail-title" tabindex="-1" @keydown="onDetailKeydown">
        <header class="detail-header"><h2 id="library-detail-title">{{ detail.filename }}</h2><button type="button" class="button icon-button button-quiet" aria-label="关闭详情" @click="closeDetail"><X :size="19" /></button></header>
        <div class="detail-content">
          <div class="detail-media">
            <video v-if="isVideo(detail)" :src="libraryMediaUrl(detail.id)" controls preload="metadata" :class="{ 'media-obscured': detailObscured }" />
            <button
              v-else
              type="button"
              class="detail-media-button"
              :disabled="detailObscured"
              :aria-label="`放大查看本地图片 ${detail.filename}`"
              @click="openOriginalPreview"
            >
              <img :src="libraryMediaUrl(detail.id)" :alt="detail.filename" :class="{ 'media-obscured': detailObscured }">
            </button>
            <button v-if="detailObscured" type="button" class="reveal-button" :aria-label="`显示详情 ${detail.filename}`" @click="detailRevealed = true">
              <Eye :size="16" /> 显示内容
            </button>
          </div>
          <div class="detail-stats">
            <div class="detail-stat"><small>文件大小</small><strong>{{ formatBytes(detail.size_bytes) }}</strong></div>
            <div class="detail-stat"><small>尺寸</small><strong>{{ detail.width && detail.height ? `${detail.width} × ${detail.height}` : '未知' }}</strong></div>
            <div class="detail-stat"><small>帖子</small><strong>{{ detail.post_id ? `#${detail.post_id}` : '本地文件' }}</strong></div>
            <div class="detail-stat"><small>帖子发布时间</small><strong>{{ formatPostCreatedAt(detail.post_created_at) }}</strong></div>
          </div>
          <div class="tag-section"><h3>精确标签</h3><div class="tag-list"><button v-for="tag in detail.tags" :key="tag" type="button" class="tag" @click="appendTagToSearch(tag)">{{ tag }}</button></div></div>
          <div class="tag-section"><h3>相对路径</h3><code style="font-size: 11px; color: var(--text-secondary); word-break: break-all">{{ detail.relative_path }}</code></div>
        </div>
      </aside>
      <div
        v-if="originalPreviewOpen"
        ref="originalPreviewPanel"
        class="original-preview"
        role="dialog"
        aria-modal="true"
        :aria-label="`原图预览 ${detail.filename}`"
        tabindex="-1"
        @click.self="closeOriginalPreview"
        @keydown="onOriginalPreviewKeydown"
      >
        <button type="button" class="button icon-button original-preview-close" aria-label="关闭原图预览" @click="closeOriginalPreview"><X :size="20" /></button>
        <img :src="libraryMediaUrl(detail.id)" :alt="`${detail.filename} 原图`">
      </div>
    </template>
    <ConfirmDialog
      :open="reindexConfirmationOpen"
      title="后台重建图库索引"
      confirm-label="备份并开始重建"
      :busy="indexing"
      @cancel="reindexConfirmationOpen = false"
      @confirm="rebuildLibrary"
    >
      <p>系统会先创建 SQLite 一致性备份，再分批扫描并更新索引。原媒体文件不会移动或删除；任务可在任务中心暂停、恢复或取消。</p>
    </ConfirmDialog>
  </div>
</template>
