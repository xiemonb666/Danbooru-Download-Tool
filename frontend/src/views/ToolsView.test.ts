import { fireEvent, render, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ToolsView from './ToolsView.vue'

const mocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  getMediaDirectories: vi.fn(),
  getMediaRoots: vi.fn(),
  getQuarantine: vi.fn(),
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
