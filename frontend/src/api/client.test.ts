import { afterEach, describe, expect, it, vi } from 'vitest'
import { analyzeLoraSvd, apiClient, getDanbooruPosts, getDownloadHistory, getLibraryItem, getTaskDetails, saveSecret } from './index'

describe('apiClient', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('unwraps the unified success envelope with native fetch', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { status: 'ok' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    const result = await apiClient.get<{ status: string }>('/health')

    expect(result).toEqual({ status: 'ok' })
    expect(fetchMock).toHaveBeenCalledWith('/api/health', expect.objectContaining({ method: 'GET' }))
  })

  it('preserves structured API errors for actionable UI feedback', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({
        error: { code: 'rate_limited', message: '请稍后再试', retryable: true },
        request_id: 'req-7',
      }), { status: 429, headers: { 'Content-Type': 'application/json' } }),
    ))

    await expect(apiClient.get('/danbooru/posts')).rejects.toMatchObject({
      name: 'ApiError', code: 'rate_limited', retryable: true, status: 429, requestId: 'req-7',
    })
  })

  it('loads persistent download history with a bounded cursor query', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { items: [], next_cursor: 'next-50' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await expect(getDownloadHistory({ limit: 50, cursor: 'record-50' })).resolves.toEqual({
      items: [],
      next_cursor: 'next-50',
    })
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/downloads/history?limit=50&cursor=record-50',
      expect.objectContaining({ method: 'GET' }),
    )
  })

  it('loads a bounded, filterable task item page with a cancellable request', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({
        data: {
          task: { id: 'task/1' },
          item_counts: { total: 0, queued: 0, completed: 0, skipped: 0, failed: 0, retryable_failed: 0 },
          items: [],
        },
      }), { status: 200, headers: { 'Content-Type': 'application/json' } }),
    )
    vi.stubGlobal('fetch', fetchMock)
    const controller = new AbortController()

    await getTaskDetails('task/1', { itemStatus: 'failed', itemCursor: 'item-50', itemLimit: 50 }, controller.signal)

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/tasks/task%2F1?item_status=failed&item_cursor=item-50&item_limit=50',
      expect.objectContaining({ method: 'GET', signal: controller.signal }),
    )
  })

  it('loads one local media snapshot by an encoded stable ID', async () => {
    const media = {
      id: 'media/1',
      root_id: 'root-1',
      filename: 'one.jpg',
      relative_path: 'one.jpg',
      mime_type: 'image/jpeg',
      size_bytes: 1024,
      tags: ['cat'],
      created_at: '2026-07-16T00:00:00Z',
    }
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: media }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)
    const controller = new AbortController()

    await expect(getLibraryItem('media/1', controller.signal)).resolves.toEqual(media)
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/library/items/media%2F1',
      expect.objectContaining({ method: 'GET', signal: controller.signal }),
    )
  })

  it('preserves whether a secret was stored in the system vault or only for the session', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { configured: true, storage: 'session' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    ))

    const result = await saveSecret('danbooru', 'secret-value')

    expect(result.storage).toBe('session')
  })

  it('starts a LoRA SVD analysis with an explicit runtime and auto device selection', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: { id: 'analysis-1', reports: [] } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await analyzeLoraSvd({
      runtime_profile_id: 'conda:lora',
      files: [{ path: 'D:/models/one.safetensors', label: 'epoch 1' }],
      device: 'auto',
    })

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/training/lora-svd/analyses',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          runtime_profile_id: 'conda:lora',
          files: [{ path: 'D:/models/one.safetensors', label: 'epoch 1' }],
          device: 'auto',
        }),
      }),
    )
  })

  it('normalizes missing or unexpected Danbooru ratings to unknown', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({
        data: {
          posts: [
            { id: 1, rating: 'future-rating' },
            { id: 2 },
          ],
          page: 1,
        },
      }), { status: 200, headers: { 'Content-Type': 'application/json' } }),
    ))

    const result = await getDanbooruPosts({ query: '', page: '1' })

    expect(result.posts.map((post) => post.rating)).toEqual(['unknown', 'unknown'])
  })
})
