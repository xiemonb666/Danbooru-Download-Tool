<script setup lang="ts">
import { computed } from 'vue'
import { Cpu, Gauge, MemoryStick, Thermometer, Zap } from '@lucide/vue'
import type { TrainingGpu } from '../api'

const props = withDefaults(defineProps<{
  gpus: TrainingGpu[]
  selectedGpuIds?: string[]
}>(), {
  selectedGpuIds: () => [],
})

const visibleGpus = computed(() => {
  const selected = props.gpus.filter((gpu) => props.selectedGpuIds.includes(gpu.id))
  return selected.length ? selected : props.gpus
})

function percentage(value: number): number {
  return Math.max(0, Math.min(100, Math.round(Number.isFinite(value) ? value : 0)))
}

function memoryPercentage(gpu: TrainingGpu): number {
  if (!gpu.memory_total_mib) return 0
  return percentage(gpu.memory_used_mib / gpu.memory_total_mib * 100)
}

function gibibytes(value: number): string {
  return (value / 1024).toFixed(1)
}

function memoryLabel(gpu: TrainingGpu): string {
  return `${gibibytes(gpu.memory_used_mib)} / ${gibibytes(gpu.memory_total_mib)} GiB`
}

function integerMetric(value: number | null | undefined, suffix: string): string {
  return value == null || !Number.isFinite(value) ? '—' : `${Math.round(value)} ${suffix}`
}

function powerMetric(value: number | null | undefined): string {
  return value == null || !Number.isFinite(value) ? '—' : `${Math.round(value)} W`
}

function powerDetail(gpu: TrainingGpu): string {
  if (gpu.power_draw_w == null || gpu.power_limit_w == null) return '功耗读取中'
  return `${powerMetric(gpu.power_draw_w)} / ${powerMetric(gpu.power_limit_w)}`
}
</script>

<template>
  <section class="surface training-gpu-live" aria-label="GPU 实时状态">
    <header class="gpu-live-heading">
      <div><Cpu :size="17" /><strong>GPU 实时状态</strong><span class="gpu-live-indicator"><i />实时</span></div>
      <small>每 3 秒刷新</small>
    </header>

    <p v-if="!visibleGpus.length" class="gpu-live-empty">正在等待本机 GPU 遥测…</p>

    <div v-else class="gpu-live-list">
      <article v-for="gpu in visibleGpus" :key="gpu.id" class="gpu-live-card">
        <div class="gpu-live-primary">
          <div
            class="gpu-utilization-ring"
            role="img"
            :aria-label="`GPU ${gpu.id} 利用率 ${percentage(gpu.utilization_percent)}%`"
          >
            <svg viewBox="0 0 64 64" aria-hidden="true">
              <circle class="gpu-ring-track" cx="32" cy="32" r="26" pathLength="100" />
              <circle
                class="gpu-ring-value"
                cx="32"
                cy="32"
                r="26"
                pathLength="100"
                :style="{ strokeDashoffset: String(100 - percentage(gpu.utilization_percent)) }"
              />
            </svg>
            <span><strong>{{ percentage(gpu.utilization_percent) }}%</strong><small>利用率</small></span>
          </div>
          <div class="gpu-live-name">
            <span>GPU {{ gpu.id }}</span>
            <strong :title="gpu.name">{{ gpu.name }}</strong>
            <small><Zap :size="12" /> {{ powerDetail(gpu) }}</small>
          </div>
        </div>

        <div class="gpu-live-memory">
          <div><span><MemoryStick :size="13" />显存</span><strong>{{ memoryLabel(gpu) }}</strong></div>
          <div class="gpu-memory-track" :aria-label="`GPU ${gpu.id} 显存已使用 ${memoryPercentage(gpu)}%`"><i :style="{ width: `${memoryPercentage(gpu)}%` }" /></div>
          <small>{{ memoryPercentage(gpu) }}% 已使用</small>
        </div>

        <dl class="gpu-live-metrics">
          <div><dt><Gauge :size="12" />核心频率</dt><dd>{{ integerMetric(gpu.graphics_clock_mhz, 'MHz') }}</dd></div>
          <div><dt><MemoryStick :size="12" />显存频率</dt><dd>{{ integerMetric(gpu.memory_clock_mhz, 'MHz') }}</dd></div>
          <div><dt><Zap :size="12" />当前功耗</dt><dd>{{ powerMetric(gpu.power_draw_w) }}</dd></div>
          <div><dt><Thermometer :size="12" />温度 / 风扇</dt><dd>{{ integerMetric(gpu.temperature_c, '°C') }}<span v-if="gpu.fan_speed_percent != null"> · {{ Math.round(gpu.fan_speed_percent) }}%</span></dd></div>
        </dl>
        <div v-if="gpu.external_processes?.length" class="gpu-live-processes">
          <strong>外部计算进程</strong>
          <span v-for="process in gpu.external_processes.slice(0, 3)" :key="`${process.pid}-${process.process_name}`" :title="`${process.process_name} · PID ${process.pid}`">{{ process.process_name }} · {{ process.memory_used_mib }} MiB</span>
        </div>
      </article>
    </div>
  </section>
</template>
