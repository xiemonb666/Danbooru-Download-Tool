<script setup lang="ts">
import { computed, onMounted, reactive, ref, type Component } from 'vue'
import { ArchiveRestore, Bot, FileCode2, FileCheck2, FileImage, Images, ScanSearch, Tags, Trash2, WandSparkles } from '@lucide/vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import {
  createTask,
  downloadClTaggerModel,
  exportTrainingPreset,
  getClTaggerHealth,
  getClTaggerModel,
  getMediaDirectories,
  getMediaRoots,
  getQuarantine,
  getTrainingAdapters,
  getTrainingPresets,
  getTrainingRuntimeProfiles,
  getVisionCropRuntimeHealth,
  importTrainingPreset,
  installVisionCropRuntime,
  purgeQuarantine,
  restoreQuarantine,
  updateTrainingPresetToml,
  type ClTaggerHealth,
  type MediaRoot,
  type QuarantineEntry,
  type RootTaskRequest,
  type TrainingAdapter,
  type TrainingPreset,
  type TrainingRuntimeProfile,
  type VisionCropRuntimeHealth,
} from '../api'
import { useTasksStore } from '../stores/tasks'
import { useConfigStore } from '../stores/config'
import { useToastStore } from '../stores/toast'

interface ToolDefinition {
  type: Exclude<RootTaskRequest['type'], 'delete_selected'>
  title: string
  description: string
  action: string
  icon: Component
  preflight: boolean
}

const definitions: ToolDefinition[] = [
  { type: 'integrity_scan', title: '完整性检查', description: '支持的图片格式会进行完整解码；其他媒体容器仅执行基础文件检查。', action: '配置任务', icon: FileCheck2, preflight: true },
  { type: 'exact_dedup', title: '精确去重', description: '按文件大小与 SHA-256 识别完全相同的文件，先生成预检清单。', action: '配置任务', icon: ScanSearch, preflight: true },
  { type: 'near_dedup', title: '相似图片检查', description: '使用感知哈希寻找近似图片，结果始终需要人工确认；视频不会参与。', action: '配置任务', icon: Images, preflight: true },
  { type: 'resize', title: '安全缩放', description: '最长边和 JPEG 质量可调；原文件先进入隔离区，再原子发布结果。', action: '配置任务', icon: FileImage, preflight: true },
  { type: 'heic_convert', title: 'HEIC 转换', description: '可按相对目录批量选择 HEIC/HEIF，先预检再转换为 JPEG。', action: '配置任务', icon: FileImage, preflight: true },
  { type: 'delete_by_tag', title: '按标签隔离', description: '规范化标签 token 后精确匹配，将匹配项移入可恢复隔离区。', action: '配置任务', icon: Trash2, preflight: true },
  { type: 'tag_pipeline', title: '标签处理', description: '恢复分类排序、过滤和 artist:/@ 前缀规则，预检后原子替换。', action: '配置任务', icon: Tags, preflight: true },
  { type: 'vllm_tag', title: '视觉模型打标', description: '可按相对目录批量打标；语言、提示词、联网校验和并发由设置控制。', action: '配置任务', icon: Bot, preflight: false },
  { type: 'dataset_augmentation', title: '数据集增广', description: '原图留在源目录，生成无损派生图；按 family 防泄漏切分，并将元数据独立保存。', action: '配置任务', icon: WandSparkles, preflight: false },
]

const roots = ref<MediaRoot[]>([])
const rootId = ref('')
const quarantine = ref<QuarantineEntry[]>([])
const quarantinePage = ref(1)
const loading = ref(false)
const creating = ref(false)
const selectedTool = ref<ToolDefinition | null>(null)
const confirmingPurge = ref(false)
const tag = ref('')
const phashDistance = ref(8)
const scope = ref<'root' | 'directory'>('root')
const relativeDirectory = ref('')
const directories = ref<string[]>([])
const directoriesLoading = ref(false)
const directoryLoadError = ref(false)
const manualDirectory = ref(false)
const resizeMaxSize = ref(1216)
const resizeQuality = ref(100)
const datasetMinMegapixels = ref(1.8)
const datasetMinLongSide = ref(1536)
const datasetMinShortSide = ref(768)
const datasetHorizontalFlip = ref(false)
const datasetTrainPercent = ref(90)
const datasetValidationPercent = ref(5)
const datasetTestPercent = ref(5)
const datasetSmartCropEnabled = ref(true)
const datasetSmartCropRuntimeProfileId = ref('conda:lora')
const datasetSmartCropGpuId = ref('0')
const datasetSmartCropPortrait = ref(true)
const datasetSmartCropUpperBody = ref(true)
const datasetSmartCropCowboyShot = ref(true)
const datasetSmartCropFullBody = ref(true)
const datasetSmartCropLowerBody = ref(true)
const datasetSmartCropFeet = ref(true)
const datasetSmartCropRequireBothFeet = ref(false)
const datasetRetagMode = ref<'none' | 'cl_tagger' | 'vllm'>('none')
const datasetPreserveArtistCharacterTags = ref(true)
const retagParamsOpen = ref(false)
const vllmParams = reactive({
  baseUrl: 'http://127.0.0.1:8000/v1',
  model: 'unsloth/Qwen3.8-27B-NVFP4',
  systemPrompt: 'You are an image description assistant. Describe the visible content in concise, objective, natural English and return only the description inside exactly one <tag>...</tag> block. Do not add explanations or unrelated content.',
  language: 'en' as 'zh' | 'en',
  maxLength: 400,
  concurrency: 8,
})
const clTaggerParams = reactive({
  modelPath: '',
  generalThreshold: 0.35,
  characterThreshold: 0.6,
  copyrightThreshold: 0.6,
  qualityThreshold: 0.35,
  maxTags: 60,
})
const vllmTagMode = ref<'vllm' | 'cl_tagger'>('vllm')
const clTaggerHealth = ref<ClTaggerHealth | null>(null)
const clTaggerBusy = ref(false)
const vllmPromptPresets = {
  zh: '你是图像描述助手。请使用简洁、客观、自然的中文描述画面中可见的内容，并且只在一个 <tag>...</tag> 块中返回描述；不要添加解释或无关内容。',
  en: 'You are an image description assistant. Describe the visible content in concise, objective, natural English and return only the description inside exactly one <tag>...</tag> block. Do not add explanations or unrelated content.',
} as const

function applyVllmPromptPreset(): void {
  vllmParams.systemPrompt = vllmPromptPresets[vllmParams.language]
}

const config = useConfigStore()
const taggerDefaultsSeeded = ref(false)

function seedTaggerDefaults(): void {
  if (taggerDefaultsSeeded.value) return
  taggerDefaultsSeeded.value = true
  if (config.config.vllm_base_url) vllmParams.baseUrl = config.config.vllm_base_url
  if (config.config.vllm_model) vllmParams.model = config.config.vllm_model
}

function openRetagParams(): void {
  seedTaggerDefaults()
  retagParamsOpen.value = true
  startClTaggerPolling()
}
const clTaggerPolling = ref<ReturnType<typeof setInterval> | null>(null)

function startClTaggerPolling(): void {
  if (clTaggerPolling.value) return
  void refreshClTaggerHealth()
  clTaggerPolling.value = setInterval(() => void refreshClTaggerHealth(), 2000)
}

function stopClTaggerPolling(): void {
  if (clTaggerPolling.value) {
    clearInterval(clTaggerPolling.value)
    clTaggerPolling.value = null
  }
}

async function refreshClTaggerHealth(): Promise<void> {
  try {
    clTaggerHealth.value = await getClTaggerHealth()
  } catch {
    clTaggerHealth.value = null
  }
}

async function refreshClTaggerModel(): Promise<void> {
  clTaggerBusy.value = true
  try {
    const result = await getClTaggerModel()
    await refreshClTaggerHealth()
    if (clTaggerHealth.value?.model_path) clTaggerParams.modelPath = clTaggerHealth.value.model_path
    if (result.model_path) clTaggerParams.modelPath = result.model_path
    toast.success(result.cached ? '已从 HuggingFace 缓存检测到 CL Tagger 模型' : '未在缓存中找到 CL Tagger 模型，可点击下载')
  } catch (reason: unknown) {
    toast.error('无法检测 CL Tagger 模型', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    clTaggerBusy.value = false
  }
}

async function downloadClTaggerModelNow(): Promise<void> {
  clTaggerBusy.value = true
  try {
    const result = await downloadClTaggerModel()
    clTaggerParams.modelPath = result.model_path ?? clTaggerParams.modelPath
    await refreshClTaggerHealth()
    toast.success('CL Tagger 模型下载完成')
  } catch (reason: unknown) {
    toast.error('无法下载 CL Tagger 模型', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    clTaggerBusy.value = false
  }
}
const visionCropHealth = ref<VisionCropRuntimeHealth | null>(null)
const visionCropBusy = ref(false)
const artistPrefix = ref<'artist' | 'at'>('artist')
const trainingAdapters = ref<TrainingAdapter[]>([])
const trainingProfiles = ref<TrainingRuntimeProfile[]>([])
const trainingPresets = ref<TrainingPreset[]>([])
const trainingPresetId = ref('')
const trainingPresetName = ref('')
const trainingPresetAdapterId = ref('sdxl-lora')
const trainingPresetRuntimeProfileId = ref('windows')
const trainingPresetToml = ref('')
const trainingPresetLoading = ref(false)
const tasks = useTasksStore()
const toast = useToastStore()
const boundedSelectionTasks = new Set<RootTaskRequest['type']>(['resize', 'heic_convert', 'tag_pipeline', 'vllm_tag', 'dataset_augmentation'])

const activeRoot = computed(() => roots.value.find((root) => root.id === rootId.value) ?? null)
const selectedTrainingPreset = computed(() => trainingPresets.value.find((preset) => preset.id === trainingPresetId.value))
const QUARANTINE_PAGE_SIZE = 50
const quarantinePageCount = computed(() => Math.max(1, Math.ceil(quarantine.value.length / QUARANTINE_PAGE_SIZE)))
const visibleQuarantine = computed(() => {
  const start = (quarantinePage.value - 1) * QUARANTINE_PAGE_SIZE
  return quarantine.value.slice(start, start + QUARANTINE_PAGE_SIZE)
})

function formatDirectory(path: string): string {
  return path.split('/').join(' / ')
}

async function loadDirectories(): Promise<void> {
  relativeDirectory.value = ''
  directories.value = []
  directoryLoadError.value = false
  manualDirectory.value = false
  if (!rootId.value) return
  directoriesLoading.value = true
  try {
    const result = await getMediaDirectories(rootId.value)
    directories.value = result.directories
  } catch {
    directoryLoadError.value = true
    manualDirectory.value = true
  } finally {
    directoriesLoading.value = false
  }
}

async function changeRoot(): Promise<void> {
  await Promise.all([loadQuarantine(), loadDirectories()])
}

async function loadQuarantine(): Promise<void> {
  if (!rootId.value) {
    quarantine.value = []
    quarantinePage.value = 1
    return
  }
  loading.value = true
  try {
    quarantine.value = await getQuarantine(rootId.value)
    quarantinePage.value = 1
  } catch (reason: unknown) {
    toast.error('无法读取隔离区', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    loading.value = false
  }
}

async function refreshTrainingPresetTools(): Promise<void> {
  try {
    const [adapters, profiles, presets] = await Promise.all([
      getTrainingAdapters(),
      getTrainingRuntimeProfiles(),
      getTrainingPresets(),
    ])
    trainingAdapters.value = adapters
    trainingProfiles.value = profiles
    trainingPresets.value = presets
    if (!adapters.some((adapter) => adapter.id === trainingPresetAdapterId.value)) trainingPresetAdapterId.value = adapters[0]?.id ?? ''
    if (!profiles.some((profile) => profile.id === trainingPresetRuntimeProfileId.value)) trainingPresetRuntimeProfileId.value = profiles[0]?.id ?? ''
  } catch (reason: unknown) {
    toast.error('无法读取训练预设', reason instanceof Error ? reason.message : '请检查本地训练服务')
  }
}

async function selectTrainingPreset(): Promise<void> {
  const preset = selectedTrainingPreset.value
  if (!preset) {
    trainingPresetName.value = ''
    trainingPresetToml.value = ''
    return
  }
  trainingPresetName.value = preset.name
  trainingPresetAdapterId.value = preset.training.adapter_id
  trainingPresetRuntimeProfileId.value = preset.training.runtime_profile_id
  trainingPresetLoading.value = true
  try {
    trainingPresetToml.value = (await exportTrainingPreset(preset.id)).toml
  } catch (reason: unknown) {
    toast.error('无法读取预设 TOML', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    trainingPresetLoading.value = false
  }
}

async function downloadTrainingPresetToml(): Promise<void> {
  const preset = selectedTrainingPreset.value
  if (!preset) return
  trainingPresetLoading.value = true
  try {
    const exported = await exportTrainingPreset(preset.id)
    const blob = new Blob([exported.toml], { type: 'application/toml;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `${exported.name.replace(/[\\/:*?"<>|]/g, '_') || 'lora-preset'}.toml`
    anchor.click()
    URL.revokeObjectURL(url)
    toast.success('已导出 Lora-scripts TOML')
  } catch (reason: unknown) {
    toast.error('无法导出预设', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    trainingPresetLoading.value = false
  }
}

async function saveTrainingPresetToml(): Promise<void> {
  const name = trainingPresetName.value.trim()
  if (!name) {
    toast.warning('请输入预设名称')
    return
  }
  if (!trainingPresetToml.value.trim()) {
    toast.warning('请粘贴或读取 Lora-scripts TOML')
    return
  }
  trainingPresetLoading.value = true
  try {
    const input = {
      name,
      adapter_id: trainingPresetAdapterId.value,
      runtime_profile_id: trainingPresetRuntimeProfileId.value,
      gpu_ids: selectedTrainingPreset.value?.training.gpu_ids ?? [],
      toml: trainingPresetToml.value,
    }
    const preset = selectedTrainingPreset.value
      ? await updateTrainingPresetToml(selectedTrainingPreset.value.id, input)
      : await importTrainingPreset(input)
    trainingPresetId.value = preset.id
    await refreshTrainingPresetTools()
    toast.success(preset.version_count > 1 ? '预设已保存为新版本' : '已导入训练预设', `当前保留 ${preset.version_count} 个版本。`)
  } catch (reason: unknown) {
    toast.error('无法保存训练预设', reason instanceof Error ? reason.message : '请确认 TOML 与所选模型适配器匹配')
  } finally {
    trainingPresetLoading.value = false
  }
}

function openTrainingWithPreset(): void {
  if (!trainingPresetId.value) {
    toast.warning('请先选择一个训练预设')
    return
  }
  window.location.assign(`/training?tab=setup&preset=${encodeURIComponent(trainingPresetId.value)}`)
}

function chooseTool(tool: ToolDefinition): void {
  if (!rootId.value) {
    toast.warning('请选择媒体库')
    return
  }
  if (tool.type === 'vllm_tag') seedTaggerDefaults()
  selectedTool.value = tool
}

async function checkVisionCropRuntime(): Promise<void> {
  visionCropBusy.value = true
  try {
    visionCropHealth.value = await getVisionCropRuntimeHealth(datasetSmartCropRuntimeProfileId.value)
    if (!visionCropHealth.value.ready) toast.warning('智能裁剪运行时未就绪', visionCropHealth.value.message)
  } catch (reason: unknown) {
    visionCropHealth.value = null
    toast.error('无法检查智能裁剪运行时', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    visionCropBusy.value = false
  }
}

async function installVisionCropModels(): Promise<void> {
  visionCropBusy.value = true
  try {
    visionCropHealth.value = await installVisionCropRuntime(datasetSmartCropRuntimeProfileId.value)
    toast.success('已开始安装并预热检测模型', '完成后请点击“检查运行时”确认 GPU 与模型状态。')
  } catch (reason: unknown) {
    toast.error('无法安装智能裁剪模型', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    visionCropBusy.value = false
  }
}

async function createSelectedTask(): Promise<void> {
  if (!selectedTool.value || !rootId.value) return
  if (selectedTool.value.type === 'delete_by_tag' && !tag.value.trim()) {
    toast.warning('请输入精确标签')
    return
  }
  if (scope.value === 'directory' && !relativeDirectory.value.trim()) {
    toast.warning('请输入根目录内的相对目录')
    return
  }
  creating.value = true
  const kind = selectedTool.value.type
  const relative_directory = scope.value === 'directory'
    ? relativeDirectory.value.trim()
    : boundedSelectionTasks.has(kind) ? '.' : undefined
  let request: RootTaskRequest
  if (kind === 'delete_by_tag') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, tag: tag.value.trim(), relative_directory } }
  } else if (kind === 'near_dedup') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, distance: phashDistance.value, relative_directory } }
  } else if (kind === 'exact_dedup' || kind === 'integrity_scan') {
    request = { type: kind, root_id: rootId.value, options: { preflight: true, relative_directory } }
  } else if (kind === 'resize') {
    request = { type: kind, root_id: rootId.value, options: { relative_directory, max_size: resizeMaxSize.value, quality: resizeQuality.value } }
  } else if (kind === 'heic_convert') {
    request = { type: kind, root_id: rootId.value, options: { relative_directory } }
  } else if (kind === 'vllm_tag') {
    request = {
      type: kind,
      root_id: rootId.value,
      options: {
        relative_directory,
        ...(vllmTagMode.value === 'cl_tagger'
          ? {
              mode: 'cl_tagger',
              cl_tagger: {
                model_path: clTaggerParams.modelPath.trim(),
                general_threshold: clTaggerParams.generalThreshold,
                character_threshold: clTaggerParams.characterThreshold,
                copyright_threshold: clTaggerParams.copyrightThreshold,
                quality_threshold: clTaggerParams.qualityThreshold,
                max_tags: clTaggerParams.maxTags,
              },
            }
          : {
              vllm: {
                base_url: vllmParams.baseUrl.trim(),
                model: vllmParams.model.trim(),
                system_prompt: vllmParams.systemPrompt.trim(),
                language: vllmParams.language,
                max_length: vllmParams.maxLength,
                concurrency: vllmParams.concurrency,
              },
            }),
      },
    }
  } else if (kind === 'dataset_augmentation') {
    request = {
      type: kind,
      root_id: rootId.value,
      options: {
        relative_directory,
        min_megapixels: datasetMinMegapixels.value,
        min_long_side: datasetMinLongSide.value,
        min_short_side: datasetMinShortSide.value,
        horizontal_flip: datasetHorizontalFlip.value,
        train_percent: datasetTrainPercent.value,
        validation_percent: datasetValidationPercent.value,
        test_percent: datasetTestPercent.value,
        smart_crop: {
          enabled: datasetSmartCropEnabled.value,
          runtime_profile_id: datasetSmartCropRuntimeProfileId.value.trim(),
          gpu_id: datasetSmartCropGpuId.value.trim(),
          quality_profile: 'anime-quality',
          portrait: datasetSmartCropPortrait.value,
          upper_body: datasetSmartCropUpperBody.value,
          cowboy_shot: datasetSmartCropCowboyShot.value,
          full_body_tight: datasetSmartCropFullBody.value,
          lower_body: datasetSmartCropLowerBody.value,
          feet: datasetSmartCropFeet.value,
          require_both_feet: datasetSmartCropRequireBothFeet.value,
          max_derived_per_family: 6,
        },
        retagging: {
          send_to_vllm: datasetRetagMode.value !== 'none',
          preserve_artist_character_tags: datasetPreserveArtistCharacterTags.value,
          mode: datasetRetagMode.value === 'none' ? undefined : datasetRetagMode.value,
          vllm: {
            base_url: vllmParams.baseUrl.trim(),
            model: vllmParams.model.trim(),
            system_prompt: vllmParams.systemPrompt.trim(),
            language: vllmParams.language,
            max_length: vllmParams.maxLength,
            concurrency: vllmParams.concurrency,
          },
          cl_tagger: {
            model_path: clTaggerParams.modelPath.trim(),
            general_threshold: clTaggerParams.generalThreshold,
            character_threshold: clTaggerParams.characterThreshold,
            copyright_threshold: clTaggerParams.copyrightThreshold,
            quality_threshold: clTaggerParams.qualityThreshold,
            max_tags: clTaggerParams.maxTags,
          },
        },
      },
    }
  } else {
    request = { type: kind, root_id: rootId.value, options: { relative_directory, artist_prefix: artistPrefix.value } }
  }
  try {
    await createTask(request)
    await tasks.loadSnapshot()
    toast.success(selectedTool.value.preflight ? '预检任务已加入队列' : '处理任务已加入队列')
    selectedTool.value = null
  } catch (reason: unknown) {
    toast.error('无法创建任务', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creating.value = false
  }
}

async function restore(entry: QuarantineEntry): Promise<void> {
  try {
    await restoreQuarantine(entry.id)
    quarantine.value = quarantine.value.filter((item) => item.id !== entry.id)
    quarantinePage.value = Math.min(quarantinePage.value, quarantinePageCount.value)
    toast.success('文件已恢复', '发生路径冲突时服务器不会覆盖现有文件。')
  } catch (reason: unknown) {
    toast.error('无法恢复文件', reason instanceof Error ? reason.message : '未知错误')
  }
}

async function purge(): Promise<void> {
  if (!rootId.value) return
  creating.value = true
  try {
    const result = await purgeQuarantine(rootId.value)
    quarantine.value = []
    quarantinePage.value = 1
    confirmingPurge.value = false
    toast.success(`已永久清理 ${result.purged} 项`)
  } catch (reason: unknown) {
    toast.error('无法清理隔离区', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    creating.value = false
  }
}

function formatBytes(value: number): string {
  return value < 1024 * 1024 ? `${Math.ceil(value / 1024)} KB` : `${(value / 1024 / 1024).toFixed(1)} MB`
}

onMounted(async () => {
  try {
    roots.value = await getMediaRoots()
    rootId.value = roots.value[0]?.id ?? ''
    await Promise.all([loadQuarantine(), loadDirectories()])
  } catch (reason: unknown) {
    toast.error('工具初始化失败', reason instanceof Error ? reason.message : '未知错误')
  }
  void refreshTrainingPresetTools()
})
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Processing tools</p>
        <h1 class="page-title">处理与隔离</h1>
        <p class="page-description">所有处理均进入统一任务系统。同一媒体库同时只执行一个写任务，危险操作先预检再确认。</p>
      </div>
      <select v-model="rootId" class="select" style="width: min(260px, 100%)" aria-label="媒体库" @change="changeRoot">
        <option value="" disabled>选择媒体库</option>
        <option v-for="root in roots" :key="root.id" :value="root.id">{{ root.name }}</option>
      </select>
    </header>

    <div v-if="!roots.length" class="empty-state">
      <div><Images :size="30" /><strong>先添加一个下载位置</strong><p>为了防止误处理系统文件，工具只能处理已添加媒体库中的内容。</p><RouterLink to="/settings" class="button button-primary" style="margin-top: 16px">前往设置</RouterLink></div>
    </div>

    <template v-else>
      <div class="notice warning" style="margin-bottom: 18px">预检任务不会删除文件。每个工具都可处理整个 {{ activeRoot?.name }}，或递归处理根目录内的相对目录；也可继续在图库中精确勾选媒体。数据集增广始终新建输出目录，不会改写输入图片。</div>
      <section class="tool-grid" aria-label="处理工具">
        <article v-for="tool in definitions" :key="tool.type" class="tool-card">
          <div class="tool-heading"><span><component :is="tool.icon" :size="18" /></span><h2>{{ tool.title }}</h2></div>
          <p>{{ tool.description }}</p>
          <button type="button" class="button" @click="chooseTool(tool)">{{ tool.action }}</button>
        </article>
      </section>

      <section class="surface" style="margin-top: 24px">
        <header class="surface-header">
          <div><h2 class="section-title">隔离区</h2><p class="section-copy">保留原相对路径，恢复时从不覆盖冲突文件。</p></div>
          <button type="button" class="button button-small button-danger" :disabled="!quarantine.length" @click="confirmingPurge = true"><Trash2 :size="14" /> 手动清空</button>
        </header>
        <div class="surface-body">
          <div v-if="loading" class="section-copy">正在读取隔离区</div>
          <div v-else-if="quarantine.length" class="stack">
            <div v-for="entry in visibleQuarantine" :key="entry.id" class="root-card">
              <div class="root-card-header"><strong>{{ entry.original_relative_path }}</strong><button type="button" class="button button-small" @click="restore(entry)"><ArchiveRestore :size="14" /> 恢复</button></div>
              <div class="path-row"><span>{{ formatBytes(entry.size_bytes) }}</span><code>{{ entry.reason }}</code></div>
            </div>
            <nav v-if="quarantinePageCount > 1" class="quarantine-pagination" aria-label="隔离区分页"><span>第 {{ quarantinePage }} / {{ quarantinePageCount }} 页 · 共 {{ quarantine.length }} 项</span><div><button type="button" class="button button-small" :disabled="quarantinePage <= 1" @click="quarantinePage -= 1">上一页</button><button type="button" class="button button-small" :disabled="quarantinePage >= quarantinePageCount" @click="quarantinePage += 1">下一页</button></div></nav>
          </div>
          <div v-else class="section-copy">隔离区为空。应用不会自动清空之后加入的内容。</div>
        </div>
      </section>
    </template>

    <section class="surface training-preset-tool" aria-label="训练预设与 TOML">
      <header>
        <div><FileCode2 :size="18" /><span><h2 class="section-title">训练预设与 TOML</h2><p class="section-copy">在工具页管理版本化预设、导入或导出 lora-scripts TOML；训练配置页保持专注于本次实验。</p></span></div>
        <small>{{ trainingPresets.length ? `${trainingPresets.length} 个服务端预设` : '尚未保存预设' }}</small>
      </header>
      <div class="training-preset-tool-grid">
        <label>预设名称<input v-model="trainingPresetName" placeholder="例如：Odette · 1024 · LoCon" /></label>
        <label>模型适配器<select v-model="trainingPresetAdapterId"><option v-for="adapter in trainingAdapters" :key="adapter.id" :value="adapter.id">{{ adapter.label }} · {{ adapter.version }}</option></select></label>
        <label>运行时<select v-model="trainingPresetRuntimeProfileId"><option v-for="profile in trainingProfiles" :key="profile.id" :value="profile.id">{{ profile.label }}</option></select></label>
        <label class="wide">训练预设<select v-model="trainingPresetId" aria-label="训练预设" :disabled="trainingPresetLoading" @change="selectTrainingPreset"><option value="">新建预设</option><option v-for="preset in trainingPresets" :key="preset.id" :value="preset.id">{{ preset.name }} · v{{ preset.version_count }}</option></select></label>
      </div>
      <div class="training-preset-tool-actions"><button class="button" type="button" :disabled="!trainingPresetId || trainingPresetLoading" @click="selectTrainingPreset">读取 TOML</button><button class="button" type="button" :disabled="!trainingPresetId || trainingPresetLoading" @click="downloadTrainingPresetToml">导出 TOML</button><button class="button button-primary" type="button" :disabled="!trainingPresetId" @click="openTrainingWithPreset">{{ trainingPresetId ? '在训练中加载' : '选择预设后加载' }}</button></div>
      <details class="training-preset-toml" open><summary>{{ trainingPresetId ? '编辑 TOML 后保存为新版本' : '导入 lora-scripts TOML 为新预设' }}</summary><textarea v-model="trainingPresetToml" rows="9" aria-label="Lora-scripts TOML" placeholder="粘贴原始 lora-scripts TOML 参数…" /><div><button class="button button-primary" type="button" :disabled="trainingPresetLoading || !trainingPresetToml.trim()" @click="saveTrainingPresetToml">{{ trainingPresetId ? '保存新版本' : '导入并版本化保存' }}</button><small v-if="selectedTrainingPreset">当前预设已保留 {{ selectedTrainingPreset.version_count }} 个版本；保存不会覆盖旧版本。</small></div></details>
    </section>

    <ConfirmDialog
      :open="selectedTool !== null"
      :title="selectedTool?.preflight ? `创建${selectedTool?.title}预检` : `创建${selectedTool?.title}任务`"
      :confirm-label="selectedTool?.preflight ? '开始预检' : '创建任务'"
      :wide="selectedTool?.type === 'dataset_augmentation'"
      :busy="creating"
      @cancel="selectedTool = null; stopClTaggerPolling()"
      @confirm="createSelectedTask(); stopClTaggerPolling()"
    >
      <p style="margin-top: 0">{{ selectedTool?.description }}</p>
      <div class="field">
        <label class="field-label" for="tool-scope">处理范围</label>
        <select id="tool-scope" v-model="scope" class="select">
          <option value="root">整个媒体库</option>
          <option value="directory">媒体库内的文件夹</option>
        </select>
      </div>
      <div v-if="scope === 'directory'" class="field">
        <label class="field-label" for="tool-relative-directory">库内文件夹</label>
        <select v-if="!manualDirectory" id="tool-relative-directory" v-model="relativeDirectory" class="select" :disabled="directoriesLoading">
          <option value="" disabled>{{ directoriesLoading ? '正在读取文件夹…' : directories.length ? '选择一个已有文件夹' : '没有找到已有文件夹' }}</option>
          <option v-for="directory in directories" :key="directory" :value="directory">{{ formatDirectory(directory) }}</option>
        </select>
        <input v-else id="tool-relative-directory" v-model="relativeDirectory" class="input" placeholder="例如：portraits/2026" autocomplete="off">
        <span class="inline">
          <button type="button" class="button button-small button-quiet" @click="manualDirectory = !manualDirectory; relativeDirectory = ''">{{ manualDirectory ? '从已有文件夹选择' : '手动输入路径' }}</button>
          <RouterLink to="/settings" class="button button-small button-quiet">管理分类文件夹</RouterLink>
        </span>
        <span v-if="directoryLoadError" class="field-help">暂时无法读取文件夹，已切换为手动输入。</span>
        <span v-else class="field-help">递归处理该文件夹及其子目录；只包含已刷新到图库中的媒体。</span>
      </div>
      <div v-if="selectedTool?.type === 'delete_by_tag'" class="field">
        <label class="field-label" for="delete-tag">精确标签</label>
        <input id="delete-tag" v-model="tag" class="input" placeholder="例如：watermark">
      </div>
      <div v-if="selectedTool?.type === 'near_dedup'" class="field">
        <label class="field-label" for="phash-distance">感知哈希距离阈值</label>
        <input id="phash-distance" v-model.number="phashDistance" class="input" type="number" min="1" max="32">
      </div>
      <template v-if="selectedTool?.type === 'resize'">
        <div class="field">
          <label class="field-label" for="resize-max-size">最长边像素</label>
          <input id="resize-max-size" v-model.number="resizeMaxSize" class="input" type="number" min="1" max="8192">
        </div>
        <div class="field">
          <label class="field-label" for="resize-quality">JPEG 质量</label>
          <input id="resize-quality" v-model.number="resizeQuality" class="input" type="number" min="1" max="100">
        </div>
      </template>
      <template v-if="selectedTool?.type === 'dataset_augmentation'">
        <section class="dataset-augmentation-form">
          <p class="dataset-augmentation-summary">原图继续留在所选源目录，不会另存为 <code>original</code>。派生训练图写入 <code>.augmentation/&lt;任务 ID&gt;/</code>，JSONL、状态和拒绝记录独立写入 <code>.augmentation-metadata/&lt;任务 ID&gt;/</code>。</p>
          <div class="dataset-augmentation-grid">
            <section class="dataset-augmentation-section dataset-crop-section">
              <div class="dataset-augmentation-section-header">
                <div><strong>智能裁剪</strong><span>GPU 动漫人物、头部、手部、姿态与分割联合保护；低置信和多人重叠会拒绝个人裁剪。</span></div>
                <label class="dataset-inline-check" for="dataset-smart-crop-enabled"><input id="dataset-smart-crop-enabled" v-model="datasetSmartCropEnabled" type="checkbox"> 开启</label>
              </div>
              <div v-if="datasetSmartCropEnabled" class="dataset-runtime-grid">
                <div class="field"><label class="field-label" for="dataset-smart-crop-runtime">Python 运行时</label><select id="dataset-smart-crop-runtime" v-model="datasetSmartCropRuntimeProfileId" class="select"><option value="conda:lora">conda:lora（推荐）</option><option v-for="profile in trainingProfiles" :key="profile.id" :value="profile.id">{{ profile.label }}</option></select></div>
                <div class="field"><label class="field-label" for="dataset-smart-crop-gpu">GPU 编号</label><input id="dataset-smart-crop-gpu" v-model="datasetSmartCropGpuId" class="input" inputmode="numeric" pattern="[0-9]*"></div>
              </div>
              <div v-if="datasetSmartCropEnabled" class="dataset-runtime-status">
                <div class="inline"><button class="button button-small" type="button" :disabled="visionCropBusy" @click="checkVisionCropRuntime">检查运行时</button><button class="button button-small" type="button" :disabled="visionCropBusy" @click="installVisionCropModels">安装并预热检测模型</button></div>
                <span v-if="visionCropHealth" class="field-help">{{ visionCropHealth.ready ? `已就绪：${visionCropHealth.gpu_name ?? 'GPU'}；${visionCropHealth.providers.join(', ')}` : visionCropHealth.message }}</span>
                <span v-else class="field-help">anime-quality；任务会再次检查 CUDA provider、可用显存与模型状态，不会回退到 CPU。</span>
              </div>
              <div class="dataset-checkbox-grid">
                <label class="checkbox-row" for="dataset-smart-crop-portrait"><input id="dataset-smart-crop-portrait" v-model="datasetSmartCropPortrait" type="checkbox"> 生成肖像裁剪</label>
                <label class="checkbox-row" for="dataset-smart-crop-upper-body"><input id="dataset-smart-crop-upper-body" v-model="datasetSmartCropUpperBody" type="checkbox"> 生成上半身裁剪</label>
                <label class="checkbox-row" for="dataset-smart-crop-cowboy"><input id="dataset-smart-crop-cowboy" v-model="datasetSmartCropCowboyShot" type="checkbox"> 生成牛仔视角裁剪</label>
                <label class="checkbox-row" for="dataset-smart-crop-full-body"><input id="dataset-smart-crop-full-body" v-model="datasetSmartCropFullBody" type="checkbox"> 生成紧凑全身裁剪</label>
                <label class="checkbox-row" for="dataset-smart-crop-lower-body"><input id="dataset-smart-crop-lower-body" v-model="datasetSmartCropLowerBody" type="checkbox"> 生成下半身裁剪</label>
                <label class="checkbox-row" for="dataset-smart-crop-feet"><input id="dataset-smart-crop-feet" v-model="datasetSmartCropFeet" type="checkbox"> 生成脚部视角裁剪</label>
              </div>
              <label v-if="datasetSmartCropFeet" class="checkbox-row dataset-foot-quality" for="dataset-smart-crop-both-feet"><input id="dataset-smart-crop-both-feet" v-model="datasetSmartCropRequireBothFeet" type="checkbox"> 仅生成完整双脚（关闭时允许完整单脚）</label>
            </section>

            <section class="dataset-augmentation-section">
              <div class="dataset-augmentation-section-header"><div><strong>输出位置与质量筛选</strong><span>派生图保存为无损 PNG；不缩放、不拉伸，也不重新 JPEG 压缩。</span></div></div>
              <p class="field-help">路径由处理范围自动决定：<code>所选原始目录/.augmentation/&lt;任务 ID&gt;/</code>。因此导入训练时可自动绑定原图与每个派生子集。</p>
              <div class="dataset-resolution-grid">
                <div class="field"><label class="field-label" for="dataset-min-megapixels">最小像素数（MP）</label><input id="dataset-min-megapixels" v-model.number="datasetMinMegapixels" class="input" type="number" min="0.1" max="1000" step="0.1"></div>
                <div class="field"><label class="field-label" for="dataset-min-long-side">最小长边像素</label><input id="dataset-min-long-side" v-model.number="datasetMinLongSide" class="input" type="number" min="1" max="100000"></div>
                <div class="field"><label class="field-label" for="dataset-min-short-side">最小原生短边</label><input id="dataset-min-short-side" v-model.number="datasetMinShortSide" class="input" type="number" min="1" max="100000"></div>
              </div>
              <label class="checkbox-row" for="dataset-horizontal-flip"><input id="dataset-horizontal-flip" v-model="datasetHorizontalFlip" type="checkbox"> 生成水平翻转副本（需要重新打标）</label>
            </section>

            <section class="dataset-augmentation-section">
              <div class="dataset-augmentation-section-header"><div><strong>二次打标与数据切分</strong><span>CL Tagger 输出 Danbooru 标签；vLLM 视觉模型输出自然语言描述。选择引擎后弹出专属参数页配置。</span></div></div>
              <div class="field">
                <label class="field-label" for="dataset-retag-mode">二次打标引擎</label>
                <select id="dataset-retag-mode" v-model="datasetRetagMode" class="select">
                  <option value="none">不二次打标</option>
                  <option value="cl_tagger">CL Tagger（Danbooru 标签）</option>
                  <option value="vllm">vLLM 视觉模型（自然语言描述）</option>
                </select>
                <span class="inline" style="margin-top: 8px">
                  <button type="button" class="button button-small" :disabled="datasetRetagMode === 'none'" @click="openRetagParams">{{ datasetRetagMode === 'cl_tagger' ? '配置 CL Tagger 参数' : datasetRetagMode === 'vllm' ? '配置 vLLM 参数' : '选择引擎后配置参数' }}</button>
                  <label v-if="datasetRetagMode !== 'none'" class="checkbox-row" for="dataset-retag-identity" style="margin: 0"><input id="dataset-retag-identity" v-model="datasetPreserveArtistCharacterTags" type="checkbox"> 将原图 artist / character 标签置于新标签最前（逗号分隔）</label>
                </span>
              </div>
              <div class="dataset-split-grid">
                <div class="field"><label class="field-label" for="dataset-train-percent">训练集比例</label><input id="dataset-train-percent" v-model.number="datasetTrainPercent" class="input" type="number" min="0" max="100"></div>
                <div class="field"><label class="field-label" for="dataset-validation-percent">验证集比例</label><input id="dataset-validation-percent" v-model.number="datasetValidationPercent" class="input" type="number" min="0" max="100"></div>
                <div class="field"><label class="field-label" for="dataset-test-percent">测试集比例</label><input id="dataset-test-percent" v-model.number="datasetTestPercent" class="input" type="number" min="0" max="100"></div>
              </div>
              <span class="field-help">三项必须合计 100；只含成功写入新 Caption 的派生图才会加入对应的 ready 子集。</span>
            </section>
          </div>
        </section>
      </template>
      <template v-if="selectedTool?.type === 'vllm_tag'">
        <div class="field"><label class="field-label" for="vllm-tag-mode">打标引擎</label><select id="vllm-tag-mode" v-model="vllmTagMode" class="select" @change="startClTaggerPolling"><option value="vllm">vLLM 视觉模型（自然语言描述）</option><option value="cl_tagger">CL Tagger（Danbooru 标签）</option></select></div>
        <template v-if="vllmTagMode === 'vllm'">
          <div class="field"><label class="field-label" for="vllm-url">vLLM Base URL</label><input id="vllm-url" v-model="vllmParams.baseUrl" class="input" placeholder="http://127.0.0.1:8000/v1"></div>
          <div class="field"><label class="field-label" for="vllm-model">vLLM 模型</label><input id="vllm-model" v-model="vllmParams.model" class="input" placeholder="model/name"></div>
          <div class="field"><label class="field-label" for="vllm-concurrency">vLLM 并发数</label><input id="vllm-concurrency" v-model.number="vllmParams.concurrency" class="input" type="number" min="1" max="64"></div>
          <div class="field"><label class="field-label" for="vllm-language">输出格式</label><select id="vllm-language" v-model="vllmParams.language" class="select" @change="applyVllmPromptPreset"><option value="zh">中文描述</option><option value="en">英文描述</option></select></div>
          <div class="field"><label class="field-label" for="vllm-max-length">最大输出长度</label><input id="vllm-max-length" v-model.number="vllmParams.maxLength" class="input" type="number" min="1" max="4000"></div>
          <div class="field"><label class="field-label" for="vllm-prompt">系统提示词</label><span class="field-help">切换输出格式会载入匹配模板，载入后仍可编辑</span><textarea id="vllm-prompt" v-model="vllmParams.systemPrompt" class="textarea"></textarea></div>
        </template>
        <template v-else>
          <div class="field"><label class="field-label" for="vllm-tag-cl-model">CL Tagger 模型目录</label><input id="vllm-tag-cl-model" v-model="clTaggerParams.modelPath" class="input" placeholder="留空自动检测 HuggingFace 缓存或自动下载"></div>
          <div class="dataset-runtime-status">
            <div class="inline"><button class="button button-small" type="button" :disabled="clTaggerBusy" @click="refreshClTaggerModel">检测模型</button><button class="button button-small" type="button" :disabled="clTaggerBusy || clTaggerHealth?.downloading" @click="downloadClTaggerModelNow">下载模型</button></div>
            <span v-if="clTaggerHealth?.downloading" class="field-help">正在下载：{{ clTaggerHealth.downloaded_bytes ?? 0 }} / {{ clTaggerHealth.total_bytes ?? '?' }} 字节</span>
            <span v-else-if="clTaggerHealth?.loading" class="field-help">模型正在加载…</span>
            <span v-else-if="clTaggerHealth?.download_error" class="field-help">下载失败：{{ clTaggerHealth.download_error }}</span>
            <span v-else-if="clTaggerHealth?.loaded" class="field-help">模型已加载：{{ clTaggerHealth.model_path }}</span>
            <span v-else class="field-help">本地 CPU 推理（ONNX）；留空模型目录时任务会自动检测缓存，缺失则自动下载。</span>
          </div>
          <div class="dataset-resolution-grid">
            <div class="field"><label class="field-label" for="vllm-tag-cl-general">general 阈值</label><input id="vllm-tag-cl-general" v-model.number="clTaggerParams.generalThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
            <div class="field"><label class="field-label" for="vllm-tag-cl-character">character 阈值</label><input id="vllm-tag-cl-character" v-model.number="clTaggerParams.characterThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
            <div class="field"><label class="field-label" for="vllm-tag-cl-copyright">copyright 阈值</label><input id="vllm-tag-cl-copyright" v-model.number="clTaggerParams.copyrightThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
            <div class="field"><label class="field-label" for="vllm-tag-cl-quality">quality 阈值</label><input id="vllm-tag-cl-quality" v-model.number="clTaggerParams.qualityThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
            <div class="field"><label class="field-label" for="vllm-tag-cl-max-tags">最大标签数</label><input id="vllm-tag-cl-max-tags" v-model.number="clTaggerParams.maxTags" class="input" type="number" min="1" max="200"></div>
          </div>
        </template>
      </template>
      <div v-if="selectedTool?.type === 'tag_pipeline'" class="field">
        <label class="field-label" for="artist-prefix">艺术家标签前缀</label>
        <select id="artist-prefix" v-model="artistPrefix" class="select">
          <option value="artist">artist:标签</option>
          <option value="at">@标签</option>
        </select>
      </div>
    </ConfirmDialog>

    <ConfirmDialog
      :open="retagParamsOpen"
      :title="datasetRetagMode === 'cl_tagger' ? 'CL Tagger 参数' : 'vLLM 参数'"
      confirm-label="完成"
      :wide="true"
      @cancel="retagParamsOpen = false; stopClTaggerPolling()"
      @confirm="retagParamsOpen = false; stopClTaggerPolling()"
    >
      <template v-if="datasetRetagMode === 'vllm'">
        <p style="margin-top: 0">vLLM 视觉模型生成自然语言描述或标签；端点仅允许 loopback，除非设置页配置了额外 allowlist。API Key 仍由系统凭据库提供。</p>
        <div class="field"><label class="field-label" for="retag-vllm-url">vLLM Base URL</label><input id="retag-vllm-url" v-model="vllmParams.baseUrl" class="input" placeholder="http://127.0.0.1:8000/v1"></div>
        <div class="field"><label class="field-label" for="retag-vllm-model">vLLM 模型</label><input id="retag-vllm-model" v-model="vllmParams.model" class="input" placeholder="model/name"></div>
        <div class="field"><label class="field-label" for="retag-vllm-concurrency">并发数</label><input id="retag-vllm-concurrency" v-model.number="vllmParams.concurrency" class="input" type="number" min="1" max="64"></div>
        <div class="field"><label class="field-label" for="retag-vllm-language">输出格式</label><select id="retag-vllm-language" v-model="vllmParams.language" class="select" @change="applyVllmPromptPreset"><option value="zh">中文描述</option><option value="en">英文描述</option></select></div>
        <div class="field"><label class="field-label" for="retag-vllm-max-length">最大输出长度</label><input id="retag-vllm-max-length" v-model.number="vllmParams.maxLength" class="input" type="number" min="1" max="4000"></div>
        <div class="field"><label class="field-label" for="retag-vllm-prompt">系统提示词</label><span class="field-help">切换输出格式会载入匹配模板，载入后仍可编辑</span><textarea id="retag-vllm-prompt" v-model="vllmParams.systemPrompt" class="textarea"></textarea></div>
      </template>
      <template v-else-if="datasetRetagMode === 'cl_tagger'">
        <p style="margin-top: 0">CL Tagger 在本地 CPU 推理（ONNX），输出 Danbooru 标签格式；首次运行会在任务开始时加载模型。</p>
        <div class="field"><label class="field-label" for="retag-cl-model">CL Tagger 模型目录</label><input id="retag-cl-model" v-model="clTaggerParams.modelPath" class="input" placeholder="留空自动检测 HuggingFace 缓存或自动下载"></div>
        <div class="dataset-runtime-status">
          <div class="inline"><button class="button button-small" type="button" :disabled="clTaggerBusy" @click="refreshClTaggerModel">检测模型</button><button class="button button-small" type="button" :disabled="clTaggerBusy || clTaggerHealth?.downloading" @click="downloadClTaggerModelNow">下载模型</button></div>
          <span v-if="clTaggerHealth?.downloading" class="field-help">正在下载：{{ clTaggerHealth.downloaded_bytes ?? 0 }} / {{ clTaggerHealth.total_bytes ?? '?' }} 字节</span>
          <span v-else-if="clTaggerHealth?.loading" class="field-help">模型正在加载…</span>
          <span v-else-if="clTaggerHealth?.download_error" class="field-help">下载失败：{{ clTaggerHealth.download_error }}</span>
          <span v-else-if="clTaggerHealth?.loaded" class="field-help">模型已加载：{{ clTaggerHealth.model_path }}</span>
          <span v-else class="field-help">留空模型目录时任务会自动检测缓存，缺失则自动下载。</span>
        </div>
        <div class="field"><label class="field-label" for="retag-cl-general">general 置信度阈值</label><input id="retag-cl-general" v-model.number="clTaggerParams.generalThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
        <div class="field"><label class="field-label" for="retag-cl-character">character 置信度阈值</label><input id="retag-cl-character" v-model.number="clTaggerParams.characterThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
        <div class="field"><label class="field-label" for="retag-cl-copyright">copyright 置信度阈值</label><input id="retag-cl-copyright" v-model.number="clTaggerParams.copyrightThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
        <div class="field"><label class="field-label" for="retag-cl-quality">quality 置信度阈值</label><input id="retag-cl-quality" v-model.number="clTaggerParams.qualityThreshold" class="input" type="number" min="0" max="1" step="0.05"></div>
        <div class="field"><label class="field-label" for="retag-cl-max-tags">最大标签数</label><input id="retag-cl-max-tags" v-model.number="clTaggerParams.maxTags" class="input" type="number" min="1" max="200"></div>
      </template>
    </ConfirmDialog>

    <ConfirmDialog
      :open="confirmingPurge"
      title="永久清空隔离区"
      confirm-label="永久删除"
      destructive
      :busy="creating"
      @cancel="confirmingPurge = false"
      @confirm="purge"
    >
      此操作将永久删除 {{ quarantine.length }} 个隔离项，且无法恢复。隔离区默认不会自动清空，只有在确认不再需要恢复时才执行。
    </ConfirmDialog>
  </div>
</template>
