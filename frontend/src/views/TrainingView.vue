<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Activity, BarChart3, ChevronDown, CirclePlay, Cpu, FileCode2, FolderOpen, Gauge, Layers3, Plus, RefreshCw, SlidersHorizontal, Sparkles, Trash2 } from '@lucide/vue'
import GpuLiveStatus from '../components/GpuLiveStatus.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import InfoTooltip from '../components/InfoTooltip.vue'
import {
  createTrainingTask,
  discoverTrainingGalleryAugmentations,
  deleteTrainingTask,
  browseTrainingPath,
  getMediaDirectories,
  getMediaRoots,
  getTrainingAdapters,
  getTrainingCleanupPreview,
  getTrainingGpus,
  getTrainingPresets,
  getTrainingQueue,
  getTrainingRuntimeDiagnostics,
  getTrainingRuntimeProfiles,
  installTrainingRuntime,
  preflightTraining,
  previewTraining,
  previewTrainingGalleryDataset,
  type MediaRoot,
  type TrainingAdapter,
  type TrainingCleanupPreview,
  type TrainingField,
  type TrainingGalleryDataset,
  type TrainingAugmentationSubset,
  type TrainingGalleryDatasetPreview,
  type TrainingGpu,
  type TrainingPathBrowser,
  type TrainingPreflight,
  type TrainingQueueEntry,
  type TrainingRuntimeDiagnostics,
  type TrainingRuntimeProfile,
  type TrainingSampleSettings,
  type TaskSummary,
} from '../api'
import { useToastStore } from '../stores/toast'
import { useTasksStore } from '../stores/tasks'

const TrainingMonitor = defineAsyncComponent(() => import('../components/TrainingMonitor.vue'))
const LoraSvdAnalysis = defineAsyncComponent(() => import('../components/LoraSvdAnalysis.vue'))

const toast = useToastStore()
const tasks = useTasksStore()
const adapters = ref<TrainingAdapter[]>([])
const profiles = ref<TrainingRuntimeProfile[]>([])
const gpus = ref<TrainingGpu[]>([])
const adapterId = ref('sdxl-lora')
const runtimeProfileId = ref('windows')
const values = ref<Record<string, unknown>>({})
const selectedGroups = ref(new Set<string>(['model', 'dataset', 'training', 'network', 'optimizer', 'performance', 'saving']))
const query = ref('')
const selectedGpuIds = ref<string[]>([])
const loading = ref(true)
const submitting = ref(false)
const preflightLoading = ref(false)
const preflight = ref<TrainingPreflight | null>(null)
const preview = ref('')
const requestedTab = new URLSearchParams(window.location.search).get('tab')
const requestedPresetId = new URLSearchParams(window.location.search).get('preset')
const activeTab = ref<'setup' | 'monitor' | 'svd'>(requestedTab === 'monitor' || requestedTab === 'svd' ? requestedTab : 'setup')
const monitorVisited = ref(activeTab.value === 'monitor')
const selectedTrainingTaskId = ref('')
const cleanupTask = ref<TaskSummary | null>(null)
const cleanupPreview = ref<TrainingCleanupPreview | null>(null)
const cleanupPreviewLoading = ref(false)
const cleanupDeleting = ref(false)
let gpuRefreshTimer: ReturnType<typeof setInterval> | undefined
let runtimeRefreshTimer: ReturnType<typeof setInterval> | undefined
let queueRefreshTimer: ReturnType<typeof setInterval> | undefined
const runtimeDiagnostics = ref<TrainingRuntimeDiagnostics | null>(null)
const runtimeActionLoading = ref(false)
const gpuFloatOpen = ref(true)
const gpuFloatOpenMonitor = ref(true)
const queueEntries = ref<TrainingQueueEntry[]>([])
const mediaRoots = ref<MediaRoot[]>([])
const galleryDirectories = ref<string[]>([])
const galleryRootId = ref('')
const galleryDirectory = ref('')
const galleryRepeat = ref(1)
const galleryCaptionExtension = ref('.txt')
const galleryAdditionalDatasets = ref<TrainingGalleryDataset[]>([])
const galleryAugmentationDatasets = ref<GalleryAugmentationDataset[]>([])
const galleryAugmentationLoading = ref(false)
const datasetMode = ref<'path' | 'gallery'>('path')
const galleryPreview = ref<TrainingGalleryDatasetPreview | null>(null)
const galleryPreviewLoading = ref(false)
const pathBrowser = ref<{ kind: 'model' | 'dataset' | 'output'; target: string; data: TrainingPathBrowser } | null>(null)
const pathBrowserLoading = ref(false)
const sampleEnabled = ref(false)
const samplePromptSource = ref<'manual' | 'dataset_captions'>('manual')
const samplePrompt = ref('')
const sampleNegativePrompt = ref('')
const sampleCaptionCount = ref(4)
const sampleSteps = ref(30)
const sampleWidth = ref(1024)
const sampleHeight = ref(1024)
const sampleEveryNEpochs = ref(1)
const fallbackSampleSamplers = ['ddim', 'pndm', 'lms', 'euler', 'euler_a', 'heun', 'dpm_2', 'dpm_2_a', 'dpmsolver', 'dpmsolver++', 'dpmsingle', 'k_lms', 'k_euler', 'k_euler_a', 'k_dpm_2', 'k_dpm_2_a']

type OptimizerTuningKind = 'number' | 'boolean' | 'text'

interface OptimizerTuningField {
  key: string
  label: string
  kind: OptimizerTuningKind
  default: number | boolean | string
  help: string
}

interface GalleryAugmentationDataset extends TrainingGalleryDataset {
  taskId: string
  label: string
  imageCount: number
  enabled: boolean
}

const adamTuning: OptimizerTuningField[] = [
  { key: 'betas', label: 'Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
  { key: 'eps', label: '数值稳定 ε', kind: 'number', default: 1e-8, help: '优化器计算的数值稳定项。' },
  { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0.01, help: '对可训练权重施加的衰减系数。' },
]
const optimizerTuningCatalog: Record<string, OptimizerTuningField[]> = {
  adamw: adamTuning,
  adamw8bit: adamTuning,
  pagedadamw8bit: adamTuning,
  lion: [
    { key: 'betas', label: 'Lion Beta 系数', kind: 'text', default: '(0.9, 0.99)', help: 'Python 元组，例如 (0.9, 0.99)。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  lion8bit: [
    { key: 'betas', label: 'Lion Beta 系数', kind: 'text', default: '(0.9, 0.99)', help: 'Python 元组，例如 (0.9, 0.99)。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  adafactor: [
    { key: 'relative_step', label: 'AdaFactor 相对步长', kind: 'boolean', default: true, help: '启用后将由 AdaFactor 计算相对学习率。' },
    { key: 'scale_parameter', label: 'AdaFactor 参数缩放', kind: 'boolean', default: true, help: '按参数尺度调整更新幅度。' },
    { key: 'warmup_init', label: 'AdaFactor 预热初始化', kind: 'boolean', default: false, help: '启用时会配合相对步长进行预热。' },
    { key: 'beta1', label: 'AdaFactor 一阶动量', kind: 'text', default: 'None', help: 'Python 数值或 None；None 使用无一阶动量模式。' },
    { key: 'decay_rate', label: 'AdaFactor 衰减率', kind: 'number', default: -0.8, help: '二阶统计量的时间衰减指数。' },
    { key: 'clip_threshold', label: 'AdaFactor 裁剪阈值', kind: 'number', default: 1, help: 'RMS 更新裁剪阈值。' },
    { key: 'eps', label: 'AdaFactor ε', kind: 'text', default: '(1e-30, 1e-3)', help: 'Python 元组，例如 (1e-30, 1e-3)。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  prodigy: [
    { key: 'betas', label: 'Prodigy Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
    { key: 'beta3', label: 'Prodigy beta3', kind: 'text', default: 'None', help: 'Python 数值或 None。' },
    { key: 'd_coef', label: 'Prodigy D 系数', kind: 'number', default: 1, help: 'D 自适应估计的缩放系数。' },
    { key: 'growth_rate', label: 'Prodigy 增长率', kind: 'number', default: 1, help: 'D 估计的增长率上限；1 表示不额外放大。' },
    { key: 'decouple', label: 'Prodigy 解耦权重衰减', kind: 'boolean', default: true, help: '将权重衰减从自适应更新中解耦。' },
    { key: 'use_bias_correction', label: 'Prodigy 偏差修正', kind: 'boolean', default: true, help: '使用一阶、二阶动量的偏差修正。' },
    { key: 'safeguard_warmup', label: 'Prodigy 预热保护', kind: 'boolean', default: false, help: '在学习率预热阶段保护 D 估计。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  dadaptation: [
    { key: 'betas', label: 'D-Adapt Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
    { key: 'd0', label: 'D-Adapt 初始 D', kind: 'number', default: 1e-6, help: '自适应 D 估计的初始值。' },
    { key: 'growth_rate', label: 'D-Adapt 增长率', kind: 'number', default: 1, help: 'D 估计的增长率上限。' },
    { key: 'decouple', label: 'D-Adapt 解耦权重衰减', kind: 'boolean', default: true, help: '将权重衰减从自适应更新中解耦。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  dadaptadam: [
    { key: 'betas', label: 'D-Adapt Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
    { key: 'd0', label: 'D-Adapt 初始 D', kind: 'number', default: 1e-6, help: '自适应 D 估计的初始值。' },
    { key: 'growth_rate', label: 'D-Adapt 增长率', kind: 'number', default: 1, help: 'D 估计的增长率上限。' },
    { key: 'decouple', label: 'D-Adapt 解耦权重衰减', kind: 'boolean', default: true, help: '将权重衰减从自适应更新中解耦。' },
    { key: 'use_bias_correction', label: 'D-Adapt 偏差修正', kind: 'boolean', default: false, help: '使用一阶、二阶动量的偏差修正。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  dadaptlion: [
    { key: 'betas', label: 'D-Adapt Lion Beta 系数', kind: 'text', default: '(0.9, 0.99)', help: 'Python 元组，例如 (0.9, 0.99)。' },
    { key: 'd0', label: 'D-Adapt 初始 D', kind: 'number', default: 1e-6, help: '自适应 D 估计的初始值。' },
    { key: 'growth_rate', label: 'D-Adapt 增长率', kind: 'number', default: 1, help: 'D 估计的增长率上限。' },
    { key: 'decouple', label: 'D-Adapt 解耦权重衰减', kind: 'boolean', default: true, help: '将权重衰减从自适应更新中解耦。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  sgdnesterov: [
    { key: 'momentum', label: 'SGD 动量', kind: 'number', default: 0.9, help: 'Nesterov 动量系数。' },
    { key: 'dampening', label: 'SGD 阻尼', kind: 'number', default: 0, help: '动量阻尼系数。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
    { key: 'nesterov', label: '启用 Nesterov', kind: 'boolean', default: true, help: '启用 Nesterov 加速动量。' },
  ],
  radamschedulefree: [
    { key: 'betas', label: 'ScheduleFree Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
    { key: 'r', label: 'ScheduleFree R', kind: 'number', default: 0, help: 'ScheduleFree 的插值参数。' },
    { key: 'weight_lr_power', label: 'ScheduleFree 学习率幂', kind: 'number', default: 2, help: '权重平均使用的学习率幂。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
  'pytorch_optimizer.came': [
    { key: 'betas', label: 'CAME Beta 系数', kind: 'text', default: '(0.9, 0.999)', help: 'Python 元组，例如 (0.9, 0.999)。' },
    { key: 'eps', label: 'CAME ε', kind: 'text', default: '(1e-30, 1e-16)', help: 'Python 元组，例如 (1e-30, 1e-16)。' },
    { key: 'clip_threshold', label: 'CAME 裁剪阈值', kind: 'number', default: 1, help: 'RMS 更新裁剪阈值。' },
    { key: 'decay_rate', label: 'CAME 衰减率', kind: 'number', default: -0.8, help: '二阶统计量的时间衰减指数。' },
    { key: 'weight_decay', label: '权重衰减', kind: 'number', default: 0, help: '对可训练权重施加的衰减系数。' },
  ],
}
const optimizerTuningValues = ref<Record<string, Record<string, number | boolean | string>>>({})

const sampleSamplerChoices = computed(() => {
  const choices = adapter.value?.fields.find((field) => field.key === 'sample_sampler')?.choices ?? []
  return choices.length ? choices : fallbackSampleSamplers
})

interface TrainingParameterHistory {
  id: string
  label: string
  savedAt: number
  adapterId: string
  runtimeProfileId: string
  gpuIds: string[]
  parameters: Record<string, unknown>
  galleryDataset?: TrainingGalleryDataset
  sample?: TrainingSampleSettings
}

const PARAMETER_HISTORY_KEY = 'danbooru.training.parameter-history.v1'
const historyLabel = ref('')
const selectedHistoryId = ref('')
const parameterHistory = ref<TrainingParameterHistory[]>(readParameterHistory())

const adapter = computed(() => adapters.value.find((item) => item.id === adapterId.value) ?? adapters.value[0])
const adapterFamilies = computed(() => {
  const seen = new Set<string>()
  return adapters.value.filter((item) => {
    const family = item.family || item.id.split('-')[0]
    if (seen.has(family)) return false
    seen.add(family)
    return true
  })
})
const selectedFamily = ref('sdxl')
const familyMethods = computed(() => adapters.value.filter((item) => (item.family || item.id.split('-')[0]) === selectedFamily.value))
const usesImageDataset = computed(() => adapter.value?.training_type !== 'leco')
const supportsSamples = computed(() => !['leco', 'textual_inversion'].includes(adapter.value?.training_type ?? 'lora'))
const modelPathLabel = computed(() => `${adapter.value?.family_label || '底模'} 路径`)
const outputArtifactLabel = computed(() => adapter.value?.training_type === 'textual_inversion' ? 'Embedding 输出' : adapter.value?.training_type === 'leco' ? 'LECO 输出' : `${adapter.value?.training_type_label || 'LoRA'} 输出`)
const adapterDescription = computed(() => `${adapter.value?.family_label || '模型'} · ${adapter.value?.training_type_label || '训练方式'}；仅显示与当前上游入口兼容的参数。`)
const activeProfile = computed(() => profiles.value.find((item) => item.id === runtimeProfileId.value) ?? profiles.value[0])
const runtimeActionLabel = computed(() => activeProfile.value?.managed ? '安装运行时' : '同步训练源码')
const runtimeStatusLabel = computed(() => {
  const profile = activeProfile.value
  if (!profile) return '加载中'
  if (profile.installed) return profile.managed ? '环境已检测到' : '外部环境已连接'
  if (profile.installing) return profile.managed ? '正在后台安装' : '正在同步训练源码'
  return profile.managed ? '环境待安装' : '等待同步锁定训练源码'
})
const groups = computed(() => adapter.value?.groups ?? [])
const selectedGpuSummary = computed(() => {
  if (!selectedGpuIds.value.length) return '自动选择空闲 GPU'
  return selectedGpuIds.value.map((gpuId) => {
    const gpu = gpus.value.find((item) => item.id === gpuId)
    return gpu ? `GPU ${gpu.id} · ${gpu.name}` : `GPU ${gpuId}`
  }).join('，')
})
const trainingTasks = computed(() => tasks.sortedTasks().filter((task) => task.kind === 'training'))
const selectedTrainingTask = computed(() => trainingTasks.value.find((task) => task.id === selectedTrainingTaskId.value) ?? trainingTasks.value[0])
const selectedQueueEntry = computed(() => queueEntries.value.find((entry) => entry.task_id === selectedTrainingTask.value?.id))
const selectedQueueExternalProcesses = computed(() => (selectedQueueEntry.value?.gpu_ids ?? [])
  .flatMap((gpuId) => gpus.value.find((gpu) => gpu.id === gpuId)?.external_processes ?? [])
  .sort((left, right) => right.memory_used_mib - left.memory_used_mib))
const galleryDataset = computed<TrainingGalleryDataset | null>(() => {
  if (!usesImageDataset.value || datasetMode.value !== 'gallery' || !galleryRootId.value) return null
  return {
    root_id: galleryRootId.value,
    relative_directory: galleryDirectory.value,
    repeats: Math.max(1, Math.min(10_000, Math.floor(Number(galleryRepeat.value) || 1))),
    caption_extension: galleryCaptionExtension.value.trim() || '.txt',
  }
})
const galleryDatasets = computed<TrainingGalleryDataset[]>(() => {
  const primary = galleryDataset.value
  if (!primary) return []
  const normalizedAuto = galleryAugmentationDatasets.value
    .filter((dataset) => dataset.enabled && dataset.root_id && dataset.relative_directory.trim())
    .map((dataset) => ({
      ...dataset,
      repeats: Math.max(1, Math.min(10_000, Math.floor(Number(dataset.repeats) || 1))),
      caption_extension: dataset.caption_extension?.trim() || '.txt',
    }))
  const knownDirectories = new Set([primary.relative_directory, ...normalizedAuto.map((dataset) => dataset.relative_directory)])
  return [primary, ...normalizedAuto, ...galleryAdditionalDatasets.value
    .filter((dataset) => !knownDirectories.has(dataset.relative_directory))
    .filter((dataset) => dataset.root_id && dataset.relative_directory.trim())
    .map((dataset) => ({
      ...dataset,
      repeats: Math.max(1, Math.min(10_000, Math.floor(Number(dataset.repeats) || 1))),
      caption_extension: dataset.caption_extension?.trim() || '.txt',
    }))]
})
const enabledAugmentationCount = computed(() => galleryAugmentationDatasets.value.filter((dataset) => dataset.enabled).length)
const sampleSettings = computed<TrainingSampleSettings | null>(() => {  if (!supportsSamples.value || !sampleEnabled.value) return null
  return {
    enabled: true,
    prompt_source: samplePromptSource.value,
    prompt: samplePrompt.value,
    negative_prompt: sampleNegativePrompt.value,
    dataset_caption_count: Math.max(1, Math.min(32, Math.floor(Number(sampleCaptionCount.value) || 1))),
    steps: Math.max(1, Math.min(1000, Math.floor(Number(sampleSteps.value) || 30))),
    width: Math.max(64, Math.min(4096, Math.floor(Number(sampleWidth.value) || 1024))),
    height: Math.max(64, Math.min(4096, Math.floor(Number(sampleHeight.value) || 1024))),
    every_n_epochs: Math.max(1, Math.min(100000, Math.floor(Number(sampleEveryNEpochs.value) || 1))),
  }
})
const sampleOutputDirectory = computed(() => {
  const outputDir = String(values.value.output_dir ?? '').trim()
  if (!outputDir) return `${outputArtifactLabel.value}文件夹 / samples`
  return `${outputDir.replace(/[\\/]$/, '')}/samples`
})
const fieldsByGroup = computed(() => {
  const result = new Map<string, Map<string, TrainingField[]>>()
  const normalized = query.value.trim().toLowerCase()
  for (const field of adapter.value?.fields ?? []) {
    if (!shouldShowField(field)) continue
    if (normalized && !`${field.label} ${field.key} ${field.help} ${field.description}`.toLowerCase().includes(normalized)) continue
    const subgroups = result.get(field.group) ?? new Map<string, TrainingField[]>()
    const fields = subgroups.get(field.subgroup) ?? []
    fields.push(field)
    subgroups.set(field.subgroup, fields)
    result.set(field.group, subgroups)
  }
  return result
})
const subgroupLabel = (groupId: string, subgroupId: string): string =>
  groups.value.find((group) => group.id === groupId)?.subgroups.find((subgroup) => subgroup.id === subgroupId)?.label ?? subgroupId
const subgroupHint = (groupId: string, subgroupId: string): string =>
  subgroupId === 'upstream' ? '由当前 lora-scripts parser 自动导出，含义随上游版本变化。' : ''
const groupFieldCount = (groupId: string): number =>
  [...(fieldsByGroup.value.get(groupId)?.values() ?? [])].reduce((sum, fields) => sum + fields.length, 0)
const fieldsBySubgroup = (groupId: string): Array<[string, TrainingField[]]> =>
  [...(fieldsByGroup.value.get(groupId) ?? new Map<string, TrainingField[]>()).entries()]
const activeOptimizerName = computed(() => String(values.value.optimizer_type ?? '').trim().toLowerCase())
const activeOptimizerTuning = computed(() => optimizerTuningCatalog[activeOptimizerName.value] ?? [])

function initialValues(current?: TrainingAdapter, previous: Record<string, unknown> = {}): Record<string, unknown> {
  return Object.fromEntries((current?.fields ?? []).map((field) => {
    // A one-choice network module is the semantic lock used by LoHa/LoKr.
    // It must replace a previous standard-LoRA module instead of carrying an
    // incompatible value across the training-method transition.
    const isLockedModule = field.key === 'network_module' && field.choices.length === 1
    return [field.key, !isLockedModule && field.key in previous ? previous[field.key] : field.default]
  }))
}

function normalizeValue(field: TrainingField): unknown {
  const value = values.value[field.key]
  if (field.kind === 'number') return value === '' || value == null ? null : Number(value)
  if (field.kind === 'json') {
    if (typeof value !== 'string') return value ?? {}
    const source = value.trim()
    if (!source) return {}
    try {
      return JSON.parse(source)
    } catch {
      return value
    }
  }
  if (field.kind === 'list') {
    if (Array.isArray(value)) return value
    return String(value ?? '').split('\n').map((item) => item.trim()).filter(Boolean)
  }
  return value
}

function parameters(): Record<string, unknown> {
  const result = Object.fromEntries((adapter.value?.fields ?? []).map((field) => [field.key, normalizeValue(field)]))
  if ('optimizer_args' in result) result.optimizer_args = optimizerArgs()
  // Sampling is managed by the explicit sample panel below.  A raw prompt
  // path from an imported TOML must never silently enable generation.
  for (const key of ['sample_prompts', 'sample_every_n_epochs', 'sample_every_n_steps', 'sample_at_first']) delete result[key]
  return result
}

function shouldShowField(field: TrainingField): boolean {
  if (['pretrained_model_name_or_path', 'train_data_dir', 'dataset_config', 'output_dir', 'output_name', 'prompts_file', 'sample_prompts', 'sample_sampler', 'sample_every_n_epochs', 'sample_every_n_steps', 'sample_at_first', 'optimizer_args'].includes(field.key)) return false
  return true
}

function optimizerTuningValue(field: OptimizerTuningField): number | boolean | string {
  return optimizerTuningValues.value[activeOptimizerName.value]?.[field.key] ?? field.default
}

function setOptimizerTuningValue(field: OptimizerTuningField, event: Event): void {
  const target = event.target as HTMLInputElement
  const value: number | boolean | string = field.kind === 'boolean'
    ? target.checked
    : field.kind === 'number'
      ? (target.value === '' ? field.default : Number(target.value))
      : target.value
  optimizerTuningValues.value = {
    ...optimizerTuningValues.value,
    [activeOptimizerName.value]: {
      ...optimizerTuningValues.value[activeOptimizerName.value],
      [field.key]: value,
    },
  }
}

function optimizerLiteral(field: OptimizerTuningField): string {
  const value = optimizerTuningValue(field)
  if (field.kind === 'boolean') return value ? 'True' : 'False'
  if (field.kind === 'number') return Number(value).toString()
  return String(value).trim() || String(field.default)
}

function rawOptimizerArgs(): string[] {
  const raw = values.value.optimizer_args
  return Array.isArray(raw)
    ? raw.map(String).map((item) => item.trim()).filter(Boolean)
    : String(raw ?? '').split('\n').map((item) => item.trim()).filter(Boolean)
}

function optimizerArgs(): string[] {
  const tuning = activeOptimizerTuning.value
  const known = new Set(tuning.map((field) => field.key))
  const generated = tuning.map((field) => `${field.key}=${optimizerLiteral(field)}`)
  const extra = rawOptimizerArgs().filter((entry) => !known.has(entry.split('=', 1)[0]?.trim()))
  return [...generated, ...extra]
}

function hydrateOptimizerTuning(): void {
  const tuning = activeOptimizerTuning.value
  if (!tuning.length) return
  const known = new Set(tuning.map((field) => field.key))
  const parsed: Record<string, number | boolean | string> = {}
  const extra: string[] = []
  for (const entry of rawOptimizerArgs()) {
    const separator = entry.indexOf('=')
    const key = separator < 0 ? '' : entry.slice(0, separator).trim()
    const source = separator < 0 ? '' : entry.slice(separator + 1).trim()
    const field = tuning.find((item) => item.key === key)
    if (!field) {
      extra.push(entry)
      continue
    }
    if (field.kind === 'boolean') parsed[key] = /^true$/i.test(source)
    else if (field.kind === 'number') parsed[key] = Number(source)
    else parsed[key] = source
  }
  if (Object.keys(parsed).length) {
    optimizerTuningValues.value = {
      ...optimizerTuningValues.value,
      [activeOptimizerName.value]: { ...optimizerTuningValues.value[activeOptimizerName.value], ...parsed },
    }
  }
  if (extra.length !== rawOptimizerArgs().length) values.value.optimizer_args = extra
}

function readParameterHistory(): TrainingParameterHistory[] {
  try {
    const raw = window.localStorage.getItem(PARAMETER_HISTORY_KEY)
    const records: unknown = raw ? JSON.parse(raw) : []
    if (!Array.isArray(records)) return []
    return records.filter((record): record is TrainingParameterHistory =>
      typeof record === 'object' && record !== null
      && typeof record.id === 'string'
      && typeof record.label === 'string'
      && typeof record.savedAt === 'number'
      && typeof record.adapterId === 'string'
      && typeof record.runtimeProfileId === 'string'
      && Array.isArray(record.gpuIds)
      && typeof record.parameters === 'object' && record.parameters !== null,
    )
  } catch {
    return []
  }
}

function writeParameterHistory(records: TrainingParameterHistory[]): void {
  parameterHistory.value = records
  try {
    window.localStorage.setItem(PARAMETER_HISTORY_KEY, JSON.stringify(records))
  } catch {
    // The training form remains usable when the browser denies local storage.
  }
}

function historyParameters(): Record<string, unknown> {
  const result = parameters()
  for (const field of adapter.value?.fields ?? []) {
    if (field.kind === 'secret') delete result[field.key]
  }
  return result
}

function saveParameterHistory(silent = false): void {
  if (!adapter.value || !activeProfile.value) return
  const savedAt = Date.now()
  const generatedLabel = `${adapter.value.label} · ${new Date(savedAt).toLocaleString('zh-CN')}`
  const record: TrainingParameterHistory = {
    id: globalThis.crypto?.randomUUID?.() ?? `training-${savedAt}`,
    label: historyLabel.value.trim() || generatedLabel,
    savedAt,
    adapterId: adapter.value.id,
    runtimeProfileId: activeProfile.value.id,
    gpuIds: [...selectedGpuIds.value],
    parameters: historyParameters(),
    galleryDataset: galleryDataset.value ?? undefined,
    sample: sampleSettings.value ?? undefined,
  }
  writeParameterHistory([record, ...parameterHistory.value].slice(0, 30))
  selectedHistoryId.value = record.id
  historyLabel.value = ''
  if (!silent) toast.success('参数记录已保存', '可从“历史参数”下拉列表随时加载。')
}

async function loadParameterHistory(): Promise<void> {
  const record = parameterHistory.value.find((item) => item.id === selectedHistoryId.value)
  if (!record) return
  adapterId.value = record.adapterId
  runtimeProfileId.value = record.runtimeProfileId
  await nextTick()
  values.value = { ...initialValues(adapters.value.find((item) => item.id === record.adapterId)), ...record.parameters }
  const availableIds = new Set(gpus.value.map((gpu) => gpu.id))
  selectedGpuIds.value = record.gpuIds.filter((gpuId) => availableIds.has(gpuId))
  if (record.galleryDataset) applyGalleryDataset(record.galleryDataset)
  else datasetMode.value = 'path'
  applySampleSettings(record.sample ?? null)
  const unavailableCount = record.gpuIds.length - selectedGpuIds.value.length
  toast.success('已加载参数记录', unavailableCount ? `${unavailableCount} 张历史 GPU 当前不可用，已改为自动或其余选择。` : '表单已恢复为保存时的配置。')
}

function applyGalleryDataset(dataset: TrainingGalleryDataset): void {
  datasetMode.value = 'gallery'
  galleryRootId.value = dataset.root_id
  galleryDirectory.value = dataset.relative_directory
  galleryRepeat.value = dataset.repeats
  galleryCaptionExtension.value = dataset.caption_extension || '.txt'
  galleryAugmentationDatasets.value = []
}

function applyGalleryDatasets(datasets: TrainingGalleryDataset[]): void {
  const [primary, ...additional] = datasets
  if (primary) applyGalleryDataset(primary)
  galleryAdditionalDatasets.value = additional.map((dataset) => ({ ...dataset }))
  galleryAugmentationDatasets.value = []
}

function addGallerySubset(): void {
  galleryAdditionalDatasets.value.push({
    root_id: galleryRootId.value || mediaRoots.value[0]?.id || '',
    relative_directory: '',
    repeats: 1,
    caption_extension: galleryCaptionExtension.value.trim() || '.txt',
  })
}

function removeGallerySubset(index: number): void {
  galleryAdditionalDatasets.value.splice(index, 1)
}

function applySampleSettings(settings: TrainingSampleSettings | null): void {
  sampleEnabled.value = settings?.enabled === true
  samplePromptSource.value = settings?.prompt_source ?? 'manual'
  samplePrompt.value = settings?.prompt ?? ''
  sampleNegativePrompt.value = settings?.negative_prompt ?? ''
  sampleCaptionCount.value = settings?.dataset_caption_count ?? 4
  sampleSteps.value = settings?.steps ?? 30
  sampleWidth.value = settings?.width ?? 1024
  sampleHeight.value = settings?.height ?? 1024
  sampleEveryNEpochs.value = settings?.every_n_epochs ?? 1
}

async function refreshGalleryDirectories(): Promise<void> {
  const previousDirectory = galleryDirectory.value
  galleryDirectories.value = []
  galleryPreview.value = null
  galleryAugmentationDatasets.value = []
  if (!galleryRootId.value) {
    galleryDirectory.value = ''
    return
  }
  try {
    const result = await getMediaDirectories(galleryRootId.value)
    galleryDirectories.value = result.directories
    if (previousDirectory && !result.directories.includes(previousDirectory)) galleryDirectory.value = ''
  } catch (reason: unknown) {
    galleryDirectory.value = ''
    toast.error('无法读取图库目录', reason instanceof Error ? reason.message : '请检查媒体根是否可访问')
  }
}

async function discoverGalleryAugmentations(): Promise<void> {
  if (datasetMode.value !== 'gallery' || !galleryRootId.value) {
    galleryAugmentationDatasets.value = []
    return
  }
  galleryAugmentationLoading.value = true
  try {
    const discovery = await discoverTrainingGalleryAugmentations(galleryRootId.value, galleryDirectory.value)
    galleryAugmentationDatasets.value = discovery.subsets.map((subset: TrainingAugmentationSubset) => ({
      root_id: galleryRootId.value,
      relative_directory: subset.relative_directory,
      repeats: subset.repeats,
      caption_extension: subset.caption_extension,
      taskId: subset.task_id,
      label: subset.label,
      imageCount: subset.image_count,
      enabled: true,
    }))
  } catch (reason: unknown) {
    galleryAugmentationDatasets.value = []
    toast.error('无法识别增广子集', reason instanceof Error ? reason.message : '请检查增广任务是否已完成二次打标')
  } finally {
    galleryAugmentationLoading.value = false
  }
}

async function refreshGalleryPreview(): Promise<void> {
  const dataset = galleryDataset.value
  galleryPreview.value = null
  if (!dataset) return
  galleryPreviewLoading.value = true
  try {
    galleryPreview.value = await previewTrainingGalleryDataset(dataset)
  } catch (reason: unknown) {
    toast.error('图库训练集预检失败', reason instanceof Error ? reason.message : '请检查所选目录')
  } finally {
    galleryPreviewLoading.value = false
  }
}

async function loadPresetRequestedFromTools(id: string): Promise<void> {
  try {
    const preset = (await getTrainingPresets()).find((item) => item.id === id)
    if (!preset) {
      toast.warning('所选训练预设不存在或已被移除')
      return
    }
    adapterId.value = preset.training.adapter_id
    runtimeProfileId.value = preset.training.runtime_profile_id
    await nextTick()
    values.value = { ...initialValues(adapters.value.find((item) => item.id === preset.training.adapter_id)), ...preset.training.parameters }
    selectedGpuIds.value = preset.training.gpu_ids.filter((gpuId) => gpus.value.some((gpu) => gpu.id === gpuId))
    if (preset.training.gallery_datasets?.length) applyGalleryDatasets(preset.training.gallery_datasets)
    else if (preset.training.gallery_dataset) applyGalleryDataset(preset.training.gallery_dataset)
    else datasetMode.value = 'path'
    applySampleSettings(preset.training.sample ?? null)
    toast.success('已从工具页加载训练预设', `${preset.name} · 第 ${preset.version_count} 个保存版本。`)
  } catch (reason: unknown) {
    toast.error('无法加载训练预设', reason instanceof Error ? reason.message : '请返回工具页重试')
  }
}

async function openPathBrowser(kind: 'model' | 'dataset' | 'output', target: string): Promise<void> {
  pathBrowserLoading.value = true
  try {
    const currentValue = String(values.value[target] ?? '')
    const data = await browseTrainingPath(kind, currentValue)
    pathBrowser.value = { kind, target, data }
  } catch (reason: unknown) {
    toast.warning('无法浏览该路径', reason instanceof Error ? reason.message : '请先填入一个可访问的已有文件夹或文件路径')
  } finally {
    pathBrowserLoading.value = false
  }
}

async function navigatePathBrowser(path: string): Promise<void> {
  const browser = pathBrowser.value
  if (!browser) return
  pathBrowserLoading.value = true
  try {
    browser.data = await browseTrainingPath(browser.kind, path)
  } catch (reason: unknown) {
    toast.error('无法打开文件夹', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    pathBrowserLoading.value = false
  }
}

function choosePathBrowserEntry(path: string): void {
  const browser = pathBrowser.value
  if (!browser) return
  values.value[browser.target] = path
  pathBrowser.value = null
}

async function refreshQueue(): Promise<void> {
  try {
    queueEntries.value = (await getTrainingQueue()).entries
  } catch {
    // The task stream remains authoritative when a one-off queue refresh fails.
  }
}

function deleteParameterHistory(): void {
  if (!selectedHistoryId.value) return
  writeParameterHistory(parameterHistory.value.filter((item) => item.id !== selectedHistoryId.value))
  selectedHistoryId.value = ''
  toast.success('参数记录已删除')
}

function toggleGroup(id: string): void {
  const next = new Set(selectedGroups.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  selectedGroups.value = next
}

function switchTrainingTab(tab: 'setup' | 'monitor' | 'svd'): void {
  activeTab.value = tab
  if (tab === 'monitor') monitorVisited.value = true
  const url = new URL(window.location.href)
  url.searchParams.set('tab', tab)
  window.history.replaceState(window.history.state, '', url)
  syncTrainingPolling()
}

function isTerminalTrainingTask(task: TaskSummary | undefined): boolean {
  return task?.kind === 'training' && ['completed', 'failed', 'cancelled'].includes(task.status)
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  const unit = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024)))
  return `${(value / 1024 ** unit).toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`
}

async function openTrainingCleanup(task: TaskSummary): Promise<void> {
  if (!isTerminalTrainingTask(task) || cleanupPreviewLoading.value || cleanupDeleting.value) return
  cleanupTask.value = task
  cleanupPreview.value = null
  cleanupPreviewLoading.value = true
  try {
    cleanupPreview.value = await getTrainingCleanupPreview(task.id)
  } catch (reason: unknown) {
    cleanupTask.value = null
    toast.error('无法生成清理预览', reason instanceof Error ? reason.message : '该运行可能仍在活动中')
  } finally {
    cleanupPreviewLoading.value = false
  }
}

function closeTrainingCleanup(): void {
  if (cleanupDeleting.value) return
  cleanupTask.value = null
  cleanupPreview.value = null
}

async function confirmTrainingCleanup(): Promise<void> {
  const task = cleanupTask.value
  if (!task || !cleanupPreview.value || cleanupDeleting.value) return
  cleanupDeleting.value = true
  try {
    const result = await deleteTrainingTask(task.id)
    const remaining = trainingTasks.value.filter((entry) => entry.id !== task.id)
    tasks.tasks = tasks.tasks.filter((entry) => entry.id !== task.id)
    if (selectedTrainingTaskId.value === task.id) selectedTrainingTaskId.value = remaining[0]?.id ?? ''
    cleanupTask.value = null
    cleanupPreview.value = null
    await tasks.loadSnapshot()
    toast.success('训练运行已永久删除', `已清理 ${result.deleted.reduce((sum, item) => sum + item.file_count, 0)} 个可验证归属文件${result.retained.length ? `；保留 ${result.retained.length} 项无法安全归属的内容` : ''}`)
  } catch (reason: unknown) {
    toast.error('无法删除训练运行', reason instanceof Error ? reason.message : '请确认运行已经结束')
  } finally {
    cleanupDeleting.value = false
  }
}

function inputId(field: TrainingField): string { return `training-${field.key}` }

function fieldTextValue(field: TrainingField): string {
  const value = values.value[field.key]
  if (field.kind === 'json') {
    if (typeof value === 'string') return value
    try {
      return JSON.stringify(value ?? {}, null, 2)
    } catch {
      return ''
    }
  }
  if (Array.isArray(value)) return value.join('\n')
  return value === undefined || value === null ? '' : String(value)
}

function setTextValue(key: string, event: Event): void {
  values.value[key] = (event.target as HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement).value
}

function setBooleanValue(key: string, event: Event): void {
  values.value[key] = (event.target as HTMLInputElement).checked
}

function toggleGpu(gpuId: string, event: Event): void {
  const enabled = (event.target as HTMLInputElement).checked
  selectedGpuIds.value = enabled
    ? [...new Set([...selectedGpuIds.value, gpuId])]
    : selectedGpuIds.value.filter((id) => id !== gpuId)
}

async function refreshGpuTelemetry(): Promise<void> {
  try {
    gpus.value = await getTrainingGpus()
  } catch {
    // The last successful telemetry remains visible, and transient nvidia-smi failures do not interrupt form editing.
  }
}

async function refreshRuntimeProfiles(): Promise<void> {
  try {
    const wasInstalled = activeProfile.value?.installed ?? false
    const profileList = await getTrainingRuntimeProfiles()
    profiles.value = profileList
    if (!profileList.some((item) => item.id === runtimeProfileId.value)) runtimeProfileId.value = profileList[0]?.id ?? ''
    if (!wasInstalled && profileList.find((item) => item.id === runtimeProfileId.value)?.installed) {
      adapters.value = await getTrainingAdapters()
    }
  } catch {
    // Keep the last known runtime state visible while a background installer is running.
  }
}

async function installActiveRuntime(): Promise<void> {
  if (!activeProfile.value || runtimeActionLoading.value) return
  runtimeActionLoading.value = true
  try {
    await installTrainingRuntime(activeProfile.value.id)
    await refreshRuntimeProfiles()
    toast.success(
      activeProfile.value.managed ? '训练运行时安装任务已加入队列' : '训练源码同步任务已加入队列',
      '可前往任务中心查看安装进度与结果。',
    )
  } catch (reason: unknown) {
    toast.error('无法启动训练运行时安装', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    runtimeActionLoading.value = false
  }
}

async function inspectActiveRuntime(): Promise<void> {
  if (!activeProfile.value || runtimeActionLoading.value) return
  runtimeActionLoading.value = true
  try {
    runtimeDiagnostics.value = await getTrainingRuntimeDiagnostics(activeProfile.value.id)
  } catch (reason: unknown) {
    toast.error('训练运行时诊断失败', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    runtimeActionLoading.value = false
  }
}

async function refreshPreview(): Promise<void> {
  if (!adapter.value) return
  try {
    const toml = (await previewTraining(adapter.value.id, parameters())).toml
    const sample = sampleSettings.value
    preview.value = sample
      ? `${toml}\n# 样图由训练工作台在任务启动时生成\n# source = ${sample.prompt_source}\n# steps = ${sample.steps}, resolution = ${sample.width}x${sample.height}\n# output = ${sampleOutputDirectory.value}`
      : toml
  } catch (reason: unknown) {
    preview.value = reason instanceof Error ? `# ${reason.message}` : '# 无法生成 TOML'
  }
}

async function runPreflight(): Promise<void> {
  if (!adapter.value || !activeProfile.value) return
  preflightLoading.value = true
  try {
    preflight.value = await preflightTraining({
      adapter_id: adapter.value.id,
      runtime_profile_id: activeProfile.value.id,
      parameters: parameters(),
      gpu_ids: selectedGpuIds.value,
      gallery_datasets: galleryDatasets.value,
      sample: sampleSettings.value,
    })
  } catch (reason: unknown) {
    toast.error('训练预检失败', reason instanceof Error ? reason.message : '请检查当前字段')
  } finally {
    preflightLoading.value = false
  }
}

async function submit(): Promise<void> {
  if (!adapter.value || !activeProfile.value) return
  if (!activeProfile.value.installed) {
    toast.warning('训练运行时尚未就绪', activeProfile.value.managed
      ? `请先把内置运行时安装到 ${activeProfile.value.runtime_root}`
      : '请先同步锁定训练源码，再使用所选 Conda 环境进行诊断。')
    return
  }
  submitting.value = true
  try {
    const gpu_ids = selectedGpuIds.value
    if (usesImageDataset.value && datasetMode.value === 'gallery' && !galleryPreview.value) {
      await refreshGalleryPreview()
    }
    if (usesImageDataset.value && datasetMode.value === 'gallery' && !galleryPreview.value) return
    await createTrainingTask({
      type: 'training', root_id: '__training__',
      training: {
        adapter_id: adapter.value.id,
        runtime_profile_id: activeProfile.value.id,
        parameters: parameters(),
        gpu_ids,
        gallery_datasets: galleryDatasets.value,
        sample: sampleSettings.value,
      },
    })
    saveParameterHistory(true)
    await tasks.loadSnapshot()
    toast.success('训练已加入队列', gpu_ids.length ? `已预约 GPU ${gpu_ids.join(', ')}` : '将自动选择空闲 GPU')
  } catch (reason: unknown) {
    toast.error('无法创建训练任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    submitting.value = false
  }
}

watch(adapter, (next) => {
  if (next) selectedFamily.value = next.family || next.id.split('-')[0]
  values.value = initialValues(next, values.value)
}, { immediate: true })
watch(selectedFamily, (family) => {
  if (adapter.value?.family === family) return
  const first = adapters.value.find((item) => (item.family || item.id.split('-')[0]) === family)
  if (first) adapterId.value = first.id
})
watch([activeOptimizerName, () => values.value.optimizer_args], hydrateOptimizerTuning, { deep: true })
watch(values, () => { void refreshPreview() }, { deep: true })
watch([sampleEnabled, samplePromptSource, samplePrompt, sampleNegativePrompt, sampleCaptionCount, sampleSteps, sampleWidth, sampleHeight, sampleEveryNEpochs], () => { void refreshPreview() })
watch(galleryRootId, () => { void refreshGalleryDirectories() })
watch([galleryDirectory, galleryRepeat, galleryCaptionExtension, datasetMode], () => {
  if (datasetMode.value === 'gallery') void refreshGalleryPreview()
})
watch([galleryRootId, galleryDirectory, datasetMode], () => {
  void discoverGalleryAugmentations()
})
watch(trainingTasks, (next) => {
  if (!next.some((task) => task.id === selectedTrainingTaskId.value)) selectedTrainingTaskId.value = next[0]?.id ?? ''
}, { immediate: true })

function stopTrainingPolling(): void {
  if (gpuRefreshTimer) clearInterval(gpuRefreshTimer)
  if (runtimeRefreshTimer) clearInterval(runtimeRefreshTimer)
  if (queueRefreshTimer) clearInterval(queueRefreshTimer)
  gpuRefreshTimer = undefined
  runtimeRefreshTimer = undefined
  queueRefreshTimer = undefined
}

function syncTrainingPolling(): void {
  stopTrainingPolling()
  if (document.visibilityState === 'hidden') return

  if (activeTab.value === 'setup') {
    void refreshGpuTelemetry()
    gpuRefreshTimer = setInterval(() => { void refreshGpuTelemetry() }, 5_000)
    if (profiles.value.some((profile) => profile.installing)) {
      runtimeRefreshTimer = setInterval(() => { void refreshRuntimeProfiles() }, 5_000)
    }
    return
  }

  if (activeTab.value !== 'monitor') return
  void refreshQueue()
  queueRefreshTimer = setInterval(() => { void refreshQueue() }, 5_000)
  if (selectedTrainingTask.value && !isTerminalTrainingTask(selectedTrainingTask.value)) {
    void refreshGpuTelemetry()
    gpuRefreshTimer = setInterval(() => { void refreshGpuTelemetry() }, 5_000)
  }
}

function onVisibilityChange(): void {
  syncTrainingPolling()
}

watch(activeTab, syncTrainingPolling)
watch(() => profiles.value.some((profile) => profile.installing), syncTrainingPolling)

onMounted(async () => {
  try {
    const [adapterList, profileList, gpuList, roots] = await Promise.all([getTrainingAdapters(), getTrainingRuntimeProfiles(), getTrainingGpus(), getMediaRoots()])
    adapters.value = adapterList
    profiles.value = profileList
    gpus.value = gpuList
    mediaRoots.value = roots
    if (!adapterList.some((item) => item.id === adapterId.value)) adapterId.value = adapterList[0]?.id ?? ''
    if (!profileList.some((item) => item.id === runtimeProfileId.value)) runtimeProfileId.value = profileList[0]?.id ?? ''
    if (requestedPresetId) await loadPresetRequestedFromTools(requestedPresetId)
  } catch (reason: unknown) {
    toast.error('无法加载训练工作台', reason instanceof Error ? reason.message : '请检查本地服务')
  } finally {
    loading.value = false
  }
  document.addEventListener('visibilitychange', onVisibilityChange)
  syncTrainingPolling()
})

onBeforeUnmount(() => {
  document.removeEventListener('visibilitychange', onVisibilityChange)
  stopTrainingPolling()
})
</script>

<template>
  <section class="page-shell training-page">
    <header class="page-header training-header">
      <div>
        <p class="eyebrow">LOCAL TRAINING</p>
        <h1 class="page-title">训练工作台</h1>
        <p class="page-description">创建可复现的 LoRA 训练配置，跟踪指标与队列状态，并以奇异值分解评估已导出权重的 rank 余量。</p>
      </div>
      <div v-if="activeTab === 'setup'" class="training-actions">
        <button class="button" type="button" :disabled="preflightLoading" @click="runPreflight"><Gauge :size="16" /> {{ preflightLoading ? '预检中…' : '训练预检' }}</button>
        <button class="button" type="button" @click="refreshPreview"><FileCode2 :size="16" /> 预览 TOML</button>
        <button class="button button-primary" type="button" :disabled="submitting || loading || !adapter" @click="submit"><CirclePlay :size="16" /> {{ submitting ? '正在加入…' : '加入训练队列' }}</button>
      </div>
    </header>

    <nav class="training-subnav" aria-label="训练子界面">
      <button type="button" :class="{ active: activeTab === 'setup' }" @click="switchTrainingTab('setup')"><SlidersHorizontal :size="16" /> 配置训练</button>
      <button type="button" :class="{ active: activeTab === 'monitor' }" @click="switchTrainingTab('monitor')"><Activity :size="16" /> 训练监控<span v-if="trainingTasks.length">{{ trainingTasks.length }}</span></button>
      <button type="button" :class="{ active: activeTab === 'svd' }" @click="switchTrainingTab('svd')"><BarChart3 :size="16" /> LoRA SVD 分析</button>
    </nav>

    <section v-show="activeTab === 'setup'" class="training-setup-page">
      <div class="training-status-grid">
        <article class="surface training-status-card training-runtime-card"><Cpu :size="19" /><div><span>训练运行时</span><strong>{{ activeProfile?.label ?? '加载中' }}</strong><small :class="activeProfile?.installed ? 'ok-text' : 'warning-text'">{{ runtimeStatusLabel }}</small><small v-if="activeProfile && !activeProfile.managed" class="training-runtime-external">外部环境 · {{ activeProfile.python_path }}</small><div class="training-runtime-actions"><button v-if="!activeProfile?.installed" class="button button-small button-primary" type="button" :disabled="runtimeActionLoading || !activeProfile || activeProfile.installing" @click="installActiveRuntime">{{ activeProfile?.installing ? '处理中…' : runtimeActionLabel }}</button><button class="button button-small" type="button" :disabled="runtimeActionLoading || !activeProfile" @click="inspectActiveRuntime">运行诊断</button></div></div></article>
        <article class="surface training-status-card"><Gauge :size="19" /><div><span>调度策略</span><strong>GPU 独占队列</strong><small>同一卡不会并发启动两个训练</small></div></article>
        <article class="surface training-status-card"><Activity :size="19" /><div><span>实验记录</span><strong>原生实时监控</strong><small>无需运行 TensorBoard 服务</small></div></article>
      </div>

      <section v-if="runtimeDiagnostics" class="surface training-runtime-diagnostics" aria-label="训练运行时诊断">
        <div><strong>{{ runtimeDiagnostics.profile.label }} 诊断</strong><small>{{ runtimeDiagnostics.profile.runtime_root }}</small></div>
        <span v-for="check in runtimeDiagnostics.checks" :key="check.id" :class="check.ok ? 'ok' : 'failed'"><b>{{ check.ok ? '通过' : '待处理' }}</b>{{ check.id }} · {{ check.detail }}</span>
      </section>
      <section v-if="preflight" class="surface training-runtime-diagnostics" aria-label="训练预检结果">
        <header><strong>{{ preflight.ready ? '预检通过' : '预检发现阻塞项' }}</strong><span>有效步数 {{ preflight.effective_steps.toLocaleString() }} · 预计显存 {{ preflight.estimated_vram_mib.toLocaleString() }} MiB</span></header>
        <ul>
          <li v-for="check in preflight.checks" :key="check.id" :class="check.ok ? 'ok-text' : 'warning-text'">
            <strong>{{ check.ok ? '通过' : '需处理' }}</strong><span>{{ check.message }}<small v-if="check.recovery">{{ check.recovery }}</small></span>
          </li>
        </ul>
        <div v-if="preflight.suggestions.length" class="training-preflight-suggestions">
          <strong>参数建议（不会自动应用）</strong>
          <p v-for="suggestion in preflight.suggestions" :key="suggestion.field"><code>{{ suggestion.field }} = {{ suggestion.value }}</code> · {{ suggestion.reason }}</p>
        </div>
      </section>

      <div class="surface training-toolbar">
        <label>模型家族 <InfoTooltip title="模型家族" description="决定可用训练入口、底模组件和参数集合。界面仅提供 kohya_ss v26.0.0 已确认支持的组合。" /><select v-model="selectedFamily"><option v-for="item in adapterFamilies" :key="item.family" :value="item.family">{{ item.family_label }}</option></select></label>
        <label>训练方式 <InfoTooltip title="训练方式" description="不同训练方式会切换上游脚本和专属参数；无法与当前模型家族组合的方式不会显示为可提交选项。" /><select v-model="adapterId"><option v-for="item in familyMethods" :key="item.id" :value="item.id">{{ item.training_type_label }}</option></select></label>
        <label>运行时 <InfoTooltip title="训练运行时" description="训练将在所选 Python/Conda 环境中启动。外部环境只会在你点击“同步训练源码”后安装或更新依赖。" /><select v-model="runtimeProfileId"><option v-for="item in profiles" :key="item.id" :value="item.id">{{ item.label }}{{ item.managed ? ' · 内置' : ' · 已发现' }}</option></select><small v-if="activeProfile && !activeProfile.managed">点击同步后才会对这个外部环境安装 kohya_ss 依赖。</small></label>
        <div class="training-gpu-field">
          <span>目标 GPU</span>
          <details v-if="gpus.length" class="training-gpu-picker">
            <summary :title="selectedGpuSummary"><span>{{ selectedGpuSummary }}</span><ChevronDown :size="16" /></summary>
            <div class="training-gpu-menu">
              <label v-for="gpu in gpus" :key="gpu.id" class="training-gpu-option">
                <input type="checkbox" :checked="selectedGpuIds.includes(gpu.id)" @change="toggleGpu(gpu.id, $event)" />
                <span><strong>GPU {{ gpu.id }} · {{ gpu.name }}</strong><small>{{ gpu.memory_used_mib }} / {{ gpu.memory_total_mib }} MiB · {{ gpu.utilization_percent }}% 利用率</small></span>
              </label>
            </div>
          </details>
          <div v-else class="training-gpu-unavailable">未检测到可用 GPU</div>
          <small id="training-gpu-help" class="training-toolbar-hint">{{ gpus.length ? '可选择任意多张 GPU；不选择将自动安排空闲 GPU。' : '未检测到可用 GPU，提交后由调度器自动探测。' }}</small>
        </div>
        <label class="training-search">查找参数<input v-model="query" placeholder="名称、CLI 参数或说明" /></label>
      </div>

      <div class="surface training-history-bar">
        <div class="training-history-copy"><strong>参数历史</strong><small>保存后可一键恢复表单、运行时和目标 GPU；密钥字段不会写入浏览器记录。</small></div>
        <label>记录名称<input v-model="historyLabel" placeholder="例如：角色 LoRA · 1024px" /></label>
        <label>历史参数<select v-model="selectedHistoryId"><option value="">选择一条已保存记录</option><option v-for="record in parameterHistory" :key="record.id" :value="record.id">{{ record.label }}</option></select></label>
        <div class="training-history-actions"><button class="button" type="button" :disabled="!selectedHistoryId" @click="loadParameterHistory">加载记录</button><button class="button" type="button" :disabled="!selectedHistoryId" @click="deleteParameterHistory">删除</button><button class="button button-primary" type="button" :disabled="!adapter || !activeProfile" @click="() => saveParameterHistory()">保存参数记录</button></div>
      </div>

      <section class="surface training-data-workflow" aria-label="训练数据、模型与输出路径">
        <header>
          <div><Layers3 :size="18" /><span><strong>训练输入与输出 <InfoTooltip title="输入与输出" :description="adapterDescription" /></strong><small>将数据、底模和成品目录集中设置；任务运行时会保存实际使用的不可变路径快照。</small></span></div>
          <span class="training-flow-note"><FolderOpen :size="14" />数据只引用原目录，不复制或改写图库素材</span>
        </header>
        <div class="training-path-grid">
          <section v-if="usesImageDataset" class="training-path-card training-dataset-card">
            <div class="training-card-heading"><strong>1 · 数据集 <InfoTooltip title="训练数据集" description="LoRA、DreamBooth、Fine-tuning 与 Textual Inversion 使用图像和 Caption。图库模式只引用原始文件，不复制或改写素材。" /></strong><span class="training-mode-switch"><button type="button" :class="{ active: datasetMode === 'gallery' }" @click="datasetMode = 'gallery'">从图库引用</button><button type="button" :class="{ active: datasetMode === 'path' }" @click="datasetMode = 'path'">本地路径</button></span></div>
            <template v-if="datasetMode === 'gallery'">
              <label>图库根<select v-model="galleryRootId"><option value="">选择一个已配置图库</option><option v-for="root in mediaRoots" :key="root.id" :value="root.id">{{ root.name }} · {{ root.media_count }} 项</option></select></label>
              <label>图库目录<select v-model="galleryDirectory" :disabled="!galleryRootId"><option value="">根目录（全部图片）</option><option v-for="directory in galleryDirectories" :key="directory" :value="directory">{{ directory }}</option></select></label>
              <div class="training-inline-fields"><label>Repeat<input v-model.number="galleryRepeat" aria-label="图库 Repeat" type="number" min="1" max="10000" /></label><label>Caption 扩展名<input v-model="galleryCaptionExtension" placeholder=".txt" /></label></div>
              <div class="training-gallery-preview" :class="{ loading: galleryPreviewLoading }"><template v-if="galleryPreview"><strong>{{ galleryPreview.image_count }} 张图片 × {{ galleryPreview.repeats }} repeat</strong><span>预计 {{ galleryPreview.effective_image_count }} 个图片轮次 · {{ galleryPreview.caption_count }} 个 Caption</span><small :title="galleryPreview.image_dir">{{ galleryPreview.image_dir }}</small></template><template v-else><strong>{{ galleryPreviewLoading ? '正在预检图库…' : '选择图库和目录后预检' }}</strong><span>预检会在提交前检查图片、Caption 与引用路径。</span></template></div>
              <div class="training-gallery-subsets">
                <header class="training-subsets-header">
                  <span class="training-subsets-title"><strong>绑定子集</strong><span v-if="galleryDatasets.length" class="training-subsets-badge">{{ galleryDatasets.length }} 个</span></span>
                  <span class="training-subsets-actions">
                    <button class="button button-small" type="button" :disabled="galleryAugmentationLoading" @click="discoverGalleryAugmentations"><RefreshCw :size="13" /> {{ galleryAugmentationLoading ? '识别中…' : '刷新增广子集' }}</button>
                    <button class="button button-small" type="button" @click="addGallerySubset"><Plus :size="13" /> 添加子集</button>
                  </span>
                </header>
                <p class="training-subsets-note">原图直接引用当前目录；系统自动识别已重新打标的 <code>.augmentation</code> 子集，每种裁剪策略可单独设置 repeat，并可按需启用或停用。</p>
                <div v-if="galleryAugmentationDatasets.length" class="training-subsets-zone">
                  <div class="training-subsets-zone-heading">
                    <span><Sparkles :size="13" /><strong>自动识别增广子集</strong><small>点击开关启用或停用</small></span>
                    <span class="training-subsets-zone-meta">{{ enabledAugmentationCount }} / {{ galleryAugmentationDatasets.length }} 已启用</span>
                  </div>
                  <div class="training-subsets-list">
                    <div v-for="subset in galleryAugmentationDatasets" :key="`auto-${subset.taskId}-${subset.relative_directory}`" class="training-subset-row" :class="{ 'is-disabled': !subset.enabled }">
                      <label class="training-subset-switch" :title="subset.enabled ? '点击停用该子集' : '点击启用该子集'">
                        <input v-model="subset.enabled" type="checkbox" :aria-label="`${subset.enabled ? '停用' : '启用'} ${subset.label}`" />
                        <span class="training-subset-switch-track" aria-hidden="true"><i /></span>
                      </label>
                      <span class="training-subset-info">
                        <strong>{{ subset.label }}<em>{{ subset.imageCount }} 张</em></strong>
                        <code :title="subset.relative_directory">{{ subset.relative_directory }}</code>
                      </span>
                      <label class="training-subset-repeats">
                        <span>Repeat</span>
                        <input v-model.number="subset.repeats" type="number" min="1" max="10000" :disabled="!subset.enabled" :aria-label="`${subset.label} Repeat`" />
                      </label>
                    </div>
                  </div>
                </div>
                <div v-else-if="!galleryAugmentationLoading" class="training-subsets-empty"><Sparkles :size="14" /><span>尚未识别增广子集：增广任务完成二次打标后，点击“刷新增广子集”自动发现。</span></div>
                <div v-if="galleryAdditionalDatasets.length" class="training-subsets-zone">
                  <div class="training-subsets-zone-heading">
                    <span><Layers3 :size="13" /><strong>手动绑定子集</strong><small>从其他图库目录额外绑定</small></span>
                  </div>
                  <div class="training-subsets-list">
                    <div v-for="(subset, index) in galleryAdditionalDatasets" :key="`${index}-${subset.root_id}-${subset.relative_directory}`" class="training-subset-row training-subset-row-manual">
                      <select v-model="subset.root_id"><option value="">选择图库</option><option v-for="root in mediaRoots" :key="root.id" :value="root.id">{{ root.name }}</option></select>
                      <input v-model="subset.relative_directory" placeholder="例如 characters/alice/.augmentation/任务/ready/train/portrait/images" />
                      <label class="training-subset-repeats">
                        <span>Repeat</span>
                        <input v-model.number="subset.repeats" type="number" min="1" max="10000" aria-label="子集 Repeat" />
                      </label>
                      <input v-model="subset.caption_extension" placeholder=".txt" aria-label="子集 Caption 扩展名" />
                      <button class="button button-small button-quiet" type="button" aria-label="移除子集" @click="removeGallerySubset(index)"><Trash2 :size="13" /> 移除</button>
                    </div>
                  </div>
                </div>
              </div>
            </template>
            <template v-else>
              <label>训练集目录<span class="training-path-input"><input v-model="values.train_data_dir" placeholder="D:\\datasets\\my-character" /><button class="button button-small" type="button" :disabled="pathBrowserLoading" @click="openPathBrowser('dataset', 'train_data_dir')">浏览</button></span></label>
              <small>可用 lora-scripts 的 DreamBooth 命名目录，或在高级参数填入 dataset_config。</small>
            </template>
          </section>

          <section v-else class="training-path-card training-dataset-card">
            <div class="training-card-heading"><strong>1 · LECO 概念编辑 <InfoTooltip title="LECO Prompt" description="LECO 不使用常规图片数据集；它使用 Prompt TOML 定义要擦除或编辑的概念与方向。" /></strong></div>
            <label>概念编辑 Prompt 文件<span class="training-path-input"><input v-model="values.prompts_file" placeholder="D:\\training\\leco-prompts.toml" /><button class="button button-small" type="button" :disabled="pathBrowserLoading" @click="openPathBrowser('model', 'prompts_file')">浏览</button></span></label>
            <label>待编辑网络权重<span class="training-path-input"><input v-model="values.network_weights" placeholder="D:\\models\\existing-adapter.safetensors" /><button class="button button-small" type="button" :disabled="pathBrowserLoading" @click="openPathBrowser('model', 'network_weights')">浏览</button></span></label>
            <small>LECO 会依据 Prompt 文件执行概念擦除或编辑；开始前请确认目标权重与备份策略。</small>
          </section>

          <section class="training-path-card">
            <div class="training-card-heading"><strong>2 · 模型本体 <InfoTooltip title="底模" description="路径由所选模型家族的上游训练器解释；可使用该训练器支持的 Safetensors checkpoint 或模型目录。" /></strong><span>{{ adapter?.family_label || '不绑定固定底模' }}</span></div>
            <label>{{ modelPathLabel }}<span class="training-path-input"><input v-model="values.pretrained_model_name_or_path" placeholder="D:\\models\\base.safetensors" /><button class="button button-small" type="button" :disabled="pathBrowserLoading" @click="openPathBrowser('model', 'pretrained_model_name_or_path')">浏览</button></span></label>
            <label>可选 VAE<input v-model="values.vae" placeholder="留空则使用底模 VAE" /></label>
            <small>支持 Safetensors checkpoint 或 Diffusers 模型目录；路径只会写入本次运行快照。</small>
          </section>

          <section class="training-path-card">
            <div class="training-card-heading"><strong>3 · {{ outputArtifactLabel }} <InfoTooltip title="训练输出" description="权重/Embedding 与 checkpoint 保存到此目录；配置、日志和指标会保存在应用管理的运行快照目录。" /></strong><span>成品与运行记录分离</span></div>
            <label>{{ outputArtifactLabel }}文件夹<span class="training-path-input"><input v-model="values.output_dir" placeholder="D:\\training-output\\adapter" /><button class="button button-small" type="button" :disabled="pathBrowserLoading" @click="openPathBrowser('output', 'output_dir')">浏览</button></span></label>
            <label>输出名称<input v-model="values.output_name" placeholder="character-xl-lora" /></label>
            <small>成品与 checkpoint 保存到此处；配置、日志、指标和图库 dataset TOML 保存到应用的训练运行目录。</small>
          </section>
        </div>
        <section v-if="pathBrowser" class="training-path-browser" aria-label="训练路径浏览器">
          <header><strong>路径浏览 · {{ pathBrowser.kind === 'model' ? '模型文件' : pathBrowser.kind === 'dataset' ? '数据集目录' : '输出目录' }}</strong><span><button v-if="pathBrowser.kind !== 'model'" class="button button-small" type="button" @click="choosePathBrowserEntry(pathBrowser.data.current_path)">使用此文件夹</button><button class="button button-small" type="button" @click="pathBrowser = null">关闭</button></span></header>
          <div class="training-path-crumb"><button v-if="pathBrowser.data.parent_path" type="button" @click="navigatePathBrowser(pathBrowser.data.parent_path)">上级</button><code>{{ pathBrowser.data.current_path }}</code></div>
          <div class="training-path-browser-grid"><button v-for="entry in pathBrowser.data.directories" :key="entry.path" type="button" @click="navigatePathBrowser(entry.path)"><FolderOpen :size="14" />{{ entry.name }}</button><button v-for="entry in pathBrowser.data.files" :key="entry.path" type="button" class="file" @click="choosePathBrowserEntry(entry.path)"><FileCode2 :size="14" />{{ entry.name }}</button></div>
          <small>点击文件夹继续浏览；点击文件填入当前字段。输出和数据集目录可直接手动填写目标路径。</small>
        </section>
      </section>

      <section v-if="supportsSamples" class="surface training-sample-settings" aria-label="训练样图设置">
        <header>
          <div><Activity :size="18" /><span><strong>训练样图 <InfoTooltip title="训练样图" description="按固定 Prompt 定期生成对照图片，用于观察训练变化；它不等同于验证集指标。" /></strong><small>启用后按指定 Epoch 使用当前适配器生成对照样图；不启用时不会传递样图 Prompt 或生成图片。</small></span></div>
          <label class="training-sample-toggle"><input v-model="sampleEnabled" type="checkbox" aria-label="生成训练样图" /><span><b>{{ sampleEnabled ? '已开启' : '已关闭' }}</b>生成训练样图</span></label>
        </header>
        <div v-if="sampleEnabled" class="training-sample-body">
          <div class="training-sample-source" role="radiogroup" aria-label="样图 Prompt 来源">
            <label :class="{ active: samplePromptSource === 'manual' }"><input v-model="samplePromptSource" type="radio" value="manual" />手动填写 Prompt</label>
            <label :class="{ active: samplePromptSource === 'dataset_captions' }"><input v-model="samplePromptSource" type="radio" value="dataset_captions" />从数据集抽取 Caption TXT</label>
          </div>
          <div class="training-sample-grid">
            <label v-if="samplePromptSource === 'manual'" class="wide">样图正面 Prompt<textarea v-model="samplePrompt" rows="3" placeholder="每行一个样图正面 Prompt" /></label>
            <label v-else>抽取 Caption 数量<input v-model.number="sampleCaptionCount" type="number" min="1" max="32" aria-label="抽取 Caption 数量" /><small>训练任务开始时只读抽取前 {{ sampleCaptionCount || 1 }} 条有效 TXT；不会改写训练数据。</small></label>
            <label class="wide">样图负面 Prompt<textarea v-model="sampleNegativePrompt" rows="3" placeholder="例如：low quality, blurry, bad anatomy" /></label>
            <label>样图采样步数<input v-model.number="sampleSteps" type="number" min="1" max="1000" /></label>
            <label>样图采样器<select v-model="values.sample_sampler" aria-label="样图采样器"><option v-for="sampler in sampleSamplerChoices" :key="sampler" :value="sampler">{{ sampler }}</option></select></label>
            <label>样图宽度<input v-model.number="sampleWidth" type="number" min="64" max="4096" step="8" /></label>
            <label>样图高度<input v-model.number="sampleHeight" type="number" min="64" max="4096" step="8" /></label>
            <label>每 N Epoch 生成<input v-model.number="sampleEveryNEpochs" type="number" min="1" max="100000" /></label>
          </div>
          <div class="training-sample-output"><FolderOpen :size="15" /><span><strong>样图输出位置</strong><code>{{ sampleOutputDirectory }}</code></span><small>该目录会同时保存本次固定使用的 <code>sample_prompts.txt</code>；便于复现每张样图。</small></div>
        </div>
      </section>

      <div v-if="loading" class="surface training-empty">正在读取模型适配器与运行时能力…</div>
      <div v-else-if="!adapter" class="surface training-empty">当前没有可用训练适配器。</div>
      <div v-else class="training-config-layout">
        <div class="training-form">
          <article v-for="group in groups" :key="group.id" class="surface training-group" :class="{ 'is-open': selectedGroups.has(group.id) }">
            <button class="group-heading" type="button" @click="toggleGroup(group.id)">
              <span><strong>{{ group.label }}</strong><small>{{ group.description }}</small></span><ChevronDown :size="18" />
            </button>
            <div v-if="selectedGroups.has(group.id) && (groupFieldCount(group.id) > 0 || group.id === 'optimizer')" class="field-grid">
              <template v-for="[subgroupId, subgroupFields] in fieldsBySubgroup(group.id)" :key="`${group.id}-${subgroupId}`">
                <h4 v-if="subgroupId" class="training-subgroup-heading" :class="{ 'is-collapsed': false }">
                  <span>{{ subgroupLabel(group.id, subgroupId) }}<small>{{ subgroupFields.length }} 项</small></span>
                  <small v-if="subgroupHint(group.id, subgroupId)" class="training-subgroup-hint">{{ subgroupHint(group.id, subgroupId) }}</small>
                </h4>
                <label v-for="field in subgroupFields" :key="field.key" class="training-field" :class="{ wide: field.kind === 'list' || field.kind === 'json' || field.kind === 'secret' }">
                  <span>{{ field.label }} <InfoTooltip :title="field.label" :description="field.description || field.help" :when-to-adjust="field.when_to_adjust" /> <code>--{{ field.key }}</code><em v-if="field.required">必填</em></span>
                  <select v-if="field.kind === 'select'" :value="fieldTextValue(field)" @change="setTextValue(field.key, $event)"><option v-for="choice in field.choices" :key="choice" :value="choice">{{ choice }}</option></select>
                  <input v-else-if="field.kind === 'boolean'" :checked="values[field.key] === true" type="checkbox" class="checkbox" @change="setBooleanValue(field.key, $event)" />
                  <textarea v-else-if="field.kind === 'list'" :value="fieldTextValue(field)" :id="inputId(field)" rows="3" placeholder="每行一个值" @input="setTextValue(field.key, $event)" />
                  <textarea v-else-if="field.kind === 'json'" :value="fieldTextValue(field)" :id="inputId(field)" rows="8" placeholder="JSON 对象，例如：{ &quot;new_upstream_option&quot;: true }" @input="setTextValue(field.key, $event)" />
                  <input v-else :value="fieldTextValue(field)" :id="inputId(field)" :type="field.kind === 'secret' ? 'password' : field.kind === 'number' ? 'number' : 'text'" @input="setTextValue(field.key, $event)" />
                  <small class="training-field-hint">悬停标题旁的问号查看参数说明</small>
                </label>
              </template>
              <section v-if="group.id === 'optimizer'" class="training-optimizer-tuning wide" aria-label="优化器专属超参数">
                <header><div><strong>{{ values.optimizer_type || '当前优化器' }} 专属超参数</strong><small>只显示所选优化器会实际接收的参数；切换优化器后不会把无关参数传给训练器。</small></div><span>{{ activeOptimizerTuning.length }} 项</span></header>
                <div v-if="activeOptimizerTuning.length" class="training-optimizer-tuning-grid">
                  <label v-for="field in activeOptimizerTuning" :key="field.key" class="training-field">
                    <span>{{ field.label }} <InfoTooltip :title="field.label" :description="field.help" /> <code>optimizer_args · {{ field.key }}</code></span>
                    <input v-if="field.kind === 'boolean'" :checked="optimizerTuningValue(field) === true" type="checkbox" class="checkbox" @change="setOptimizerTuningValue(field, $event)" />
                    <input v-else :value="optimizerTuningValue(field)" :type="field.kind === 'number' ? 'number' : 'text'" :step="field.kind === 'number' ? 'any' : undefined" @input="setOptimizerTuningValue(field, $event)" />
                    <small class="training-field-hint">悬停标题旁的问号查看参数说明</small>
                  </label>
                </div>
                <p v-else>该优化器没有内置结构化参数描述；可在下面按 lora-scripts 的 <code>key=Python字面量</code> 格式补充。</p>
                <details class="training-optimizer-raw"><summary>额外原始优化器参数（上游扩展）</summary><textarea :value="rawOptimizerArgs().join('\n')" rows="3" placeholder="例如：foreach=True" @input="setTextValue('optimizer_args', $event)" /><small>只填写当前优化器专属界面未覆盖的参数。结构化项会优先写入并自动使用 Python 的 True/False、数字或元组格式。</small></details>
              </section>
            </div>
          </article>
        </div>
        <aside class="training-side-panel">
          <div class="surface training-preview-card">
            <div class="monitor-heading"><FileCode2 :size="17" /><strong>配置快照</strong></div>
            <p>提交前由后端校验。任务启动后会保存不可变的配置副本。</p>
            <pre>{{ preview || '# 点击“预览 TOML”生成配置' }}</pre>
          </div>
        </aside>
      </div>

      <div class="training-gpu-float" role="region" aria-label="GPU 实时状态悬浮面板">
        <button type="button" class="training-gpu-float-toggle" :aria-expanded="gpuFloatOpen" @click="gpuFloatOpen = !gpuFloatOpen">
          <Cpu :size="16" />
          <span>{{ gpuFloatOpen ? '收起 GPU 状态' : `GPU 状态${gpus.length ? ` · ${gpus.length} 张` : ''}` }}</span>
        </button>
        <div v-show="gpuFloatOpen" class="training-gpu-float-panel">
          <GpuLiveStatus :gpus="gpus" :selected-gpu-ids="selectedGpuIds" />
        </div>
      </div>
    </section>

    <section v-if="monitorVisited" v-show="activeTab === 'monitor'" class="training-monitor-page">
      <div class="surface training-monitor-intro">
        <div><p class="eyebrow">EXPERIMENTS</p><h2 class="section-title">训练监控</h2><p class="section-copy">选择一次训练运行，查看多曲线指标、资源、日志、样图与产物；队列状态在此实时同步。</p></div>
        <div class="training-monitor-intro-actions">
          <button v-if="isTerminalTrainingTask(selectedTrainingTask)" class="button button-danger" type="button" @click="openTrainingCleanup(selectedTrainingTask!)"><Trash2 :size="15" /> 删除当前运行</button>
          <a href="/tasks" class="button">打开任务中心</a>
        </div>
      </div>
      <section v-if="selectedQueueEntry" class="surface training-queue-summary" aria-label="所选训练队列状态">
        <strong>{{ selectedQueueEntry.status === 'queued' ? `队列 #${selectedQueueEntry.queue_position ?? '—'}` : 'GPU 已分配' }}</strong>
        <span>{{ selectedQueueEntry.assigned_gpu_ids.length ? `运行在 GPU ${selectedQueueEntry.assigned_gpu_ids.join(', ')}` : `目标 GPU ${selectedQueueEntry.gpu_ids.join(', ') || '自动选择'}` }}</span>
        <small v-if="selectedQueueEntry.wait_reason">{{ selectedQueueEntry.wait_reason }}<template v-if="selectedQueueEntry.estimated_wait_seconds != null"> · 预计 {{ selectedQueueEntry.estimated_wait_seconds }} 秒</template></small>
        <small v-if="selectedQueueExternalProcesses.length" class="training-external-gpu-warning">外部 GPU 进程：{{ selectedQueueExternalProcesses.slice(0, 2).map(process => `${process.process_name} (${process.memory_used_mib} MiB)`).join('，') }}</small>
      </section>
      <div v-if="!trainingTasks.length" class="surface training-empty">尚无训练记录。创建训练任务后，实时指标会出现在这里。</div>
      <div v-else class="training-monitor-layout">
        <aside class="surface training-run-list" aria-label="训练运行列表">
          <button v-for="task in trainingTasks" :key="task.id" type="button" :class="{ active: selectedTrainingTask?.id === task.id }" @click="selectedTrainingTaskId = task.id">
            <strong>{{ task.training?.output_name || task.training?.adapter_id || 'LoRA 训练' }}</strong>
            <span>{{ task.status === 'running' ? '进行中' : task.status === 'queued' ? '等待中' : task.status }}</span>
            <small>{{ task.training?.gpu_ids?.length ? `GPU ${task.training.gpu_ids.join(', ')}` : '自动选择 GPU' }}</small>
          </button>
        </aside>
        <TrainingMonitor v-if="selectedTrainingTask" :task-id="selectedTrainingTask.id" :active="['queued', 'running', 'pausing', 'paused', 'cancelling'].includes(selectedTrainingTask.status)" :visible="activeTab === 'monitor'" />
      </div>
      <div class="training-gpu-float training-gpu-float-left" role="region" aria-label="GPU 实时状态悬浮面板">
        <button type="button" class="training-gpu-float-toggle" :aria-expanded="gpuFloatOpenMonitor" @click="gpuFloatOpenMonitor = !gpuFloatOpenMonitor">
          <Cpu :size="16" />
          <span>{{ gpuFloatOpenMonitor ? '收起 GPU 状态' : `GPU 状态${gpus.length ? ` · ${gpus.length} 张` : ''}` }}</span>
        </button>
        <div v-show="gpuFloatOpenMonitor" class="training-gpu-float-panel">
          <GpuLiveStatus :gpus="gpus" :selected-gpu-ids="selectedGpuIds" />
        </div>
      </div>
    </section>
    <LoraSvdAnalysis v-if="activeTab === 'svd'" :profiles="profiles" :training-tasks="trainingTasks" />
    <ConfirmDialog
      :open="Boolean(cleanupTask)"
      title="永久删除训练运行"
      confirm-label="永久删除"
      destructive
      :busy="cleanupPreviewLoading || cleanupDeleting"
      @cancel="closeTrainingCleanup"
      @confirm="confirmTrainingCleanup"
    >
      <p>此操作不可恢复。将删除该运行可验证归属的监控数据、控制台日志、配置快照、任务记录及任务独占输出目录中的产物。</p>
      <p v-if="cleanupPreviewLoading">正在核对可安全删除的文件…</p>
      <template v-else-if="cleanupPreview">
        <ul class="training-cleanup-list" aria-label="训练运行清理预览">
          <li v-for="item in cleanupPreview.deletable" :key="`delete-${item.kind}-${item.path}`"><strong>删除 · {{ item.kind }}</strong><span>{{ item.path }}</span><small>{{ item.file_count }} 个文件 · {{ formatBytes(item.bytes) }}</small></li>
        </ul>
        <p v-if="cleanupPreview.retained.length" class="training-cleanup-retained">为保护共享目录，以下 {{ cleanupPreview.retained.length }} 项不会删除：{{ cleanupPreview.retained.map(item => item.reason || item.path).join('；') }}</p>
      </template>
    </ConfirmDialog>
  </section>
</template>
