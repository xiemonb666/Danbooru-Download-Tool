import { fireEvent, render } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import DownloadDestinationPicker from './DownloadDestinationPicker.vue'

const mocks = vi.hoisted(() => ({
  createMediaDirectory: vi.fn(),
  getMediaDirectories: vi.fn(),
}))

vi.mock('../api', () => ({
  createMediaDirectory: mocks.createMediaDirectory,
  getMediaDirectories: mocks.getMediaDirectories,
}))

const roots = [{
  id: 'root-1',
  name: '我的图库',
  windows_path: 'C:\\Media',
  linux_path: '/mnt/c/Media',
  indexed: true,
  media_count: 12,
  created_at: '',
  updated_at: '',
}]

describe('DownloadDestinationPicker', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.getMediaDirectories.mockResolvedValue({
      directories: ['人物', '人物/爱丽丝', '风景'],
      truncated: false,
    })
  })

  it('loads and selects an existing folder inside the chosen media library', async () => {
    const view = render(DownloadDestinationPicker, {
      props: { roots, rootId: 'root-1', directory: '' },
    })

    const folder = await view.findByRole('combobox', { name: '库内文件夹' })
    expect(mocks.getMediaDirectories).toHaveBeenCalledWith('root-1')
    expect(view.getByRole('option', { name: '人物 / 爱丽丝' })).toBeVisible()

    await fireEvent.update(folder, '人物/爱丽丝')

    expect(view.emitted('update:directory')).toContainEqual(['人物/爱丽丝'])
  })

  it('creates a nested folder and immediately selects it as the destination', async () => {
    mocks.createMediaDirectory.mockResolvedValue({ relative_path: '角色/爱丽丝' })
    const view = render(DownloadDestinationPicker, {
      props: { roots, rootId: 'root-1', directory: '' },
    })
    await view.findByRole('combobox', { name: '库内文件夹' })

    await fireEvent.click(view.getByRole('button', { name: '新建文件夹' }))
    await fireEvent.update(view.getByLabelText('新文件夹路径'), '  角色\\爱丽丝  ')
    await fireEvent.click(view.getByRole('button', { name: '创建并选择' }))

    expect(mocks.createMediaDirectory).toHaveBeenCalledWith('root-1', '角色/爱丽丝')
    expect(view.emitted('update:directory')).toContainEqual(['角色/爱丽丝'])
    expect(await view.findByRole('option', { name: '角色 / 爱丽丝' })).toBeVisible()
  })
})
