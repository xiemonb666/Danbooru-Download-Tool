<script setup lang="ts">
import { computed, onMounted, ref, type Component } from 'vue'
import { ArchiveRestore, Bot, FileCheck2, FileImage, Images, ScanSearch, Tags, Trash2 } from '@lucide/vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import {
  createTask,
  getMediaDirectories,
  getMediaRoots,
  getQuarantine,
  purgeQuarantine,
  restoreQuarantine,
  type MediaRoot,
  type QuarantineEntry,
  type RootTaskRequest,
} from '../api'
import { useTasksStore } from '../stores/tasks'
import { useToastStore } from '../stores/toast'

interface ToolDefinition {
  type: RootTaskRequest['type']
  title: string
  description: string
  action: string
  icon: Component
  preflight: boolean
}

const definitions: ToolDefinition[] = [
  { type: 'integrity_scan', title: '完整性检查', description: '支持的图片格式会进行完整解码；其他媒体容器仅执行基础文件检查。', action: '配置任务', icon: FileCheck2, preflight: true },
  { type: 'exact_dedup', title: '精确去重', description: '按文件大小与 SHA-256 识别完全相同的文件，先生成预检清单。', action: '配置任务', icon: ScanSearch, preflight: true },
  { type: 'near_dedup', title: '相似图片检查', description: '使用感知哈希寻找近似图片，结果始终需要人工确认；视频不会参与。', action: '配置任务', icon: Images, preflight: true },
  { type: 'resize', title: '安全缩放', description: '最长边和 JPEG 质量可调；原文件先进入隔离区，再原子发布结果。', action: '配置任务', icon: FileImage, preflight: true },
  { type: 'heic_convert', title: 'HEIC 转换', description: '可按相对目录批量选择 HEIC/HEIF，先预检再转换为 JPEG。', action: '配置任务', icon: FileImage, preflight: true },
  { type: 'delete_by_tag', title: '按标签隔离', description: '规范化标签 token 后精确匹配，将匹配项移入可恢复隔离区。', action: '配置任务', icon: Trash2, preflight: true },
  { type: 'tag_pipeline', title: '标签处理', description: '恢复分类排序、过滤和 artist:/@ 前缀规则，预检后原子替换。', action: '配置任务', icon: Tags, preflight: true },
  { type: 'vllm_tag', title: '视觉模型打标', description: '可按相对目录批量打标；语言、提示词、联网校验和并发由设置控制。', action: '配置任务', icon: Bot, preflight: false },
]

const roots = ref<MediaRoot[]>([])
const rootId = ref('')
const quarantine = ref<QuarantineEntry[]>([])
const loading = ref(false)
const creating = ref(false)
const selectedTool = ref<ToolDefinition | null>(null)
const confirmingPurge = ref(false)
const tag = ref('')
const phashDistance = ref(8)
const scope = ref<'root' | 'directory'>('root')
const relativeDirectory = ref('')
const directories = ref<string[]>([])
const directoriesLoading = ref(false)
const directoryLoadError = ref(false)
const manualDirectory = ref(false)
const resizeMaxSize = ref(1216)
const resizeQuality = ref(90)
const artistPrefix = ref<'artist' | 'at'>('artist')
const tasks = useTasksStore()
const toast = useToastStore()
const boundedSelectionTasks = new Set<RootTaskRequest['type']>(['resize', 'heic_convert', 'tag_pipeline', 'vllm_tag'])

const activeRoot = computed(() => roots.value.find((root) => root.id === rootId.value) ?? null)

function formatDirectory(path: string): string {
  return path.split('/').join(' / ')
}

async function loadDirectories(): Promise<void> {
  relativeDirectory.value = ''
  directories.value = []
  directoryLoadError.value = false
  manualDirectory.value = false
  if (!rootId.value) return
  directoriesLoading.value = true
  try {
    const result = await getMediaDirectories(rootId.value)
    directories.value = result.directories
  } catch {
    directoryLoadError.value = true
    manualDirectory.value = true
  } finally {
    directoriesLoading.value = false
  }
}

async function changeRoot(): Promise<void> {
  await Promise.all([loadQuarantine(), loadDirectories()])
}

async function loadQuarantine(): Promise<void> {
  if (!rootId.value) {
    quarantine.value = []
    return
  }
  loading.value = true
  try {
    quarantine.value = await getQuarantine(rootId.value)
  } catch (reason: unknown) {
    toast.error('无法读取隔离区', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    loading.value = false
  }
}

function chooseTool(tool: ToolDefinition): void {
  if (!rootId.value) {
    toast.warning('请选择媒体库')
    return
  }
  selectedTool.value = tool
}

async function createSelectedTask(): Promise<void> {
  if (!selectedTool.value || !rootId.value) return
  if (selectedTool.value.type === 'delete_by_tag' && !tag.value.trim()) {
    toast.warning('请输入精确标签')
    return
  }
  if (scope.value === 'directory' && !relativeDirectory.value.trim()) {
    toast.warning('请输入根目录内的相对目录')
    return
  }
  creating.value = true
  const kind = selectedTool.value.type
  const relative_directory = scope.value === 'directory'
    ? relativeDirectory.value.trim()
    : boundedSelectionTasks.has(kind) ? '.' : undefined
  let request: RootTaskRequest
  if (kind === 'delete_by_tag') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, tag: tag.value.trim(), relative_directory } }
  } else if (kind === 'near_dedup') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, distance: phashDistance.value, relative_directory } }
  } else if (kind === 'exact_dedup' || kind === 'integrity_scan') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, relative_directory } }
  } else if (kind === 'resize') {
    request = { type: kind, root_id: rootId.value, options: { relative_directory, max_size: resizeMaxSize.value, quality: resizeQuality.value } }
  } else if (kind === 'heic_convert' || kind === 'vllm_tag') {
    request = { type: kind, root_id: rootId.value, options: { relative_directory } }
  } else {
    request = { type: kind, root_id: rootId.value, options: { relative_directory, artist_prefix: artistPrefix.value } }
  }
  try {
    await createTask(request)
    await tasks.loadSnapshot()
    toast.success(selectedTool.value.preflight ? '预检任务已加入队列' : '处理任务已加入队列')
    selectedTool.value = null
  } catch (reason: unknown) {
    toast.error('无法创建任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creating.value = false
  }
}

async function restore(entry: QuarantineEntry): Promise<void> {
  try {
    await restoreQuarantine(entry.id)
    quarantine.value = quarantine.value.filter((item) => item.id !== entry.id)
    toast.success('文件已恢复', '发生路径冲突时服务器不会覆盖现有文件。')
  } catch (reason: unknown) {
    toast.error('无法恢复文件', reason instanceof Error ? reason.message : '未知错误')
  }
}

async function purge(): Promise<void> {
  if (!rootId.value) return
  creating.value = true
  try {
    const result = await purgeQuarantine(rootId.value)
    quarantine.value = []
    confirmingPurge.value = false
    toast.success(`已永久清理 ${result.purged} 项`)
  } catch (reason: unknown) {
    toast.error('无法清理隔离区', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creating.value = false
  }
}

function formatBytes(value: number): string {
  return value < 1024 * 1024 ? `${Math.ceil(value / 1024)} KB` : `${(value / 1024 / 1024).toFixed(1)} MB`
}

onMounted(async () => {
  try {
    roots.value = await getMediaRoots()
    rootId.value = roots.value[0]?.id ?? ''
    await Promise.all([loadQuarantine(), loadDirectories()])
  } catch (reason: unknown) {
    toast.error('工具初始化失败', reason instanceof Error ? reason.message : '未知错误')
  }
})
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Processing tools</p>
        <h1 class="page-title">处理与隔离</h1>
        <p class="page-description">所有处理均进入统一任务系统。同一媒体库同时只执行一个写任务，危险操作先预检再确认。</p>
      </div>
      <select v-model="rootId" class="select" style="width: min(260px, 100%)" aria-label="媒体库" @change="changeRoot">
        <option value="" disabled>选择媒体库</option>
        <option v-for="root in roots" :key="root.id" :value="root.id">{{ root.name }}</option>
      </select>
    </header>

    <div v-if="!roots.length" class="empty-state">
      <div><Images :size="30" /><strong>先添加一个下载位置</strong><p>为了防止误处理系统文件，工具只能处理已添加媒体库中的内容。</p><RouterLink to="/settings" class="button button-primary" style="margin-top: 16px">前往设置</RouterLink></div>
    </div>

    <template v-else>
      <div class="notice warning" style="margin-bottom: 18px">预检任务不会删除文件。每个工具都可处理整个 {{ activeRoot?.name }}，或递归处理根目录内的相对目录；也可继续在图库中精确勾选媒体。</div>
      <section class="tool-grid" aria-label="处理工具">
        <article v-for="tool in definitions" :key="tool.type" class="tool-card">
          <div class="tool-heading"><span><component :is="tool.icon" :size="18" /></span><h2>{{ tool.title }}</h2></div>
          <p>{{ tool.description }}</p>
          <button type="button" class="button" @click="chooseTool(tool)">{{ tool.action }}</button>
        </article>
      </section>

      <section class="surface" style="margin-top: 24px">
        <header class="surface-header">
          <div><h2 class="section-title">隔离区</h2><p class="section-copy">保留原相对路径，恢复时从不覆盖冲突文件。</p></div>
          <button type="button" class="button button-small button-danger" :disabled="!quarantine.length" @click="confirmingPurge = true"><Trash2 :size="14" /> 手动清空</button>
        </header>
        <div class="surface-body">
          <div v-if="loading" class="section-copy">正在读取隔离区</div>
          <div v-else-if="quarantine.length" class="stack">
            <div v-for="entry in quarantine" :key="entry.id" class="root-card">
              <div class="root-card-header"><strong>{{ entry.original_relative_path }}</strong><button type="button" class="button button-small" @click="restore(entry)"><ArchiveRestore :size="14" /> 恢复</button></div>
              <div class="path-row"><span>{{ formatBytes(entry.size_bytes) }}</span><code>{{ entry.reason }}</code></div>
            </div>
          </div>
          <div v-else class="section-copy">隔离区为空。应用不会自动清空之后加入的内容。</div>
        </div>
      </section>
    </template>

    <ConfirmDialog
      :open="selectedTool !== null"
      :title="selectedTool?.preflight ? `创建${selectedTool?.title}预检` : `创建${selectedTool?.title}任务`"
      :confirm-label="selectedTool?.preflight ? '开始预检' : '创建任务'"
      :busy="creating"
      @cancel="selectedTool = null"
      @confirm="createSelectedTask"
    >
      <p style="margin-top: 0">{{ selectedTool?.description }}</p>
      <div class="field">
        <label class="field-label" for="tool-scope">处理范围</label>
        <select id="tool-scope" v-model="scope" class="select">
          <option value="root">整个媒体库</option>
          <option value="directory">媒体库内的文件夹</option>
        </select>
      </div>
      <div v-if="scope === 'directory'" class="field">
        <label class="field-label" for="tool-relative-directory">库内文件夹</label>
        <select v-if="!manualDirectory" id="tool-relative-directory" v-model="relativeDirectory" class="select" :disabled="directoriesLoading">
          <option value="" disabled>{{ directoriesLoading ? '正在读取文件夹…' : directories.length ? '选择一个已有文件夹' : '没有找到已有文件夹' }}</option>
          <option v-for="directory in directories" :key="directory" :value="directory">{{ formatDirectory(directory) }}</option>
        </select>
        <input v-else id="tool-relative-directory" v-model="relativeDirectory" class="input" placeholder="例如：portraits/2026" autocomplete="off">
        <span class="inline">
          <button type="button" class="button button-small button-quiet" @click="manualDirectory = !manualDirectory; relativeDirectory = ''">{{ manualDirectory ? '从已有文件夹选择' : '手动输入路径' }}</button>
          <RouterLink to="/settings" class="button button-small button-quiet">管理分类文件夹</RouterLink>
        </span>
        <span v-if="directoryLoadError" class="field-help">暂时无法读取文件夹，已切换为手动输入。</span>
        <span v-else class="field-help">递归处理该文件夹及其子目录；只包含已刷新到图库中的媒体。</span>
      </div>
      <div v-if="selectedTool?.type === 'delete_by_tag'" class="field">
        <label class="field-label" for="delete-tag">精确标签</label>
        <input id="delete-tag" v-model="tag" class="input" placeholder="例如：watermark">
      </div>
      <div v-if="selectedTool?.type === 'near_dedup'" class="field">
        <label class="field-label" for="phash-distance">感知哈希距离阈值</label>
        <input id="phash-distance" v-model.number="phashDistance" class="input" type="number" min="1" max="32">
      </div>
      <template v-if="selectedTool?.type === 'resize'">
        <div class="field">
          <label class="field-label" for="resize-max-size">最长边像素</label>
          <input id="resize-max-size" v-model.number="resizeMaxSize" class="input" type="number" min="1" max="8192">
        </div>
        <div class="field">
          <label class="field-label" for="resize-quality">JPEG 质量</label>
          <input id="resize-quality" v-model.number="resizeQuality" class="input" type="number" min="1" max="100">
        </div>
      </template>
      <div v-if="selectedTool?.type === 'tag_pipeline'" class="field">
        <label class="field-label" for="artist-prefix">艺术家标签前缀</label>
        <select id="artist-prefix" v-model="artistPrefix" class="select">
          <option value="artist">artist:标签</option>
          <option value="at">@标签</option>
        </select>
      </div>
    </ConfirmDialog>

    <ConfirmDialog
      :open="confirmingPurge"
      title="永久清空隔离区"
      confirm-label="永久删除"
      destructive
      :busy="creating"
      @cancel="confirmingPurge = false"
      @confirm="purge"
    >
      此操作将永久删除 {{ quarantine.length }} 个隔离项，且无法恢复。隔离区默认不会自动清空，只有在确认不再需要恢复时才执行。
    </ConfirmDialog>
  </div>
</template>
