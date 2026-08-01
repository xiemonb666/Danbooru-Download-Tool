export const BATCH_DOWNLOAD_HISTORY_KEY = 'danbooru-batch-download-history-v1'
const MAX_HISTORY_ITEMS = 10

export interface BatchDownloadSettings {
  includeTags: string
  excludeTags: string
  minimumScore: number
  minimumResolution?: number
  limit: number
  prioritizeScore: boolean
  prioritizeResolution: boolean
  keepSidecarTxt?: boolean
  staticImagesOnly?: boolean
  rootId: string
  directory: string
  savedAt: number
}

export type BatchDownloadSettingsInput = Omit<BatchDownloadSettings, 'savedAt'>

interface StorageAdapter {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

interface SaveOptions {
  storage?: StorageAdapter | null
  now?: number
}

function browserStorage(): StorageAdapter | null {
  if (typeof window === 'undefined') return null
  try {
    return window.localStorage
  } catch {
    return null
  }
}

function isHistoryItem(value: unknown): value is BatchDownloadSettings {
  if (!value || typeof value !== 'object') return false
  const item = value as Record<string, unknown>
  return typeof item.includeTags === 'string'
    && typeof item.excludeTags === 'string'
    && Number.isFinite(item.minimumScore)
    && (item.minimumResolution === undefined || Number.isFinite(item.minimumResolution))
    && Number.isFinite(item.limit)
    && typeof item.prioritizeScore === 'boolean'
    && typeof item.prioritizeResolution === 'boolean'
    && (item.keepSidecarTxt === undefined || typeof item.keepSidecarTxt === 'boolean')
    && (item.staticImagesOnly === undefined || typeof item.staticImagesOnly === 'boolean')
    && typeof item.rootId === 'string'
    && typeof item.directory === 'string'
    && Number.isFinite(item.savedAt)
}

function sameSettings(left: BatchDownloadSettings, right: BatchDownloadSettingsInput): boolean {
  return left.includeTags === right.includeTags
    && left.excludeTags === right.excludeTags
    && left.minimumScore === right.minimumScore
    && (left.minimumResolution ?? 0) === (right.minimumResolution ?? 0)
    && left.limit === right.limit
    && left.prioritizeScore === right.prioritizeScore
    && left.prioritizeResolution === right.prioritizeResolution
    && (left.keepSidecarTxt ?? true) === (right.keepSidecarTxt ?? true)
    && (left.staticImagesOnly ?? false) === (right.staticImagesOnly ?? false)
    && left.rootId === right.rootId
    && left.directory === right.directory
}

export function loadBatchDownloadHistory(storage = browserStorage()): BatchDownloadSettings[] {
  if (!storage) return []
  try {
    const raw = storage.getItem(BATCH_DOWNLOAD_HISTORY_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter(isHistoryItem)
      .sort((left, right) => right.savedAt - left.savedAt)
      .slice(0, MAX_HISTORY_ITEMS)
  } catch {
    return []
  }
}

export function saveBatchDownloadHistory(
  settings: BatchDownloadSettingsInput,
  options: SaveOptions = {},
): BatchDownloadSettings[] {
  const storage = options.storage ?? browserStorage()
  const entry: BatchDownloadSettings = {
    ...settings,
    savedAt: options.now ?? Date.now(),
  }
  const next = [
    entry,
    ...loadBatchDownloadHistory(storage).filter((saved) => !sameSettings(saved, settings)),
  ].slice(0, MAX_HISTORY_ITEMS)
  if (!storage) return next
  try {
    storage.setItem(BATCH_DOWNLOAD_HISTORY_KEY, JSON.stringify(next))
  } catch {
    // Storage can be unavailable in private browsing; the current entry remains usable in memory.
  }
  return next
}
