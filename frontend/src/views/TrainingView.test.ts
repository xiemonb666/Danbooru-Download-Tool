import { fireEvent, render, waitFor, within } from '@testing-library/vue'
import { createPinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const apiMocks = vi.hoisted(() => ({
  createTrainingTask: vi.fn(),
  getTrainingCleanupPreview: vi.fn(),
  deleteTrainingTask: vi.fn(),
}))

vi.mock('../api', () => ({
  createTrainingTask: apiMocks.createTrainingTask,
  getTrainingCleanupPreview: apiMocks.getTrainingCleanupPreview,
  deleteTrainingTask: apiMocks.deleteTrainingTask,
  getTrainingAdapters: vi.fn().mockResolvedValue([{
    id: 'sdxl-lora', version: 'test', label: 'SDXL LoRA', trainer: 'trainer.py',
    groups: [
      { id: 'network', label: 'LoRA 网络', description: '网络设置' },
      { id: 'optimizer', label: '优化器与学习率', description: '优化器设置' },
      { id: 'logging', label: '日志与分布式', description: '实验日志和多卡设置' },
      { id: 'advanced', label: '高级参数', description: '原始上游参数' },
    ],
    fields: [
      { key: 'network_module', label: '网络模块', group: 'network', kind: 'select', default: 'networks.lora', choices: ['networks.lora', 'lycoris.kohya'], required: true, advanced: false, help: '选择网络实现' },
      { key: 'bucket_reso_steps', label: 'Bucket 步长', group: 'network', kind: 'number', default: 32, choices: [], required: false, advanced: false, help: 'SDXL 必须为 32 的倍数' },
      { key: 'sample_sampler', label: '样图采样器', group: 'saving', kind: 'select', default: 'euler_a', choices: ['ddim', 'euler_a'], required: false, advanced: true, help: '样图采样器' },
      { key: 'conv_dim', label: 'Conv Dim', group: 'network', kind: 'number', default: null, choices: [], required: false, advanced: true, help: '卷积 rank' },
      { key: 'optimizer_type', label: '优化器', group: 'optimizer', kind: 'select', default: 'AdamW8bit', choices: ['AdamW8bit', 'AdaFactor', 'Prodigy'], required: true, advanced: false, help: '优化器类型' },
      { key: 'optimizer_args', label: '优化器参数', group: 'optimizer', kind: 'list', default: [], choices: [], required: false, advanced: true, help: '额外参数' },
      { key: 'log_with', label: '日志后端', group: 'logging', kind: 'select', default: 'tensorboard', choices: ['tensorboard', 'wandb'], required: false, advanced: false, help: '日志记录器' },
      { key: 'advanced_parameters', label: '原始高级参数', group: 'advanced', kind: 'json', default: {}, choices: [], required: false, advanced: true, help: 'JSON 覆盖' },
    ],
  }]),
  getTrainingRuntimeProfiles: vi.fn().mockResolvedValue([
    { id: 'windows', label: 'Windows 原生 Python', kind: 'windows', managed: true, installed: false, installing: false, runtime_root: 'D:/runtime', python_path: 'D:/runtime/venv/Scripts/python.exe' },
    { id: 'conda:lora', label: 'Conda · lora', kind: 'conda', managed: false, installed: true, installing: false, runtime_root: 'D:/runtime', python_path: 'C:/Users/XieMo/anaconda3/envs/lora/python.exe' },
  ]),
  installTrainingRuntime: vi.fn().mockResolvedValue({ id: 'windows', label: 'Windows 原生 Python', kind: 'windows', managed: true, installed: false, installing: true, runtime_root: 'D:/runtime', python_path: 'D:/runtime/venv/Scripts/python.exe' }),
  getTrainingRuntimeDiagnostics: vi.fn().mockResolvedValue({ profile: { id: 'windows', label: 'Windows 原生 Python', kind: 'windows', managed: true, installed: false, installing: false, runtime_root: 'D:/runtime', python_path: 'D:/runtime/venv/Scripts/python.exe' }, checks: [] }),
  getTrainingGpus: vi.fn().mockResolvedValue([{ id: '0', name: 'RTX 5090', memory_total_mib: 32768, memory_used_mib: 1024, utilization_percent: 12 }]),
  getTrainingQueue: vi.fn().mockResolvedValue({ entries: [] }),
  getTrainingPresets: vi.fn().mockResolvedValue([]),
  getMediaRoots: vi.fn().mockResolvedValue([{ id: 'gallery-root', name: '角色图库', media_count: 24 }]),
  getMediaDirectories: vi.fn().mockResolvedValue({ directories: ['odette'] }),
  previewTrainingGalleryDataset: vi.fn().mockImplementation((dataset) => Promise.resolve({ root_id: 'gallery-root', root_name: '角色图库', relative_directory: 'odette', image_dir: 'D:/gallery/odette', caption_extension: '.txt', image_count: 12, caption_count: 12, repeats: dataset.repeats, effective_image_count: 12 * dataset.repeats })),
  exportTrainingPreset: vi.fn(),
  importTrainingPreset: vi.fn(),
  updateTrainingPreset: vi.fn(),
  previewTraining: vi.fn().mockResolvedValue({ toml: '' }),
}))

vi.mock('../components/TrainingMonitor.vue', () => ({
  default: { template: '<section aria-label="训练监控占位">训练监控占位</section>' },
}))

import TrainingView from './TrainingView.vue'
import { useTasksStore } from '../stores/tasks'

describe('TrainingView', () => {
  beforeEach(() => {
    window.localStorage.clear()
    apiMocks.createTrainingTask.mockReset()
    apiMocks.createTrainingTask.mockResolvedValue({ id: 'training-1' })
    apiMocks.getTrainingCleanupPreview.mockReset()
    apiMocks.getTrainingCleanupPreview.mockResolvedValue({ deletable: [], retained: [] })
    apiMocks.deleteTrainingTask.mockReset()
    apiMocks.deleteTrainingTask.mockResolvedValue({ task_id: 'training-1', deleted: [], retained: [] })
  })

  it('uses detected GPUs and keeps convolution rank hidden for normal LoRA', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    expect(view.getByRole('heading', { name: '训练工作台' })).toBeVisible()
    expect(view.getByRole('button', { name: '配置训练' })).toBeVisible()
    expect(view.getByRole('button', { name: '训练监控' })).toBeVisible()
    expect(view.getByRole('button', { name: 'LoRA SVD 分析' })).toBeVisible()
    await fireEvent.click(await view.findByText('自动选择空闲 GPU'))
    expect(await view.findByRole('checkbox', { name: /GPU 0 · RTX 5090/ })).toBeVisible()
    expect(view.queryByLabelText(/Conv Dim/)).not.toBeInTheDocument()
    expect(view.getByRole('button', { name: '保存参数记录' })).toBeVisible()
  })

  it('switches between setup and monitor with in-page tab buttons instead of reloading the route', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })

    expect(await view.findByRole('button', { name: '配置训练' })).toBeVisible()
    expect(view.getByRole('button', { name: '训练监控' })).toBeVisible()
    expect(view.getByRole('button', { name: 'LoRA SVD 分析' })).toBeVisible()
  })

  it('reveals convolution rank for LyCORIS and restores a saved parameter record', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const networkModule = await view.findByLabelText(/网络模块/)
    await fireEvent.update(networkModule, 'lycoris.kohya')
    expect(await view.findByLabelText(/Conv Dim/)).toBeVisible()

    await fireEvent.click(view.getByText('自动选择空闲 GPU'))
    const gpu = await view.findByRole('checkbox', { name: /GPU 0 · RTX 5090/ })
    await fireEvent.click(gpu)
    await fireEvent.update(view.getByLabelText('记录名称'), 'LyCORIS 实验')
    await fireEvent.click(view.getByRole('button', { name: '保存参数记录' }))

    const history = view.getByLabelText('历史参数') as HTMLSelectElement
    const record = await view.findByRole('option', { name: 'LyCORIS 实验' }) as HTMLOptionElement
    await fireEvent.update(history, record.value)
    await fireEvent.update(networkModule, 'networks.lora')
    expect(view.queryByLabelText(/Conv Dim/)).not.toBeInTheDocument()
    await fireEvent.click(view.getByRole('button', { name: '加载记录' }))

    await waitFor(() => expect(networkModule).toHaveValue('lycoris.kohya'))
    expect(await view.findByLabelText(/Conv Dim/)).toBeVisible()
    expect(gpu).toBeChecked()
  })

  it('expands advanced parameters into a JSON editor', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const page = within(view.container as HTMLElement)
    await fireEvent.click(await page.findByRole('button', { name: /高级参数/ }))

    await waitFor(() => {
      expect(view.container.querySelector('#training-advanced_parameters')).toBeInstanceOf(HTMLTextAreaElement)
    })
  })

  it('shows the full relevant tuning set for the selected optimizer', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const optimizer = await view.findByLabelText(/优化器 --optimizer_type/) as HTMLSelectElement

    await fireEvent.update(optimizer, 'Prodigy')
    expect(await view.findByLabelText(/Prodigy D 系数/)).toBeVisible()
    expect(view.getByLabelText(/Prodigy beta3/)).toBeVisible()
    expect(view.queryByLabelText(/AdaFactor 相对步长/)).not.toBeInTheDocument()

    await fireEvent.update(optimizer, 'AdaFactor')
    expect(await view.findByLabelText(/AdaFactor 相对步长/)).toBeVisible()
    expect(view.getByLabelText(/AdaFactor 裁剪阈值/)).toBeVisible()
    expect(view.queryByLabelText(/Prodigy D 系数/)).not.toBeInTheDocument()
  })

  it('serializes structured optimizer controls as lora-scripts optimizer arguments', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const runtime = await view.findByLabelText('运行时') as HTMLSelectElement
    await view.findByRole('option', { name: /Conda · lora · 已发现/ })
    await fireEvent.update(runtime, 'conda:lora')
    await view.findByText('外部环境 · C:/Users/XieMo/anaconda3/envs/lora/python.exe')
    const optimizer = view.getByLabelText(/优化器 --optimizer_type/) as HTMLSelectElement
    await fireEvent.update(optimizer, 'Prodigy')
    await fireEvent.update(await view.findByLabelText(/Prodigy D 系数/), '1.5')
    await fireEvent.click(view.getByRole('button', { name: '加入训练队列' }))

    await waitFor(() => expect(apiMocks.createTrainingTask).toHaveBeenCalledWith(expect.objectContaining({
      training: expect.objectContaining({
        parameters: expect.objectContaining({
          optimizer_type: 'Prodigy',
          optimizer_args: expect.arrayContaining(['d_coef=1.5', 'decouple=True']),
        }),
      }),
    })))
  })

  it('keeps logging and distributed controls collapsed until they are explicitly opened', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })

    expect(view.queryByLabelText(/日志后端/)).not.toBeInTheDocument()
    await fireEvent.click(await view.findByRole('button', { name: /日志与分布式/ }))
    expect(await view.findByLabelText(/日志后端/)).toBeVisible()
  })

  it('offers installation and diagnostics when the selected runtime is absent', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })

    expect(await view.findByRole('button', { name: '安装运行时' })).toBeVisible()
    expect(view.getByRole('button', { name: '运行诊断' })).toBeVisible()
  })

  it('keeps server preset and TOML management out of the training workspace', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    await view.findByRole('heading', { name: '训练工作台' })

    expect(view.queryByLabelText('服务端训练预设')).not.toBeInTheDocument()
    expect(view.queryByText('版本化预设与 Lora-scripts TOML')).not.toBeInTheDocument()
  })

  it('lists discovered Conda environments and only changes them after explicit sync', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const profile = await view.findByLabelText('运行时') as HTMLSelectElement
    expect(await view.findByRole('option', { name: /Conda · lora · 已发现/ })).toBeVisible()

    await fireEvent.update(profile, 'conda:lora')
    expect(await view.findByText('外部环境 · C:/Users/XieMo/anaconda3/envs/lora/python.exe')).toBeVisible()
    expect(view.getByText(/点击同步后才会对这个外部环境安装 kohya_ss 依赖/)).toBeVisible()
  })

  it('offers a gallery-referenced dataset workflow with repeat instead of copying media', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    await fireEvent.click(await view.findByRole('button', { name: '从图库引用' }))
    const root = await view.findByLabelText('图库根') as HTMLSelectElement
    await fireEvent.update(root, 'gallery-root')
    await fireEvent.update(await view.findByLabelText('图库目录'), 'odette')
    await fireEvent.update(view.getByLabelText('Repeat'), '3')

    expect(await view.findByText('12 张图片 × 3 repeat')).toBeVisible()
    expect(view.getByText(/数据只引用原目录，不复制或改写图库素材/)).toBeVisible()
  })

  it('only enables sample generation after an explicit choice and keeps negative prompt independent', async () => {
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })

    const enabled = await view.findByRole('checkbox', { name: '生成训练样图' })
    expect(enabled).not.toBeChecked()
    expect(view.queryByLabelText('样图正面 Prompt')).not.toBeInTheDocument()

    await fireEvent.click(enabled)
    expect(await view.findByLabelText('样图正面 Prompt')).toBeVisible()
    expect(view.getByLabelText('样图负面 Prompt')).toBeVisible()
    expect(view.getByLabelText('样图采样步数')).toHaveValue(30)
    expect(view.getByLabelText('样图宽度')).toHaveValue(1024)
    expect(view.getByLabelText('样图高度')).toHaveValue(1024)
    const sampler = view.getByLabelText('样图采样器')
    expect(sampler).toHaveValue('euler_a')
    expect(sampler.closest('.training-sample-settings')).not.toBeNull()

    await fireEvent.click(view.getByRole('radio', { name: /从数据集抽取 Caption TXT/ }))
    expect(await view.findByLabelText('抽取 Caption 数量')).toHaveValue(4)
  })

  it('keeps a saved SDXL configuration with its valid 32-pixel bucket step', async () => {
    window.localStorage.setItem('danbooru.training.parameter-history.v1', JSON.stringify([{
      id: 'odette', label: 'odette', savedAt: Date.now(), adapterId: 'sdxl-lora', runtimeProfileId: 'conda:lora', gpuIds: [],
      parameters: { network_module: 'networks.lora', bucket_reso_steps: 32 },
    }]))
    const view = render(TrainingView, { global: { plugins: [createPinia()] } })
    const history = await view.findByLabelText('历史参数') as HTMLSelectElement
    await fireEvent.update(history, 'odette')
    await fireEvent.click(view.getByRole('button', { name: '加载记录' }))

    expect(await view.findByLabelText(/Bucket 步长/)).toHaveValue(32)
  })

  it('previews permanent cleanup for a terminal training run before deleting its owned files', async () => {
    const pinia = createPinia()
    const store = useTasksStore(pinia)
    store.tasks = [{
      id: 'training-1', kind: 'training', status: 'completed', revision: 2, title: '训练 odette',
      progress: { completed: 10, total: 10, bytes_downloaded: 0, speed_bytes_per_sec: 0 }, failures: [],
      created_at: '2026-08-01T00:00:00Z', updated_at: '2026-08-01T01:00:00Z',
      training: { adapter_id: 'sdxl-lora', output_name: 'odette', gpu_ids: ['0'] },
    }] as never
    vi.spyOn(store, 'loadSnapshot').mockResolvedValue()
    apiMocks.getTrainingCleanupPreview.mockResolvedValue({
      deletable: [{ kind: 'output', path: 'D:/outputs/training-1', file_count: 3, bytes: 4096 }],
      retained: [{ kind: 'external_output', path: 'D:/outputs', file_count: 0, bytes: 0, reason: '共享输出目录没有归属清单' }],
    })
    apiMocks.deleteTrainingTask.mockResolvedValue({
      task_id: 'training-1',
      deleted: [{ kind: 'output', path: 'D:/outputs/training-1', file_count: 3, bytes: 4096 }],
      retained: [],
    })
    const view = render(TrainingView, { global: { plugins: [pinia] } })

    await fireEvent.click(await view.findByRole('button', { name: /训练监控/ }))
    await fireEvent.click(await view.findByRole('button', { name: '删除当前运行' }))
    expect(await view.findByRole('dialog', { name: '永久删除训练运行' })).toBeVisible()
    expect(await view.findByText('D:/outputs/training-1')).toBeVisible()
    expect(view.getByText(/共享输出目录没有归属清单/)).toBeVisible()

    await fireEvent.click(view.getByRole('button', { name: '永久删除' }))
    await waitFor(() => expect(apiMocks.deleteTrainingTask).toHaveBeenCalledWith('training-1'))
    await waitFor(() => expect(view.queryByRole('button', { name: '删除当前运行' })).not.toBeInTheDocument())
  })
})
