<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { BarChart3, FileDown, FilePlus2, FolderOpen, LoaderCircle, Plus, Trash2 } from '@lucide/vue'
import { init, use, type ECharts } from 'echarts/core'
import { BarChart, LineChart } from 'echarts/charts'
import { GridComponent, LegendComponent, TooltipComponent } from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import {
  analyzeLoraSvd,
  browseTrainingPath,
  getTrainingArtifacts,
  loraSvdExportUrl,
  type LoraSvdAnalysisFile,
  type LoraSvdAnalysisResult,
  type LoraSvdModelReport,
  type TrainingArtifact,
  type TaskSummary,
  type TrainingPathBrowser,
  type TrainingRuntimeProfile,
} from '../api'
import InfoTooltip from './InfoTooltip.vue'

use([LineChart, BarChart, GridComponent, LegendComponent, TooltipComponent, CanvasRenderer])

const props = defineProps<{
  profiles: TrainingRuntimeProfile[]
  trainingTasks: TaskSummary[]
}>()

const selectedFiles = ref<LoraSvdAnalysisFile[]>([])
const localPath = ref('')
const runtimeProfileId = ref('')
const loading = ref(false)
const error = ref('')
const analysis = ref<LoraSvdAnalysisResult | null>(null)
const selectedReportId = ref('')
const selectedTaskId = ref('')
const loadingArtifacts = ref(false)
const taskLoraArtifacts = ref<TrainingArtifact[]>([])
const selectedTaskArtifactPaths = ref<string[]>([])
const browser = ref<TrainingPathBrowser | null>(null)
const browserLoading = ref(false)
const moduleQuery = ref('')
const spectrumHost = ref<HTMLElement | null>(null)
const energyHost = ref<HTMLElement | null>(null)
const comparisonHost = ref<HTMLElement | null>(null)
let spectrumChart: ECharts | null = null
let energyChart: ECharts | null = null
let comparisonChart: ECharts | null = null
let resizeObserver: ResizeObserver | null = null

const availableProfiles = computed(() => props.profiles.filter((profile) => profile.installed))
const selectedReport = computed(() => analysis.value?.reports.find((report) => report.id === selectedReportId.value) ?? analysis.value?.reports[0] ?? null)
const spectrumPointCount = computed(() => selectedReport.value?.global_singular_values_count ?? selectedReport.value?.global_singular_values.length ?? 0)
const selectedTask = computed(() => props.trainingTasks.find((task) => task.id === selectedTaskId.value))
const selectedTaskArtifacts = computed(() => taskLoraArtifacts.value.filter((artifact) => selectedTaskArtifactPaths.value.includes(artifact.path)))
const remainingTaskArtifactSlots = computed(() => Math.max(0, 5 - selectedFiles.value.length))
const visibleModules = computed(() => {
  const query = moduleQuery.value.trim().toLowerCase()
  const modules = selectedReport.value?.modules ?? []
  const filtered = query ? modules.filter((module) => `${module.id} ${module.component} ${module.flag ?? ''}`.toLowerCase().includes(query)) : modules
  return filtered.slice(0, 120)
})
const canAnalyze = computed(() => selectedFiles.value.length > 0 && !!runtimeProfileId.value && !loading.value)

watch(availableProfiles, (profiles) => {
  if (!profiles.some((profile) => profile.id === runtimeProfileId.value)) runtimeProfileId.value = profiles[0]?.id ?? ''
}, { immediate: true })

watch(selectedTaskId, () => {
  taskLoraArtifacts.value = []
  selectedTaskArtifactPaths.value = []
})

watch(selectedReport, async () => {
  await nextTick()
  renderCharts()
})

function basename(path: string): string {
  return path.split(/[\\/]/).pop()?.replace(/\.safetensors$/i, '') || 'LoRA'
}

function addFile(file: LoraSvdAnalysisFile): void {
  const path = file.path.trim()
  if (!path || !/\.safetensors$/i.test(path)) {
    error.value = '请选择 .safetensors 格式的 LoRA 权重。'
    return
  }
  if (selectedFiles.value.some((entry) => entry.path === path)) return
  if (selectedFiles.value.length >= 5) {
    error.value = '最多可比较 5 个 LoRA checkpoint。'
    return
  }
  selectedFiles.value.push({ path, label: file.label?.trim() || basename(path) })
  localPath.value = ''
  error.value = ''
}

function addLocalPath(): void {
  addFile({ path: localPath.value, label: basename(localPath.value) })
}

function removeFile(path: string): void {
  selectedFiles.value = selectedFiles.value.filter((file) => file.path !== path)
}

async function openBrowser(path = localPath.value): Promise<void> {
  browserLoading.value = true
  error.value = ''
  try {
    browser.value = await browseTrainingPath('model', path)
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : '无法浏览此路径'
  } finally {
    browserLoading.value = false
  }
}

async function loadTaskArtifacts(): Promise<void> {
  if (!selectedTaskId.value || loadingArtifacts.value) return
  loadingArtifacts.value = true
  error.value = ''
  try {
    const artifacts = await getTrainingArtifacts(selectedTaskId.value)
    const loras = artifacts.artifacts
      .filter((artifact) => artifact.kind === 'lora' && /\.safetensors$/i.test(artifact.path))
      .sort((left, right) => (left.step ?? Number.MAX_SAFE_INTEGER) - (right.step ?? Number.MAX_SAFE_INTEGER) || left.modified_at - right.modified_at || left.name.localeCompare(right.name))
    if (!loras.length) {
      error.value = '该训练任务尚未发现可分析的 Safetensors LoRA 产物。'
      return
    }
    taskLoraArtifacts.value = loras
    selectedTaskArtifactPaths.value = []
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : '无法读取训练产物'
  } finally {
    loadingArtifacts.value = false
  }
}

function addSelectedTaskArtifacts(): void {
  const additions = selectedTaskArtifacts.value.filter((artifact) => !selectedFiles.value.some((file) => file.path === artifact.path))
  if (!additions.length) {
    error.value = '请先勾选至少一个尚未加入分析列表的训练产物。'
    return
  }
  if (additions.length > remainingTaskArtifactSlots.value) {
    error.value = `最多可分析 5 个 checkpoint；当前还可加入 ${remainingTaskArtifactSlots.value} 个。`
    return
  }
  selectedFiles.value.push(...additions.map((artifact) => ({ path: artifact.path, label: artifact.name })))
  selectedTaskArtifactPaths.value = []
  error.value = ''
}

async function runAnalysis(): Promise<void> {
  if (!canAnalyze.value) return
  loading.value = true
  error.value = ''
  try {
    analysis.value = await analyzeLoraSvd({ runtime_profile_id: runtimeProfileId.value, files: selectedFiles.value, device: 'auto' })
    selectedReportId.value = analysis.value.reports[0]?.id ?? ''
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : 'LoRA SVD 分析失败'
  } finally {
    loading.value = false
  }
}

function percent(value: number): string {
  return `${(value * 100).toFixed(value < 0.1 ? 2 : 1)}%`
}

function format(value: number): string {
  if (!Number.isFinite(value)) return '—'
  if (Math.abs(value) >= 1000 || (Math.abs(value) > 0 && Math.abs(value) < 0.001)) return value.toExponential(3)
  return value.toFixed(value < 1 ? 3 : 1)
}

function verdictLabel(report: LoraSvdModelReport): string {
  return ({
    high_compression_headroom: '高压缩余量',
    compressible: '存在压缩余量',
    well_utilized: 'rank 利用合理',
    saturation_signal: '可能接近容量边界',
    partial_evidence: '部分证据',
  })[report.verdict]
}

function chartOption(report: LoraSvdModelReport): void {
  if (report.svd_applicable === false) {
    const unsupportedOption = {
      animation: false,
      xAxis: { show: false },
      yAxis: { show: false },
      series: [],
      graphic: [{ type: 'text', left: 'center', top: 'middle', style: { text: '此适配器不是标准 LoRA 因子结构\n不生成误导性的 SVD/rank 图表', fill: '#64748b', font: '13px sans-serif', textAlign: 'center', lineHeight: 21 } }],
    }
    spectrumChart?.setOption(unsupportedOption as never, { notMerge: true })
    energyChart?.setOption(unsupportedOption as never, { notMerge: true })
    return
  }
  const singular = report.global_singular_values.slice(0, 512)
  spectrumChart?.setOption({
    animation: false,
    grid: { left: 52, right: 18, top: 32, bottom: 38 },
    tooltip: { trigger: 'axis' },
    xAxis: { type: 'category', name: '全局奇异方向', data: singular.map((_, index) => String(index + 1)), axisLabel: { show: false } },
    yAxis: { type: 'log', name: 'σ', min: 'dataMin', axisLabel: { formatter: (value: number) => format(value) } },
    series: [{ type: 'line', data: singular, symbol: 'none', lineStyle: { width: 2.5, color: '#2563eb' }, areaStyle: { color: 'rgba(37,99,235,.12)' } }],
  } as never, { notMerge: true })
  energyChart?.setOption({
    animation: false,
    grid: { left: 52, right: 18, top: 32, bottom: 38 },
    tooltip: { trigger: 'axis', valueFormatter: (value: number) => percent(value) },
    xAxis: { type: 'category', name: '排序后的方向', data: report.global_cumulative_energy.slice(0, 512).map((_, index) => String(index + 1)), axisLabel: { show: false } },
    yAxis: { type: 'value', min: 0, max: 1, axisLabel: { formatter: (value: number) => percent(value) } },
    series: [{ type: 'line', data: report.global_cumulative_energy.slice(0, 512), symbol: 'none', lineStyle: { width: 2.5, color: '#10b981' }, markLine: { symbol: 'none', label: { formatter: ({ value }: { value: number }) => percent(value) }, data: [{ yAxis: 0.95 }, { yAxis: 0.99 }, { yAxis: 0.999 }] } }],
  } as never, { notMerge: true })
}

function comparisonOption(): void {
  const comparison = analysis.value?.comparison
  if (!comparison?.checkpoints.length) return
  const checkpoints = comparison.checkpoints
  comparisonChart?.setOption({
    animation: false,
    grid: { left: 52, right: 18, top: 32, bottom: 38 },
    tooltip: { trigger: 'axis' },
    legend: { data: ['99% 有效 rank', 'rank 利用率'], top: 4 },
    xAxis: { type: 'category', data: checkpoints.map((checkpoint) => checkpoint.step ? `Step ${checkpoint.step}` : checkpoint.label), axisLabel: { rotate: 18 } },
    yAxis: [{ type: 'value', name: 'rank' }, { type: 'value', name: '利用率', min: 0, max: 1, axisLabel: { formatter: (value: number) => percent(value) } }],
    series: [
      { name: '99% 有效 rank', type: 'line', data: checkpoints.map((checkpoint) => checkpoint.effective_rank.energy_99), symbolSize: 7, lineStyle: { color: '#8b5cf6', width: 2.5 } },
      { name: 'rank 利用率', type: 'bar', yAxisIndex: 1, data: checkpoints.map((checkpoint) => checkpoint.rank_utilization), itemStyle: { color: 'rgba(16,185,129,.65)' } },
    ],
  } as never, { notMerge: true })
}

function renderCharts(): void {
  if (spectrumHost.value && !spectrumChart) spectrumChart = init(spectrumHost.value)
  if (energyHost.value && !energyChart) energyChart = init(energyHost.value)
  if (comparisonHost.value && !comparisonChart) comparisonChart = init(comparisonHost.value)
  if (selectedReport.value) chartOption(selectedReport.value)
  comparisonOption()
  if (!resizeObserver && spectrumHost.value && typeof ResizeObserver !== 'undefined') {
    resizeObserver = new ResizeObserver(() => {
      spectrumChart?.resize()
      energyChart?.resize()
      comparisonChart?.resize()
    })
    resizeObserver.observe(spectrumHost.value)
  }
}

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  spectrumChart?.dispose()
  energyChart?.dispose()
  comparisonChart?.dispose()
})
</script>

<template>
  <section class="svd-page" aria-label="LoRA SVD 分析工作区">
    <header class="surface svd-intro">
      <div>
        <p class="eyebrow">LOW-RANK DIAGNOSTICS</p>
        <h2>LoRA SVD 分析</h2>
        <p>从已训练权重的奇异值谱观察 rank 余量与容量边界信号；结论应与样图和 rank 消融实验共同判断。<InfoTooltip title="LoRA SVD 分析" description="仅分析标准 LoRA 的 down/up 或 A/B 因子对。它反映当前 ΔW 的谱结构，不能单独证明训练质量或最佳 rank。" /></p>
      </div>
      <a v-if="analysis" class="button" :href="loraSvdExportUrl(analysis.id)" download><FileDown :size="16" /> 导出 JSON 报告</a>
    </header>

    <section class="surface svd-source" aria-label="选择 LoRA 权重">
      <div class="svd-source-header"><strong><FilePlus2 :size="17" /> 分析对象 <InfoTooltip title="分析对象" description="可从本地路径或训练任务的任意产物中勾选 1–5 个 checkpoint。列表会排序，但不会自动选择前五个。" /></strong><small>支持 1–5 个 Safetensors LoRA；多个 checkpoint 将按训练步数或修改时间排序。</small></div>
      <div class="svd-source-controls">
        <label>分析运行时<select v-model="runtimeProfileId"><option value="" disabled>选择已安装运行时</option><option v-for="profile in availableProfiles" :key="profile.id" :value="profile.id">{{ profile.label }}</option></select></label>
        <label class="svd-path-field">本地 LoRA 路径<span><input v-model="localPath" placeholder="D:\models\epoch-0001.safetensors" @keyup.enter="addLocalPath" /><button class="button button-small" type="button" @click="addLocalPath"><Plus :size="15" /> 加入分析列表</button><button class="button button-small" type="button" :disabled="browserLoading" @click="openBrowser()"><FolderOpen :size="15" /> 浏览</button></span></label>
        <label v-if="trainingTasks.length">训练任务产物<select v-model="selectedTaskId"><option value="">选择已有训练任务</option><option v-for="task in trainingTasks" :key="task.id" :value="task.id">{{ task.training?.output_name || task.training?.adapter_id || task.id }}</option></select></label>
        <button v-if="trainingTasks.length" class="button button-small" type="button" :disabled="!selectedTask || loadingArtifacts" @click="loadTaskArtifacts">{{ loadingArtifacts ? '正在读取…' : '读取任务 LoRA' }}</button>
      </div>
      <div v-if="browser" class="svd-browser" aria-label="LoRA 文件浏览器">
        <div><strong>{{ browser.current_path }}</strong><button type="button" @click="browser = null">关闭</button></div>
        <div class="svd-browser-list"><button v-if="browser.parent_path" type="button" @click="openBrowser(browser.parent_path)">↥ 上一级</button><button v-for="directory in browser.directories" :key="directory.path" type="button" @click="openBrowser(directory.path)">📁 {{ directory.name }}</button><button v-for="file in browser.files.filter((item) => /\.safetensors$/i.test(item.name))" :key="file.path" type="button" @click="addFile({ path: file.path, label: file.name })">+ {{ file.name }}</button></div>
      </div>
      <section v-if="taskLoraArtifacts.length" class="svd-task-artifacts" aria-label="从训练产物选择 LoRA">
        <header><div><strong>从任务产物选择 checkpoint</strong><small>已加载 {{ taskLoraArtifacts.length }} 个 LoRA；可任意勾选后加入，最多分析 5 个。</small></div><button class="button button-small" type="button" :disabled="!selectedTaskArtifacts.length" @click="addSelectedTaskArtifacts">加入所选 {{ selectedTaskArtifacts.length }} 个</button></header>
        <div class="svd-task-artifact-list"><label v-for="artifact in taskLoraArtifacts" :key="artifact.id"><input v-model="selectedTaskArtifactPaths" type="checkbox" :value="artifact.path" :aria-label="`选择 ${artifact.name}`" /><span><strong>{{ artifact.name }}</strong><small>{{ artifact.step != null ? `Step ${artifact.step} · ` : '' }}{{ new Date(artifact.modified_at * 1000).toLocaleString() }} · {{ (artifact.size_bytes / 1024 / 1024).toFixed(1) }} MiB</small></span></label></div>
      </section>
      <div class="svd-file-list"><span v-if="!selectedFiles.length">尚未选择 LoRA 权重。</span><div v-for="file in selectedFiles" :key="file.path"><strong>{{ file.label }}</strong><small :title="file.path">{{ file.path }}</small><button type="button" :aria-label="`移除 ${file.label}`" @click="removeFile(file.path)"><Trash2 :size="14" /></button></div></div>
      <p v-if="error" class="svd-error">{{ error }}</p>
      <button class="button button-primary" type="button" :disabled="!canAnalyze" @click="runAnalysis"><LoaderCircle v-if="loading" class="spin" :size="16" /><BarChart3 v-else :size="16" /> {{ loading ? '正在分解权重…' : '开始 SVD 分析' }}</button>
    </section>

    <template v-if="analysis && selectedReport">
      <section class="svd-report-toolbar">
        <label v-if="analysis.reports.length > 1">查看权重<select v-model="selectedReportId"><option v-for="report in analysis.reports" :key="report.id" :value="report.id">{{ report.label }}{{ report.step ? ` · Step ${report.step}` : '' }}</option></select></label>
        <span>计算设备：<b>{{ analysis.execution.device }}</b> · {{ analysis.execution.selection_reason || analysis.execution.reason }} · {{ analysis.execution.duration_ms }} ms</span>
      </section>

      <section class="surface svd-verdict" :class="selectedReport.verdict">
        <div><span>研究判断 <InfoTooltip title="研究判断" description="结论基于 99% 有效 rank、统一 rank 与尾部能量的谨慎规则；覆盖不足或混合 rank 会降级为部分证据。" /></span><strong>{{ verdictLabel(selectedReport) }}</strong><p>{{ selectedReport.verdict_message }}</p></div>
        <div class="svd-stat"><span>99% 有效 rank <InfoTooltip title="99% 有效 rank" description="达到 99% 累计谱能量所需的方向数。它是压缩余量信号，不是训练质量评分。" /></span><strong>{{ selectedReport.effective_rank.energy_99 }}</strong><small>当前主 rank {{ selectedReport.rank_distribution.modal }}</small></div>
        <div class="svd-stat"><span>尾部 20% 能量 <InfoTooltip title="尾部 20% 能量" description="排序后最后 20% 奇异方向占据的能量比例。较高时可能表示当前 rank 仍被充分使用。" /></span><strong>{{ percent(selectedReport.tail_energy_20) }}</strong><small>用于识别容量边界信号</small></div>
        <div class="svd-stat"><span>覆盖率 <InfoTooltip title="覆盖率" description="成功分析的标准 LoRA 模块占候选模块数量。DoRA、LoHa、LoKr 等非标准结构会明确排除。" /></span><strong>{{ selectedReport.coverage.analyzed_modules }}/{{ selectedReport.coverage.candidate_modules }}</strong><small>{{ selectedReport.coverage.unsupported_modules }} 个未覆盖模块</small></div>
      </section>

      <section v-if="selectedReport.svd_applicable === false" class="surface svd-not-applicable" role="note"><strong>标准 LoRA SVD 不适用</strong><span>LoHa 使用 Hadamard 结构，LoKr 使用 Kronecker 结构；这不是 down/up 因子 ΔW 的同一数学对象。工作台保留格式与覆盖信息，但不会给出虚假的有效 rank、压缩余量或饱和结论。</span></section>

      <section class="svd-facts">
        <article class="surface"><span>模型识别 <InfoTooltip title="模型识别" description="优先读取可信 safetensors 元数据，其次使用排他性权重键。冲突时显示未知/冲突，不会根据普通文本猜测架构。" /></span><strong>{{ selectedReport.architecture }}</strong><small>{{ selectedReport.format }} · {{ selectedReport.file_size_bytes.toLocaleString() }} bytes</small></article>
        <article class="surface"><span>rank 分布 <InfoTooltip title="rank 分布" description="各模块的实际因子 rank。混合 rank 会降低全局结论的确定性。" /></span><strong>{{ selectedReport.rank_distribution.minimum }}–{{ selectedReport.rank_distribution.maximum }}</strong><small>{{ selectedReport.rank_distribution.uniform ? '所有模块一致' : '存在混合 rank，结论已降级' }}</small></article>
        <article class="surface"><span>保留阈值 <InfoTooltip title="累计能量阈值" description="按奇异值平方累加的能量占比。95%、99%、99.9% 展示不同保守程度下的有效 rank。" /></span><strong>r{{ selectedReport.effective_rank.energy_95 }} / r{{ selectedReport.effective_rank.energy_99 }} / r{{ selectedReport.effective_rank.energy_999 }}</strong><small>95% / 99% / 99.9% 累计能量</small></article>
        <article class="surface"><span>文件指纹</span><strong class="svd-hash">{{ selectedReport.sha256 }}</strong><small>{{ selectedReport.path }}</small></article>
      </section>

      <section class="svd-chart-grid">
        <article class="surface svd-chart-card"><header><strong>全局奇异值谱 <InfoTooltip title="全局奇异值谱" description="将每个模块的奇异值汇总排序后展示。对数纵轴有助于观察长尾方向，不代表单个层的原始矩阵。" /></strong><small>按奇异值大小排序，使用对数纵轴 · 共 {{ spectrumPointCount.toLocaleString() }} 个方向，图中显示前 512 个</small></header><div ref="spectrumHost" class="svd-chart" aria-label="全局奇异值谱图" /></article>
        <article class="surface svd-chart-card"><header><strong>累计能量 <InfoTooltip title="累计能量" description="从最大奇异方向开始累计平方能量。阈值线显示达到各能量占比所需的方向数量。" /></strong><small>95% / 99% / 99.9% 阈值线 · 图中显示前 512 个方向</small></header><div ref="energyHost" class="svd-chart" aria-label="累计能量曲线图" /></article>
      </section>

      <section v-if="analysis.comparison" class="surface svd-comparison"><header><div><strong>Checkpoint 对比 <InfoTooltip title="Checkpoint 对比" description="只有架构和适配器格式一致时，曲线才能解释为同一次训练的演化；否则仅用于并列观察。" /></strong><small>{{ analysis.comparison.reason }}</small></div><span :class="analysis.comparison.comparable ? 'ok-text' : 'warning-text'">{{ analysis.comparison.comparable ? '可解释训练轨迹' : '仅并列比较' }}</span></header><div ref="comparisonHost" class="svd-chart" aria-label="checkpoint rank 对比图" /></section>

      <section class="surface svd-modules"><header><div><strong>模块级证据</strong><small>显示能量最大的模块；搜索可按模块、组件或信号筛选。</small></div><label>筛选模块<input v-model="moduleQuery" placeholder="例如 transformer、saturation" /></label></header><div class="svd-module-table"><div class="svd-row svd-head"><span>模块 / 组件</span><span>rank</span><span>95 / 99 / 99.9%</span><span>稳定 rank</span><span>尾部能量</span><span>判断</span></div><div v-for="module in visibleModules" :key="module.id" class="svd-row"><span><strong :title="module.id">{{ module.id }}</strong><small>{{ module.component }}</small></span><span>r{{ module.rank }}<small>α {{ format(module.alpha) }}</small></span><span>r{{ module.effective_rank.energy_95 }} / r{{ module.effective_rank.energy_99 }} / r{{ module.effective_rank.energy_999 }}</span><span>{{ format(module.stable_rank) }}</span><span>{{ percent(module.tail_energy_20) }}</span><span :class="module.flag || ''">{{ module.flag === 'saturation_signal' ? '可能饱和' : module.flag === 'compression_headroom' ? '高余量' : module.flag === 'compressible' ? '可压缩' : '利用合理' }}</span></div></div><p v-if="selectedReport.modules.length > visibleModules.length" class="svd-table-note">为保持页面响应，当前只显示前 {{ visibleModules.length }} 个匹配模块。</p><details v-if="selectedReport.excluded.length" class="svd-excluded"><summary>未覆盖模块（{{ selectedReport.excluded.length }}）</summary><ul><li v-for="item in selectedReport.excluded" :key="`${item.id}:${item.reason}`"><code>{{ item.id }}</code>：{{ item.reason }}</li></ul></details></section>

      <p class="svd-disclaimer">SVD 展示的是当前 LoRA ΔW 的压缩余量和尾部方向信号；它不能单独证明 rank 过低或生成质量不足。请结合样图、验证集和一致设置的 rank 消融实验作最终决策。</p>
    </template>
  </section>
</template>

<style scoped>
.svd-page { display: grid; gap: 16px; }.svd-intro, .svd-source, .svd-chart-card, .svd-comparison, .svd-modules { padding: 18px; }.svd-intro, .svd-source-header, .svd-task-artifacts > header, .svd-modules > header, .svd-comparison > header, .svd-report-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 16px; }.svd-intro h2 { margin: 0; font-size: 22px; }.svd-intro p { max-width: 780px; margin: 5px 0 0; color: var(--text-secondary); font-size: 13px; line-height: 1.6; }.svd-source { display: grid; gap: 13px; }.svd-source-header strong, .svd-source-header small { display: flex; gap: 7px; align-items: center; }.svd-source-header small { color: var(--text-secondary); font-size: 11px; }.svd-source-controls { display: grid; grid-template-columns: minmax(180px, .75fr) minmax(320px, 1.8fr) minmax(180px, .85fr) auto; gap: 10px; align-items: end; }.svd-source label, .svd-modules label { display: grid; gap: 5px; color: var(--text-secondary); font-size: 11px; font-weight: 600; }.svd-source select, .svd-source input, .svd-modules input { min-height: 36px; border: 1px solid var(--border); border-radius: 8px; background: white; padding: 0 9px; color: var(--text); font: inherit; }.svd-path-field > span { display: flex; gap: 6px; }.svd-path-field input { min-width: 0; flex: 1; }.svd-file-list { display: grid; gap: 6px; }.svd-file-list > span { color: var(--text-secondary); font-size: 12px; }.svd-file-list > div { display: grid; grid-template-columns: minmax(120px, .35fr) minmax(0, 1fr) auto; align-items: center; gap: 10px; padding: 8px 10px; border: 1px solid var(--border); border-radius: 8px; background: var(--surface-muted); }.svd-file-list strong, .svd-file-list small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }.svd-file-list small { color: var(--text-secondary); font-family: var(--font-mono); font-size: 10px; }.svd-file-list button, .svd-browser button { border: 0; background: transparent; color: var(--text-secondary); cursor: pointer; }.svd-error { margin: 0; color: var(--red); font-size: 12px; }.svd-browser { display: grid; gap: 8px; border: 1px solid var(--border); border-radius: 9px; padding: 10px; }.svd-browser > div:first-child { display: flex; justify-content: space-between; gap: 8px; }.svd-browser strong { overflow: hidden; font-family: var(--font-mono); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }.svd-browser-list { display: flex; flex-wrap: wrap; gap: 6px; }.svd-browser-list button { border: 1px solid var(--border); border-radius: 6px; padding: 5px 7px; background: white; font-size: 11px; }.svd-task-artifacts { display: grid; gap: 10px; border: 1px solid var(--border); border-radius: 9px; padding: 12px; background: color-mix(in srgb, var(--surface-muted) 70%, white); }.svd-task-artifacts > header > div { display: grid; gap: 3px; }.svd-task-artifacts strong { font-size: 12px; }.svd-task-artifacts small { color: var(--text-secondary); font-size: 10px; }.svd-task-artifact-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(230px, 1fr)); gap: 7px; max-height: 210px; overflow: auto; padding-right: 3px; }.svd-task-artifact-list label { display: flex; align-items: center; gap: 8px; padding: 8px 9px; border: 1px solid var(--border); border-radius: 7px; background: white; cursor: pointer; }.svd-task-artifact-list input { min-height: 15px; width: 15px; accent-color: var(--blue); }.svd-task-artifact-list span { display: grid; min-width: 0; gap: 2px; }.svd-task-artifact-list strong, .svd-task-artifact-list small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }.svd-report-toolbar { min-height: 35px; color: var(--text-secondary); font-size: 11px; }.svd-report-toolbar label { display: flex; gap: 7px; align-items: center; }.svd-report-toolbar select { max-width: 360px; border: 1px solid var(--border); border-radius: 7px; background: white; padding: 6px; }.svd-verdict { display: grid; grid-template-columns: minmax(260px, 1.8fr) repeat(3, minmax(135px, 1fr)); gap: 14px; border-left: 4px solid var(--blue); padding: 18px; }.svd-verdict.saturation_signal { border-left-color: #f59e0b; }.svd-verdict.high_compression_headroom { border-left-color: #8b5cf6; }.svd-verdict > div:first-child { display: grid; gap: 4px; }.svd-verdict span, .svd-stat span, .svd-facts span { color: var(--text-secondary); font-size: 11px; }.svd-verdict strong { font-size: 20px; }.svd-verdict p { margin: 0; color: var(--text-secondary); font-size: 12px; line-height: 1.55; }.svd-stat { display: grid; align-content: start; gap: 3px; padding-left: 12px; border-left: 1px solid var(--border); }.svd-stat strong { font-variant-numeric: tabular-nums; }.svd-stat small, .svd-facts small { color: var(--text-secondary); font-size: 10px; line-height: 1.45; }.svd-facts { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }.svd-facts article { display: grid; min-width: 0; gap: 4px; padding: 12px; }.svd-facts strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }.svd-facts small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }.svd-hash { font-family: var(--font-mono); font-size: 10px !important; }.svd-chart-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }.svd-chart-card { display: grid; gap: 8px; }.svd-chart-card header, .svd-comparison header { display: grid; gap: 2px; }.svd-chart-card small, .svd-comparison small { color: var(--text-secondary); font-size: 11px; }.svd-chart { height: 255px; }.svd-not-applicable { display: grid; gap: 5px; padding: 15px 18px; border-left: 4px solid #f59e0b; color: var(--text-secondary); font-size: 12px; line-height: 1.55; }.svd-not-applicable strong { color: var(--text); }.svd-modules { display: grid; gap: 12px; }.svd-modules > header > div { display: grid; gap: 2px; }.svd-modules > header small { color: var(--text-secondary); font-size: 11px; }.svd-module-table { overflow: auto; border: 1px solid var(--border); border-radius: 8px; }.svd-row { display: grid; grid-template-columns: minmax(300px, 2fr) 100px minmax(160px, 1fr) 100px 100px 100px; min-width: 900px; gap: 10px; align-items: center; padding: 9px 11px; border-bottom: 1px solid var(--border); font-size: 11px; }.svd-row:last-child { border-bottom: 0; }.svd-row > span { min-width: 0; }.svd-row > span:first-child { display: grid; gap: 2px; }.svd-row strong, .svd-row small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }.svd-row small { display: block; color: var(--text-secondary); font-size: 10px; }.svd-head { background: var(--surface-muted); color: var(--text-secondary); font-size: 10px; font-weight: 700; }.compression_headroom { color: #7c3aed; }.compressible { color: #0284c7; }.well_utilized { color: #059669; }.saturation_signal { color: #d97706; }.svd-table-note, .svd-disclaimer { margin: 0; color: var(--text-secondary); font-size: 11px; line-height: 1.55; }.svd-excluded { color: var(--text-secondary); font-size: 11px; }.svd-excluded ul { display: grid; gap: 4px; max-height: 150px; overflow: auto; }.svd-excluded code { color: var(--text); }.svd-disclaimer { padding: 0 4px; }.spin { animation: svd-spin .85s linear infinite; }@keyframes svd-spin { to { transform: rotate(360deg); } }
@media (max-width: 1100px) { .svd-source-controls, .svd-facts, .svd-chart-grid, .svd-verdict { grid-template-columns: 1fr 1fr; }.svd-path-field { grid-column: span 2; }.svd-verdict > div:first-child { grid-column: span 2; }.svd-stat { border-left: 0; padding-left: 0; } }@media (max-width: 700px) { .svd-intro, .svd-source-header, .svd-task-artifacts > header, .svd-modules > header, .svd-comparison > header, .svd-report-toolbar { align-items: flex-start; flex-direction: column; }.svd-source-controls, .svd-facts, .svd-chart-grid, .svd-verdict { grid-template-columns: 1fr; }.svd-path-field, .svd-verdict > div:first-child { grid-column: auto; }.svd-path-field > span { flex-wrap: wrap; }.svd-path-field input { flex-basis: 100%; }.svd-chart { height: 225px; } }
</style>
