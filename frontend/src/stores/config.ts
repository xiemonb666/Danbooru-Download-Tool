import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { getConfig, updateConfig, type AppConfig } from '../api'

const defaults: AppConfig = {
  danbooru_username: '',
  danbooru_api_key_configured: false,
  vllm_api_key_configured: false,
  vllm_base_url: 'http://127.0.0.1:8000/v1',
  vllm_allowed_hosts: [],
  vllm_model: 'unsloth/Qwen3.6-27B-NVFP4',
  vllm_system_prompt: 'Analyze the image and return concise Danbooru-style tags inside exactly one <tag>...</tag> block. Use lowercase tags separated by commas; do not put explanations inside the tag block.',
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
  background_image: '',
  background_opacity: 18,
}

export const useConfigStore = defineStore('config', () => {
  const config = ref<AppConfig>({ ...defaults })
  const loading = ref(false)
  const loaded = ref(false)
  const error = ref<string | null>(null)
  let pending: Promise<void> | null = null

  function load(): Promise<void> {
    if (loaded.value) return Promise.resolve()
    if (pending) return pending
    loading.value = true
    error.value = null
    pending = getConfig()
      .then((value) => {
        config.value = value
        loaded.value = true
      })
      .catch((reason: unknown) => {
        error.value = reason instanceof Error ? reason.message : '配置加载失败'
      })
      .finally(() => {
        loading.value = false
        pending = null
      })
    return pending
  }

  async function save(): Promise<void> {
    loading.value = true
    error.value = null
    try {
      const {
        danbooru_api_key_configured: _danbooruSecretStatus,
        vllm_api_key_configured: _vllmSecretStatus,
        ...editable
      } = config.value
      config.value = await updateConfig(editable)
      loaded.value = true
    } catch (reason: unknown) {
      error.value = reason instanceof Error ? reason.message : '配置保存失败'
      throw reason
    } finally {
      loading.value = false
    }
  }

  return {
    config,
    loading,
    loaded,
    error,
    ready: computed(() => loaded.value && !loading.value),
    load,
    save,
  }
})
