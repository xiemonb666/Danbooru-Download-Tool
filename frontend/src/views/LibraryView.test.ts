import { fireEvent, render, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import LibraryView from './LibraryView.vue'

const mocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  getLibrary: vi.fn(),
  getLibraryItem: vi.fn(),
  getMediaDirectories: vi.fn(),
  getMediaRoots: vi.fn(),
  loadSnapshot: vi.fn(),
  replace: vi.fn(),
  push: vi.fn(),
  scrollTo: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  blurSensitiveMedia: true,
}))

vi.mock('../api', () => ({
  createTask: mocks.createTask,
  getLibrary: mocks.getLibrary,
  getLibraryItem: mocks.getLibraryItem,
  getMediaDirectories: mocks.getMediaDirectories,
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
  useToastStore: () => ({ success: mocks.success, error: mocks.error, warning: mocks.warning }),
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
    mocks.getLibrary.mockReset()
    mocks.getMediaDirectories.mockReset()
    mocks.getMediaRoots.mockResolvedValue([root])
    mocks.getMediaDirectories.mockResolvedValue({ directories: [], truncated: false })
    mocks.getLibrary.mockResolvedValue({ items, total: items.length })
    mocks.getLibraryItem.mockReset()
    mocks.getLibraryItem.mockImplementation(async (id: string) => items.find((item) => item.id === id))
    mocks.error.mockReset()
    mocks.scrollTo.mockReset()
    vi.stubGlobal('scrollTo', mocks.scrollTo)
    mocks.createTask.mockResolvedValue(undefined)
    mocks.loadSnapshot.mockResolvedValue(undefined)
  })

  it('creates a resize task from individually selected media IDs', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 one.jpg' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 two.png' }))

    await fireEvent.click(view.getByRole('button', { name: '安全缩放所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: { media_ids: ['media-1', 'media-2'], max_size: 1216 },
    })
  })

  it('selects every matching library item instead of only the current 60-item page', async () => {
    mocks.getLibrary.mockResolvedValueOnce({ items, total: 120, next_cursor: 'media-3' })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '全选搜索结果' }))

    expect(view.getByText('已选择 120 项')).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '安全缩放所选' }))
    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: { library_query: '', excluded_media_ids: [], max_size: 1216 },
    })
  })

  it('keeps a selected-search exception when an item is unchecked', async () => {
    mocks.getLibrary.mockResolvedValueOnce({ items, total: 120, next_cursor: 'media-3' })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '全选搜索结果' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 two.png' }))

    expect(view.getByText('已选择 119 项')).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '标签处理所选' }))
    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'tag_pipeline',
      root_id: root.id,
      options: { library_query: '', excluded_media_ids: ['media-2'] },
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

  it('follows numbered pages and returns to the previous page', async () => {
    mocks.getLibrary
      .mockResolvedValueOnce({ items: [items[0]], total: 2, page: 1, total_pages: 2, score_ranges: [] })
      .mockResolvedValueOnce({ items: [items[1]], total: 2, page: 2, total_pages: 2, score_ranges: [] })
      .mockResolvedValueOnce({ items: [items[0]], total: 2, page: 1, total_pages: 2, score_ranges: [] })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    expect(await view.findByRole('img', { name: 'one.jpg' })).toBeVisible()

    await fireEvent.click(view.getByRole('button', { name: '下一页' }))

    expect(mocks.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: 'smooth' })
    expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id, directory: '', query: '', page: 2, limit: 60,
    }, expect.any(AbortSignal))
    expect(await view.findByRole('img', { name: 'two.png' })).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '上一页' }))
    expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id, directory: '', query: '', page: 1, limit: 60,
    }, expect.any(AbortSignal))
    expect(await view.findByRole('img', { name: 'one.jpg' })).toBeVisible()
  })

  it('filters by a dynamic score interval and jumps directly to a page', async () => {
    mocks.getLibrary.mockResolvedValue({
      items,
      total: 120,
      page: 1,
      total_pages: 2,
      score_ranges: [{ score_min: 0, score_max: 9, count: 80 }],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await view.findByRole('option', { name: '0–9（80 项）' })
    await fireEvent.update(await view.findByRole('combobox', { name: '评分区间' }), '0:9')
    await waitFor(() => expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id,
      directory: '',
      query: '',
      page: 1,
      scoreMin: 0,
      scoreMax: 9,
      limit: 60,
    }, expect.any(AbortSignal)))

    await fireEvent.update(view.getByLabelText('跳转至页码'), '2')
    await fireEvent.click(view.getByRole('button', { name: '跳转' }))

    await waitFor(() => expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id,
      directory: '',
      query: '',
      page: 2,
      scoreMin: 0,
      scoreMax: 9,
      limit: 60,
    }, expect.any(AbortSignal)))
    expect(view.getByText(/共 2 页/)).toBeVisible()
  })

  it('filters by a dynamic resolution interval instead of a fixed minimum', async () => {
    mocks.getLibrary.mockResolvedValue({
      items,
      total: 80,
      page: 1,
      total_pages: 2,
      score_ranges: [],
      resolution_ranges: [{ resolution_min: 512, resolution_max: 1023, count: 80 }],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await view.findByRole('option', { name: '512–1023px（80 项）' })
    await fireEvent.update(view.getByRole('combobox', { name: '分辨率区间' }), '512:1023')

    await waitFor(() => expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id,
      directory: '',
      query: '',
      page: 1,
      resolutionMin: 512,
      resolutionMax: 1023,
      limit: 60,
    }, expect.any(AbortSignal)))
  })

  it('keeps the active score and resolution filters when selecting every matching item', async () => {
    mocks.getLibrary.mockResolvedValue({
      items,
      total: 120,
      page: 1,
      total_pages: 2,
      score_ranges: [{ score_min: 0, score_max: 9, count: 80 }],
      resolution_ranges: [{ resolution_min: 512, resolution_max: 1023, count: 80 }],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await view.findByRole('option', { name: '0–9（80 项）' })
    await fireEvent.update(view.getByRole('combobox', { name: '评分区间' }), '0:9')
    await fireEvent.update(view.getByRole('combobox', { name: '分辨率区间' }), '512:1023')
    await fireEvent.click(view.getByRole('checkbox', { name: '全选搜索结果' }))
    await fireEvent.click(view.getByRole('button', { name: '安全缩放所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'resize',
      root_id: root.id,
      options: {
        library_query: '',
        library_score_min: 0,
        library_score_max: 9,
        library_resolution_min: 512,
        library_resolution_max: 1023,
        excluded_media_ids: [],
        max_size: 1216,
      },
    })
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

  it('creates a confirmation-gated delete task for selected media IDs', async () => {
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('checkbox', { name: '选择 two.png' }))
    await fireEvent.click(view.getByRole('checkbox', { name: '选择 one.jpg' }))
    await fireEvent.click(view.getByRole('button', { name: '删除所选' }))

    expect(mocks.createTask).toHaveBeenCalledWith({
      type: 'delete_selected',
      root_id: root.id,
      options: { media_ids: ['media-1', 'media-2'] },
    })
    expect(mocks.success).toHaveBeenCalledWith(
      '删除任务已加入队列',
      '请在任务页确认后，所选媒体及同名标签文件会移入隔离区，可在隔离区恢复。',
    )
    expect(mocks.push).toHaveBeenCalledWith('/tasks')
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
      directory: '',
      query: 'character_exact',
      page: 1,
      limit: 60,
    }, expect.any(AbortSignal)))
    expect(view.queryByRole('dialog', { name: 'one.jpg' })).not.toBeInTheDocument()
  })

  it('appends a detail tag to an existing library search', async () => {
    mocks.getLibraryItem.mockResolvedValueOnce({
      ...items[0],
      tags: ['character_exact'],
    })
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })
    await fireEvent.update(await view.findByPlaceholderText('精确标签，例如：1girl landscape（所有标签都必须匹配）'), 'existing_tag')
    await fireEvent.click(view.getByRole('button', { name: '查看 one.jpg' }))

    await fireEvent.click(await view.findByRole('button', { name: 'character_exact' }))

    await waitFor(() => expect(mocks.getLibrary).toHaveBeenLastCalledWith({
      rootId: root.id,
      directory: '',
      query: 'existing_tag character_exact',
      page: 1,
      limit: 60,
    }, expect.any(AbortSignal)))
  })

  it('opens a full-size local image preview from the detail panel', async () => {
    mocks.blurSensitiveMedia = false
    const view = render(LibraryView, {
      global: {
        stubs: { RouterLink: { template: '<a><slot /></a>' } },
      },
    })

    await fireEvent.click(await view.findByRole('button', { name: '查看 one.jpg' }))
    const previewOpener = await view.findByRole('button', { name: '放大查看本地图片 one.jpg' })
    previewOpener.focus()
    await fireEvent.click(previewOpener)

    const dialog = view.getByRole('dialog', { name: '原图预览 one.jpg' })
    await waitFor(() => expect(dialog).toHaveFocus())
    await fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(view.queryByRole('dialog', { name: '原图预览 one.jpg' })).not.toBeInTheDocument()
    await waitFor(() => expect(previewOpener).toHaveFocus())
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
