<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch, type ComponentPublicInstance } from 'vue'
import { Activity, Gauge, Image, LineChart, MoveHorizontal, RefreshCw, RotateCcw, Timer, X, ZoomIn, ZoomOut } from '@lucide/vue'
import { graphic, init, use, type ECharts } from 'echarts/core'
import { LineChart as EChartsLineChart } from 'echarts/charts'
import { DataZoomComponent, GridComponent, TooltipComponent } from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import {
  getTrainingArtifacts,
  getTrainingLogs,
  getTrainingMetrics,
  getTrainingMetricsOverview,
  trainingMetricEventsUrl,
  type TrainingArtifact,
  type TrainingMetric,
  type TrainingMetricSeriesSummary,
} from '../api'
import InfoTooltip from './InfoTooltip.vue'

use([EChartsLineChart, DataZoomComponent, GridComponent, TooltipComponent, CanvasRenderer])

const props = withDefaults(defineProps<{ taskId: string; active: boolean; visible?: boolean }>(), {
  visible: true,
})

interface MetricLine {
  step: number
  timestamp: number
  metrics: Record<string, number>
}

const MAX_CHART_POINTS = 5_000
const RECENT_WINDOW = 20
const colors = ['#2563eb', '#10b981', '#f59e0b', '#8b5cf6', '#ef4444', '#06b6d4', '#ec4899']

const monitorRoot = ref<HTMLElement | null>(null)
const chartHost = ref<HTMLElement | null>(null)
const metrics = shallowRef<TrainingMetric[]>([])
const overview = shallowRef<TrainingMetricSeriesSummary[]>([])
const loading = ref(false)
const error = ref('')
const focusSeries = ref('')
const axisMode = ref<'step' | 'time'>('step')
const smoothing = ref(0)
const zoomLevel = ref(1)
const logs = ref('')
const logsLoading = ref(false)
const artifacts = ref<TrainingArtifact[]>([])
const artifactsLoading = ref(false)
const lightboxArtifact = ref<TrainingArtifact | null>(null)

function openSampleLightbox(artifact: TrainingArtifact): void {
  lightboxArtifact.value = artifact
}

function closeSampleLightbox(): void {
  lightboxArtifact.value = null
}

let chart: ECharts | null = null
let resizeObserver: ResizeObserver | null = null
let resizeFrame: number | null = null
let renderFrame: number | null = null
let metricFlushFrame: number | null = null
let eventSource: EventSource | null = null
let logRefreshTimer: ReturnType<typeof setInterval> | undefined
let requestController: AbortController | null = null
let lifecycleRevision = 0
let streamCursor = 0
let dataZoomStart = 0
let dataZoomEnd = 100
let monitorPollCycle = 0
const pendingMetricLines: Array<{ cursor: number; line: MetricLine }> = []

const series = computed(() => overview.value
  .map((item) => item.series)
  .filter((name) => name !== 'train.max_steps')
  .sort())
const focusPoints = computed(() => metrics.value
  .filter((metric) => metric.series === focusSeries.value)
  .slice()
  .sort((left, right) => coordinate(left) - coordinate(right)))
const lineColor = computed(() => colors[Math.max(0, series.value.indexOf(focusSeries.value)) % colors.length])
const currentMetric = computed(() => focusPoints.value[focusPoints.value.length - 1] ?? null)
const minMetric = computed(() => focusPoints.value.length ? Math.min(...focusPoints.value.map((item) => item.value)) : null)
const maxMetric = computed(() => focusPoints.value.length ? Math.max(...focusPoints.value.map((item) => item.value)) : null)
const firstMetric = computed(() => focusPoints.value[0] ?? null)
const recentAverage = computed(() => average(focusPoints.value.slice(-RECENT_WINDOW).map((item) => item.value)))
const previousAverage = computed(() => {
  const points = focusPoints.value
  return average(points.slice(Math.max(0, points.length - RECENT_WINDOW * 2), Math.max(0, points.length - RECENT_WINDOW)).map((item) => item.value))
})
const deltaFromStart = computed(() => currentMetric.value && firstMetric.value ? currentMetric.value.value - firstMetric.value.value : null)
const recentTrend = computed(() => recentAverage.value !== null && previousAverage.value !== null ? recentAverage.value - previousAverage.value : null)
const lastMetrics = computed(() => overview.value
  .filter((item) => item.series !== 'train.max_steps')
  .map((item) => item.latest)
  .sort((left, right) => left.series.localeCompare(right.series)))
const trainingStepTarget = computed(() => overview.value.find((item) => item.series === 'train.max_steps')?.latest.value ?? null)
const etaSeconds = computed(() => overview.value.find((item) => item.series === 'train.eta_seconds')?.latest.value ?? null)
const currentTrainingStep = computed(() => Math.max(0, ...overview.value.map((item) => item.latest.step), ...metrics.value.map((item) => item.step)))
const trainingProgressPercent = computed(() => trainingStepTarget.value
  ? clamp(currentTrainingStep.value / trainingStepTarget.value * 100, 0, 100)
  : 0)
const sampleArtifacts = computed(() => artifacts.value.filter((artifact) => artifact.kind === 'sample'))
const loraArtifacts = computed(() => artifacts.value.filter((artifact) => artifact.kind === 'lora'))

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}

function average(values: number[]): number | null {
  return values.length ? values.reduce((total, value) => total + value, 0) / values.length : null
}

function coordinate(metric: TrainingMetric): number {
  return axisMode.value === 'step' ? metric.step : timestampMilliseconds(metric.timestamp)
}

function dedupeByCoordinate(points: TrainingMetric[]): TrainingMetric[] {
  const result: TrainingMetric[] = []
  for (const point of points) {
    const last = result[result.length - 1]
    if (last && coordinate(last) === coordinate(point)) {
      result[result.length - 1] = point
      continue
    }
    result.push(point)
  }
  return result
}

function movingAverage(points: TrainingMetric[], window: number): TrainingMetric[] {
  const size = Math.min(window, points.length)
  if (size <= 1) return points
  const prefix: number[] = [0]
  for (const point of points) prefix.push(prefix[prefix.length - 1] + point.value)
  const half = Math.floor((size - 1) / 2)
  return points.map((point, index) => {
    const start = Math.max(0, index - half)
    const end = Math.min(points.length - 1, index + half)
    const count = end - start + 1
    return { ...point, value: (prefix[end + 1] - prefix[start]) / count }
  })
}

function timestampMilliseconds(timestamp: number): number {
  return timestamp < 10_000_000_000 ? timestamp * 1000 : timestamp
}

function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return '—'
  const total = Math.round(seconds)
  if (total >= 3600) return `${Math.floor(total / 3600)}h ${Math.floor((total % 3600) / 60)}m`
  if (total >= 60) return `${Math.floor(total / 60)}m ${total % 60}s`
  return `${total}s`
}

function format(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return '—'
  if (Math.abs(value) >= 1000 || (Math.abs(value) < 0.001 && value !== 0)) return value.toExponential(3)
  return value.toFixed(Math.abs(value) < 1 ? 5 : 3)
}

function formatSigned(value: number | null): string {
  if (value === null) return '—'
  return `${value > 0 ? '+' : ''}${format(value)}`
}

function preferredSeries(): string {
  return series.value.find((item) => /(^|[._-])loss($|[._-])|loss/i.test(item))
    ?? series.value.find((item) => /learning_rate|(^|[._-])lr($|[._-])/i.test(item))
    ?? series.value[0]
    ?? ''
}

function ensureFocusSeries(): void {
  if (!series.value.includes(focusSeries.value)) focusSeries.value = preferredSeries()
}

function boundedMetrics(values: TrainingMetric[]): TrainingMetric[] {
  if (values.length <= MAX_CHART_POINTS) return values
  const ordered = values.slice().sort((left, right) => coordinate(left) - coordinate(right))
  const bucketCount = Math.max(1, Math.floor((MAX_CHART_POINTS - 2) / 2))
  const bucketSize = Math.ceil((ordered.length - 2) / bucketCount)
  const selected = new Map<string, TrainingMetric>()
  const keep = (metric: TrainingMetric) => selected.set(`${metric.step}:${metric.timestamp}:${metric.series}:${metric.value}`, metric)
  keep(ordered[0])
  keep(ordered[ordered.length - 1])
  for (let start = 1; start < ordered.length - 1; start += bucketSize) {
    const bucket = ordered.slice(start, Math.min(ordered.length - 1, start + bucketSize))
    const low = bucket.reduce((candidate, metric) => metric.value < candidate.value ? metric : candidate)
    const high = bucket.reduce((candidate, metric) => metric.value > candidate.value ? metric : candidate)
    keep(low)
    keep(high)
  }
  return [...selected.values()]
    .sort((left, right) => coordinate(left) - coordinate(right))
    .slice(0, MAX_CHART_POINTS)
}

function updateOverview(lines: MetricLine[]): void {
  const values = new Map(overview.value.map((item) => [item.series, { ...item }]))
  for (const line of lines) {
    for (const [seriesName, value] of Object.entries(line.metrics)) {
      if (!Number.isFinite(value)) continue
      const metric: TrainingMetric = { series: seriesName, value, step: line.step, timestamp: line.timestamp }
      const current = values.get(seriesName)
      if (!current) {
        values.set(seriesName, { series: seriesName, count: 1, first: metric, latest: metric, minimum: metric, maximum: metric })
        continue
      }
      current.count += 1
      current.latest = metric
      if (metric.value < current.minimum.value) current.minimum = metric
      if (metric.value > current.maximum.value) current.maximum = metric
    }
  }
  overview.value = [...values.values()]
}

function flushMetricLines(): void {
  metricFlushFrame = null
  if (!pendingMetricLines.length) return
  const lines = pendingMetricLines.splice(0)
  streamCursor = Math.max(streamCursor, ...lines.map((item) => item.cursor))
  updateOverview(lines.map((item) => item.line))
  const focus = focusSeries.value
  const appended = lines.flatMap(({ line }) => Object.entries(line.metrics)
    .filter(([name, value]) => name === focus && Number.isFinite(value))
    .map(([series, value]) => ({ series, value, step: line.step, timestamp: line.timestamp })))
  if (appended.length) metrics.value = boundedMetrics([...metrics.value, ...appended])
}

function queueMetricLine(cursor: number, line: MetricLine): void {
  pendingMetricLines.push({ cursor, line })
  if (metricFlushFrame !== null) return
  metricFlushFrame = requestAnimationFrame(flushMetricLines)
}

function scheduleChartRender(): void {
  if (renderFrame !== null) return
  renderFrame = requestAnimationFrame(() => {
    renderFrame = null
    renderChart()
  })
}

function renderChart(): void {
  if (!chart) return
  const rawPoints = dedupeByCoordinate(focusPoints.value)
  const smoothWindow = smoothing.value === 0
    ? 1
    : Math.max(2, Math.round((smoothing.value / 100) * Math.max(5, Math.round(rawPoints.length * 0.08))))
  const points = smoothWindow <= 1 ? rawPoints : movingAverage(rawPoints, smoothWindow)
  const data = points.map((metric) => ({ value: [coordinate(metric), metric.value], metric }))
  chart.setOption({
    animation: false,
    backgroundColor: 'transparent',
    grid: { left: 68, right: 24, top: 26, bottom: 30, containLabel: false },
    xAxis: {
      type: axisMode.value === 'time' ? 'time' : 'value',
      scale: true,
      axisLine: { lineStyle: { color: '#cad8ed' } },
      axisTick: { show: false },
      axisLabel: { color: '#7184a0', fontFamily: 'var(--font-mono)', fontSize: 10, hideOverlap: true },
      splitLine: { lineStyle: { color: '#e2ebf7', type: 'dashed' } },
    },
    yAxis: {
      type: 'value',
      scale: true,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: '#7184a0', fontFamily: 'var(--font-mono)', fontSize: 10 },
      splitLine: { lineStyle: { color: '#e2ebf7', type: 'dashed' } },
    },
    tooltip: {
      trigger: 'axis',
      animation: false,
      appendToBody: true,
      borderColor: '#9fb7de',
      backgroundColor: 'rgba(255,255,255,.97)',
      textStyle: { color: '#1e293b', fontSize: 11 },
      formatter: (items: Array<{ data?: { metric?: TrainingMetric } }>) => {
        const metric = items[0]?.data?.metric
        if (!metric) return ''
        const time = new Date(timestampMilliseconds(metric.timestamp)).toLocaleTimeString()
        return `<b>${focusSeries.value}</b> &nbsp; ${format(metric.value)}<br/>Step ${metric.step} · ${time}`
      },
    },
    dataZoom: [{
      type: 'inside',
      xAxisIndex: 0,
      start: dataZoomStart,
      end: dataZoomEnd,
      zoomOnMouseWheel: true,
      moveOnMouseMove: true,
      moveOnMouseWheel: false,
      filterMode: 'none',
    }],
    series: [{
      type: 'line',
      name: focusSeries.value,
      data,
      showSymbol: false,
      symbol: 'circle',
      smooth: false,
      clip: true,
      lineStyle: { width: 3.5, color: lineColor.value, cap: 'round', join: 'round' },
      itemStyle: { color: lineColor.value },
      areaStyle: {
        color: new graphic.LinearGradient(0, 0, 0, 1, [
          { offset: 0, color: `${lineColor.value}42` },
          { offset: 1, color: `${lineColor.value}05` },
        ]),
      },
    }],
  } as never, { notMerge: true, lazyUpdate: true })
}

function resetViewport(): void {
  dataZoomStart = 0
  dataZoomEnd = 100
  zoomLevel.value = 1
  chart?.dispatchAction({ type: 'dataZoom', start: dataZoomStart, end: dataZoomEnd })
}

function zoomChart(factor: number): void {
  const span = clamp((dataZoomEnd - dataZoomStart) / factor, 2, 100)
  const midpoint = (dataZoomStart + dataZoomEnd) / 2
  dataZoomStart = clamp(midpoint - span / 2, 0, 100 - span)
  dataZoomEnd = dataZoomStart + span
  zoomLevel.value = Number((100 / span).toFixed(1))
  chart?.dispatchAction({ type: 'dataZoom', start: dataZoomStart, end: dataZoomEnd })
}

function onChartKeydown(event: KeyboardEvent): void {
  if (event.key === '+' || event.key === '=') { event.preventDefault(); zoomChart(1.32); return }
  if (event.key === '-') { event.preventDefault(); zoomChart(1 / 1.32); return }
  if (event.key === 'Home') { event.preventDefault(); resetViewport() }
}

async function loadArtifacts(): Promise<void> {
  artifactsLoading.value = true
  try {
    artifacts.value = (await getTrainingArtifacts(props.taskId)).artifacts
  } catch {
    artifacts.value = []
  } finally {
    artifactsLoading.value = false
  }
}

async function loadLogs(): Promise<void> {
  logsLoading.value = true
  try {
    logs.value = (await getTrainingLogs(props.taskId)).text
  } catch {
    // Metrics remain useful even when a runner has not produced console output.
  } finally {
    logsLoading.value = false
  }
}

function stopStream(): void {
  eventSource?.close()
  eventSource = null
}

function startStream(cursor: number): void {
  stopStream()
  if (!props.active || !props.visible || typeof EventSource === 'undefined') return
  streamCursor = cursor
  eventSource = new EventSource(trainingMetricEventsUrl(props.taskId, cursor))
  eventSource.addEventListener('metrics', (event) => {
    try {
      const message = event as MessageEvent<string>
      const line = JSON.parse(message.data) as MetricLine
      const eventCursor = Number(message.lastEventId)
      queueMetricLine(Number.isFinite(eventCursor) && eventCursor > 0 ? eventCursor : streamCursor, line)
    } catch {
      // Ignore a corrupt or incomplete line. The backend retains the cursor until a complete line exists.
    }
  })
}

function stopLogPolling(): void {
  if (logRefreshTimer) clearInterval(logRefreshTimer)
  logRefreshTimer = undefined
}

function startLogPolling(): void {
  stopLogPolling()
  if (!props.active || !props.visible) return
  void loadLogs()
  logRefreshTimer = setInterval(() => {
    void loadLogs()
    monitorPollCycle += 1
    if (monitorPollCycle % 5 === 0) void loadArtifacts()
  }, 4_000)
}

function stopMonitorActivity(): void {
  lifecycleRevision += 1
  stopStream()
  stopLogPolling()
  requestController?.abort()
  requestController = null
  if (metricFlushFrame !== null) cancelAnimationFrame(metricFlushFrame)
  metricFlushFrame = null
  pendingMetricLines.length = 0
}

async function loadMonitor(): Promise<void> {
  const revision = ++lifecycleRevision
  requestController?.abort()
  requestController = new AbortController()
  loading.value = true
  error.value = ''
  try {
    const response = await getTrainingMetricsOverview(props.taskId, requestController.signal)
    if (revision !== lifecycleRevision) return
    overview.value = response.series
    ensureFocusSeries()
    if (!focusSeries.value) {
      metrics.value = []
      return
    }
    const snapshot = await getTrainingMetrics(props.taskId, { series: [focusSeries.value], maxPoints: MAX_CHART_POINTS }, requestController.signal)
    if (revision !== lifecycleRevision) return
    metrics.value = boundedMetrics(snapshot.metrics)
    streamCursor = snapshot.cursor
    startStream(snapshot.cursor)
    void loadArtifacts()
    startLogPolling()
  } catch (reason: unknown) {
    if (!requestController?.signal.aborted) error.value = reason instanceof Error ? reason.message : '无法加载训练指标'
  } finally {
    if (revision === lifecycleRevision) loading.value = false
  }
}

function selectSeries(seriesName: string): void {
  if (focusSeries.value === seriesName) return
  focusSeries.value = seriesName
  resetViewport()
  stopStream()
  if (props.visible) void loadMonitor()
}

function refresh(): void {
  stopMonitorActivity()
  if (props.visible) void loadMonitor()
}

watch(() => [props.taskId, props.active, props.visible], () => {
  stopMonitorActivity()
  if (props.visible) void loadMonitor()
}, { immediate: true })
watch([focusPoints, axisMode, smoothing, lineColor], scheduleChartRender)

function attachChart(host: HTMLElement | null): void {
  if (!host || chart) return
  chart = init(host, undefined, { renderer: 'canvas', useDirtyRect: false })
  if (typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => {
      if (resizeFrame !== null) return
      resizeFrame = requestAnimationFrame(() => {
        resizeFrame = null
        chart?.resize()
      })
    })
    resizeObserver.observe(host)
  }
  scheduleChartRender()
}

function setChartHost(host: Element | ComponentPublicInstance | null): void {
  const element = host instanceof HTMLElement ? host : null
  chartHost.value = element
  attachChart(element)
}

onMounted(() => {
  attachChart(chartHost.value)
  document.addEventListener('keydown', onLightboxKeydown)
})

function onLightboxKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape' && lightboxArtifact.value) closeSampleLightbox()
}

onBeforeUnmount(() => {
  document.removeEventListener('keydown', onLightboxKeydown)
  stopMonitorActivity()
  requestController?.abort()
  if (renderFrame !== null) cancelAnimationFrame(renderFrame)
  if (resizeFrame !== null) cancelAnimationFrame(resizeFrame)
  resizeObserver?.disconnect()
  resizeObserver = null
  chart?.dispose()
  chart = null
})
</script>

<template>
  <section ref="monitorRoot" class="training-live-monitor" aria-label="原生训练监控">
    <header>
      <div><LineChart :size="17" /><strong>原生训练监控 <InfoTooltip title="训练监控" description="读取训练器的 Accelerate 指标与本地资源采样。没有上报的指标会保持缺失，不会由界面猜测。" /></strong><span>增量实时指标流 · 长训练内存有界</span></div>
      <button class="button button-small" type="button" :disabled="loading" @click="refresh"><RefreshCw :size="14" /> 刷新</button>
    </header>
    <section v-if="trainingStepTarget" class="training-step-progress">
      <div><span>训练步骤 <InfoTooltip title="训练步骤" description="当前已完成的优化 step 与任务配置中的最大 step。部分上游训练器只能在启动后确定总步数。" /></span><strong>Step {{ currentTrainingStep }} / {{ trainingStepTarget }}</strong><small>{{ trainingProgressPercent.toFixed(1) }}% · 预计剩余 {{ formatDuration(etaSeconds) }}</small></div>
      <div class="training-step-progress-track" role="progressbar" aria-label="训练步骤进度" :aria-valuemin="0" :aria-valuemax="trainingStepTarget" :aria-valuenow="currentTrainingStep"><i :style="{ width: `${trainingProgressPercent}%` }" /></div>
    </section>
    <p v-if="error" class="training-monitor-error">{{ error }}</p>
    <p v-else-if="loading && !overview.length" class="training-monitor-empty">正在读取训练指标…</p>
    <p v-else-if="!overview.length" class="training-monitor-empty">训练器开始上报后，这里会显示 loss、学习率、吞吐、ETA、梯度及资源曲线。</p>
    <template v-else>
      <section class="training-scalar-workbench training-monitor-panel">
        <header class="training-scalar-heading">
          <div>
            <span class="training-scalar-kicker">SCALARS</span>
            <strong>交互式训练曲线 <InfoTooltip title="训练曲线" description="纵轴始终为原始数值；平滑只改变绘制方式。比较实验时请使用同一指标、相同横轴与相近采样频率。" /></strong>
            <small>Canvas 渲染 · 真实数值纵轴 · 平滑只改变曲线形态，不改变原始指标</small>
          </div>
          <div class="training-scalar-overview">
            <span><small>当前</small><strong>{{ format(currentMetric?.value ?? null) }}</strong></span>
            <span><small>最小</small><strong>{{ format(minMetric) }}</strong></span>
            <span><small>最大</small><strong>{{ format(maxMetric) }}</strong></span>
          </div>
        </header>
        <div class="training-chart-toolbar">
          <div class="training-series-tabs" aria-label="选择图表指标">
            <button v-for="item in series" :key="item" type="button" :class="{ active: focusSeries === item }" @click="selectSeries(item)">
              <i :style="{ backgroundColor: colors[Math.max(0, series.indexOf(item)) % colors.length] }" />{{ item }}
            </button>
          </div>
          <div class="training-chart-controls">
            <span class="training-axis-switch" aria-label="横轴单位"><button type="button" :class="{ active: axisMode === 'step' }" @click="axisMode = 'step'">Step</button><button type="button" :class="{ active: axisMode === 'time' }" @click="axisMode = 'time'">时间</button></span>
            <label class="training-smoothing-control">平滑 <InfoTooltip title="曲线平滑" description="仅帮助观察趋势，不会修改训练数据。检查异常尖峰或梯度爆炸时应先回到 0%。" /><input v-model.number="smoothing" type="range" min="0" max="100" step="5" aria-label="曲线平滑程度" /><output>{{ smoothing }}%</output></label>
            <button type="button" aria-label="缩小曲线" title="缩小曲线" :disabled="zoomLevel <= 1" @click="zoomChart(1 / 1.32)"><ZoomOut :size="13" /></button>
            <button type="button" aria-label="放大曲线" title="放大曲线" @click="zoomChart(1.32)"><ZoomIn :size="13" /></button>
            <button type="button" aria-label="重置曲线视图" title="重置视图" :disabled="zoomLevel <= 1" @click="resetViewport"><RotateCcw :size="13" /></button>
          </div>
        </div>
        <div class="training-interactive-chart" role="application" aria-label="交互式训练曲线" aria-roledescription="可缩放训练标量图" tabindex="0" @keydown="onChartKeydown">
          <div :ref="setChartHost" class="training-echarts-canvas" aria-label="训练曲线画布" />
          <footer><MoveHorizontal :size="13" /><span>滚轮缩放、拖拽平移；悬停查看训练点详情。平滑 0% 保留尖角，100% 为连续平滑曲线。</span><strong v-if="zoomLevel > 1">{{ zoomLevel.toFixed(1) }}×</strong></footer>
        </div>
        <section class="training-research-summary" aria-label="研究摘要">
          <strong>研究摘要 <InfoTooltip title="研究摘要" description="根据当前选中的标量计算变化、近期均值和趋势；它是描述性统计，不代表模型质量结论。" /></strong>
          <span><small>相对起点</small><b>{{ formatSigned(deltaFromStart) }}</b></span>
          <span><small>近 {{ Math.min(RECENT_WINDOW, focusPoints.length) }} 点均值</small><b>{{ format(recentAverage) }}</b></span>
          <span><small>近期趋势</small><b>{{ formatSigned(recentTrend) }}</b></span>
          <span><small>样本数</small><b>{{ focusPoints.length.toLocaleString() }}</b></span>
        </section>
      </section>
      <div class="training-metric-cards training-monitor-panel">
        <button v-for="metric in lastMetrics.slice(0, 12)" :key="metric.series" type="button" :class="{ selected: metric.series === focusSeries }" @click="selectSeries(metric.series)">
          <span><Gauge v-if="metric.series.startsWith('resource.')" :size="13" /><Activity v-else :size="13" />{{ metric.series }}</span><strong>{{ format(metric.value) }}</strong><small>step {{ metric.step }}</small>
        </button>
      </div>
    </template>
    <section class="training-artifacts training-monitor-panel" aria-label="样图与训练产物">
      <header><div><Image :size="15" /><strong>样图与产物 <InfoTooltip title="样图与产物" description="样图用于定性对照，权重文件用于 checkpoint 与 SVD 分析；两者都应结合验证集或固定 Prompt 判断。" /></strong><small>{{ artifactsLoading ? '正在索引…' : `${artifacts.length} 项` }}</small></div><button class="button button-small" type="button" :disabled="artifactsLoading" @click="loadArtifacts"><RefreshCw :size="13" /> 刷新产物</button></header>
      <div v-if="sampleArtifacts.length" class="training-sample-grid">
        <button v-for="artifact in sampleArtifacts" :key="artifact.id" type="button" class="training-sample-thumb" :title="artifact.step != null ? `Step ${artifact.step} · 单击放大` : '单击放大'" @click="openSampleLightbox(artifact)">
          <img :src="artifact.url" loading="lazy" :alt="artifact.name" /><span>{{ artifact.name }}</span>
        </button>
      </div>
      <p v-else class="training-monitor-empty">训练样图生成后将在这里显示。</p>
      <div v-if="loraArtifacts.length" class="training-artifact-list"><a v-for="artifact in loraArtifacts" :key="artifact.id" :href="artifact.url" target="_blank" rel="noreferrer"><span><b>LoRA</b>{{ artifact.name }}</span><small>{{ (artifact.size_bytes / 1024 / 1024).toFixed(2) }} MiB</small></a></div>
    </section>
    <section class="training-monitor-logs training-monitor-panel" aria-label="训练运行日志">
      <header><strong><Timer :size="14" />运行日志 <InfoTooltip title="运行日志" description="保留上游训练器的控制台输出，用于定位参数、数据集和 CUDA 失败；日志内容不是结构化指标。" /></strong><button class="button button-small" type="button" :disabled="logsLoading" @click="loadLogs"><RefreshCw :size="13" /> 刷新日志</button></header>
      <pre>{{ logs || '训练启动后会在这里持续显示控制台输出。' }}</pre>
    </section>

    <Transition name="toast">
      <div v-if="lightboxArtifact" class="sample-lightbox" role="dialog" aria-modal="true" aria-label="样图放大预览" @click.self="closeSampleLightbox">
        <button type="button" class="sample-lightbox-close" aria-label="关闭预览" @click="closeSampleLightbox"><X :size="20" /></button>
        <figure>
          <img :src="lightboxArtifact.url" :alt="lightboxArtifact.name" />
          <figcaption>
            <strong>{{ lightboxArtifact.name }}</strong>
            <span v-if="lightboxArtifact.step != null">Step {{ lightboxArtifact.step }}</span>
          </figcaption>
        </figure>
        <pre v-if="lightboxArtifact.prompt" class="sample-lightbox-prompt"><strong>Sample Prompt</strong>{{ lightboxArtifact.prompt }}</pre>
        <a v-else class="sample-lightbox-open" :href="lightboxArtifact.url" target="_blank" rel="noreferrer">在新窗口打开原图</a>
      </div>
    </Transition>
  </section>
</template>
