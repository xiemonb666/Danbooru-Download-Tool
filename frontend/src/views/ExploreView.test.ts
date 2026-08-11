import { fireEvent, render, waitFor } from '@testing-library/vue'
import { ref } from 'vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { getCachedPost } from '../utils/postCache'
import ExploreView from './ExploreView.vue'

const mocks = vi.hoisted(() => ({
  countDanbooruPosts: vi.fn(),
  createTask: vi.fn(),
  getDanbooruPost: vi.fn(),
  getDanbooruPosts: vi.fn(),
  getMediaDirectories: vi.fn(),
  getMediaRoots: vi.fn(),
  createMediaDirectory: vi.fn(),
  push: vi.fn(),
  scrollTo: vi.fn(),
  setAutocompleteSuggestions: undefined as undefined | ((items: Array<Record<string, unknown>>) => void),
  warning: vi.fn(),
}))

vi.mock('../api', () => ({
  countDanbooruPosts: mocks.countDanbooruPosts,
  createTask: mocks.createTask,
  danbooruMediaUrl: (id: number, variant: string) => `/media/${id}/${variant}`,
  getDanbooruPost: mocks.getDanbooruPost,
  getDanbooruPosts: mocks.getDanbooruPosts,
  getMediaDirectories: mocks.getMediaDirectories,
  getMediaRoots: mocks.getMediaRoots,
  createMediaDirectory: mocks.createMediaDirectory,
}))

vi.mock('../stores/config', () => ({
  useConfigStore: () => ({
    load: vi.fn(),
    config: {
      download_concurrency: 8,
      filename_template: '{id}.{ext}',
      ugoira_policy: 'webm_and_zip',
      blur_sensitive_media: true,
    },
  }),
}))

vi.mock('../stores/tasks', () => ({
  useTasksStore: () => ({ loadSnapshot: vi.fn() }),
}))

vi.mock('../stores/toast', () => ({
  useToastStore: () => ({ success: vi.fn(), warning: mocks.warning, error: vi.fn() }),
}))

vi.mock('../composables/useTagAutocomplete', () => ({
  useTagAutocomplete: () => {
    const suggestions = ref<Array<Record<string, unknown>>>([])
    mocks.setAutocompleteSuggestions = (items) => { suggestions.value = items }
    return {
      query: ref(''),
      suggestions,
      loading: ref(false),
      dispose: vi.fn(),
    }
  },
}))

vi.mock('vue-router', () => ({
  useRoute: () => ({ fullPath: '/explore?q=cat&page=1', query: { q: 'cat', page: '1' } }),
  useRouter: () => ({ push: mocks.push }),
}))

describe('ExploreView result count', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.stubGlobal('scrollTo', mocks.scrollTo)
    localStorage.clear()
    mocks.getDanbooruPosts.mockResolvedValue({ posts: [], page: 1 })
    mocks.countDanbooruPosts.mockResolvedValue({ count: 12_345, exact: false })
    mocks.getMediaRoots.mockResolvedValue([])
    mocks.getMediaDirectories.mockResolvedValue({ directories: [], truncated: false })
    mocks.createTask.mockResolvedValue({})
    mocks.setAutocompleteSuggestions = undefined
  })

  it('loads the dedicated count endpoint and labels an estimated result total', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(await view.findByText('约 12,345 项结果 · 第 1 页')).toBeVisible()
    expect(mocks.countDanbooruPosts).toHaveBeenCalledWith('cat', expect.any(AbortSignal))
  })

  it('scrolls to the top when moving to the next explore page', async () => {
    mocks.getDanbooruPosts.mockResolvedValueOnce({
      posts: [{
        id: 44, rating: 's', score: 7, fav_count: 2, image_width: 800, image_height: 600,
        file_ext: 'jpg', file_size: 1024, is_video: false, is_ugoira: false,
        restricted: false, downloaded: false,
        tags: { general: ['cat'], artist: [], copyright: [], character: [], meta: [] },
      }],
      page: 1,
      next_page: '2',
    })
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(await view.findByRole('button', { name: '下一页' }))

    expect(mocks.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: 'smooth' })
    expect(mocks.push).toHaveBeenLastCalledWith({ path: '/explore', query: { page: '2', q: 'cat' } })
  })

  it('stores loaded search posts in the bounded local post cache', async () => {
    const post = {
      id: 41, rating: 's', score: 7, fav_count: 2, image_width: 800, image_height: 600,
      file_ext: 'jpg', file_size: 1024, is_video: false, is_ugoira: false,
      restricted: false, downloaded: false,
      tags: { general: ['cached'], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await view.findByRole('button', { name: '打开帖子 41' })

    expect(getCachedPost(41)?.tags.general).toEqual(['cached'])
  })

  it('separates browsing and legacy-style batch download into two subviews', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')

    expect(view.getByRole('tab', { name: '浏览与选择' })).toHaveAttribute('aria-selected', 'true')
    expect(view.getByRole('textbox', { name: 'Danbooru 查询' })).toBeVisible()
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))

    expect(view.getByRole('tab', { name: '标签批量下载' })).toHaveAttribute('aria-selected', 'true')
    expect(view.queryByRole('textbox', { name: 'Danbooru 查询' })).not.toBeInTheDocument()
    expect(view.getByLabelText('包含标签')).toBeVisible()
    expect(view.getByLabelText('最大下载数量')).toBeVisible()
  })

  it('suggests and inserts matching tags in the batch include field', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    mocks.setAutocompleteSuggestions?.([{
      value: 'cat_ears', label: 'cat ears', category: 'general', post_count: 1234,
    }])

    await fireEvent.update(view.getByLabelText('包含标签'), 'cat')

    expect(await view.findByRole('option', { name: /cat ears/ })).toBeVisible()
    await fireEvent.mouseDown(view.getByRole('option', { name: /cat ears/ }))
    expect(view.getByLabelText('包含标签')).toHaveValue('cat_ears')
  })

  it('closes autocomplete suggestions when switching to another subview', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    mocks.setAutocompleteSuggestions?.([{
      value: 'cat_ears', label: 'cat ears', category: 'general', post_count: 1234,
    }])
    await fireEvent.update(view.getByLabelText('包含标签'), 'cat')
    expect(await view.findByRole('option', { name: /cat ears/ })).toBeVisible()

    await fireEvent.click(view.getByRole('tab', { name: '浏览与选择' }))
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))

    expect(view.queryByRole('option', { name: /cat ears/ })).not.toBeInTheDocument()
  })

  it('writes the native query and quick filters into the explore URL', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.update(view.getByRole('textbox', { name: 'Danbooru 查询' }), 'cat_ears score:>=20')
    await fireEvent.update(view.getByRole('combobox', { name: '内容分级' }), 'q')
    await fireEvent.update(view.getByRole('combobox', { name: '排序' }), 'score')
    await fireEvent.update(view.getByRole('combobox', { name: '文件格式' }), 'webm')
    await fireEvent.click(view.getByRole('button', { name: '执行查询' }))

    expect(mocks.push).toHaveBeenLastCalledWith({
      path: '/explore',
      query: {
        page: '1',
        q: 'cat_ears score:>=20',
        rating: 'q',
        order: 'score',
        format: 'webm',
      },
    })
  })

  it('allows Danbooru custom ordering without requiring a separate tag', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.update(view.getByRole('textbox', { name: 'Danbooru 查询' }), '')
    mocks.push.mockClear()

    await fireEvent.update(view.getByRole('combobox', { name: '排序' }), 'score')

    expect(mocks.warning).not.toHaveBeenCalled()
    expect(mocks.push).toHaveBeenLastCalledWith({
      path: '/explore',
      query: { page: '1', order: 'score' },
    })
  })

  it('adds the selected minimum resolution to the explore query', async () => {
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')

    await fireEvent.update(view.getByRole('combobox', { name: '最低分辨率' }), '4')

    expect(mocks.push).toHaveBeenLastCalledWith({
      path: '/explore',
      query: { page: '1', q: 'cat', resolution: '4' },
    })
  })

  it('refreshes an opened post and displays its duration and source', async () => {
    const post = {
      id: 42,
      rating: 's',
      score: 8,
      fav_count: 3,
      image_width: 800,
      image_height: 600,
      file_ext: 'mp4',
      file_size: 1024,
      is_video: true,
      is_ugoira: false,
      restricted: false,
      downloaded: false,
      tags: { general: ['cat'], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    mocks.getDanbooruPost.mockResolvedValueOnce({
      ...post,
      duration: 12.5,
      source: 'https://example.test/source',
    })
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(await view.findByRole('button', { name: '打开帖子 42' }))

    expect(mocks.getDanbooruPost).toHaveBeenCalledWith(42, expect.any(AbortSignal))
    expect(view.getByRole('dialog', { name: '帖子 #42' })).toHaveClass('is-landscape')
    const video = view.container.querySelector('video')
    expect(video).not.toBeNull()
    expect(video).toHaveAttribute('poster', '/media/42/sample')
    expect(video).toHaveAttribute('width', '800')
    expect(video).toHaveAttribute('height', '600')
    expect(video).toHaveAttribute('playsinline', '')
    expect(await view.findByText('12.5 秒')).toBeVisible()
    expect(view.getByText('https://example.test/source')).toBeVisible()
  })

  it('focuses the detail dialog, closes it with Escape, and restores the opener focus', async () => {
    const post = {
      id: 43,
      rating: 's',
      score: 9,
      fav_count: 4,
      image_width: 800,
      image_height: 600,
      file_ext: 'jpg',
      file_size: 2048,
      is_video: false,
      is_ugoira: false,
      restricted: false,
      downloaded: false,
      tags: { general: ['dog'], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    mocks.getDanbooruPost.mockResolvedValueOnce(post)
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    const opener = await view.findByRole('button', { name: '打开帖子 43' })
    opener.focus()

    await fireEvent.click(opener)

    const dialog = await view.findByRole('dialog', { name: '帖子 #43' })
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    await waitFor(() => expect(dialog).toHaveFocus())
    await fireEvent.keyDown(dialog, { key: 'Escape' })
    expect(view.queryByRole('dialog', { name: '帖子 #43' })).not.toBeInTheDocument()
    await waitFor(() => expect(opener).toHaveFocus())
  })

  it('appends a detail tag to the existing explore query', async () => {
    const post = {
      id: 46,
      rating: 's',
      score: 1,
      fav_count: 0,
      image_width: 800,
      image_height: 600,
      file_ext: 'jpg',
      file_size: 1024,
      is_video: false,
      is_ugoira: false,
      restricted: false,
      downloaded: false,
      tags: { general: ['added_tag'], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    mocks.getDanbooruPost.mockResolvedValueOnce(post)
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(await view.findByRole('button', { name: '打开帖子 46' }))
    await fireEvent.click(await view.findByRole('button', { name: 'added_tag' }))

    expect(mocks.push).toHaveBeenLastCalledWith({
      path: '/explore',
      query: { page: '1', q: 'cat added_tag' },
    })
  })

  it('opens a still image at the proxied original resolution from post details', async () => {
    const post = {
      id: 44,
      rating: 's',
      score: 10,
      fav_count: 5,
      image_width: 1600,
      image_height: 2400,
      file_ext: 'png',
      file_size: 4096,
      is_video: false,
      is_ugoira: false,
      restricted: false,
      downloaded: false,
      tags: { general: ['portrait'], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    mocks.getDanbooruPost.mockResolvedValueOnce(post)
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(await view.findByRole('button', { name: '打开帖子 44' }))
    await fireEvent.click(await view.findByRole('button', { name: '放大查看帖子 44 原图' }))

    const originalDialog = view.getByRole('dialog', { name: '原图预览 #44' })
    expect(originalDialog).toBeVisible()
    await waitFor(() => expect(originalDialog).toHaveFocus())
    expect(view.getByRole('img', { name: '帖子 44 原图' })).toHaveAttribute('src', '/media/44/original')
    await fireEvent.keyDown(originalDialog, { key: 'Escape' })
    expect(view.queryByRole('dialog', { name: '原图预览 #44' })).not.toBeInTheDocument()
  })

  it('uses every safe still-image fallback when the detail preview fails', async () => {
    const post = {
      id: 45, rating: 's', score: 1, fav_count: 0, image_width: 1200, image_height: 800,
      file_ext: 'jpg', file_size: 2048, is_video: false, is_ugoira: false,
      restricted: false, downloaded: false,
      tags: { general: [], artist: [], copyright: [], character: [], meta: [] },
    }
    mocks.getDanbooruPosts.mockResolvedValueOnce({ posts: [post], page: 1 })
    mocks.getDanbooruPost.mockResolvedValueOnce(post)
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await fireEvent.click(await view.findByRole('button', { name: '打开帖子 45' }))
    const image = await view.findByRole('img', { name: '帖子 45 预览' })

    await fireEvent.error(image)
    expect(image).toHaveAttribute('src', '/media/45/preview')
    await fireEvent.error(image)
    expect(image).toHaveAttribute('src', '/media/45/large')
    await fireEvent.error(image)
    expect(image).toHaveAttribute('src', '/media/45/original')
    await fireEvent.error(image)
    expect(view.getByText('暂无可访问的详情预览')).toBeVisible()
  })

  it('creates a batch download from separate include and exclude tag fields', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(view.getByLabelText('包含标签'), 'cat_ears')
    await fireEvent.update(view.getByLabelText('排除标签'), 'animated, lowres')
    await fireEvent.update(view.getByLabelText('最低评分'), '10')
    await fireEvent.update(view.getByLabelText('最大下载数量'), '250')
    await fireEvent.click(view.getByLabelText('评分优先排序'))
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      type: 'download',
      source: { type: 'query', query: 'cat_ears -animated -lowres score:>=10' },
      root_id: 'root-1',
      limit: 250,
    })))
  })

  it('keeps score and high-resolution priorities enabled in one batch task', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(view.getByLabelText('包含标签'), '1girl')
    await fireEvent.click(view.getByLabelText('评分优先排序'))
    await fireEvent.click(view.getByLabelText('高分辨率优先'))
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      source: { type: 'query', query: '1girl score:>=0' },
      prioritize_score: true,
      prioritize_resolution: true,
    })))
  })

  it('sends the selected minimum resolution for a batch task', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(view.getByLabelText('包含标签'), '1girl')
    await fireEvent.update(view.getByLabelText('最低分辨率'), '2048')
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      source: { type: 'query', query: '1girl score:>=0 width:>=2048 height:>=2048' },
      batch_filter: expect.objectContaining({ minimum_resolution: 2048 }),
    })))
  })

  it('lets a batch task download static images only', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(view.getByLabelText('包含标签'), '1girl')
    expect(view.getByLabelText('仅下载静态图片')).not.toBeChecked()
    await fireEvent.click(view.getByLabelText('仅下载静态图片'))
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      static_images_only: true,
    })))
  })

  it('lets a batch task opt out of preserving same-name TXT tag files', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(view.getByLabelText('包含标签'), '1girl')
    expect(view.getByLabelText('保留同名 TXT 标签文件')).toBeChecked()
    await fireEvent.click(view.getByLabelText('保留同名 TXT 标签文件'))
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      keep_sidecar_txt: false,
    })))
  })

  it('loads the most recent batch settings and lists reusable parameter history', async () => {
    localStorage.setItem('danbooru-batch-download-history-v1', JSON.stringify([{
      includeTags: '1girl solo',
      excludeTags: 'comic watermark',
      minimumScore: 20,
      limit: 250,
      prioritizeScore: true,
      prioritizeResolution: true,
      rootId: 'root-1',
      directory: '角色图',
      savedAt: 1_700_000_000_000,
    }]))
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '下载目录', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))

    expect(view.getByText('批量参数历史')).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '加载上次设置' }))

    expect(view.getByLabelText('包含标签')).toHaveValue('1girl solo')
    expect(view.getByLabelText('排除标签')).toHaveValue('comic watermark')
    expect(view.getByLabelText('最低评分')).toHaveValue(20)
    expect(view.getByLabelText('最大下载数量')).toHaveValue(250)
    expect(view.getByLabelText('评分优先排序')).toBeChecked()
    expect(view.getByLabelText('高分辨率优先')).toBeChecked()
    expect(view.getByRole('button', { name: '使用历史设置 1girl solo' })).toBeVisible()
  })

  it('sends the selected library folder with a batch download task', async () => {
    mocks.getMediaRoots.mockResolvedValueOnce([{
      id: 'root-1', name: '训练图库', windows_path: null, linux_path: '/media', indexed: true,
      media_count: 0, created_at: '', updated_at: '',
    }])
    mocks.getMediaDirectories.mockResolvedValueOnce({
      directories: ['项目', '项目/角色图'],
      truncated: false,
    })
    const view = render(ExploreView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    await view.findByText('约 12,345 项结果 · 第 1 页')
    await fireEvent.click(view.getByRole('tab', { name: '标签批量下载' }))
    await fireEvent.update(await view.findByRole('combobox', { name: '库内文件夹' }), '项目/角色图')
    await fireEvent.update(view.getByLabelText('包含标签'), '1girl')
    await fireEvent.click(view.getByRole('button', { name: '开始批量下载' }))

    await waitFor(() => expect(mocks.createTask).toHaveBeenCalledWith(expect.objectContaining({
      root_id: 'root-1',
      relative_directory: '项目/角色图',
    })))
  })
})
