import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { getTasks, taskAction, type TaskEvent, type TaskStatus, type TaskSummary } from '../api'

const ACTIVE_TASK_SNAPSHOT_INTERVAL_MS = 5_000
const TASK_PROGRESS_FLUSH_INTERVAL_MS = 250

function isTaskEvent(value: unknown): value is TaskEvent {
  if (typeof value !== 'object' || value === null) return false
  const eventType = Reflect.get(value, 'event_type')
  const task = Reflect.get(value, 'task')
  return typeof Reflect.get(value, 'sequence') === 'number'
    && typeof Reflect.get(value, 'task_id') === 'string'
    && typeof Reflect.get(value, 'revision') === 'number'
    && (eventType === 'created' || eventType === 'updated' || eventType === 'deleted')
    && typeof task === 'object'
    && task !== null
}

export const useTasksStore = defineStore('tasks', () => {
  const tasks = ref<TaskSummary[]>([])
  const lastEventId = ref(0)
  const connection = ref<'idle' | 'connecting' | 'live' | 'offline'>('idle')
  const loading = ref(false)
  const error = ref<string | null>(null)
  let source: EventSource | null = null
  let syncing: Promise<void> | null = null
  let retryTimer: ReturnType<typeof setTimeout> | null = null
  let snapshotTimer: ReturnType<typeof setInterval> | null = null
  let progressTimer: ReturnType<typeof setTimeout> | null = null
  let shouldConnect = false
  const pendingProgressEvents = new Map<string, TaskEvent>()

  const activeCount = computed(() => tasks.value.filter((task) =>
    ['queued', 'running', 'pausing', 'paused', 'cancelling', 'awaiting_confirmation'].includes(task.status),
  ).length)

  function updateSnapshotPolling(): void {
    const shouldPoll = shouldConnect && activeCount.value > 0
    if (shouldPoll && snapshotTimer === null) {
      snapshotTimer = setInterval(() => { void loadSnapshot(true) }, ACTIVE_TASK_SNAPSHOT_INTERVAL_MS)
    } else if (!shouldPoll && snapshotTimer !== null) {
      clearInterval(snapshotTimer)
      snapshotTimer = null
    }
  }

  watch(activeCount, updateSnapshotPolling, { flush: 'sync' })

  function sortedTasks(): TaskSummary[] {
    return [...tasks.value].sort((a, b) => b.updated_at.localeCompare(a.updated_at))
  }

  function mergeSnapshot(snapshotTasks: TaskSummary[]): TaskSummary[] {
    const currentTasks = new Map(tasks.value.map((task) => [task.id, task]))
    const snapshotIds = new Set(snapshotTasks.map((task) => task.id))
    const merged = snapshotTasks.map((task) => {
      const current = currentTasks.get(task.id)
      return current && current.revision > task.revision ? current : task
    })
    for (const task of tasks.value) {
      if (!snapshotIds.has(task.id)) merged.push(task)
    }
    return merged
  }

  function loadSnapshot(silent = false): Promise<void> {
    if (syncing) return syncing
    if (!silent) loading.value = true
    syncing = getTasks()
      .then((snapshot) => {
        if (snapshot.last_event_id < lastEventId.value) return
        pendingProgressEvents.clear()
        tasks.value = mergeSnapshot(snapshot.tasks)
        lastEventId.value = snapshot.last_event_id
        error.value = null
      })
      .catch((reason: unknown) => {
        error.value = reason instanceof Error ? reason.message : '任务状态同步失败'
        connection.value = 'offline'
      })
      .finally(() => {
        syncing = null
        if (!silent) loading.value = false
      })
    return syncing
  }

  function applyTaskEvent(event: TaskEvent): void {
    const index = tasks.value.findIndex((task) => task.id === event.task_id)
    if (index === -1) tasks.value.unshift(event.task)
    else if (event.revision >= tasks.value[index].revision) tasks.value[index] = event.task
  }

  function flushProgressEvents(): void {
    progressTimer = null
    const events = [...pendingProgressEvents.values()]
    pendingProgressEvents.clear()
    for (const event of events) applyTaskEvent(event)
  }

  function queueProgressEvent(event: TaskEvent): void {
    pendingProgressEvents.set(event.task_id, event)
    if (progressTimer !== null) return
    progressTimer = setTimeout(flushProgressEvents, TASK_PROGRESS_FLUSH_INTERVAL_MS)
  }

  function applyEvent(event: TaskEvent): void {
    if (event.sequence <= lastEventId.value) return
    if (event.sequence !== lastEventId.value + 1) {
      lastEventId.value = event.sequence
      void loadSnapshot()
      return
    }
    lastEventId.value = event.sequence
    if (event.event_type === 'deleted') {
      pendingProgressEvents.delete(event.task_id)
      tasks.value = tasks.value.filter((task) => task.id !== event.task_id)
      return
    }
    if (event.task.status === 'running') {
      queueProgressEvent(event)
      return
    }
    pendingProgressEvents.delete(event.task_id)
    applyTaskEvent(event)
  }

  function scheduleReconnect(): void {
    if (!shouldConnect || retryTimer) return
    retryTimer = setTimeout(() => {
      retryTimer = null
      void connect()
    }, 3_000)
  }

  async function connect(): Promise<void> {
    shouldConnect = true
    updateSnapshotPolling()
    if (source) return
    connection.value = 'connecting'
    await loadSnapshot()
    if (error.value) {
      scheduleReconnect()
      return
    }
    source = new EventSource(`/api/tasks/events?after=${lastEventId.value}`)
    source.onopen = () => { connection.value = 'live' }
    source.onerror = () => {
      connection.value = 'offline'
      source?.close()
      source = null
      scheduleReconnect()
    }
    source.onmessage = (message) => {
      try {
        const value: unknown = JSON.parse(message.data)
        if (isTaskEvent(value)) applyEvent(value)
        else void loadSnapshot()
      } catch {
        void loadSnapshot()
      }
    }
  }

  function disconnect(): void {
    shouldConnect = false
    updateSnapshotPolling()
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = null
    source?.close()
    source = null
    if (progressTimer) clearTimeout(progressTimer)
    progressTimer = null
    pendingProgressEvents.clear()
    connection.value = 'idle'
  }

  async function control(id: string, action: 'pause' | 'resume' | 'cancel' | 'retry' | 'confirm'): Promise<void> {
    const updated = await taskAction(id, action)
    const index = tasks.value.findIndex((task) => task.id === id)
    if (index === -1) tasks.value.unshift(updated)
    else tasks.value[index] = updated
  }

  function byStatus(...statuses: TaskStatus[]): TaskSummary[] {
    return sortedTasks().filter((task) => statuses.includes(task.status))
  }

  return { tasks, lastEventId, connection, loading, error, activeCount, sortedTasks, byStatus, connect, disconnect, loadSnapshot, control }
})
