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
})
