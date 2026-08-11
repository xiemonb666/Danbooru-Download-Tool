<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref } from 'vue'
import { CircleHelp } from '@lucide/vue'

const props = withDefaults(defineProps<{
  title: string
  description: string
  whenToAdjust?: string
}>(), { whenToAdjust: '' })

const open = ref(false)
const trigger = ref<HTMLElement | null>(null)
const panel = ref<HTMLElement | null>(null)
const panelId = `info-tooltip-${Math.random().toString(36).slice(2)}`
const panelStyle = ref<Record<string, string>>({})
let closeTimer: number | undefined

function cancelClose(): void {
  if (closeTimer !== undefined) window.clearTimeout(closeTimer)
  closeTimer = undefined
}

function queueClose(): void {
  cancelClose()
  closeTimer = window.setTimeout(() => { open.value = false }, 120)
}

function positionPanel(): void {
  const anchor = trigger.value?.getBoundingClientRect()
  const element = panel.value
  if (!anchor || !element) return
  const gap = 10
  const padding = 12
  const width = Math.min(336, window.innerWidth - padding * 2)
  const height = element.offsetHeight || 120
  const rightSpace = window.innerWidth - anchor.right
  const leftSpace = anchor.left
  let left: number
  let top: number
  if (rightSpace >= width + gap || leftSpace >= width + gap) {
    left = rightSpace >= width + gap ? anchor.right + gap : anchor.left - width - gap
    top = Math.max(padding, Math.min(anchor.top - 14, window.innerHeight - height - padding))
  } else {
    left = Math.max(padding, Math.min(anchor.left - width / 2 + anchor.width / 2, window.innerWidth - width - padding))
    top = anchor.top >= height + gap + padding ? anchor.top - height - gap : anchor.bottom + gap
  }
  panelStyle.value = { width: `${width}px`, left: `${left}px`, top: `${top}px` }
}

async function show(): Promise<void> {
  cancelClose()
  open.value = true
  await nextTick()
  positionPanel()
}

function toggle(): void {
  if (open.value) {
    open.value = false
  } else {
    void show()
  }
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    toggle()
    return
  }

  if (event.key === 'Escape') {
    event.preventDefault()
    open.value = false
  }
}

function refreshPosition(): void {
  if (open.value) positionPanel()
}

window.addEventListener('resize', refreshPosition)
window.addEventListener('scroll', refreshPosition, true)

onBeforeUnmount(() => {
  cancelClose()
  window.removeEventListener('resize', refreshPosition)
  window.removeEventListener('scroll', refreshPosition, true)
})
</script>

<template>
  <span class="info-tooltip">
    <span
      ref="trigger"
      role="button"
      tabindex="0"
      class="info-tooltip-trigger"
      aria-label="查看说明"
      :aria-describedby="open ? panelId : undefined"
      :aria-expanded="open"
      @mouseenter="show"
      @mouseleave="queueClose"
      @focus="show"
      @blur="queueClose"
      @click.stop="toggle"
      @keydown="onKeydown"
    ><CircleHelp :size="14" stroke-width="2" /></span>
  </span>
  <Teleport to="body">
    <section
      v-if="open"
      :id="panelId"
      ref="panel"
      class="info-tooltip-panel"
      role="tooltip"
      :style="panelStyle"
      @mouseenter="cancelClose"
      @mouseleave="queueClose"
    >
      <strong>{{ props.title }}</strong>
      <p>{{ props.description }}</p>
      <small v-if="props.whenToAdjust"><b>何时调整：</b>{{ props.whenToAdjust }}</small>
    </section>
  </Teleport>
</template>

<style scoped>
.info-tooltip { display: inline-flex; vertical-align: middle; }
.info-tooltip-trigger { display: inline-grid; width: 18px; height: 18px; place-items: center; padding: 0; border: 0; border-radius: 999px; background: transparent; color: #7a8493; cursor: help; }
.info-tooltip-trigger:hover, .info-tooltip-trigger[aria-expanded='true'] { background: #eaf2ff; color: var(--blue); }
.info-tooltip-panel { position: fixed; z-index: 1200; display: grid; gap: 6px; padding: 13px 14px; border: 1px solid #cfdced; border-radius: 12px; background: rgb(255 255 255 / 0.98); box-shadow: 0 16px 42px rgb(23 23 23 / 0.16); color: var(--text); font-size: 12px; line-height: 1.55; pointer-events: auto; }
.info-tooltip-panel strong { font-size: 12px; font-weight: 700; }
.info-tooltip-panel p, .info-tooltip-panel small { margin: 0; color: var(--text-secondary); }
.info-tooltip-panel small { padding-top: 7px; border-top: 1px solid var(--border); font-size: 11px; }
.info-tooltip-panel small b { color: var(--text); }
</style>
