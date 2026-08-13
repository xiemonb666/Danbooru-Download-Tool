<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { AlertCircle, Check, CirclePause, Download, History, ListTodo, Pause, Play, RefreshCw, RotateCcw, Trash2, X } from '@lucide/vue'
import {
  createTask,
  deleteTask,
  getDownloadHistory,
  getTaskDetails,
  type DownloadHistoryRecord,
  type TaskDetails,
  type TaskDetailsParams,
  type TaskItemStatus,
  type TaskStatus,
  type TaskSummary,
} from '../api'
import { useTasksStore } from '../stores/tasks'
import { useToastStore } from '../stores/toast'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import TrainingMonitor from '../components/TrainingMonitor.vue'
import { scrollToPageTop } from '../utils/pageScroll'

const tasks = useTasksStore()
const toast = useToastStore()
const filter = ref<'all' | 'active' | 'completed' | 'failed'>('all')
const section = ref<'tasks' | 'history'>('tasks')
const controlling = ref<Set<string>>(new Set())
const confirmingTask = ref<TaskSummary | null>(null)
const deletingTask = ref<TaskSummary | null>(null)
const historyItems = ref<DownloadHistoryRecord[]>([])
const historyLoading = ref(false)
const historyError = ref<string | null>(null)
const historyNextCursor = ref<string | undefined>()
const historyLoaded = ref(false)
const repeating = ref<Set<string>>(new Set())
const expandedTaskId = ref<string | null>(null)
const details = ref<TaskDetails | null>(null)
const detailsLoading = ref(false)
const detailsError = ref<string | null>(null)
const detailsStatus = ref<TaskItemStatus | 'all'>('all')
let detailsController: AbortController | null = null

const visibleTasks = computed(() => {
  const all = tasks.sortedTasks()
  const filtered = filter.value === 'active'
    ? all.filter((task) => ['queued', 'running', 'pausing', 'paused', 'cancelling', 'awaiting_confirmation'].includes(task.status))
    : filter.value === 'completed'
      ? all.filter((task) => task.status === 'completed')
      : filter.value === 'failed'
        ? all.filter((task) => task.status === 'failed' || task.status === 'cancelled')
        : all
  const priority = (task: TaskSummary): number => ({
    running: 0, pausing: 1, queued: 2, paused: 3, cancelling: 4, awaiting_confirmation: 5,
    completed: 6, failed: 7, cancelled: 8,
  }[task.status] ?? 9)
  return [...filtered].sort((left, right) => {
    const byStatus = priority(left) - priority(right)
    if (byStatus !== 0) return byStatus
    return right.updated_at.localeCompare(left.updated_at)
  })
})

const labels: Record<TaskStatus, string> = {
  queued: '等待中', running: '进行中', pausing: '正在暂停', paused: '已暂停', cancelling: '正在取消',
  awaiting_confirmation: '等待确认', completed: '已完成', failed: '失败', cancelled: '已取消',
}

const kindLabels: Record<string, string> = {
  download: '下载', index_library: '刷新图库', reindex_library: '重建图库索引', integrity_scan: '完整性检查', exact_dedup: '精确去重',
  near_dedup: '相似图片检查', resize: '缩放图片', heic_convert: 'HEIC 转换', delete_by_tag: '按标签隔离', delete_selected: '删除所选媒体',
  tag_pipeline: '标签处理', vllm_tag: '视觉模型打标', dataset_augmentation: '数据集增广', training: 'LoRA 训练', runtime_install: '运行时安装',
}

const resourceLabels: Record<string, string> = {
  network: '网络', io: '磁盘 I/O', cpu: 'CPU 重任务', gpu: 'GPU 独占', maintenance: '维护',
}

function percent(task: TaskSummary): number {
  if (task.progress.total <= 0) return task.status === 'completed' ? 100 : 0
  return Math.min(100, Math.round((task.progress.completed / task.progress.total) * 100))
}

function formatBytes(value: number): string {
  if (value <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const index = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024)))
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`
}

function formatEta(seconds?: number | null): string {
  if (seconds == null || !Number.isFinite(seconds)) return '计算中'
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} 秒`
  const minutes = Math.round(seconds / 60)
  return minutes < 60 ? `${minutes} 分钟` : `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分钟`
}

function formatDateTime(value?: string | null): string {
  if (!value) return '尚未结束'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString()
}

type ResultObject = Record<string, unknown>

interface ResultSummary {
  label: string
  value: string
}

interface ResultItem {
  id: string
  dimensions?: string
  bytes?: number
}

function asResultObject(value: unknown): ResultObject | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as ResultObject
    : null
}

function resultNumber(value: unknown, key: string): number | undefined {
  const candidate = asResultObject(value)?.[key]
  return typeof candidate === 'number' && Number.isFinite(candidate) ? candidate : undefined
}

function resultBoolean(value: unknown, key: string): boolean {
  return asResultObject(value)?.[key] === true
}

function resultText(value: unknown, key: string): string | undefined {
  const candidate = asResultObject(value)?.[key]
  return typeof candidate === 'string' ? candidate : undefined
}

function nestedResultObject(value: unknown, key: string): ResultObject | null {
  return asResultObject(asResultObject(value)?.[key])
}

function nestedResultNumber(value: unknown, parent: string, key: string): number | undefined {
  const candidate = nestedResultObject(value, parent)?.[key]
  return typeof candidate === 'number' && Number.isFinite(candidate) ? candidate : undefined
}

function resultCountEntries(value: unknown, key: string): ResultSummary[] {
  const record = nestedResultObject(value, key)
  if (!record) return []
  return Object.entries(record)
    .filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]))
    .map(([label, count]) => ({ label, value: `${count.toLocaleString()} 项` }))
    .sort((left, right) => right.value.localeCompare(left.value, 'zh-CN'))
}

function resultTags(value: unknown): string[] {
  const tags = asResultObject(value)?.tags
  return Array.isArray(tags) ? tags.filter((tag): tag is string => typeof tag === 'string') : []
}

function taskResultItems(value: unknown): ResultItem[] {
  const items = asResultObject(value)?.items
  if (!Array.isArray(items)) return []
  return items.flatMap((item, index) => {
    const record = asResultObject(item)
    if (!record) return []
    const width = typeof record.width === 'number' ? record.width : undefined
    const height = typeof record.height === 'number' ? record.height : undefined
    const bytes = typeof record.bytes === 'number' ? record.bytes : undefined
    const mediaId = typeof record.media_id === 'string' ? record.media_id : undefined
    return [{
      id: mediaId || `项目 ${index + 1}`,
      dimensions: width != null && height != null ? `${width} × ${height}` : undefined,
      bytes,
    }]
  })
}

const smartCropReasonLabels: Record<string, string> = {
  detection_failed: '检测失败',
  analysis_dimension_mismatch: '检测尺寸不一致',
  no_primary_person: '未找到主人物',
  ambiguous_overlapping_people: '多人严重重叠',
  no_head_or_face: '缺少头部或人脸',
  incomplete_pose: '姿态不完整',
  lower_body_evidence_missing: '下半身证据不足',
  feet_evidence_missing: '完整脚部证据不足',
  complete_both_feet_required: '未满足完整双脚',
  secondary_person_included: '无法排除其他人物',
  native_resolution_too_low: '原生分辨率不足',
  family_limit: '达到 family 数量上限',
  quality_rule_rejected: '未通过构图质量规则',
}

function taskResultSummary(kind: string, value: unknown): ResultSummary[] {
  const entries: ResultSummary[] = []
  const addCount = (key: string, label: string) => {
    const count = resultNumber(value, key)
    if (count != null) entries.push({ label, value: `${count.toLocaleString()} 项` })
  }
  switch (kind) {
    case 'download':
      addCount('downloaded', '已下载')
      addCount('skipped', '已跳过')
      addCount('failed', '失败')
      {
        const bytes = resultNumber(value, 'bytes')
        if (bytes != null) entries.push({ label: '下载总量', value: formatBytes(bytes) })
      }
      break
    case 'index_library':
      addCount('indexed', '已刷新')
      addCount('deleted', '已移除失效记录')
      break
    case 'resize':
    case 'heic_convert': {
      const items = taskResultItems(value)
      const processed = resultNumber(value, 'processed') ?? items.length
      if (processed) entries.push({ label: '已处理', value: `${processed.toLocaleString()} 项` })
      break
    }
    case 'exact_dedup':
    case 'near_dedup':
    case 'integrity_scan':
    case 'delete_by_tag':
    case 'delete_selected':
      addCount('moved', '已移入隔离区')
      break
    case 'tag_pipeline':
      addCount('changed', '已处理标签文件')
      break
    case 'vllm_tag':
      addCount('tagged', '已完成打标')
      break
    case 'dataset_augmentation':
      addCount('source_images', '源原图（未复制）')
      addCount('generated', '已生成')
      addCount('rejected', '已拒绝')
      addCount('retagging_pending', '待重新打标')
      addCount('retagged', '已二次打标')
      {
        const variants = nestedResultObject(value, 'variant_counts')
        const variantLabels: Array<[string, string]> = [
          ['horizontal_flip', '水平翻转'],
          ['portrait', '肖像裁剪'],
          ['upper_body', '上半身裁剪'],
          ['cowboy_shot', '牛仔视角裁剪'],
          ['full_body_tight', '紧凑全身裁剪'],
          ['lower_body', '下半身裁剪'],
          ['feet', '脚部视角裁剪'],
        ]
        for (const [variant, label] of variantLabels) {
          const count = variants?.[variant]
          if (typeof count === 'number' && Number.isFinite(count)) entries.push({ label, value: `${count.toLocaleString()} 项` })
        }
        const byVariant = nestedResultObject(nestedResultObject(value, 'smart_crop'), 'by_variant')
        for (const [variant, label] of variantLabels.slice(1)) {
          const details = asResultObject(byVariant?.[variant])
          if (!details) continue
          const requested = typeof details.requested === 'number' ? details.requested : 0
          const generated = typeof details.generated === 'number' ? details.generated : 0
          const rejected = typeof details.rejected === 'number' ? details.rejected : 0
          entries.push({ label: `${label}结果`, value: `${requested} 请求 / ${generated} 生成 / ${rejected} 拒绝` })
          const reasons = asResultObject(details.rejection_reasons)
          if (reasons) {
            const leading = Object.entries(reasons)
              .filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isFinite(entry[1]))
              .sort((left, right) => right[1] - left[1])[0]
            if (leading) entries.push({ label: `${label}主要拒绝`, value: `${smartCropReasonLabels[leading[0]] ?? leading[0]} ${leading[1]} 项` })
          }
        }
      }
      {
        const rejected = nestedResultNumber(value, 'smart_crop', 'rejected')
        if (rejected != null) entries.push({ label: '智能裁剪拒绝', value: `${rejected.toLocaleString()} 项` })
        const coverage = nestedResultObject(nestedResultObject(value, 'smart_crop'), 'coverage_percent')
        const coverageLabels: Array<[string, string]> = [
          ['portrait', '肖像平均保留'],
          ['upper_body', '上半身平均保留'],
          ['cowboy_shot', '牛仔视角平均保留'],
          ['full_body_tight', '紧凑全身平均保留'],
          ['lower_body', '下半身平均保留'],
          ['feet', '脚部视角平均保留'],
        ]
        for (const [variant, label] of coverageLabels) {
          const average = asResultObject(coverage?.[variant])?.average
          if (typeof average === 'number' && Number.isFinite(average)) entries.push({ label, value: `${Math.round(average)}%` })
        }
      }
      {
        const output = resultText(value, 'training_relative_directory') ?? resultText(value, 'derived_relative_directory')
        if (output) entries.push({ label: '训练目录', value: output })
      }
      break
    case 'training':
      entries.push({ label: '训练运行', value: resultText(value, 'adapter_id') ?? 'SDXL LoRA' })
      break
  }
  return entries
}

async function loadHistory(cursor?: string): Promise<void> {
  historyLoading.value = true
  historyError.value = null
  try {
    const page = await getDownloadHistory({ limit: 50, cursor })
    historyItems.value = cursor ? [...historyItems.value, ...page.items] : page.items
    historyNextCursor.value = page.next_cursor ?? undefined
    historyLoaded.value = true
  } catch (reason: unknown) {
    historyError.value = reason instanceof Error ? reason.message : '下载记录加载失败'
  } finally {
    historyLoading.value = false
  }
}

function showHistory(): void {
  section.value = 'history'
  if (!historyLoaded.value && !historyLoading.value) void loadHistory()
}

async function repeatDownload(record: DownloadHistoryRecord): Promise<void> {
  if (!record.can_repeat || !record.repeat_request) return
  repeating.value = new Set(repeating.value).add(record.id)
  try {
    await createTask(record.repeat_request)
    await tasks.loadSnapshot()
    filter.value = 'active'
    section.value = 'tasks'
  } catch (reason: unknown) {
    toast.error('无法再次下载', reason instanceof Error ? reason.message : '创建下载任务失败')
  } finally {
    const next = new Set(repeating.value)
    next.delete(record.id)
    repeating.value = next
  }
}

async function control(task: TaskSummary, action: 'pause' | 'resume' | 'cancel' | 'retry' | 'confirm'): Promise<void> {
  const next = new Set(controlling.value)
  next.add(task.id)
  controlling.value = next
  try {
    await tasks.control(task.id, action)
  } catch (reason: unknown) {
    toast.error('任务操作失败', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    const done = new Set(controlling.value)
    done.delete(task.id)
    controlling.value = done
  }
}

async function confirmReviewedTask(): Promise<void> {
  const task = confirmingTask.value
  if (!task) return
  await control(task, 'confirm')
  confirmingTask.value = null
}

async function confirmDeleteTask(): Promise<void> {
  const task = deletingTask.value
  if (!task) return
  const next = new Set(controlling.value)
  next.add(task.id)
  controlling.value = next
  try {
    await deleteTask(task.id)
    toast.success('任务记录已删除')
  } catch (reason: unknown) {
    toast.error('删除任务记录失败', reason instanceof Error ? reason.message : '请稍后重试')
  } finally {
    deletingTask.value = null
    const done = new Set(controlling.value)
    done.delete(task.id)
    controlling.value = done
    await tasks.loadSnapshot()
  }
}

function confirmationLabel(_task: TaskSummary): string {
  return '移入隔离区'
}

function taskCanRetry(task: TaskSummary): boolean {
  return task.status === 'failed' && task.failures.some((failure) => failure.retryable)
}

async function toggleDetails(task: TaskSummary): Promise<void> {
  if (expandedTaskId.value === task.id) {
    detailsController?.abort()
    expandedTaskId.value = null
    details.value = null
    detailsError.value = null
    return
  }
  detailsController?.abort()
  expandedTaskId.value = task.id
  details.value = null
  detailsError.value = null
  detailsStatus.value = 'all'
  await loadDetailsPage(task)
}

async function loadDetailsPage(task: TaskSummary, cursor?: string): Promise<void> {
  detailsController?.abort()
  const controller = new AbortController()
  detailsController = controller
  detailsError.value = null
  detailsLoading.value = true
  try {
    const params: TaskDetailsParams = { itemLimit: 50 }
    if (detailsStatus.value !== 'all') params.itemStatus = detailsStatus.value
    if (cursor) params.itemCursor = cursor
    details.value = await getTaskDetails(task.id, params, controller.signal)
  } catch (reason: unknown) {
    if (!controller.signal.aborted) {
      detailsError.value = reason instanceof Error ? reason.message : '任务详情加载失败'
    }
  } finally {
    if (detailsController === controller) detailsLoading.value = false
  }
}

function loadNextDetailsPage(task: TaskSummary): void {
  const nextCursor = details.value?.next_cursor
  if (!nextCursor) return
  scrollToPageTop()
  void loadDetailsPage(task, nextCursor)
}

watch(
  () => {
    const id = expandedTaskId.value
    if (!id) return null
    const task = visibleTasks.value.find((candidate) => candidate.id === id)
    return task ? { task, revision: task.revision } : null
  },
  (current, previous) => {
    if (current && previous
      && current.task.id === previous.task.id
      && current.revision !== previous.revision) {
      void loadDetailsPage(current.task)
    }
  },
)

onBeforeUnmount(() => detailsController?.abort())
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Task center</p>
        <h1 class="page-title">任务中心</h1>
        <p class="page-description">任务状态会跨页面保留。断线后自动通过快照恢复，事件序列缺口会触发重新同步。</p>
      </div>
      <button v-if="section === 'tasks'" type="button" class="button" :disabled="tasks.loading" @click="() => tasks.loadSnapshot()">
        <RefreshCw :size="16" /> 同步状态
      </button>
      <button v-else type="button" class="button" :disabled="historyLoading" @click="loadHistory()">
        <RefreshCw :size="16" /> 刷新记录
      </button>
    </header>

    <div class="section-switcher segmented" aria-label="任务中心视图">
      <button type="button" :class="{ active: section === 'tasks' }" @click="section = 'tasks'">实时任务</button>
      <button type="button" :class="{ active: section === 'history' }" @click="showHistory"><History :size="14" /> 下载记录</button>
    </div>

    <div v-if="section === 'tasks'" class="result-summary">
      <div class="segmented" aria-label="任务筛选">
        <button v-for="item in ([['all', '全部'], ['active', '进行中'], ['completed', '已完成'], ['failed', '失败']] as const)" :key="item[0]" type="button" :class="{ active: filter === item[0] }" @click="filter = item[0]">{{ item[1] }}</button>
      </div>
      <p>
        <span class="health-dot" :class="tasks.connection === 'live' ? 'online' : tasks.connection === 'offline' ? 'offline' : 'checking'" style="display: inline-block; margin-right: 6px" />
        {{ tasks.connection === 'live' ? '实时连接' : tasks.connection === 'offline' ? '连接中断，正在恢复' : '正在连接任务流' }}
      </p>
    </div>

    <div v-if="section === 'tasks' && visibleTasks.length" class="task-list">
      <article v-for="task in visibleTasks" :key="task.id" class="task-card">
        <div class="task-top">
          <span class="task-icon">
            <Check v-if="task.status === 'completed'" :size="18" />
            <AlertCircle v-else-if="task.status === 'failed'" :size="18" />
            <CirclePause v-else-if="task.status === 'pausing' || task.status === 'paused'" :size="18" />
            <ListTodo v-else :size="18" />
          </span>
          <div class="task-title">
            <strong>{{ task.title || kindLabels[task.kind] }}</strong>
            <small>{{ kindLabels[task.kind] }} · {{ new Date(task.updated_at).toLocaleString() }}</small>
          </div>
          <span class="status-pill" :class="task.status">{{ labels[task.status] }}</span>
        </div>

        <div class="progress-track" :aria-label="`任务进度 ${percent(task)}%`">
          <div class="progress-fill" :style="{ width: `${percent(task)}%` }" />
        </div>
        <div class="task-stats">
<span>{{ task.kind === 'training' || task.kind === 'runtime_install' ? `${task.progress.completed.toLocaleString()} / ${task.progress.total.toLocaleString()} 阶段` : `${task.progress.completed.toLocaleString()} / ${task.progress.total.toLocaleString()} 项` }}</span>
<span>{{ percent(task) }}%</span>
<span v-if="task.progress.bytes_downloaded && task.kind !== 'training' && task.kind !== 'runtime_install'">{{ formatBytes(task.progress.bytes_downloaded) }}</span>
<span v-if="task.progress.speed_bytes_per_sec && task.kind !== 'training' && task.kind !== 'runtime_install'">{{ formatBytes(task.progress.speed_bytes_per_sec) }}/s</span>
<span v-if="task.status === 'running' && task.kind !== 'training' && task.kind !== 'runtime_install'">剩余 {{ formatEta(task.progress.eta_seconds) }}</span>
<span v-if="task.scheduling">{{ resourceLabels[task.scheduling?.resource_class ?? ''] ?? task.scheduling?.resource_class }}</span>
<span v-if="task.scheduling?.queue_position">队列第 {{ task.scheduling.queue_position }} 位</span>
<span v-if="task.scheduling?.wait_reason">{{ task.scheduling.wait_reason }}</span>
<span v-if="task.scheduling?.estimated_wait_seconds">预计等待 {{ formatEta(task.scheduling.estimated_wait_seconds) }}</span>
        </div>
        <p v-if="task.scheduling?.blocking_task_ids.length" class="task-blockers">阻塞任务：{{ task.scheduling.blocking_task_ids.join('、') }}</p>

        <div v-if="task.training" class="training-queue-summary">
          <strong>{{ task.training.adapter_id }} · {{ task.training.runtime_profile_id }}</strong>
          <span>GPU {{ task.training.gpu_ids.length ? task.training.gpu_ids.join(', ') : '自动选择' }}</span>
          <span v-if="task.training.model_path">模型 {{ task.training.model_path }}</span>
          <span v-if="task.training.train_data_dir">数据集 {{ task.training.train_data_dir }}</span>
          <span v-if="task.training.output_dir">输出 {{ task.training.output_dir }}{{ task.training.output_name ? `/${task.training.output_name}` : '' }}</span>
          <small v-if="task.status === 'queued'">训练队列优先展示；等待目标 GPU 与全局工作槽同时可用。</small>
        </div>

        <div v-if="task.failures.length" class="failure-list">
          <strong>{{ task.failures.length }} 个失败项</strong>
          <span v-for="failure in task.failures.slice(0, 4)" :key="`${failure.item_id}-${failure.code}`">{{ failure.item_id ? `${failure.item_id}：` : '' }}{{ failure.message }}</span>
          <span v-if="task.failures.length > 4">其余 {{ task.failures.length - 4 }} 项可在任务详情中查看</span>
        </div>

        <div v-if="task.preview?.candidates?.length" class="preflight-list">
          <strong>预检清单：{{ task.preview.candidates.length }} 项</strong>
          <span v-for="candidate in task.preview.candidates.slice(0, 6)" :key="candidate.relative_path">
            {{ candidate.relative_path }} · {{ candidate.reason }}
          </span>
          <span v-if="task.preview.candidates.length > 6">其余 {{ task.preview.candidates.length - 6 }} 项</span>
        </div>

        <div class="task-actions">
          <button v-if="task.status === 'running' || task.status === 'queued'" type="button" class="button button-small" :disabled="controlling.has(task.id)" @click="control(task, 'pause')"><Pause :size="14" /> 暂停</button>
          <button v-if="task.status === 'paused'" type="button" class="button button-small button-primary" :disabled="controlling.has(task.id)" @click="control(task, 'resume')"><Play :size="14" /> 恢复</button>
          <button v-if="task.status === 'awaiting_confirmation'" type="button" class="button button-small button-primary" :disabled="controlling.has(task.id)" @click="confirmingTask = task"><Check :size="14" /> 审阅并确认</button>
          <button v-if="taskCanRetry(task)" type="button" class="button button-small" :disabled="controlling.has(task.id)" @click="control(task, 'retry')"><RotateCcw :size="14" /> 重试失败项</button>
          <button v-if="['queued', 'running', 'pausing', 'paused', 'awaiting_confirmation'].includes(task.status)" type="button" class="button button-small button-danger" :disabled="controlling.has(task.id)" @click="control(task, 'cancel')"><X :size="14" /> {{ task.status === 'pausing' ? '改为取消' : '取消' }}</button>
          <button
            type="button"
            class="button button-small"
            :aria-expanded="expandedTaskId === task.id"
            :aria-controls="`task-details-${task.id}`"
            @click="toggleDetails(task)"
          >
            {{ expandedTaskId === task.id ? '收起详情' : '查看详情' }}
          </button>
          <button v-if="['completed', 'failed', 'cancelled'].includes(task.status)" type="button" class="button button-small button-danger" :disabled="controlling.has(task.id)" @click="deletingTask = task"><Trash2 :size="14" /> 删除记录</button>
        </div>

        <section
          v-if="expandedTaskId === task.id"
          :id="`task-details-${task.id}`"
          class="task-details"
          aria-label="任务详情"
        >
          <p v-if="detailsLoading" role="status">正在加载任务详情</p>
          <p v-else-if="detailsError" role="alert">{{ detailsError }}</p>
          <template v-else-if="details">
            <TrainingMonitor
              v-if="task.kind === 'training'"
              :task-id="task.id"
              :active="['queued', 'running', 'pausing', 'paused', 'cancelling'].includes(task.status)"
            />
            <label class="task-detail-filter">
              <span>项目状态</span>
              <select v-model="detailsStatus" aria-label="任务项目状态" @change="loadDetailsPage(task)">
                <option value="all">全部</option>
                <option value="queued">等待中</option>
                <option value="completed">已完成</option>
                <option value="skipped">已跳过</option>
                <option value="failed">失败</option>
              </select>
            </label>
            <div class="task-detail-counts" aria-label="任务项目汇总">
              <span>共 {{ details.item_counts.total }}</span>
              <span>完成 {{ details.item_counts.completed }}</span>
              <span>跳过 {{ details.item_counts.skipped }}</span>
              <span>失败 {{ details.item_counts.failed }}</span>
              <span>可重试 {{ details.item_counts.retryable_failed }}</span>
            </div>
            <section
              v-if="taskResultSummary(task.kind, details.result).length || taskResultItems(details.result).length"
              class="rounded-lg border border-[var(--border)] bg-[var(--surface-muted)] p-3"
              aria-label="处理结果"
            >
              <strong class="text-sm">处理结果</strong>
              <ul v-if="taskResultSummary(task.kind, details.result).length" class="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-sm text-[var(--text-secondary)]">
                <li v-for="entry in taskResultSummary(task.kind, details.result)" :key="entry.label">{{ entry.label }} {{ entry.value }}</li>
              </ul>
              <p v-if="task.kind === 'dataset_augmentation' && resultText(details.result, 'next_step')" class="mt-2 text-sm text-[var(--text-secondary)]">
                {{ resultText(details.result, 'next_step') }}
              </p>
              <ul v-if="task.kind === 'dataset_augmentation' && resultCountEntries(details.result, 'rejection_reasons').length" class="mt-2 grid gap-1 text-sm text-[var(--text-secondary)]" aria-label="增广拒绝原因">
                <li v-for="entry in resultCountEntries(details.result, 'rejection_reasons')" :key="entry.label">{{ entry.label }} {{ entry.value }}</li>
              </ul>
              <ul v-if="taskResultItems(details.result).length" class="mt-2 grid gap-1 text-sm text-[var(--text-secondary)]">
                <li v-for="item in taskResultItems(details.result)" :key="item.id" class="flex flex-wrap gap-x-2">
                  <span>{{ item.id }}</span>
                  <span v-if="item.dimensions">{{ item.dimensions }}</span>
                  <span v-if="item.bytes != null">{{ formatBytes(item.bytes) }}</span>
                </li>
              </ul>
            </section>
            <ul class="task-detail-items">
              <li v-for="item in details.items" :key="item.item_id">
                <div>
                  <strong>{{ item.post_id ? `Post #${item.post_id}` : item.item_id }}</strong>
                  <span class="status-pill" :class="item.status">{{ item.status }}</span>
                </div>
                <p v-if="item.error">
                  <code>{{ item.error.code }}</code>
                  <span>{{ item.error.message }}</span>
                </p>
                <div v-if="task.kind === 'vllm_tag' && (resultTags(item.result).length || resultBoolean(item.result, 'sidecar_written'))" class="mt-2">
                  <strong v-if="resultTags(item.result).length" class="text-sm">生成标签</strong>
                  <div v-if="resultTags(item.result).length" class="mt-1 flex flex-wrap gap-1" aria-label="生成标签列表">
                    <code v-for="tag in resultTags(item.result)" :key="tag" class="rounded bg-[var(--blue-soft)] px-1.5 py-0.5 text-xs text-[var(--blue)]">{{ tag }}</code>
                  </div>
                  <small v-if="resultBoolean(item.result, 'sidecar_written')" class="mt-1 block">已写入标签文件</small>
                </div>
                <p v-else-if="item.result" class="mt-2 text-sm text-[var(--text-secondary)]">
                  <span v-if="resultNumber(item.result, 'bytes') != null">已处理 {{ formatBytes(resultNumber(item.result, 'bytes')!) }}</span>
                  <span v-if="resultText(item.result, 'reason')">{{ resultNumber(item.result, 'bytes') != null ? ' · ' : '' }}{{ resultText(item.result, 'reason') }}</span>
                </p>
                <small>尝试 {{ item.attempts }} 次</small>
              </li>
            </ul>
            <div v-if="details.next_cursor" class="task-detail-pagination">
              <button type="button" class="button button-small" :disabled="detailsLoading" @click="loadNextDetailsPage(task)">下一页</button>
            </div>
          </template>
        </section>
      </article>
    </div>

    <div v-else-if="section === 'tasks'" class="empty-state">
      <div>
        <ListTodo :size="30" />
        <strong>{{ tasks.loading ? '正在同步任务' : '此处还没有任务' }}</strong>
        <p>在探索页选择帖子下载，或从工具页创建处理任务。</p>
        <RouterLink to="/explore" class="button button-primary" style="margin-top: 16px">开始探索</RouterLink>
      </div>
    </div>

    <section v-else aria-label="下载记录" class="history-section">
      <div v-if="historyLoading && !historyItems.length" class="empty-state" role="status">
        <div>
          <RefreshCw :size="30" />
          <strong>正在加载下载记录</strong>
          <p>正在读取已经持久化的下载结果。</p>
        </div>
      </div>
      <div v-else-if="historyError && !historyItems.length" class="empty-state" role="alert">
        <div>
          <AlertCircle :size="30" />
          <strong>下载记录加载失败</strong>
          <p>{{ historyError }}</p>
          <button type="button" class="button" style="margin-top: 16px" @click="loadHistory()">重新加载</button>
        </div>
      </div>
      <div v-else-if="!historyItems.length" class="empty-state">
        <div>
          <History :size="30" />
          <strong>还没有下载记录</strong>
          <p>从探索页创建的下载会在完成、失败或取消后保留在这里。</p>
          <RouterLink to="/explore" class="button button-primary" style="margin-top: 16px">开始探索</RouterLink>
        </div>
      </div>
      <div v-else class="history-list">
        <article v-for="record in historyItems" :key="record.id" class="history-card">
          <div class="task-top">
            <span class="task-icon"><Download :size="18" /></span>
            <div class="task-title">
              <strong>{{ record.source_label || 'Danbooru 下载' }}</strong>
              <small>
                {{ record.root_name ? `${record.root_name} · ` : '' }}
                <time :datetime="record.created_at">{{ formatDateTime(record.created_at) }}</time>
              </small>
            </div>
            <span class="status-pill" :class="record.status">{{ labels[record.status] }}</span>
          </div>
          <dl class="history-stats">
            <div><dt>成功 {{ record.completed_items.toLocaleString() }}</dt></div>
            <div><dt>跳过 {{ record.skipped_items.toLocaleString() }}</dt></div>
            <div><dt>失败 {{ record.failed_items.toLocaleString() }}</dt></div>
            <div><dt>已处理</dt><dd>{{ formatBytes(record.bytes_processed) }}</dd></div>
          </dl>
          <p v-if="record.error_message" class="history-error">{{ record.error_message }}</p>
          <footer class="history-footer">
            <div>
              <span>共 {{ record.total_items.toLocaleString() }} 项</span>
              <span>结束于 {{ formatDateTime(record.finished_at) }}</span>
            </div>
            <button
              v-if="record.can_repeat && record.repeat_request"
              type="button"
              class="button button-small"
              :disabled="repeating.has(record.id)"
              @click="repeatDownload(record)"
            >
              <RotateCcw :size="14" /> {{ repeating.has(record.id) ? '正在创建' : '再次下载' }}
            </button>
          </footer>
        </article>
        <div v-if="historyNextCursor || historyError" class="history-more">
          <span v-if="historyError" role="alert">{{ historyError }}</span>
          <button v-if="historyNextCursor" type="button" class="button" :disabled="historyLoading" @click="loadHistory(historyNextCursor)">
            {{ historyLoading ? '正在加载' : '加载更多' }}
          </button>
        </div>
      </div>
    </section>

    <ConfirmDialog
      :open="Boolean(confirmingTask)"
      title="确认执行预检清单"
      :confirm-label="confirmingTask ? confirmationLabel(confirmingTask) : '确认'"
      :busy="confirmingTask ? controlling.has(confirmingTask.id) : false"
      destructive
      @cancel="confirmingTask = null"
      @confirm="confirmReviewedTask"
    >
      <p>只会处理上方已展示的根目录内相对路径。执行前后端还会重新校验清单，原文件不会被永久删除，而是移入隐藏隔离区。</p>
    </ConfirmDialog>

    <ConfirmDialog
      :open="Boolean(deletingTask)"
      title="删除任务记录"
      confirm-label="删除"
      :busy="deletingTask ? controlling.has(deletingTask.id) : false"
      destructive
      @cancel="deletingTask = null"
      @confirm="confirmDeleteTask"
    >
      <p>将永久删除该任务记录及其项目明细（{{ deletingTask ? labels[deletingTask.status] : '' }} · {{ deletingTask ? kindLabels[deletingTask.kind] : '' }}）。已下载的媒体文件与下载历史不会被删除。</p>
    </ConfirmDialog>
  </div>
</template>
