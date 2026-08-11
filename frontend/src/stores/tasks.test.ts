import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useTasksStore } from './tasks'

class EventSourceStub extends EventTarget {
  static latest: EventSourceStub | null = null
  readonly url: string
  readonly withCredentials = false
  readyState = 1
  onopen: ((event: Event) => void) | null = null
  onmessage: ((event: MessageEvent) => void) | null = null
  onerror: ((event: Event) => void) | null = null

  constructor(url: string | URL) {
    super()
    this.url = String(url)
    EventSourceStub.latest = this
  }

  close(): void { this.readyState = 2 }
  emit(data: object): void { this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(data) })) }
}

describe('tasks store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    EventSourceStub.latest = null
  })

  it('resynchronizes the snapshot when the SSE sequence has a gap', async () => {
    const fetchMock = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { tasks: [], last_event_id: 4 } }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { tasks: [], last_event_id: 6 } }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    EventSourceStub.latest?.emit({
      sequence: 6,
      task_id: 'task-1',
      revision: 1,
      event_type: 'updated',
      task: {
        id: 'task-1',
        kind: 'download',
        status: 'running',
        revision: 1,
        title: '下载',
        progress: { completed: 0, total: 1, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
        failures: [],
        created_at: '2026-07-16T00:00:00Z',
        updated_at: '2026-07-16T00:00:00Z',
      },
    })

    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2))
    expect(store.lastEventId).toBe(6)
    store.disconnect()
    vi.unstubAllGlobals()
  })

  it('retries the snapshot after the backend comes back online', async () => {
    vi.useFakeTimers()
    const fetchMock = vi.fn<typeof fetch>()
      .mockRejectedValueOnce(new TypeError('offline'))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { tasks: [], last_event_id: 0 } }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    expect(store.connection).toBe('offline')
    await vi.advanceTimersByTimeAsync(3_000)

    expect(fetchMock).toHaveBeenCalledTimes(2)
    store.disconnect()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('automatically refreshes while an active task exists', async () => {
    vi.useFakeTimers()
    const task = {
      id: 'task-1', kind: 'exact_dedup', status: 'awaiting_confirmation', revision: 1, title: '精确去重预检',
      progress: { completed: 0, total: 1, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [], created_at: '2026-07-16T00:00:00Z', updated_at: '2026-07-16T00:00:00Z',
    }
    const fetchMock = vi.fn<typeof fetch>()
      .mockResolvedValue(new Response(JSON.stringify({ data: { tasks: [task], last_event_id: 1 } }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    await vi.advanceTimersByTimeAsync(5_000)

    expect(fetchMock).toHaveBeenCalledTimes(2)
    store.disconnect()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('coalesces rapid running-task updates into one bounded 250ms store update', async () => {
    vi.useFakeTimers()
    const task = {
      id: 'task-1', kind: 'download', status: 'running', revision: 1, title: '下载',
      progress: { completed: 0, total: 100, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [], created_at: '2026-07-16T00:00:00Z', updated_at: '2026-07-16T00:00:00Z',
    }
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { tasks: [task], last_event_id: 0 } }), { status: 200 }),
    ))
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    EventSourceStub.latest?.emit({
      sequence: 1, task_id: 'task-1', revision: 2, event_type: 'updated',
      task: { ...task, revision: 2, progress: { ...task.progress, completed: 1 } },
    })
    EventSourceStub.latest?.emit({
      sequence: 2, task_id: 'task-1', revision: 3, event_type: 'updated',
      task: { ...task, revision: 3, progress: { ...task.progress, completed: 2 } },
    })

    expect(store.tasks[0].revision).toBe(1)
    await vi.advanceTimersByTimeAsync(249)
    expect(store.tasks[0].revision).toBe(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(store.tasks[0].revision).toBe(3)
    expect(store.tasks[0].progress.completed).toBe(2)

    store.disconnect()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('removes a task immediately when the server emits its deleted event', async () => {
    const task = {
      id: 'training-1', kind: 'training', status: 'completed', revision: 4, title: 'LoRA 训练',
      progress: { completed: 1, total: 1, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [], created_at: '2026-07-16T00:00:00Z', updated_at: '2026-07-16T01:00:00Z',
    }
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { tasks: [task], last_event_id: 7 } }), { status: 200 }),
    ))
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    EventSourceStub.latest?.emit({ sequence: 8, task_id: task.id, revision: 4, event_type: 'deleted', task })

    expect(store.tasks).toEqual([])
    store.disconnect()
    vi.unstubAllGlobals()
  })

  it('does not let a stale snapshot overwrite a newly received task event', async () => {
    let resolveStaleSnapshot: ((value: Response) => void) | undefined
    const task = {
      id: 'task-1', kind: 'exact_dedup', status: 'running', revision: 1, title: '精确去重',
      progress: { completed: 0, total: 1, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [], created_at: '2026-07-16T00:00:00Z', updated_at: '2026-07-16T00:00:00Z',
    }
    const fetchMock = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { tasks: [task], last_event_id: 1 } }), { status: 200 }))
      .mockImplementationOnce(() => new Promise<Response>((resolve) => { resolveStaleSnapshot = resolve }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    const syncing = store.loadSnapshot()
    EventSourceStub.latest?.emit({
      sequence: 2, task_id: task.id, revision: 2, event_type: 'updated',
      task: { ...task, status: 'awaiting_confirmation', revision: 2 },
    })
    resolveStaleSnapshot?.(new Response(JSON.stringify({ data: { tasks: [task], last_event_id: 1 } }), { status: 200 }))
    await syncing

    expect(store.lastEventId).toBe(2)
    expect(store.tasks[0].status).toBe('awaiting_confirmation')
    expect(store.tasks[0].revision).toBe(2)
    store.disconnect()
    vi.unstubAllGlobals()
  })

  it('does not let a confirmation-era snapshot undo its returned task update', async () => {
    let resolveStaleSnapshot: ((value: Response) => void) | undefined
    const awaitingConfirmation = {
      id: 'task-1', kind: 'exact_dedup', status: 'awaiting_confirmation', revision: 3, title: '精确去重预检',
      progress: { completed: 0, total: 1, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [], created_at: '2026-07-16T00:00:00Z', updated_at: '2026-07-16T00:00:00Z',
    }
    const confirmed = { ...awaitingConfirmation, status: 'queued', revision: 4 }
    const fetchMock = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { tasks: [awaitingConfirmation], last_event_id: 3 } }), { status: 200 }))
      .mockImplementationOnce(() => new Promise<Response>((resolve) => { resolveStaleSnapshot = resolve }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: confirmed }), { status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('EventSource', EventSourceStub)
    const store = useTasksStore()

    await store.connect()
    const syncing = store.loadSnapshot()
    await store.control(awaitingConfirmation.id, 'confirm')
    resolveStaleSnapshot?.(new Response(JSON.stringify({ data: { tasks: [awaitingConfirmation], last_event_id: 3 } }), { status: 200 }))
    await syncing

    expect(store.tasks[0].status).toBe('queued')
    expect(store.tasks[0].revision).toBe(4)
    store.disconnect()
    vi.unstubAllGlobals()
  })
})
