import { getCurrentScope, onScopeDispose, ref, watch } from 'vue'
import { autocompleteTags, type TagSuggestion } from '../api'

export function useTagAutocomplete() {
  const query = ref('')
  const suggestions = ref<TagSuggestion[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null
  let controller: AbortController | null = null

  const stop = watch(query, (value) => {
    if (timer) clearTimeout(timer)
    if (value.trim().length < 2) {
      controller?.abort()
      suggestions.value = []
      loading.value = false
      return
    }
    timer = setTimeout(async () => {
      controller?.abort()
      controller = new AbortController()
      loading.value = true
      error.value = null
      try {
        suggestions.value = await autocompleteTags(value.trim(), controller.signal)
      } catch (reason: unknown) {
        if (!(reason instanceof DOMException && reason.name === 'AbortError')) {
          error.value = reason instanceof Error ? reason.message : '标签建议加载失败'
        }
      } finally {
        loading.value = false
      }
    }, 200)
  })

  function dispose(): void {
    stop()
    if (timer) clearTimeout(timer)
    controller?.abort()
  }

  if (getCurrentScope()) onScopeDispose(dispose)
  return { query, suggestions, loading, error, dispose }
}
