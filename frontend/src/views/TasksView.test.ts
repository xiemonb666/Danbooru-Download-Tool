import { fireEvent, render, waitFor } from '@testing-library/vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import TasksView from './TasksView.vue'

const mocks = vi.hoisted(() => ({
  control: vi.fn(),
  loadSnapshot: vi.fn(),
  error: vi.fn(),
  getDownloadHistory: vi.fn(),
  getTaskDetails: vi.fn(),
  createTask: vi.fn(),
  taskStatus: 'awaiting_confirmation',
  taskKind: 'exact_dedup' as 'exact_dedup' | 'near_dedup' | 'download' | 'resize' | 'vllm_tag',
  taskRevision: 3,
}))

vi.mock('../api', () => ({
  getDownloadHistory: mocks.getDownloadHistory,
  getTaskDetails: mocks.getTaskDetails,
  createTask: mocks.createTask,
}))

vi.mock('../stores/tasks', () => ({
  useTasksStore: () => ({
    sortedTasks: () => [{
      id: 'task-1',
      kind: mocks.taskKind,
      status: mocks.taskStatus,
      revision: mocks.taskRevision,
      title: '精确去重预检',
      progress: { completed: 0, total: 0, bytes_downloaded: 0, speed_bytes_per_sec: 0 },
      failures: [],
      preview: {
        candidates: [{ relative_path: 'duplicates/b.jpg', reason: 'exact_duplicate_of:a.jpg', size: 42 }],
      },
      created_at: '2026-07-16T00:00:00Z',
      updated_at: '2026-07-16T00:00:00Z',
    }],
    loading: false,
    connection: 'live',
    loadSnapshot: mocks.loadSnapshot,
    control: mocks.control,
  }),
}))

vi.mock('../stores/toast', () => ({
  useToastStore: () => ({ error: mocks.error }),
}))

describe('TasksView destructive preflight', () => {
  afterEach(() => {
    mocks.taskStatus = 'awaiting_confirmation'
    mocks.taskKind = 'exact_dedup'
    mocks.taskRevision = 3
    mocks.getTaskDetails.mockReset()
  })

  it('shows the candidate list and requires a second confirmation before applying it', async () => {
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(view.getByText(/duplicates\/b\.jpg/)).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '审阅并确认' }))
    expect(await view.findByRole('dialog')).toBeVisible()
    expect(mocks.control).not.toHaveBeenCalled()
    await fireEvent.click(view.getByRole('button', { name: '移入隔离区' }))
    expect(mocks.control).toHaveBeenCalledWith('task-1', 'confirm')
  })

  it('makes near-duplicate confirmation explicit while retaining the candidate preview', async () => {
    mocks.taskKind = 'near_dedup'
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(view.getByText(/duplicates\/b\.jpg/)).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '审阅并确认' }))
    expect(await view.findByRole('dialog')).toBeVisible()
    expect(view.getByRole('button', { name: '移入隔离区' })).toBeVisible()
  })

  it('shows a persistent download history section with human-readable totals', async () => {
    mocks.getDownloadHistory.mockResolvedValue({
      items: [{
        id: 'history-1',
        task_id: 'download-1',
        status: 'completed',
        source_label: 'cat_ears rating:s',
        root_name: '训练集',
        created_at: '2026-07-16T01:00:00Z',
        finished_at: '2026-07-16T01:02:00Z',
        total_items: 3,
        completed_items: 2,
        skipped_items: 1,
        failed_items: 0,
        bytes_processed: 1_572_864,
        can_repeat: false,
      }],
    })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '下载记录' }))

    expect(await view.findByText('cat_ears rating:s')).toBeVisible()
    expect(view.getByText(/训练集/)).toBeVisible()
    expect(view.getByText('1.5 MB')).toBeVisible()
    expect(view.getByText(/成功 2/)).toBeVisible()
  })

  it('shows bounded item details with failure diagnostics for a download task', async () => {
    mocks.taskKind = 'download'
    mocks.taskStatus = 'failed'
    mocks.getTaskDetails.mockResolvedValue({
      task: { id: 'task-1' },
      item_counts: { total: 3, queued: 0, completed: 1, skipped: 1, failed: 1, retryable_failed: 1 },
      items: [{
        item_id: 'post:42',
        post_id: 42,
        status: 'failed',
        attempts: 2,
        error: { code: 'media_timeout', message: '媒体请求超时', retryable: true },
        updated_at: '2026-07-16T02:00:00Z',
      }],
    })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))

    expect(mocks.getTaskDetails).toHaveBeenCalledWith('task-1', { itemLimit: 50 }, expect.any(AbortSignal))
    expect(await view.findByText('媒体请求超时')).toBeVisible()
    expect(view.getByText('media_timeout')).toBeVisible()
    expect(view.getByText('尝试 2 次')).toBeVisible()
    expect(view.getByText('失败 1')).toBeVisible()
  })

  it('shows generated tags and write status for a visual tagging item', async () => {
    mocks.taskKind = 'vllm_tag'
    mocks.taskStatus = 'completed'
    mocks.getTaskDetails.mockResolvedValue({
      task: { id: 'task-1' },
      item_counts: { total: 1, queued: 0, completed: 1, skipped: 0, failed: 0, retryable_failed: 0 },
      items: [{
        item_id: 'media:portrait',
        status: 'completed',
        attempts: 1,
        result: { media_ids: ['portrait'], tags: ['1girl', 'blue_hair'], sidecar_written: true },
        updated_at: '2026-07-16T02:00:00Z',
      }],
    })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))

    expect(await view.findByText('生成标签')).toBeVisible()
    expect(view.getByText('1girl')).toBeVisible()
    expect(view.getByText('blue_hair')).toBeVisible()
    expect(view.getByText('已写入标签文件')).toBeVisible()
  })

  it('shows processed count and output dimensions for a resize task', async () => {
    mocks.taskKind = 'resize'
    mocks.taskStatus = 'completed'
    mocks.getTaskDetails.mockResolvedValue({
      task: { id: 'task-1' },
      result: { items: [{ media_id: 'media:forest', width: 1024, height: 683 }] },
      item_counts: { total: 0, queued: 0, completed: 0, skipped: 0, failed: 0, retryable_failed: 0 },
      items: [],
    })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))

    expect(await view.findByText('处理结果')).toBeVisible()
    expect(view.getByText('已处理 1 项')).toBeVisible()
    expect(view.getByText('1024 × 683')).toBeVisible()
  })

  it('shows and paginates complete failure items for a non-download task', async () => {
    mocks.taskKind = 'resize'
    mocks.taskStatus = 'failed'
    const counts = { total: 2, queued: 0, completed: 0, skipped: 0, failed: 2, retryable_failed: 1 }
    mocks.getTaskDetails
      .mockResolvedValueOnce({
        task: { id: 'task-1' },
        item_counts: counts,
        items: [{
          item_id: 'media:first.jpg', status: 'failed', attempts: 1,
          error: { code: 'decode_failed', message: '首个图片解码失败', retryable: false },
          updated_at: '2026-07-16T02:00:00Z',
        }],
        next_cursor: 'failure-page-2',
      })
      .mockResolvedValueOnce({
        task: { id: 'task-1' },
        item_counts: counts,
        items: [{
          item_id: 'media:second.jpg', status: 'failed', attempts: 2,
          error: { code: 'write_failed', message: '第二个图片写入失败', retryable: true },
          updated_at: '2026-07-16T02:01:00Z',
        }],
      })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))
    expect(await view.findByText('首个图片解码失败')).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '下一页' }))

    expect(mocks.getTaskDetails).toHaveBeenLastCalledWith(
      'task-1', { itemCursor: 'failure-page-2', itemLimit: 50 }, expect.any(AbortSignal),
    )
    expect(await view.findByText('第二个图片写入失败')).toBeVisible()
  })

  it('replaces the current item page when following the next cursor', async () => {
    mocks.taskKind = 'download'
    mocks.taskStatus = 'completed'
    const counts = { total: 51, queued: 0, completed: 51, skipped: 0, failed: 0, retryable_failed: 0 }
    mocks.getTaskDetails
      .mockResolvedValueOnce({
        task: { id: 'task-1' }, item_counts: counts,
        items: [{ item_id: 'post:1', post_id: 1, status: 'completed', attempts: 1, updated_at: '2026-07-16T02:00:00Z' }],
        next_cursor: 'page-2',
      })
      .mockResolvedValueOnce({
        task: { id: 'task-1' }, item_counts: counts,
        items: [{ item_id: 'post:51', post_id: 51, status: 'completed', attempts: 1, updated_at: '2026-07-16T02:01:00Z' }],
      })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))
    expect(await view.findByText('Post #1')).toBeVisible()
    await fireEvent.click(view.getByRole('button', { name: '下一页' }))

    expect(mocks.getTaskDetails).toHaveBeenLastCalledWith(
      'task-1', { itemCursor: 'page-2', itemLimit: 50 }, expect.any(AbortSignal),
    )
    expect(await view.findByText('Post #51')).toBeVisible()
    expect(view.queryByText('Post #1')).not.toBeInTheDocument()
  })

  it('filters task items by status and resets to a replaced first page', async () => {
    mocks.taskKind = 'download'
    mocks.taskStatus = 'failed'
    const counts = { total: 2, queued: 0, completed: 1, skipped: 0, failed: 1, retryable_failed: 1 }
    mocks.getTaskDetails
      .mockResolvedValueOnce({
        task: { id: 'task-1' }, item_counts: counts,
        items: [{ item_id: 'post:1', post_id: 1, status: 'completed', attempts: 1, updated_at: '2026-07-16T02:00:00Z' }],
      })
      .mockResolvedValueOnce({
        task: { id: 'task-1' }, item_counts: counts,
        items: [{ item_id: 'post:2', post_id: 2, status: 'failed', attempts: 1, error: { code: 'http_500', message: '上游错误', retryable: true }, updated_at: '2026-07-16T02:01:00Z' }],
      })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))
    expect(await view.findByText('Post #1')).toBeVisible()
    await fireEvent.update(view.getByRole('combobox', { name: '任务项目状态' }), 'failed')

    expect(mocks.getTaskDetails).toHaveBeenLastCalledWith(
      'task-1', { itemStatus: 'failed', itemLimit: 50 }, expect.any(AbortSignal),
    )
    expect(await view.findByText('Post #2')).toBeVisible()
    expect(view.queryByText('Post #1')).not.toBeInTheDocument()
  })

  it('aborts stale detail requests and refreshes the first page when the task revision changes', async () => {
    mocks.taskKind = 'download'
    mocks.taskStatus = 'running'
    let firstSignal: AbortSignal | undefined
    mocks.getTaskDetails
      .mockImplementationOnce((...args: unknown[]) => {
        firstSignal = args[2] as AbortSignal
        return new Promise(() => undefined)
      })
      .mockResolvedValueOnce({
        task: { id: 'task-1', revision: 4 },
        item_counts: { total: 1, queued: 0, completed: 1, skipped: 0, failed: 0, retryable_failed: 0 },
        items: [{ item_id: 'post:2', post_id: 2, status: 'completed', attempts: 1, updated_at: '2026-07-16T02:01:00Z' }],
      })
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '查看详情' }))
    await waitFor(() => expect(mocks.getTaskDetails).toHaveBeenCalledTimes(1))
    mocks.taskRevision = 4
    await fireEvent.click(view.getByRole('button', { name: '进行中' }))

    await waitFor(() => expect(mocks.getTaskDetails).toHaveBeenCalledTimes(2))
    expect(firstSignal?.aborted).toBe(true)
    expect(await view.findByText('Post #2')).toBeVisible()
  })

  it('repeats a saved download request and returns to live task state', async () => {
    const repeatRequest = {
      type: 'download',
      source: { type: 'query', query: 'landscape rating:g' },
      root_id: 'root-1',
      limit: 20,
      concurrency: 8,
      filename_template: '{id}_score_{score}.{ext}',
      skip_existing: true,
      media_policy: { original: true, ugoira: 'webm_and_zip' },
    }
    mocks.getDownloadHistory.mockResolvedValue({
      items: [{
        id: 'history-repeat',
        task_id: 'download-old',
        status: 'completed',
        source_label: 'landscape rating:g',
        created_at: '2026-07-16T01:00:00Z',
        finished_at: '2026-07-16T01:02:00Z',
        total_items: 20,
        completed_items: 20,
        skipped_items: 0,
        failed_items: 0,
        bytes_processed: 4096,
        can_repeat: true,
        repeat_request: repeatRequest,
      }],
    })
    mocks.createTask.mockResolvedValue({ id: 'download-new' })
    mocks.loadSnapshot.mockResolvedValue(undefined)
    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    await fireEvent.click(view.getByRole('button', { name: '下载记录' }))
    await fireEvent.click(await view.findByRole('button', { name: '再次下载' }))

    expect(mocks.createTask).toHaveBeenCalledWith(repeatRequest)
    expect(mocks.loadSnapshot).toHaveBeenCalled()
    await waitFor(() => expect(view.getByRole('button', { name: '实时任务' })).toHaveClass('active'))
  })

  it('shows a pending pause without claiming that the task is already paused', () => {
    mocks.taskStatus = 'pausing'

    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(view.getByText('正在暂停')).toBeVisible()
    expect(view.queryByRole('button', { name: '恢复' })).not.toBeInTheDocument()
  })

  it('shows a pending cancellation without claiming that the task is cancelled', () => {
    mocks.taskStatus = 'cancelling'

    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(view.getByText('正在取消')).toBeVisible()
    expect(view.queryByRole('button', { name: '重试失败项' })).not.toBeInTheDocument()
  })

  it('does not offer retry for a cancelled task', () => {
    mocks.taskStatus = 'cancelled'

    const view = render(TasksView, {
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })

    expect(view.getByText('已取消')).toBeVisible()
    expect(view.queryByRole('button', { name: '重试失败项' })).not.toBeInTheDocument()
  })
})
