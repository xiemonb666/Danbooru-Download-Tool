import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page, type Route } from '@playwright/test'

const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAF/gL+XyZT8QAAAABJRU5ErkJggg==', 'base64')

const postDimensions = [[800, 1200], [1200, 800], [800, 1600], [1600, 900]] as const
const posts = ['q', 's', 'e', 'g'].map((rating, index) => ({
  id: index + 1,
  rating,
  score: 100 - index,
  fav_count: 20 + index,
  image_width: postDimensions[index]?.[0] ?? 800,
  image_height: postDimensions[index]?.[1] ?? 1200,
  file_ext: index === 2 ? 'zip' : index === 3 ? 'mp4' : 'jpg',
  file_size: 400_000,
  is_video: index === 3,
  is_ugoira: index === 2,
  restricted: false,
  downloaded: false,
  tags: { general: ['cat'], artist: [], copyright: [], character: [], meta: [] },
}))

interface MockApiOptions {
  initialTasks?: object[]
  librarySize?: number
}

interface MockApiControls {
  setOnline(value: boolean): void
  createdTaskRequests: Array<Record<string, unknown>>
}

async function mockApi(page: Page, options: MockApiOptions = {}): Promise<MockApiControls> {
  const initialTasks = options.initialTasks ?? []
  const librarySize = options.librarySize ?? 0
  const libraryItems = Array.from({ length: Math.min(60, librarySize) }, (_, index) => ({
    id: `media-${index + 1}`,
    root_id: 'root-1',
    post_id: index + 1,
    filename: `${index + 1}_score_5.jpg`,
    relative_path: `${index + 1}_score_5.jpg`,
    mime_type: 'image/jpeg',
    width: 800,
    height: 1200,
    duration: null,
    size_bytes: 400_000,
    rating: 'g',
    tags: ['cat', `fixture_${index + 1}`],
    post_created_at: '2026-01-01T08:30:00Z',
    created_at: '2026-01-01T00:00:00Z',
  }))
  let online = true
  let createdTask: object | null = null
  let visibleTasks = [...initialTasks]
  const createdTaskRequests: Array<Record<string, unknown>> = []
  let quarantineEntries = [{
    id: 'q-1', root_id: 'root-1', original_relative_path: 'duplicates/cat.jpg',
    quarantine_relative_path: '.danbooru-quarantine/q-1/cat.jpg', size_bytes: 120_000,
    reason: 'exact_duplicate', created_at: '2026-01-01T00:00:00Z',
  }]
  await page.route('**/api/**', async (route: Route) => {
    const url = new URL(route.request().url())
    const path = url.pathname
    if (!online) {
      await route.abort('connectionrefused')
      return
    }
    if (path.includes('/media/') || path.includes('/library/media/')) {
      await route.fulfill({ status: 200, contentType: 'image/png', body: png })
      return
    }
    if (path === '/api/health') {
      await route.fulfill({ json: { data: { status: 'ok', version: '3.0.0', database: 'ok', uptime_seconds: 12 } } })
      return
    }
    if (path === '/api/vllm/health') {
      await route.fulfill({ json: { data: { available: true, models: ['local/vision-model'], message: 'vLLM 可用，发现 1 个模型' } } })
      return
    }
    if (path === '/api/config') {
      await route.fulfill({ json: { data: {
        danbooru_username: '', danbooru_api_key_configured: false, vllm_api_key_configured: false,
        vllm_base_url: 'http://127.0.0.1:8000/v1', vllm_allowed_hosts: [], proxy_url: null,
        vllm_language: 'danbooru', vllm_max_tags: 60, vllm_max_length: 400,
        vllm_verify_danbooru: true, vllm_reference_existing: false,
        download_concurrency: 8, filename_template: '{id}_score_{score}.{ext}', ugoira_policy: 'webm_and_zip', blur_sensitive_media: true,
      } } })
      return
    }
    if (path === '/api/tasks') {
      if (route.request().method() === 'POST') {
        const request = route.request().postDataJSON() as Record<string, unknown>
        createdTaskRequests.push(request)
        const kind = typeof request.type === 'string' ? request.type : 'download'
        createdTask = {
          id: 'task-1', kind, status: 'queued', revision: 1,
          title: kind === 'vllm_tag' ? '视觉模型打标' : '下载所选媒体',
          progress: { completed: 0, total: 2, bytes_downloaded: 0, speed_bytes_per_sec: 0 }, failures: [],
          created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
        }
        await route.fulfill({ json: { data: createdTask } })
      } else {
        await route.fulfill({ json: { data: { tasks: createdTask ? [createdTask] : visibleTasks, last_event_id: 0 } } })
      }
      return
    }
    if (path === '/api/tasks/events') {
      await route.fulfill({ status: 200, contentType: 'text/event-stream', body: ': connected\n\n' })
      return
    }
    if (path === '/api/training/adapters') {
      await route.fulfill({ json: { data: [{ id: 'sdxl-lora', label: 'SDXL LoRA', version: 'fixture', trainer: 'train.py', groups: [], fields: [] }] } })
      return
    }
    if (path === '/api/training/runtime-profiles') {
      await route.fulfill({ json: { data: [{ id: 'windows', label: 'Windows', kind: 'windows', managed: true, installed: true, installing: false, runtime_root: 'C:/runtime', python_path: 'C:/runtime/python.exe' }] } })
      return
    }
    if (path === '/api/training/gpus') {
      await route.fulfill({ json: { data: [] } })
      return
    }
    if (path === '/api/training/queue') {
      await route.fulfill({ json: { data: { entries: [] } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/metrics\/overview$/.test(path)) {
      await route.fulfill({ json: { data: { cursor: 240, series: [
        { series: 'loss', count: 3, first: { series: 'loss', step: 1, timestamp: 1_700_000_000, value: 0.9 }, latest: { series: 'loss', step: 3, timestamp: 1_700_000_020, value: 0.6 }, minimum: { series: 'loss', step: 3, timestamp: 1_700_000_020, value: 0.6 }, maximum: { series: 'loss', step: 1, timestamp: 1_700_000_000, value: 0.9 } },
        { series: 'train.max_steps', count: 1, first: { series: 'train.max_steps', step: 0, timestamp: 1_700_000_000, value: 10 }, latest: { series: 'train.max_steps', step: 0, timestamp: 1_700_000_000, value: 10 }, minimum: { series: 'train.max_steps', step: 0, timestamp: 1_700_000_000, value: 10 }, maximum: { series: 'train.max_steps', step: 0, timestamp: 1_700_000_000, value: 10 } },
      ] } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/metrics$/.test(path)) {
      await route.fulfill({ json: { data: { cursor: 240, metrics: [
        { series: 'loss', step: 1, timestamp: 1_700_000_000, value: 0.9 },
        { series: 'loss', step: 2, timestamp: 1_700_000_010, value: 0.72 },
        { series: 'loss', step: 3, timestamp: 1_700_000_020, value: 0.6 },
      ] } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/events$/.test(path)) {
      await route.fulfill({ status: 200, contentType: 'text/event-stream', body: ': connected\n\n' })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/artifacts$/.test(path)) {
      await route.fulfill({ json: { data: { artifacts: [] } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/logs$/.test(path)) {
      await route.fulfill({ json: { data: { text: 'fixture training log', cursor: 20, truncated: false } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+\/cleanup-preview$/.test(path)) {
      await route.fulfill({ json: { data: { deletable: [{ kind: 'owned_output', path: 'C:/outputs/training-1', file_count: 4, bytes: 4096 }], retained: [] } } })
      return
    }
    if (/^\/api\/training\/tasks\/[^/]+$/.test(path) && route.request().method() === 'DELETE') {
      const id = path.split('/').slice(-1)[0]
      visibleTasks = visibleTasks.filter((task) => (task as { id?: string }).id !== id)
      await route.fulfill({ json: { data: { task_id: id, deleted: [{ kind: 'owned_output', path: 'C:/outputs/training-1', file_count: 4, bytes: 4096 }], retained: [] } } })
      return
    }
    if (path === '/api/library/roots') {
      await route.fulfill({ json: { data: [{
        id: 'root-1', name: '素材库', windows_path: 'C:\\Media', linux_path: '/mnt/c/Media',
        indexed: true, media_count: 0, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
      }] } })
      return
    }
    if (path === '/api/library/roots/root-1/directories') {
      if (route.request().method() === 'POST') {
        const request = route.request().postDataJSON() as { relative_path: string }
        await route.fulfill({ json: { data: { relative_path: request.relative_path } } })
      } else {
        await route.fulfill({ json: { data: { directories: ['人物/爱丽丝', '项目/角色图'], truncated: false } } })
      }
      return
    }
    if (path === '/api/library/items') {
      await route.fulfill({ json: { data: {
        items: libraryItems,
        total: librarySize,
        next_cursor: librarySize > libraryItems.length ? 'cursor-60' : null,
      } } })
      return
    }
    if (path === '/api/library/facets') {
      await route.fulfill({ json: { data: {
        catalog_revision: 1,
        total: librarySize,
        score_ranges: [],
        resolution_ranges: [],
      } } })
      return
    }
    if (path === '/api/library/quarantine') {
      if (route.request().method() === 'DELETE') {
        const purged = quarantineEntries.length
        quarantineEntries = []
        await route.fulfill({ json: { data: { purged } } })
      } else {
        await route.fulfill({ json: { data: quarantineEntries } })
      }
      return
    }
    if (/\/api\/library\/quarantine\/[^/]+\/restore$/.test(path)) {
      const restored = quarantineEntries[0]
      quarantineEntries = []
      await route.fulfill({ json: { data: restored } })
      return
    }
    if (path === '/api/danbooru/posts') {
      await route.fulfill({ json: { data: { posts, page: 1, total: 4 } } })
      return
    }
    const postDetail = path.match(/^\/api\/danbooru\/posts\/(\d+)$/)
    if (postDetail) {
      const post = posts.find((candidate) => candidate.id === Number(postDetail[1]))
      await route.fulfill(post
        ? { json: { data: { ...post, duration: post.is_video ? 12.5 : null, source: 'https://example.test/source' } } }
        : { status: 404, json: { error: { code: 'post_not_found', message: 'not found', retryable: false }, request_id: 'e2e' } })
      return
    }
    if (path === '/api/danbooru/autocomplete') {
      const query = url.searchParams.get('q') ?? ''
      await route.fulfill({ json: { data: query.startsWith('cat_') ? [{
        value: 'cat_ears', label: 'cat ears', category: 'general', post_count: 1234,
      }] : [] } })
      return
    }
    await route.fulfill({ status: 404, json: { error: { code: 'not_found', message: 'not found', retryable: false }, request_id: 'e2e' } })
  })
  return { setOnline(value: boolean) { online = value }, createdTaskRequests }
}

test('every product area remains bounded across supported viewport widths', async ({ page }) => {
  await mockApi(page)
  for (const width of [390, 1024, 1440, 2560]) {
    await page.setViewportSize({ width, height: 900 })
    for (const [path, heading] of [
      ['/explore?q=cat', '探索与下载'], ['/tasks', '任务中心'], ['/library', '本地图库'],
      ['/tools', '处理与隔离'], ['/settings', '设置'],
    ] as const) {
      await page.goto(path)
      await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible()
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
    }
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze()
    expect(results.violations).toEqual([])
  }
})

test('every product area passes the desktop accessibility gate', async ({ page }) => {
  await mockApi(page)
  for (const [path, heading] of [
    ['/explore', '探索与下载'], ['/tasks', '任务中心'], ['/library', '本地图库'],
    ['/tools', '处理与隔离'], ['/settings', '设置'],
  ] as const) {
    await page.goto(path)
    await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible()
    await page.waitForTimeout(200)
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze()
    expect(results.violations).toEqual([])
  }
})

test('mixed image ratios pack vertically without grid-row holes', async ({ page }) => {
  await mockApi(page)
  await page.setViewportSize({ width: 520, height: 1000 })
  await page.goto('/explore')
  const cards = page.locator('.post-card')
  await expect(cards).toHaveCount(4)

  const boxes = (await cards.evaluateAll((elements) => elements.map((element) => {
    const box = element.getBoundingClientRect()
    return { top: box.top, right: box.right, bottom: box.bottom, left: box.left }
  }))).sort((left, right) => left.top - right.top || left.left - right.left)
  const firstTop = boxes[0]?.top ?? 0

  for (const card of boxes.filter((box) => box.top > firstTop + 1)) {
    const nearestAbove = boxes
      .filter((candidate) => candidate.bottom <= card.top + 1
        && candidate.left < card.right - 1
        && candidate.right > card.left + 1)
      .sort((left, right) => right.bottom - left.bottom)[0]
    expect(nearestAbove).toBeDefined()
    expect(card.top - (nearestAbove?.bottom ?? card.top)).toBeLessThanOrEqual(18)
  }
})

test('a non-root route survives a direct load and refresh', async ({ page }) => {
  await mockApi(page)
  await page.goto('/library')
  await expect(page.getByRole('heading', { name: '本地图库' })).toBeVisible()

  await page.reload()

  await expect(page).toHaveURL(/\/library(?:\?|$)/)
  await expect(page.getByLabel('图库文件夹')).toHaveValue('人物/爱丽丝')
  await expect(page.getByRole('heading', { name: '本地图库' })).toBeVisible()
})

test('query, reveal, select and enqueue download remain one coherent flow', async ({ page }) => {
  await mockApi(page)
  await page.goto('/explore?q=cat')
  const questionable = page.getByRole('img', { name: 'Danbooru 帖子 1' })
  const sensitive = page.getByRole('img', { name: 'Danbooru 帖子 2' })

  await expect(questionable).toHaveClass(/media-obscured/)
  await expect(sensitive).not.toHaveClass(/media-obscured/)
  await page.getByRole('button', { name: '显示敏感内容' }).first().click()
  await expect(questionable).not.toHaveClass(/media-obscured/)
  await page.getByRole('button', { name: '打开帖子 2' }).click()
  await expect(page.getByRole('dialog', { name: /帖子 #2/ })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog', { name: /帖子 #2/ })).toHaveCount(0)
  await page.getByRole('button', { name: '选择帖子 1' }).click()
  await page.getByRole('button', { name: '选择帖子 2' }).click()
  await expect(page.getByText('已选择 2 项')).toBeVisible()

  await page.getByRole('button', { name: /下载所选/ }).click()

  await expect(page).toHaveURL(/\/explore(?:\?|$)/)
  const taskOverview = page.getByRole('link', { name: '查看进行中的任务' })
  await expect(taskOverview).toBeVisible()
  await expect(taskOverview).toContainText('1 个任务进行中')
})

test('legacy-style tag conditions enqueue a bounded batch download', async ({ page }) => {
  const backend = await mockApi(page)
  await page.goto('/explore?q=cat')

  await page.getByRole('tab', { name: '标签批量下载' }).click()
  await page.getByLabel('包含标签').fill('cat_')
  await page.getByRole('option', { name: /cat ears/ }).click()
  await expect(page.getByLabel('包含标签')).toHaveValue('cat_ears')
  await page.getByLabel('包含标签').fill('cat_ears solo')
  await page.getByLabel('排除标签').fill('animated, lowres')
  await page.getByLabel('最低评分').fill('15')
  await page.getByLabel('最大下载数量').fill('250')
  await page.getByRole('checkbox', { name: /评分优先排序/ }).check()
  await page.getByLabel('库内文件夹').selectOption('项目/角色图')
  // Priorities are applied locally by the downloader so broad Danbooru queries do
  // not depend on the expensive remote `order:score` metatag.
  await expect(page.getByText('cat_ears solo -animated -lowres score:>=15')).toBeVisible()

  await page.getByRole('button', { name: '开始批量下载' }).click()

  await expect.poll(() => backend.createdTaskRequests.length).toBe(1)
  expect(backend.createdTaskRequests[0]).toMatchObject({
    type: 'download',
    source: { type: 'query', query: 'cat_ears solo -animated -lowres score:>=15' },
    root_id: 'root-1',
    relative_directory: '项目/角色图',
    limit: 250,
    skip_existing: true,
    prioritize_score: true,
  })
})

test('video and ugoira previews require an explicit play action', async ({ page }) => {
  await mockApi(page)
  await page.goto('/explore')

  const ugoiraCard = page.getByRole('button', { name: '打开帖子 3' }).locator('..')
  await ugoiraCard.getByRole('button', { name: '显示敏感内容' }).click()
  await page.getByRole('button', { name: '打开帖子 3' }).click()
  const ugoira = page.locator('.detail-panel video')
  await expect(ugoira).toHaveAttribute('controls', '')
  await expect(ugoira).not.toHaveAttribute('autoplay', '')
  await expect(ugoira).toHaveAttribute('src', /ugoira_webm$/)

  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: '打开帖子 4' }).click()
  const video = page.locator('.detail-panel video')
  await expect(video).toHaveAttribute('controls', '')
  await expect(video).not.toHaveAttribute('autoplay', '')
  await expect(video).toHaveAttribute('src', /original$/)
})

test('a 10,000 item library keeps the first page and DOM bounded', async ({ page }) => {
  await mockApi(page, { librarySize: 10_000 })
  await page.goto('/library')

  await expect(page.getByText('10,000 项本地媒体')).toBeVisible()
  await expect(page.locator('.library-card')).toHaveCount(60)
  expect(await page.locator('body *').count()).toBeLessThan(1_200)
})

test('library filters, cursor, selection and scroll survive navigation away and back', async ({ page }) => {
  await mockApi(page, { librarySize: 120 })
  await page.setViewportSize({ width: 1024, height: 700 })
  await page.goto('/library')

  await page.getByLabel('图库文件夹').selectOption('人物/爱丽丝')
  await page.getByPlaceholder(/精确标签/).fill('cat')
  await page.getByRole('button', { name: '搜索' }).click()
  await page.getByRole('checkbox', { name: '选择 1_score_5.jpg' }).check()
  await page.getByRole('button', { name: '下一批' }).click()
  await expect(page.getByText('第 2 批')).toBeVisible()
  await page.evaluate(() => window.scrollTo(0, 900))
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(500)

  await page.getByRole('link', { name: '任务', exact: true }).click()
  await expect(page.getByRole('heading', { name: '任务中心' })).toBeVisible()
  await page.getByRole('link', { name: '图库', exact: true }).click()

  await expect(page.getByLabel('图库文件夹')).toHaveValue('人物/爱丽丝')
  await expect(page.getByPlaceholder(/精确标签/)).toHaveValue('cat')
  await expect(page.getByText('第 2 批')).toBeVisible()
  await expect(page.getByRole('checkbox', { name: '选择 1_score_5.jpg' })).toBeChecked()
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(500)
})

test('library post date filters send inclusive local-day boundaries and show publication time', async ({ page }) => {
  const libraryRequests: URL[] = []
  page.on('request', (request) => {
    const url = new URL(request.url())
    if (url.pathname === '/api/library/items') libraryRequests.push(url)
  })
  await mockApi(page, { librarySize: 1 })
  await page.goto('/library')

  const expected = await page.evaluate(() => {
    const from = new Date('2026-01-02T00:00:00')
    const afterTo = new Date('2026-01-04T00:00:00')
    return {
      from: String(Math.floor(from.getTime() / 1000)),
      to: String(Math.floor(afterTo.getTime() / 1000) - 1),
    }
  })
  await page.getByLabel('帖子发布日期起').fill('2026-01-02')
  await page.getByLabel('帖子发布日期止').fill('2026-01-03')

  await expect.poll(() => libraryRequests.some((url) => (
    url.searchParams.get('post_created_from') === expected.from
    && url.searchParams.get('post_created_to') === expected.to
  ))).toBe(true)
  await expect(page.getByText(/帖子发布于/).first()).toBeVisible()
})

test('a static local image can enter the mock vLLM task pipeline by media ID', async ({ page }) => {
  const backend = await mockApi(page, { librarySize: 1 })
  await page.goto('/library')

  await page.getByRole('checkbox', { name: '选择 1_score_5.jpg' }).check()
  await page.getByRole('button', { name: '视觉模型打标所选' }).click()

  await expect.poll(() => backend.createdTaskRequests.length).toBe(1)
  expect(backend.createdTaskRequests[0]).toMatchObject({
    type: 'vllm_tag',
    root_id: 'root-1',
    options: { media_ids: ['media-1'] },
  })
})

test('the visible backend state recovers after a connection loss', async ({ page }) => {
  await page.clock.install()
  const backend = await mockApi(page)
  await page.goto('/settings')
  await expect(page.getByText('本地服务正常').first()).toBeVisible()

  backend.setOnline(false)
  await page.clock.fastForward(15_001)
  await expect(page.getByText('本地服务离线').first()).toBeVisible()

  backend.setOnline(true)
  await page.clock.fastForward(15_001)
  await expect(page.getByText('本地服务正常').first()).toBeVisible()
})

test('a quarantined file can be restored without leaving the tools page', async ({ page }) => {
  await mockApi(page)
  await page.goto('/tools')
  await expect(page.getByText('duplicates/cat.jpg')).toBeVisible()

  await page.getByRole('button', { name: '恢复' }).click()

  await expect(page.getByText('duplicates/cat.jpg')).toHaveCount(0)
  await expect(page.getByText('隔离区为空。应用不会自动清空之后加入的内容。')).toBeVisible()
})

test('two concurrent tasks remain independently visible', async ({ page }) => {
  const baseTask = {
    revision: 2, kind: 'download', status: 'running',
    progress: { completed: 4, total: 20, bytes_downloaded: 2_000_000, speed_bytes_per_sec: 512_000, eta_seconds: 32 },
    failures: [], created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:02Z',
  }
  await mockApi(page, { initialTasks: [
    { ...baseTask, id: 'task-a', title: '下载 landscape' },
    { ...baseTask, id: 'task-b', title: '下载 character', progress: { ...baseTask.progress, completed: 9 } },
  ] })
  await page.goto('/tasks')

  await expect(page.getByText('下载 landscape')).toBeVisible()
  await expect(page.getByText('下载 character')).toBeVisible()
  await expect(page.getByText('500.0 KB/s')).toHaveCount(2)
})

test('training monitor renders Canvas curves without overflow and permanently removes a completed run after preview', async ({ page }) => {
  const browserErrors: string[] = []
  page.on('pageerror', (error) => browserErrors.push(error.message))
  await mockApi(page, { initialTasks: [{
    id: 'training-1', kind: 'training', status: 'completed', revision: 3, title: 'Odette LoRA',
    progress: { completed: 10, total: 10, bytes_downloaded: 0, speed_bytes_per_sec: 0 }, failures: [],
    created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T01:00:00Z',
    training: { adapter_id: 'sdxl-lora', output_name: 'odette', gpu_ids: ['0'] },
  }] })
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.goto('/training?tab=monitor')

  await expect(page.getByText('研究摘要')).toBeVisible()
  await expect(page.locator('.training-echarts-canvas canvas')).toHaveCount(1)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  const smoothing = page.getByLabel('曲线平滑程度')
  await smoothing.evaluate((element) => {
    const input = element as HTMLInputElement
    input.value = '100'
    input.dispatchEvent(new Event('input', { bubbles: true }))
  })
  await expect(page.getByText('100%', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: '删除当前运行' }).click()
  await expect(page.getByRole('dialog', { name: '永久删除训练运行' })).toBeVisible()
  await expect(page.getByText('C:/outputs/training-1')).toBeVisible()
  await page.getByRole('button', { name: '永久删除' }).click()
  await expect(page.getByText('尚无训练记录。创建训练任务后，实时指标会出现在这里。')).toBeVisible()
  expect(browserErrors).toEqual([])
})
