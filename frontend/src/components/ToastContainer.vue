<script setup lang="ts">
import { AlertCircle, CheckCircle2, Info, TriangleAlert } from '@lucide/vue'
import { useToastStore, type Toast } from '../stores/toast'

const toast = useToastStore()
const icons: Record<Toast['type'], typeof Info> = {
  success: CheckCircle2,
  error: AlertCircle,
  info: Info,
  warning: TriangleAlert,
}
</script>

<template>
  <Teleport to="body">
    <div class="toast-region" role="region" aria-label="通知" aria-live="polite">
      <TransitionGroup name="toast">
        <button
          v-for="item in toast.toasts"
          :key="item.id"
          type="button"
          class="toast-item"
          :class="`toast-${item.type}`"
          :aria-label="`关闭通知：${item.title}`"
          @click="toast.remove(item.id)"
        >
          <component :is="icons[item.type]" :size="19" />
          <span>
            <strong>{{ item.title }}</strong>
            <small v-if="item.message">{{ item.message }}</small>
          </span>
        </button>
      </TransitionGroup>
    </div>
  </Teleport>
</template>
