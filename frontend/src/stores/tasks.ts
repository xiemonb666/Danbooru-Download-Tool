import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { getTasks, taskAction, type TaskEvent, type TaskStatus, type TaskSummary } from '../api'

function isTaskEvent(value: unknown): value is TaskEvent {
  if (typeof value !== 'object' || value === null) return false
  const eventType = Reflect.get(value, 'event_type')
  const task = Reflect.get(value, 'task')
  return typeof Reflect.get(value, 'sequence') === 'number'
    && typeof Reflect.get(value, 'task_id') === 'string'
    && typeof Reflect.get(value, 'revision') === 'number'
    && (eventType === 'created' || eventType === 'updated')
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
  let shouldConnect = false

  const activeCount = computed(() => tasks.value.filter((task) =>
    ['queued', 'running', 'pausing', 'paused', 'cancelling', 'awaiting_confirmation'].includes(task.status),
  ).length)

  function sortedTasks(): TaskSummary[] {
    return [...tasks.value].sort((a, b) => b.updated_at.localeCompare(a.updated_at))
  }

  function loadSnapshot(): Promise<void> {
    if (syncing) return syncing
    loading.value = true
    syncing = getTasks()
      .then((snapshot) => {
        tasks.value = snapshot.tasks
        lastEventId.value = snapshot.last_event_id
        error.value = null
      })
      .catch((reason: unknown) => {
        error.value = reason instanceof Error ? reason.message : '任务状态同步失败'
        connection.value = 'offline'
      })
      .finally(() => {
        syncing = null
        loading.value = false
      })
    return syncing
  }

  function applyEvent(event: TaskEvent): void {
    if (event.sequence <= lastEventId.value) return
    if (event.sequence !== lastEventId.value + 1) {
      lastEventId.value = event.sequence
      void loadSnapshot()
      return
    }
    lastEventId.value = event.sequence
    const index = tasks.value.findIndex((task) => task.id === event.task_id)
    if (index === -1) tasks.value.unshift(event.task)
    else if (event.revision >= tasks.value[index].revision) tasks.value[index] = event.task
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
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = null
    source?.close()
    source = null
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
