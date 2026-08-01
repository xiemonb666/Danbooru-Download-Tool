import { describe, expect, it } from 'vitest'
import {
  loadBatchDownloadHistory,
  saveBatchDownloadHistory,
  type BatchDownloadSettings,
} from './batchDownloadHistory'

function settings(overrides: Partial<BatchDownloadSettings> = {}): BatchDownloadSettings {
  return {
    includeTags: '1girl solo',
    excludeTags: 'comic',
    minimumScore: 20,
    limit: 100,
    prioritizeScore: true,
    prioritizeResolution: true,
    rootId: 'library-a',
    directory: 'batch',
    savedAt: 0,
    ...overrides,
  }
}

describe('batch download history', () => {
  it('keeps the newest distinct setting first and refreshes matching settings', () => {
    const storage = new Map<string, string>()
    const adapter = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    }

    saveBatchDownloadHistory(settings(), { storage: adapter, now: 100 })
    saveBatchDownloadHistory(settings({ includeTags: 'landscape' }), { storage: adapter, now: 200 })
    const history = saveBatchDownloadHistory(settings(), { storage: adapter, now: 300 })

    expect(history).toHaveLength(2)
    expect(history[0]).toMatchObject({ includeTags: '1girl solo', savedAt: 300 })
    expect(history[1]).toMatchObject({ includeTags: 'landscape', savedAt: 200 })
    expect(loadBatchDownloadHistory(adapter)).toEqual(history)
  })
})
