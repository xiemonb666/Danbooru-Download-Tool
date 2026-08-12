<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

const props = withDefaults(defineProps<{
  open: boolean
  title: string
  confirmLabel?: string
  cancelLabel?: string
  destructive?: boolean
  busy?: boolean
  wide?: boolean
}>(), {
  confirmLabel: '确认',
  cancelLabel: '取消',
  destructive: false,
  busy: false,
  wide: false,
})

const emit = defineEmits<{ confirm: []; cancel: [] }>()
const dialog = ref<HTMLElement | null>(null)
const confirmButton = ref<HTMLButtonElement | null>(null)
let previousFocus: HTMLElement | null = null

watch(() => props.open, async (open) => {
  if (open) {
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    await nextTick()
    confirmButton.value?.focus()
  } else {
    previousFocus?.focus()
    previousFocus = null
  }
}, { immediate: true })

function cancel(): void {
  if (!props.busy) emit('cancel')
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    cancel()
    return
  }
  if (event.key !== 'Tab' || !dialog.value) return
  const focusable = [...dialog.value.querySelectorAll<HTMLElement>('button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled)')]
  if (focusable.length === 0) return
  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

onBeforeUnmount(() => previousFocus?.focus())
onMounted(() => {
  if (props.open) confirmButton.value?.focus()
})
</script>

<template>
  <Teleport to="body">
    <Transition name="toast">
      <div v-if="open" class="dialog-scrim" @mousedown.self="cancel">
        <section
          ref="dialog"
          class="dialog"
          :class="{ 'dialog-wide': wide }"
          role="dialog"
          aria-modal="true"
          aria-labelledby="confirm-title"
          @keydown="onKeydown"
        >
          <header class="dialog-header">
            <h2 id="confirm-title">{{ title }}</h2>
          </header>
          <div class="dialog-body"><slot /></div>
          <footer class="dialog-actions">
            <button type="button" class="button" :disabled="busy" @click="cancel">{{ cancelLabel }}</button>
            <button
              ref="confirmButton"
              type="button"
              class="button"
              :class="destructive ? 'button-danger' : 'button-primary'"
              :disabled="busy"
              @click="emit('confirm')"
            >
              {{ busy ? '处理中' : confirmLabel }}
            </button>
          </footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>
