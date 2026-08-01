import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Toast {
  id: number
  type: 'success' | 'error' | 'info' | 'warning'
  title: string
  message: string
  duration: number
}

let nextId = 0

export const useToastStore = defineStore('toast', () => {
  const toasts = ref<Toast[]>([])

  function add(type: Toast['type'], title: string, message: string, duration = 5000) {
    const id = nextId++
    toasts.value.push({ id, type, title, message, duration })
    if (duration > 0) {
      setTimeout(() => remove(id), duration)
    }
  }

  function remove(id: number) {
    const idx = toasts.value.findIndex(t => t.id === id)
    if (idx !== -1) toasts.value.splice(idx, 1)
  }

  function success(title: string, message = '') { add('success', title, message) }
  function error(title: string, message = '') { add('error', title, message, 8000) }
  function info(title: string, message = '') { add('info', title, message) }
  function warning(title: string, message = '') { add('warning', title, message, 6000) }

  return { toasts, add, remove, success, error, info, warning }
})
