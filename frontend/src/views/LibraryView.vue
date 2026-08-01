<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Bot, ChevronLeft, ChevronRight, Eye, FileImage, FolderPlus, Images, Play, RefreshCw, Search, Tags, X } from '@lucide/vue'
import { createTask, getLibrary, getLibraryItem, getMediaRoots, libraryMediaUrl, type CreateTaskRequest, type LibraryPage, type LocalMedia, type MediaRoot } from '../api'
import { useConfigStore } from '../stores/config'
import { useTasksStore } from '../stores/tasks'
import { useToastStore } from '../stores/toast'
import { requiresContentReveal } from '../utils/contentRating'

const route = useRoute()
const router = useRouter()
const config = useConfigStore()
const tasks = useTasksStore()
const toast = useToastStore()
const roots = ref<MediaRoot[]>([])
const activeRootId = ref('')
const queryInput = ref('')
const page = ref<LibraryPage | null>(null)
const cursor = ref<string | undefined>()
const cursorHistory = ref<Array<string | undefined>>([])
const loading = ref(false)
const error = ref<string | null>(null)
const detail = ref<LocalMedia | null>(null)
const detailRevealed = ref(false)
const detailPanel = ref<HTMLElement | null>(null)
const indexing = ref(false)
const selected = ref<Set<string>>(new Set())
const selectedMedia = ref<Map<string, LocalMedia>>(new Map())
const revealedMedia = ref<Set<string>>(new Set())
const fullSizePreviews = ref<Set<string>>(new Set())
const creatingBatchTask = ref(false)
const resizeMaxSize = ref(1216)
let controller: AbortController | null = null
let detailController: AbortController | null = null
let detailOpener: HTMLElement | null = null

const activeRoot = computed(() => roots.value.find((root) => root.id === activeRootId.value) ?? null)
const selectedCount = computed(() => selected.value.size)
const currentPageIds = computed(() => page.value?.items.map((media) => media.id) ?? [])
const allCurrentPageSelected = computed(() => currentPageIds.value.length > 0 && currentPageIds.value.every((id) => selected.value.has(id)))
const someCurrentPageSelected = computed(() => currentPageIds.value.some((id) => selected.value.has(id)) && !allCurrentPageSelected.value)
const heicSelectionEligible = computed(() => selected.value.size > 0
  && selectedMedia.value.size === selected.value.size
  && Array.from(selectedMedia.value.values()).every(isHeicMedia))
const vllmSelectionEligible = computed(() => selected.value.size > 0
  && selectedMedia.value.size === selected.value.size
  && Array.from(selectedMedia.value.values()).every(isVllmMedia))
const detailObscured = computed(() => detail.value !== null
  && config.config.blur_sensitive_media
  && requiresContentReveal(detail.value.rating)
  && !detailRevealed.value)

async function loadRoots(): Promise<void> {
  roots.value = await getMediaRoots()
  const requested = typeof route.query.root === 'string' ? route.query.root : ''
  activeRootId.value = roots.value.some((root) => root.id === requested) ? requested : (roots.value[0]?.id ?? '')
}

async function loadPage(): Promise<void> {
  if (!activeRootId.value) {
    page.value = null
    return
  }
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = null
  try {
    page.value = await getLibrary({ rootId: activeRootId.value, query: queryInput.value.trim(), cursor: cursor.value, limit: 60 }, controller.signal)
  } catch (reason: unknown) {
    if (!(reason instanceof DOMException && reason.name === 'AbortError')) {
      error.value = reason instanceof Error ? reason.message : '图库加载失败'
      page.value = null
    }
  } finally {
    loading.value = false
  }
}

function selectRoot(id: string): void {
  activeRootId.value = id
  clearSelection()
  cursor.value = undefined
  cursorHistory.value = []
  void router.replace({ path: '/library', query: { root: id } })
  void loadPage()
}

function clearSelection(): void {
  selected.value = new Set()
  selectedMedia.value = new Map()
}

function toggleMedia(media: LocalMedia): void {
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

function toggleCurrentPage(): void {
  const next = new Set(selected.value)
  const nextMedia = new Map(selectedMedia.value)
  if (allCurrentPageSelected.value) {
    page.value?.items.forEach((media) => {
      next.delete(media.id)
      nextMedia.delete(media.id)
    })
  } else {
    page.value?.items.forEach((media) => {
      next.add(media.id)
      nextMedia.set(media.id, media)
    })
  }
  selected.value = next
  selectedMedia.value = nextMedia
}

async function createBatchTask(type: 'resize' | 'heic_convert' | 'tag_pipeline' | 'vllm_tag'): Promise<void> {
  if (!activeRootId.value || !selected.value.size) return
  if (type === 'heic_convert' && !heicSelectionEligible.value) return
  if (type === 'vllm_tag' && !vllmSelectionEligible.value) return
  creatingBatchTask.value = true
  const mediaIds = Array.from(selected.value).sort((left, right) => left.localeCompare(right))
  const request: CreateTaskRequest = type === 'resize'
    ? { type, root_id: activeRootId.value, options: { media_ids: mediaIds, max_size: resizeMaxSize.value } }
    : { type, root_id: activeRootId.value, options: { media_ids: mediaIds } }
  try {
    await createTask(request)
    await tasks.loadSnapshot()
    clearSelection()
    const successMessage = {
      resize: '安全缩放任务已加入队列',
      heic_convert: 'HEIC 转换预检已加入队列',
      tag_pipeline: '标签处理预检已加入队列',
      vllm_tag: '视觉模型打标任务已加入队列',
    }[type]
    toast.success(successMessage)
  } catch (reason: unknown) {
    toast.error('无法创建批处理任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creatingBatchTask.value = false
  }
}

function search(): void {
  cursor.value = undefined
  cursorHistory.value = []
  void loadPage()
}

function nextPage(): void {
  if (!page.value?.next_cursor) return
  cursorHistory.value.push(cursor.value)
  cursor.value = page.value.next_cursor
  void loadPage()
}

function previousPage(): void {
  if (!cursorHistory.value.length) return
  cursor.value = cursorHistory.value.pop()
  void loadPage()
}

async function refreshLibrary(): Promise<void> {
  if (!activeRootId.value) return
  indexing.value = true
  try {
    await createTask({ type: 'index_library', root_id: activeRootId.value })
    await tasks.loadSnapshot()
    toast.success('图库刷新已加入队列', '会读取文件夹内容以同步新增图片，不会移动或删除现有媒体。')
    void router.push('/tasks')
  } catch (reason: unknown) {
    toast.error('无法刷新图库', reason instanceof Error ? reason.message : '未知错误')
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

watch(activeRootId, () => {
  detailController?.abort()
  detailController = null
  detail.value = null
  fullSizePreviews.value = new Set()
})

onMounted(async () => {
  queryInput.value = typeof route.query.q === 'string' ? route.query.q : ''
  try {
    await loadRoots()
    await loadPage()
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : '无法读取媒体库'
  }
})

onBeforeUnmount(() => {
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

      <div class="result-summary">
        <label v-if="page?.items.length" class="page-selection">
          <input
            type="checkbox"
            aria-label="全选当前页"
            :checked="allCurrentPageSelected"
            :indeterminate="someCurrentPageSelected"
            @change="toggleCurrentPage"
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
        <article v-for="media in page.items" :key="media.id" class="library-card" :class="{ 'is-selected': selected.has(media.id) }">
          <label class="library-select">
            <input
              type="checkbox"
              :aria-label="`选择 ${media.filename}`"
              :checked="selected.has(media.id)"
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
          <footer><span>{{ media.filename }}</span><span>{{ formatBytes(media.size_bytes) }}</span></footer>
        </article>
      </div>
      <div v-else class="empty-state"><div><Images :size="30" /><strong>没有匹配的媒体</strong><p>尝试减少标签或清空查询。</p></div></div>

      <nav v-if="page?.items.length" class="pagination" aria-label="图库分页">
        <button type="button" class="button button-small" :disabled="!cursorHistory.length" @click="previousPage"><ChevronLeft :size="15" /> 上一页</button>
        <span class="pagination-info">{{ page.items.length }} 项</span>
        <button type="button" class="button button-small" :disabled="!page.next_cursor" @click="nextPage">下一页 <ChevronRight :size="15" /></button>
      </nav>

      <Transition name="toast">
        <div v-if="selectedCount" class="selection-bar" role="region" aria-label="图库批量处理栏">
          <strong>已选择 {{ selectedCount }} 项</strong>
          <span class="selection-copy">任务只接收受控媒体 ID</span>
          <div class="selection-actions">
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
            <img v-else :src="libraryMediaUrl(detail.id)" :alt="detail.filename" :class="{ 'media-obscured': detailObscured }">
            <button v-if="detailObscured" type="button" class="reveal-button" :aria-label="`显示详情 ${detail.filename}`" @click="detailRevealed = true">
              <Eye :size="16" /> 显示内容
            </button>
          </div>
          <div class="detail-stats">
            <div class="detail-stat"><small>文件大小</small><strong>{{ formatBytes(detail.size_bytes) }}</strong></div>
            <div class="detail-stat"><small>尺寸</small><strong>{{ detail.width && detail.height ? `${detail.width} × ${detail.height}` : '未知' }}</strong></div>
            <div class="detail-stat"><small>帖子</small><strong>{{ detail.post_id ? `#${detail.post_id}` : '本地文件' }}</strong></div>
          </div>
          <div class="tag-section"><h3>精确标签</h3><div class="tag-list"><button v-for="tag in detail.tags" :key="tag" type="button" class="tag" @click="queryInput = tag; search(); closeDetail()">{{ tag }}</button></div></div>
          <div class="tag-section"><h3>相对路径</h3><code style="font-size: 11px; color: var(--text-secondary); word-break: break-all">{{ detail.relative_path }}</code></div>
        </div>
      </aside>
    </template>
  </div>
</template>
