import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useHealthStore } from './health'

describe('health store', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('reports offline state from the real health request instead of a fixed badge', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockRejectedValue(new TypeError('connection refused')))
    const health = useHealthStore()

    await health.check()

    expect(health.status).toBe('offline')
    expect(health.message).toContain('无法连接')
    vi.unstubAllGlobals()
  })

  it('reports vLLM availability beside the local service health', async () => {
    vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockImplementation(async (input) => {
      const url = String(input)
      if (url.endsWith('/api/vllm/health')) {
        return new Response(JSON.stringify({
          data: { available: true, models: ['local/vision-model'], message: 'vLLM 可用' },
        }), { status: 200 })
      }
      return new Response(JSON.stringify({
        data: { status: 'ok', version: '2.0.0', database: 'ok', uptime_seconds: 10 },
      }), { status: 200 })
    }))
    const health = useHealthStore()

    await health.check()

    expect(health.status).toBe('online')
    expect(health.vllmStatus).toBe('online')
    expect(health.vllmMessage).toBe('vLLM 可用')
    vi.unstubAllGlobals()
  })
})
