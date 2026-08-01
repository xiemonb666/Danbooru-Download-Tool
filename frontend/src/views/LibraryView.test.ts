import { fireEvent, render, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import LibraryView from './LibraryView.vue'

const mocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  getLibrary: vi.fn(),
  getLibraryItem: vi.fn(),
  getMediaRoots: vi.fn(),
  loadSnapshot: vi.fn(),
  replace: vi.fn(),
  push: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  blurSensitiveMedia: true,
}))

vi.mock('../api', () => ({
  createTask: mocks.createTask,
  getLibrary: mocks.getLibrary,
  getLibraryItem: mocks.getLibraryItem,
  getMediaRoots: mocks.getMediaRoots,
  libraryMediaUrl: (id: string, variant = 'file') => `/media/${id}/${variant}`,
}))

vi.mock('../stores/tasks', () => ({
  useTasksStore: () => ({ loadSnapshot: mocks.loadSnapshot }),
}))

vi.mock('../stores/config', () => ({
  useConfigStore: () => ({
    config: {
      get blur_sensitive_media() { return mocks.blurSensitiveMedia },
    },
  }),
}))

vi.mock('../stores/toast', () => ({
  useToastStore: () => ({ success: mocks.success, error: mocks.error }),
}))

vi.mock('vue-router', () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ replace: mocks.replace, push: mocks.push }),
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

const items = [
  {
    id: 'media-1',
    root_id: root.id,
    filename: 'one.jpg',
    relative_path: 'one.jpg',
    mime_type: 'image/jpeg',
    size_bytes: 1024,
    tags: [],
    created_at: root.created_at,
  },
  {
    id: 'media-2',
    root_id: root.id,
    filename: 'two.png',
    relative_path: 'two.png',
    mime_type: 'image/png',
    size_bytes: 2048,
    tags: [],
    created_at: root.created_at,
  },
]

describe('LibraryView batch actions', () => {
  beforeEach(() => {
    mocks.blurSensitiveMedia = true
    mocks.getMediaRoots.mockResolvedValue([root])
    mocks.getLibrary.mockResolvedValue({ items, total: items.length })
    mocks.getLibraryItem.mockReset()
    mocks.getLibraryItem.mockImplementation(async (id: string) => items.find((item) => item.id === id))
    mocks.error.mockReset()
    mocks.createTask.mockResolvedValue(undefined)
    mocks.loadSnapshot.mockResolvedValue(undefined)
  })

  it('selects the current page and creates a resize task from media IDs', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const selectPage = await view.findByRole('checkbox', { name: '全选当前页' })
    await fireEvent.click(selectPage)
    expect(view.getByRole('checkbox', { name: '选择 one.jpg' })).toBeChecked()
    expect(view.getByRole('checkbox', { name: '选择 two.png' })).toBeChecked()

    await fireEvent.click(view.getByRole('button', { name: '安全缩放所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: { media_ids: ['media-1', 'media-2'], max_size: 1216 },
    })
  })

  it('uses the chosen maximum edge for a selected-image resize task', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 one.jpg' }))
    await fireEvent.update(view.getByLabelText('缩放最长边像素'), '2048')
    await fireEvent.click(view.getByRole('button', { name: '安全缩放所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: { media_ids: ['media-1'], max_size: 2048 },
    })
  })

  it('lets an already populated library refresh its files', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('button', { name: '刷新图库' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'index_library',
      root_id: root.id,
    })
  })

  it('follows the server library cursor and returns through cursor history', async () => {
    mocks.getLibrary
      .mockResolvedValueOnce({ items: [items[0]], total: 2, next_cursor: 'media-2' })
      .mockResolvedValueOnce({ items: [items[1]], total: 2 })
      .mockResolvedValueOnce({ items: [items[0]], total: 2, next_cursor: 'media-2' })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    expect(await view.findByRole('img', { name: 'one.jpg' })).toBeVisible()

    await fireEvent.click(view.getByRole('button', { name: '下一页' }))

    expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id, query: '', cursor: 'media-2', limit: 60,
    }, expect.any(AbortSignal))
    expect(await view.findByRole('img', { name: 'two.png' })).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '上一页' }))
    expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id, query: '', cursor: undefined, limit: 60,
    }, expect.any(AbortSignal))
    expect(await view.findByRole('img', { name: 'one.jpg' })).toBeVisible()
  })

  it('creates a vLLM task with stable selected media IDs', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 two.png' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 one.jpg' }))
    await fireEvent.click(view.getByRole('button', { name: '视觉模型打标所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'vllm_tag',
      root_id: root.id,
      options: { media_ids: ['media-1', 'media-2'] },
    })
  })

  it('disables vLLM tagging with a visible and accessible reason for mixed static and video media', async () => {
    mocks.getLibrary.mockResolvedValueOnce({
      items: [
        items[0],
        {
          ...items[1],
          id: 'video-1',
          filename: 'clip.mp4',
          relative_path: 'clip.mp4',
          mime_type: 'video/mp4',
        },
      ],
      total: 2,
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 one.jpg' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 clip.mp4' }))

    expect(view.getByRole('button', {
      name: '视觉模型打标不可用：所选媒体必须全部为支持的静态图片',
    })).toBeDisabled()
    expect(view.getByText('视觉模型打标仅支持 PNG、JPG/JPEG、BMP、WebP 或 GIF 图片。')).toBeVisible()
  })

  it('creates a tag pipeline task for any selected media IDs', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 two.png' }))
    await fireEvent.click(view.getByRole('button', { name: '标签处理所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'tag_pipeline',
      root_id: root.id,
      options: { media_ids: ['media-2'] },
    })
  })

  it('disables HEIC conversion with an accessible reason for mixed formats', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 one.jpg' }))

    expect(view.getByRole('button', {
      name: 'HEIC 转换不可用：所选媒体必须全部为 HEIC 或 HEIF 图片',
    })).toBeDisabled()
    expect(view.getByText('HEIC 转换仅支持所选内容全部为 HEIC/HEIF 图片。')).toBeVisible()
  })

  it('creates an HEIC conversion task when every selected item has an HEIC extension and accepted MIME', async () => {
    mocks.getLibrary.mockResolvedValueOnce({
      items: [
        {
          ...items[0],
          id: 'heic-1',
          filename: 'camera.heic',
          relative_path: 'camera.heic',
          mime_type: 'image/heic',
        },
        {
          ...items[1],
          id: 'heic-2',
          filename: 'portrait.heif',
          relative_path: 'portrait.heif',
          mime_type: 'application/octet-stream',
        },
      ],
      total: 2,
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 camera.heic' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 portrait.heif' }))
    const convert = view.getByRole('button', { name: 'HEIC 转换所选' })
    expect(convert).toBeEnabled()
    await fireEvent.click(convert)

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'heic_convert',
      root_id: root.id,
      options: { media_ids: ['heic-1', 'heic-2'] },
    })
  })

  it('rejects an HEIC MIME claim when the selected file has no HEIC or HEIF extension', async () => {
    mocks.getLibrary.mockResolvedValueOnce({
      items: [{
        ...items[0],
        id: 'mime-only',
        filename: 'untrusted.bin',
        relative_path: 'untrusted.bin',
        mime_type: 'image/heic',
      }],
      total: 1,
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 untrusted.bin' }))

    expect(view.getByRole('button', {
      name: 'HEIC 转换不可用：所选媒体必须全部为 HEIC 或 HEIF 图片',
    })).toBeDisabled()
  })

  it('obscures media with a missing rating until the card reveal control is used', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const image = await view.findByRole('img', { name: 'one.jpg' })
    expect(image).toHaveClass('media-obscured')

    await fireEvent.click(view.getByRole('button', { name: '显示 one.jpg' }))
    expect(image).not.toHaveClass('media-obscured')
  })

  it('does not obscure unrated library media when the setting is disabled', async () => {
    mocks.blurSensitiveMedia = false
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const image = await view.findByRole('img', { name: 'one.jpg' })
    expect(image).not.toHaveClass('media-obscured')
    expect(view.queryByRole('button', { name: '显示 one.jpg' })).not.toBeInTheDocument()
  })

  it('falls back to the controlled original file when thumbnail generation fails', async () => {
    mocks.blurSensitiveMedia = false
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    const image = await view.findByRole('img', { name: 'one.jpg' })

    await fireEvent.error(image)

    expect(image).toHaveAttribute('src', '/media/media-1/file')
  })

  it('keeps an unrated detail preview obscured until its own reveal control is used', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('button', { name: '查看 one.jpg' }))
    const detailImages = view.getAllByRole('img', { name: 'one.jpg' })
    const detailImage = detailImages[detailImages.length - 1]
    if (!detailImage) throw new Error('Detail image is missing')
    expect(detailImage).toHaveClass('media-obscured')

    await fireEvent.click(view.getByRole('button', { name: '显示详情 one.jpg' }))
    expect(detailImage).not.toHaveClass('media-obscured')
  })

  it('refreshes an opened detail from the server snapshot by media ID', async () => {
    mocks.getLibraryItem.mockResolvedValueOnce({
      ...items[0],
      size_bytes: 4096,
      tags: ['fresh_tag'],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('button', { name: '查看 one.jpg' }))

    await waitFor(() => expect(mocks.getLibraryItem).toHaveBeenCalledWith('media-1', expect.any(AbortSignal)))
    expect(await view.findByRole('button', { name: 'fresh_tag' })).toBeVisible()
    expect(view.getByText('4 KB')).toBeVisible()
  })

  it('filters the library by an exact tag clicked in media details', async () => {
    mocks.getLibraryItem.mockResolvedValueOnce({
      ...items[0],
      tags: ['character_exact'],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    await fireEvent.click(await view.findByRole('button', { name: '查看 one.jpg' }))
    const tag = await view.findByRole('button', { name: 'character_exact' })

    await fireEvent.click(tag)

    await waitFor(() => expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id,
      query: 'character_exact',
      cursor: undefined,
      limit: 60,
    }, expect.any(AbortSignal)))
    expect(view.queryByRole('dialog', { name: 'one.jpg' })).not.toBeInTheDocument()
  })

  it('keeps the list snapshot open and shows a toast when detail refresh fails', async () => {
    mocks.getLibraryItem.mockRejectedValueOnce(new Error('详情服务离线'))
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('button', { name: '查看 one.jpg' }))

    const dialog = view.getByRole('dialog', { name: 'one.jpg' })
    expect(within(dialog).getByRole('img', { name: 'one.jpg' })).toBeVisible()
    await waitFor(() => expect(mocks.error).toHaveBeenCalledWith('无法刷新媒体详情', '详情服务离线'))
    expect(dialog).toBeVisible()
  })

  it('moves focus into the detail dialog and returns it to the opening card', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    const opener = await view.findByRole('button', { name: '查看 one.jpg' })
    opener.focus()
    await fireEvent.click(opener)

    const dialog = view.getByRole('dialog', { name: 'one.jpg' })
    await waitFor(() => expect(dialog).toHaveFocus())
    const closeButtons = view.getAllByRole('button', { name: '关闭详情' })
    const closeButton = closeButtons[closeButtons.length - 1]
    if (!closeButton) throw new Error('Detail close button is missing')
    await fireEvent.click(closeButton)
    await waitFor(() => expect(opener).toHaveFocus())
  })

  it('closes the modal library detail with Escape and restores its opener', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    const opener = await view.findByRole('button', { name: '查看 one.jpg' })
    opener.focus()
    await fireEvent.click(opener)
    const dialog = view.getByRole('dialog', { name: 'one.jpg' })
    expect(dialog).toHaveAttribute('aria-modal', 'true')

    await fireEvent.keyDown(dialog, { key: 'Escape' })

    expect(view.queryByRole('dialog', { name: 'one.jpg' })).not.toBeInTheDocument()
    await waitFor(() => expect(opener).toHaveFocus())
  })
})
