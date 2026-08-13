import { expect, test } from '@playwright/test'

interface Envelope<Data> {
  data: Data
}

interface TaskSummary {
  id: string
  status: string
  failures: Array<{ code: string; message: string }>
}

test('the isolated Rust backend reports a healthy SQLite connection', async ({ request }) => {
  const response = await request.get('/api/health')

  expect(response.status()).toBe(200)
  await expect(response.json()).resolves.toMatchObject({
    data: { status: 'ok', database: 'ok' },
  })
})

test('the Rust server refreshes the explore route with production security headers', async ({ request }) => {
  const response = await request.get('/explore', { headers: { Accept: 'text/html' } })

  expect(response.status()).toBe(200)
  expect(await response.text()).toContain('<div id="app"></div>')
  expect(response.headers()['content-security-policy']).toContain("default-src 'self'")
  expect(response.headers()['x-content-type-options']).toBe('nosniff')
  expect(response.headers()['x-frame-options']).toBe('DENY')
  expect(response.headers()['referrer-policy']).toBe('no-referrer')
  expect(response.headers()['access-control-allow-origin']).toBeUndefined()
})

test('a real root can be indexed, listed, ranged and observed in history', async ({ request }) => {
  const mediaDir = process.env.REAL_E2E_MEDIA_DIR
  if (!mediaDir) throw new Error('REAL_E2E_MEDIA_DIR is required')
  const rootResponse = await request.post('/api/library/roots', {
    data: {
      name: 'Real E2E media',
      windows_path: process.env.REAL_E2E_BACKEND_PLATFORM === 'windows' ? mediaDir : null,
      linux_path: process.env.REAL_E2E_BACKEND_PLATFORM === 'windows' ? null : mediaDir,
    },
  })
  expect(rootResponse.status()).toBe(201)
  const root = (await rootResponse.json() as Envelope<{ id: string }>).data

  const taskResponse = await request.post('/api/tasks', {
    data: { type: 'index_library', root_id: root.id },
  })
  expect(taskResponse.status()).toBe(201)
  const createdTask = (await taskResponse.json() as Envelope<TaskSummary>).data

  let indexedTask: TaskSummary | undefined
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const snapshotResponse = await request.get('/api/tasks')
    expect(snapshotResponse.status()).toBe(200)
    const snapshot = await snapshotResponse.json() as Envelope<{ tasks: TaskSummary[] }>
    indexedTask = snapshot.data.tasks.find((task) => task.id === createdTask.id)
    if (indexedTask && ['completed', 'failed', 'cancelled'].includes(indexedTask.status)) break
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
  expect(indexedTask?.status, JSON.stringify(indexedTask?.failures ?? [])).toBe('completed')

  const libraryResponse = await request.get(`/api/library/items?root_id=${encodeURIComponent(root.id)}&limit=60`)
  expect(libraryResponse.status()).toBe(200)
  const library = await libraryResponse.json() as Envelope<{
    total: number
    items: Array<{ id: string; relative_path: string; post_id?: number; tags: string[]; size_bytes: number }>
  }>
  expect(library.data.total).toBe(1)
  expect(library.data.items).toHaveLength(1)
  expect(library.data.items[0]).toMatchObject({
    relative_path: '123_score_5.png',
    post_id: 123,
    tags: expect.arrayContaining(['cat', 'test_fixture']),
  })

  const media = library.data.items[0]
  const rangeResponse = await request.get(`/api/library/media/${encodeURIComponent(media.id)}/file`, {
    headers: { Range: 'bytes=0-9' },
  })
  expect(rangeResponse.status()).toBe(206)
  expect(rangeResponse.headers()['accept-ranges']).toBe('bytes')
  expect(rangeResponse.headers()['content-range']).toBe(`bytes 0-9/${media.size_bytes}`)
  expect((await rangeResponse.body()).byteLength).toBe(10)

  const historyResponse = await request.get('/api/downloads/history?limit=20')
  expect(historyResponse.status()).toBe(200)
  const history = await historyResponse.json() as Envelope<{ items: unknown[] }>
  expect(Array.isArray(history.data.items)).toBe(true)
})
