import { fireEvent, render, within } from '@testing-library/vue'
import { reactive } from 'vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import SettingsView from './SettingsView.vue'

const mocks = vi.hoisted(() => ({
  createMediaRoot: vi.fn(),
  deleteMediaRoot: vi.fn(),
  deleteSecret: vi.fn(),
  getMediaRoots: vi.fn(),
  getMediaDirectories: vi.fn(),
  createMediaDirectory: vi.fn(),
  saveSecret: vi.fn(),
  updateMediaRoot: vi.fn(),
  loadConfig: vi.fn(),
  saveConfig: vi.fn(),
  healthCheck: vi.fn(),
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
}))

vi.mock('../api', () => ({
  createMediaRoot: mocks.createMediaRoot,
  deleteMediaRoot: mocks.deleteMediaRoot,
  deleteSecret: mocks.deleteSecret,
  getMediaRoots: mocks.getMediaRoots,
  getMediaDirectories: mocks.getMediaDirectories,
  createMediaDirectory: mocks.createMediaDirectory,
  saveSecret: mocks.saveSecret,
  updateMediaRoot: mocks.updateMediaRoot,
}))

vi.mock('../stores/config', () => ({
  useConfigStore: () => ({
    loaded: true,
    loading: false,
    load: mocks.loadConfig,
    save: mocks.saveConfig,
    config: reactive({
      danbooru_username: '',
      danbooru_api_key_configured: false,
      vllm_api_key_configured: false,
      vllm_base_url: 'http://127.0.0.1:8000/v1',
      vllm_allowed_hosts: [],
      proxy_url: null,
      download_concurrency: 8,
      filename_template: '{id}_score_{score}.{ext}',
      ugoira_policy: 'webm_and_zip',
      blur_sensitive_media: true,
      vllm_model: 'vision-model',
      vllm_system_prompt: '生成准确标签',
      vllm_tag_mode: 'overwrite',
      vllm_concurrency: 4,
      vllm_language: 'danbooru',
      vllm_max_tags: 60,
      vllm_max_length: 400,
      vllm_verify_danbooru: true,
      vllm_reference_existing: false,
    }),
  }),
}))

vi.mock('../stores/health', () => ({
  useHealthStore: () => ({ status: 'online', message: '本地服务正常', check: mocks.healthCheck }),
}))

vi.mock('../stores/toast', () => ({
  useToastStore: () => ({ success: mocks.success, warning: mocks.warning, error: mocks.error }),
}))

const root = {
  id: 'root-1',
  name: '训练集',
  windows_path: 'C:\\Media\\Danbooru',
  linux_path: '/mnt/c/Media/Danbooru',
  indexed: false,
  media_count: 0,
  created_at: '2026-07-16T00:00:00Z',
  updated_at: '2026-07-16T00:00:00Z',
}

describe('SettingsView media root mapping', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.getMediaRoots.mockReset()
    mocks.getMediaDirectories.mockResolvedValue({ directories: [], truncated: false })
    mocks.loadConfig.mockResolvedValue(undefined)
    mocks.createMediaRoot.mockResolvedValue(root)
    mocks.getMediaRoots.mockResolvedValueOnce([]).mockResolvedValueOnce([root])
  })

  it('saves and displays a trimmed Windows/Linux mapping without indexing it', async () => {
    const view = render(SettingsView)
    await view.findByText('先添加一个顶层下载位置')
    await fireEvent.click(view.getByRole('button', { name: '添加下载位置' }))
    await fireEvent.update(view.getByLabelText('位置名称'), '  训练集  ')
    await fireEvent.update(view.getByLabelText('Windows 文件夹'), '  C:\\Media\\Danbooru  ')
    await fireEvent.update(view.getByLabelText('Linux / WSL 文件夹'), '  /mnt/c/Media/Danbooru  ')

    await fireEvent.click(view.getByRole('button', { name: '保存下载位置' }))

    expect(mocks.createMediaRoot).toHaveBeenCalledWith({
      name: '训练集',
      windows_path: 'C:\\Media\\Danbooru',
      linux_path: '/mnt/c/Media/Danbooru',
    })
    expect(await view.findByText('C:\\Media\\Danbooru')).toBeVisible()
    expect(view.getByText('/mnt/c/Media/Danbooru')).toBeVisible()
    expect(view.getByText('暂无图库媒体，可在图库中刷新')).toBeVisible()
  })

  it('uses a guided download-location flow and explains library subfolders', async () => {
    const view = render(SettingsView)

    expect(await view.findByText('先添加一个顶层下载位置')).toBeVisible()
    expect(view.getByText(/下载时再选择或新建库内分类文件夹/)).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '添加下载位置' }))

    expect(view.getByLabelText('位置名称')).toBeVisible()
    expect(view.getByLabelText('Windows 文件夹')).toBeVisible()
    expect(view.getByLabelText('Linux / WSL 文件夹')).toBeVisible()
  })

  it('removes a download location without deleting its files', async () => {
    mocks.getMediaRoots.mockReset()
    mocks.getMediaRoots.mockResolvedValueOnce([root]).mockResolvedValueOnce([])
    mocks.deleteMediaRoot.mockResolvedValue({ id: root.id })
    vi.stubGlobal('confirm', vi.fn(() => true))

    const view = render(SettingsView)
    await fireEvent.click(await view.findByRole('button', { name: '移除下载位置 训练集' }))

    expect(mocks.deleteMediaRoot).toHaveBeenCalledWith(root.id)
    expect(await view.findByText('先添加一个顶层下载位置')).toBeVisible()
    vi.unstubAllGlobals()
  })

  it('does not show a legacy path suggestion', async () => {
    const view = render(SettingsView)

    await view.findByText('下载位置')
    expect(view.queryByText('C:\\Legacy\\Danbooru')).not.toBeInTheDocument()
    expect(view.queryByRole('button', { name: '将旧路径添加为下载位置' })).not.toBeInTheDocument()
  })

  it('edits and saves the complete vLLM execution settings', async () => {
    const view = render(SettingsView)

    await fireEvent.update(await view.findByLabelText('vLLM 模型'), '  local/vision-v2  ')
    await fireEvent.update(view.getByLabelText('标签写入模式'), 'append')
    await fireEvent.update(view.getByLabelText('vLLM 并发数'), '12')
    await fireEvent.update(view.getByLabelText('输出格式'), 'zh')
    await fireEvent.update(view.getByLabelText('系统提示词'), '  只返回规范标签  ')
    await fireEvent.update(view.getByLabelText('最大标签数'), '48')
    await fireEvent.update(view.getByLabelText('最大输出长度'), '640')
    await fireEvent.click(view.getByRole('checkbox', { name: '联网校验 Danbooru 标签' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '参考现有标签文件' }))
    await fireEvent.click(view.getByRole('button', { name: '保存设置' }))

    expect(view.getByLabelText('vLLM 模型')).toHaveValue('local/vision-v2')
    expect(view.getByLabelText('系统提示词')).toHaveValue('只返回规范标签')
    expect(view.getByLabelText('标签写入模式')).toHaveValue('append')
    expect(view.getByLabelText('vLLM 并发数')).toHaveValue(12)
    expect(view.getByLabelText('输出格式')).toHaveValue('zh')
    expect(view.getByLabelText('最大标签数')).toHaveValue(48)
    expect(view.getByLabelText('最大输出长度')).toHaveValue(640)
    expect(view.getByRole('checkbox', { name: '联网校验 Danbooru 标签' })).not.toBeChecked()
    expect(view.getByRole('checkbox', { name: '参考现有标签文件' })).toBeChecked()
    expect(mocks.saveConfig).toHaveBeenCalledOnce()
  })

  it('loads a matching system prompt when the output format changes', async () => {
    const view = render(SettingsView)
    const prompt = await view.findByLabelText('系统提示词')

    await fireEvent.update(view.getByLabelText('输出格式'), 'zh')

    expect((prompt as HTMLTextAreaElement).value).toContain('中文')
    expect((prompt as HTMLTextAreaElement).value).toContain('<tag>...</tag>')

    await fireEvent.update(view.getByLabelText('输出格式'), 'en')

    expect((prompt as HTMLTextAreaElement).value).toContain('English')
    expect((prompt as HTMLTextAreaElement).value).toContain('<tag>...</tag>')
  })

  it('aligns the vLLM output options as matching settings controls', async () => {
    const view = render(SettingsView)
    const group = await view.findByRole('group', { name: '视觉模型输出选项' })
    const verify = within(group).getByRole('checkbox', { name: '联网校验 Danbooru 标签' })
    const reference = within(group).getByRole('checkbox', { name: '参考现有标签文件' })

    expect(verify.closest('label')).toHaveClass('setting-check')
    expect(reference.closest('label')).toHaveClass('setting-check')
  })

  it('lets the user disable sensitive-media blur and persists the choice', async () => {
    const view = render(SettingsView)
    const toggle = await view.findByRole('checkbox', { name: '默认模糊敏感分级' })

    expect(toggle).toBeChecked()
    await fireEvent.click(toggle)
    await fireEvent.click(view.getByRole('button', { name: '保存设置' }))

    expect(toggle).not.toBeChecked()
    expect(mocks.saveConfig).toHaveBeenCalledOnce()
  })
})
