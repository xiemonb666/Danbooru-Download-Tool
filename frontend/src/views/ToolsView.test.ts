import { fireEvent, render, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ToolsView from './ToolsView.vue'

const mocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  getMediaDirectories: vi.fn(),
  getMediaRoots: vi.fn(),
  getQuarantine: vi.fn(),
  getTrainingAdapters: vi.fn(),
  getTrainingPresets: vi.fn(),
  getTrainingRuntimeProfiles: vi.fn(),
  getVisionCropRuntimeHealth: vi.fn(),
  exportTrainingPreset: vi.fn(),
  importTrainingPreset: vi.fn(),
  updateTrainingPresetToml: vi.fn(),
  installVisionCropRuntime: vi.fn(),
  push: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}))

vi.mock('../api', () => ({
  createTask: mocks.createTask,
  getMediaDirectories: mocks.getMediaDirectories,
  getMediaRoots: mocks.getMediaRoots,
  getQuarantine: mocks.getQuarantine,
  getTrainingAdapters: mocks.getTrainingAdapters,
  getTrainingPresets: mocks.getTrainingPresets,
  getTrainingRuntimeProfiles: mocks.getTrainingRuntimeProfiles,
  getVisionCropRuntimeHealth: mocks.getVisionCropRuntimeHealth,
  exportTrainingPreset: mocks.exportTrainingPreset,
  importTrainingPreset: mocks.importTrainingPreset,
  updateTrainingPresetToml: mocks.updateTrainingPresetToml,
  installVisionCropRuntime: mocks.installVisionCropRuntime,
  purgeQuarantine: vi.fn(),
  restoreQuarantine: vi.fn(),
}))

vi.mock('../stores/tasks', () => ({
  useTasksStore: () => ({ loadSnapshot: vi.fn() }),
}))

vi.mock('../stores/toast', () => ({
  useToastStore: () => ({
    info: mocks.info,
    warning: mocks.warning,
    success: mocks.success,
    error: mocks.error,
  }),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: mocks.push }),
}))

const root = {
  id: 'root-1',
  name: '图库',
  windows_path: null,
  linux_path: '/media',
  indexed: true,
  media_count: 2,
  created_at: '2026-07-16T00:00:00Z',
  updated_at: '2026-07-16T00:00:00Z',
}

describe('ToolsView media selection boundary', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.getMediaRoots.mockResolvedValue([root])
    mocks.getMediaDirectories.mockResolvedValue({ directories: ['portraits/2025', 'portraits/2026'], truncated: false })
    mocks.getQuarantine.mockResolvedValue([])
    mocks.getTrainingAdapters.mockResolvedValue([{ id: 'sdxl-lora', version: 'test', label: 'SDXL LoRA', trainer: 'trainer.py', groups: [], fields: [] }])
    mocks.getTrainingRuntimeProfiles.mockResolvedValue([{ id: 'conda:lora', label: 'Conda · lora', kind: 'conda', managed: false, installed: true, installing: false, runtime_root: 'D:/runtime', python_path: 'C:/Python/python.exe' }])
    mocks.getTrainingPresets.mockResolvedValue([])
    mocks.exportTrainingPreset.mockResolvedValue({ name: 'Odette', toml: 'output_name = "odette"\n' })
  })

  it('opens directory configuration for vLLM without creating an ID-less task', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const heading = await view.findByRole('heading', { name: '视觉模型打标' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')
    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))

    expect(view.getByRole('dialog')).toBeVisible()
    expect(mocks.push).not.toHaveBeenCalled()
    expect(mocks.createTask).not.toHaveBeenCalled()
  })

  it('moves versioned training presets and Lora-scripts TOML management into tools', async () => {
    const view = render(ToolsView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(await view.findByRole('heading', { name: '训练预设与 TOML' })).toBeVisible()
    expect(view.getByLabelText('训练预设')).toBeVisible()
    expect(view.getByText(/在工具页管理版本化预设/)).toBeVisible()
  })

  it('paginates a large quarantine instead of rendering every recoverable file at once', async () => {
    mocks.getQuarantine.mockResolvedValue(Array.from({ length: 51 }, (_, index) => ({
      id: `quarantine-${index}`,
      original_relative_path: `characters/entry-${index}.png`,
      size_bytes: 1024,
      reason: 'test',
    })))
    const view = render(ToolsView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(await view.findByText('characters/entry-0.png')).toBeVisible()
    expect(view.queryByText('characters/entry-50.png')).not.toBeInTheDocument()
    expect(view.getByText(/第 1 \/ 2 页/)).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '下一页' }))
    expect(await view.findByText('characters/entry-50.png')).toBeVisible()
  })

  it('edits a selected preset TOML and saves it as a new server version', async () => {
    mocks.getTrainingPresets.mockResolvedValue([{
      id: 'preset-1', name: 'Odette', created_at: 1, updated_at: 1, version_count: 1,
      training: { adapter_id: 'sdxl-lora', runtime_profile_id: 'conda:lora', gpu_ids: ['0'], parameters: { output_name: 'odette' } },
    }])
    mocks.updateTrainingPresetToml.mockResolvedValue({
      id: 'preset-1', name: 'Odette', created_at: 1, updated_at: 2, version_count: 2,
      training: { adapter_id: 'sdxl-lora', runtime_profile_id: 'conda:lora', gpu_ids: ['0'], parameters: { output_name: 'odette-v2' } },
    })
    const view = render(ToolsView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await view.findByRole('option', { name: 'Odette · v1' })
    const selector = view.getByLabelText('训练预设') as HTMLSelectElement
    await fireEvent.update(selector, 'preset-1')
    await waitFor(() => expect(view.getByLabelText('Lora-scripts TOML')).toHaveValue('output_name = "odette"\n'))
    await fireEvent.update(view.getByLabelText('Lora-scripts TOML'), 'output_name = "odette-v2"\n')
    await fireEvent.click(view.getByRole('button', { name: '保存新版本' }))

    await waitFor(() => expect(mocks.updateTrainingPresetToml).toHaveBeenCalledWith('preset-1', expect.objectContaining({
      name: 'Odette', adapter_id: 'sdxl-lora', runtime_profile_id: 'conda:lora', gpu_ids: ['0'], toml: 'output_name = "odette-v2"\n',
    })))
  })

  it('opens directory configuration for HEIC conversion', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const heading = await view.findByRole('heading', { name: 'HEIC 转换' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')
    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))

    expect(view.getByRole('dialog')).toBeVisible()
    expect(mocks.push).not.toHaveBeenCalled()
    expect(mocks.createTask).not.toHaveBeenCalled()
  })

  it('opens directory and prefix configuration for tag processing', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const heading = await view.findByRole('heading', { name: '标签处理' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')
    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))

    expect(view.getByLabelText('艺术家标签前缀')).toBeVisible()
    expect(mocks.push).not.toHaveBeenCalled()
    expect(mocks.createTask).not.toHaveBeenCalled()
  })

  it('creates integrity checks as a preflight task', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const heading = await view.findByRole('heading', { name: '完整性检查' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')
    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    await fireEvent.click(within(view.getByRole('dialog')).getByRole('button', { name: /创建任务|开始预检/ }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'integrity_scan',
      root_id: root.id,
      options: { preflight: true },
    })
  })

  it('exposes the newly secured tools and describes integrity checks precisely', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await view.findByRole('heading', { name: '完整性检查' })
    expect(view.getByRole('heading', { name: '数据集增广' })).toBeVisible()
    expect(view.getByRole('heading', { name: 'HEIC 转换' })).toBeVisible()
    expect(view.getByRole('heading', { name: '标签处理' })).toBeVisible()
    expect(view.getByText('支持的图片格式会进行完整解码；其他媒体容器仅执行基础文件检查。')).toBeVisible()
    expect(mocks.createTask).not.toHaveBeenCalled()
  })

  it('creates a directory-scoped resize task with adjustable size and quality', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    const heading = await view.findByRole('heading', { name: '安全缩放' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')

    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    const dialog = view.getByRole('dialog')
    await fireEvent.update(within(dialog).getByLabelText('处理范围'), 'directory')
    await fireEvent.update(within(dialog).getByLabelText('库内文件夹'), 'portraits/2026')
    await fireEvent.update(within(dialog).getByLabelText('最长边像素'), '2048')
    await fireEvent.update(within(dialog).getByLabelText('JPEG 质量'), '92')
    await fireEvent.click(within(dialog).getByRole('button', { name: '开始预检' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: {
        relative_directory: 'portraits/2026',
        max_size: 2048,
        quality: 92,
      },
    })
  })

  it('creates an immutable dataset-augmentation task with family split settings', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    const heading = await view.findByRole('heading', { name: '数据集增广' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')

    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    const dialog = view.getByRole('dialog')
    expect(within(dialog).getByText(/不会另存为/)).toBeVisible()
    expect(within(dialog).queryByLabelText('输出文件夹')).toBeNull()
    await fireEvent.click(within(dialog).getByLabelText('生成水平翻转副本（需要重新打标）'))
    await fireEvent.click(within(dialog).getByRole('button', { name: '创建任务' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'dataset_augmentation',
      root_id: root.id,
      options: {
        relative_directory: '.',
        min_megapixels: 1.8,
        min_long_side: 1536,
        min_short_side: 768,
        horizontal_flip: true,
        train_percent: 90,
        validation_percent: 5,
        test_percent: 5,
        smart_crop: {
          enabled: true,
          runtime_profile_id: 'conda:lora',
          gpu_id: '0',
          quality_profile: 'anime-quality',
          portrait: true,
          upper_body: true,
          cowboy_shot: true,
          full_body_tight: true,
          lower_body: true,
          feet: true,
          require_both_feet: false,
          max_derived_per_family: 6,
        },
        retagging: {
          send_to_vllm: false,
          preserve_artist_character_tags: true,
        },
      },
    })
  })

  it('enables quality smart crops in dataset augmentation by default', async () => {
    const view = render(ToolsView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    const heading = await view.findByRole('heading', { name: '数据集增广' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')

    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    const dialog = view.getByRole('dialog')
    expect(within(dialog).getByText('智能裁剪')).toBeVisible()
    expect(within(dialog).getByLabelText('生成肖像裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('生成上半身裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('生成牛仔视角裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('生成紧凑全身裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('生成下半身裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('生成脚部视角裁剪')).toBeChecked()
    expect(within(dialog).getByLabelText('仅生成完整双脚（关闭时允许完整单脚）')).not.toBeChecked()
  })

  it('groups dataset augmentation controls into a compact layout', async () => {
    const view = render(ToolsView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    const heading = await view.findByRole('heading', { name: '数据集增广' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')

    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    const dialog = view.getByRole('dialog')

    expect(dialog.querySelector('.dataset-augmentation-grid')).toBeTruthy()
    expect(dialog.querySelectorAll('.dataset-augmentation-section')).toHaveLength(3)
    expect(dialog.querySelector('.dataset-resolution-grid')).toBeTruthy()
    expect(dialog.querySelector('.dataset-split-grid')).toBeTruthy()
  })

  it('offers existing library folders instead of requiring users to remember relative paths', async () => {
    const view = render(ToolsView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    const heading = await view.findByRole('heading', { name: '视觉模型打标' })
    const card = heading.closest('article')
    if (!card) throw new Error('Tool card is missing')

    await fireEvent.click(within(card).getByRole('button', { name: '配置任务' }))
    const dialog = view.getByRole('dialog')
    await fireEvent.update(within(dialog).getByLabelText('处理范围'), 'directory')

    expect(await within(dialog).findByRole('option', { name: 'portraits / 2026' })).toBeVisible()
    expect(mocks.getMediaDirectories).toHaveBeenCalledWith(root.id)
  })
})
