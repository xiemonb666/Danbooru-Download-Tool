<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { AlertCircle, Check, CirclePause, Download, History, ListTodo, Pause, Play, RefreshCw, RotateCcw, X } from '@lucide/vue'
import {
  createTask,
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

const tasks = useTasksStore()
const toast = useToastStore()
const filter = ref<'all' | 'active' | 'completed' | 'failed'>('all')
const section = ref<'tasks' | 'history'>('tasks')
const controlling = ref<Set<string>>(new Set())
const confirmingTask = ref<TaskSummary | null>(null)
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
  if (filter.value === 'active') return all.filter((task) => ['queued', 'running', 'pausing', 'paused', 'cancelling', 'awaiting_confirmation'].includes(task.status))
  if (filter.value === 'completed') return all.filter((task) => task.status === 'completed')
  if (filter.value === 'failed') return all.filter((task) => task.status === 'failed' || task.status === 'cancelled')
  return all
})

const labels: Record<TaskStatus, string> = {
  queued: '等待中', running: '进行中', pausing: '正在暂停', paused: '已暂停', cancelling: '正在取消',
  awaiting_confirmation: '等待确认', completed: '已完成', failed: '失败', cancelled: '已取消',
}

const kindLabels: Record<TaskSummary['kind'], string> = {
  download: '下载', index_library: '刷新图库', integrity_scan: '完整性检查', exact_dedup: '精确去重',
  near_dedup: '相似图片检查', resize: '缩放图片', heic_convert: 'HEIC 转换', delete_by_tag: '按标签隔离',
  tag_pipeline: '标签处理', vllm_tag: '视觉模型打标',
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

function taskResultSummary(kind: TaskSummary['kind'], value: unknown): ResultSummary[] {
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
      addCount('moved', '已移入隔离区')
      break
    case 'tag_pipeline':
      addCount('changed', '已处理标签文件')
      break
    case 'vllm_tag':
      addCount('tagged', '已完成打标')
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
      <button v-if="section === 'tasks'" type="button" class="button" :disabled="tasks.loading" @click="tasks.loadSnapshot">
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
          <span>{{ task.progress.completed.toLocaleString() }} / {{ task.progress.total.toLocaleString() }} 项</span>
          <span>{{ percent(task) }}%</span>
          <span v-if="task.progress.bytes_downloaded">{{ formatBytes(task.progress.bytes_downloaded) }}</span>
          <span v-if="task.progress.speed_bytes_per_sec">{{ formatBytes(task.progress.speed_bytes_per_sec) }}/s</span>
          <span v-if="task.status === 'running'">剩余 {{ formatEta(task.progress.eta_seconds) }}</span>
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
              <button type="button" class="button button-small" :disabled="detailsLoading" @click="loadDetailsPage(task, details.next_cursor)">下一页</button>
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
  </div>
</template>
