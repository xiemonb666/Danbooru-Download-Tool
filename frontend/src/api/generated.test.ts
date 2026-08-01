import { describe, expect, it } from 'vitest'

import type { AppConfig, DanbooruPost, DownloadHistoryPage, HealthStatus, LocalMedia, TaskDetails } from './index'
import type { components } from './generated'

type Equal<Left, Right> =
  (<Type>() => Type extends Left ? 1 : 2) extends
    (<Type>() => Type extends Right ? 1 : 2)
    ? (<Type>() => Type extends Right ? 1 : 2) extends
        (<Type>() => Type extends Left ? 1 : 2)
      ? true
      : false
    : false

describe('generated OpenAPI contracts', () => {
  it('exposes the Rust AppConfig schema to frontend code', () => {
    const config = {
      danbooru_username: '',
      danbooru_api_key_configured: false,
      vllm_api_key_configured: false,
      vllm_base_url: 'http://127.0.0.1:8000/v1',
      vllm_allowed_hosts: [],
      vllm_model: 'unsloth/Qwen3.6-27B-NVFP4',
      vllm_system_prompt: 'return tags',
      vllm_tag_mode: 'overwrite',
      vllm_concurrency: 16,
      vllm_language: 'danbooru',
      vllm_max_tags: 60,
      vllm_max_length: 400,
      vllm_verify_danbooru: true,
      vllm_reference_existing: false,
      proxy_url: null,
      download_concurrency: 8,
      filename_template: '{id}_score_{score}.{ext}',
      ugoira_policy: 'webm_and_zip',
      blur_sensitive_media: true,
    } satisfies components['schemas']['AppConfig']

    const usesGeneratedContract: Equal<AppConfig, components['schemas']['AppConfig']> = true
    const usesGeneratedTaskDetails: Equal<TaskDetails, components['schemas']['TaskDetails']> = true
    const usesGeneratedHistory: Equal<DownloadHistoryPage, components['schemas']['DownloadHistoryPage']> = true
    const usesGeneratedPost: Equal<DanbooruPost, components['schemas']['DanbooruPost']> = true
    const usesGeneratedLocalMedia: Equal<LocalMedia, components['schemas']['LocalMedia']> = true
    const usesGeneratedHealth: Equal<HealthStatus, components['schemas']['HealthStatus']> = true
    expect(config.download_concurrency).toBe(8)
    expect(usesGeneratedContract).toBe(true)
    expect(usesGeneratedTaskDetails).toBe(true)
    expect(usesGeneratedHistory).toBe(true)
    expect(usesGeneratedPost).toBe(true)
    expect(usesGeneratedLocalMedia).toBe(true)
    expect(usesGeneratedHealth).toBe(true)
  })
})
