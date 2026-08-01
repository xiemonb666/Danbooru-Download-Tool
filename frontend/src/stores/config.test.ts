import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AppConfig } from '../api'
import { useConfigStore } from './config'

describe('config store', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('deduplicates concurrent config loads', async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(new Response(JSON.stringify({ data: {
      danbooru_username: '', danbooru_api_key_configured: false,
      vllm_api_key_configured: false, vllm_base_url: 'http://127.0.0.1:8000/v1',
      vllm_allowed_hosts: [], vllm_model: 'model', vllm_system_prompt: 'prompt',
      vllm_tag_mode: 'overwrite', vllm_concurrency: 16,
      vllm_language: 'danbooru', vllm_max_tags: 60, vllm_max_length: 400,
      vllm_verify_danbooru: true, vllm_reference_existing: false,
      proxy_url: null, download_concurrency: 8,
      filename_template: '{id}_score_{score}.{ext}', ugoira_policy: 'webm_and_zip',
      blur_sensitive_media: true,
    } }), { status: 200, headers: { 'Content-Type': 'application/json' } }))
    vi.stubGlobal('fetch', fetchMock)
    const store = useConfigStore()

    await Promise.all([store.load(), store.load()])

    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(store.loaded).toBe(true)
    vi.unstubAllGlobals()
  })

  it('sends only editable settings when saving config', async () => {
    const response: AppConfig = {
      danbooru_username: 'local-user', danbooru_api_key_configured: true,
      vllm_api_key_configured: false, vllm_base_url: 'http://127.0.0.1:8000/v1',
      vllm_allowed_hosts: [], vllm_model: 'model', vllm_system_prompt: 'prompt',
      vllm_tag_mode: 'overwrite', vllm_concurrency: 16,
      vllm_language: 'danbooru', vllm_max_tags: 60, vllm_max_length: 400,
      vllm_verify_danbooru: true, vllm_reference_existing: false,
      proxy_url: null, download_concurrency: 8,
      filename_template: '{id}_score_{score}.{ext}', ugoira_policy: 'webm_and_zip',
      blur_sensitive_media: true,
    }
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(new Response(
      JSON.stringify({ data: response }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    ))
    vi.stubGlobal('fetch', fetchMock)
    const store = useConfigStore()
    store.config = { ...response }

    await store.save()

    const request = fetchMock.mock.calls[0]?.[1]
    const body = JSON.parse(String(request?.body)) as Record<string, unknown>
    expect(body).not.toHaveProperty('danbooru_api_key_configured')
    expect(body).not.toHaveProperty('vllm_api_key_configured')
    expect(body).not.toHaveProperty('legacy_media_path_suggestion')
    expect(body.danbooru_username).toBe('local-user')
    vi.unstubAllGlobals()
  })
})
