import type { DanbooruPost } from '../api'

const CACHE_KEY = 'danbooru-tool:post-cache:v1'
const DEFAULT_TTL_MS = 24 * 60 * 60 * 1_000
const DEFAULT_MAX_ENTRIES = 300

interface CacheEntry {
  post: DanbooruPost
  accessedAt: number
  expiresAt: number
}

interface CacheEnvelope {
  version: 1
  entries: CacheEntry[]
}

export interface PostCacheOptions {
  storage?: Storage
  now?: number
  ttlMs?: number
  maxEntries?: number
}

function resolveStorage(options: PostCacheOptions): Storage | null {
  if (options.storage) return options.storage
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

function readEntries(storage: Storage): CacheEntry[] {
  try {
    const value = JSON.parse(storage.getItem(CACHE_KEY) ?? '') as Partial<CacheEnvelope>
    if (value.version !== 1 || !Array.isArray(value.entries)) return []
    return value.entries.filter((entry): entry is CacheEntry => Boolean(
      entry
      && typeof entry.accessedAt === 'number'
      && typeof entry.expiresAt === 'number'
      && typeof entry.post?.id === 'number',
    ))
  } catch {
    return []
  }
}

function writeEntries(storage: Storage, entries: CacheEntry[]): void {
  try {
    storage.setItem(CACHE_KEY, JSON.stringify({ version: 1, entries } satisfies CacheEnvelope))
  } catch {
    // Cache quota and privacy-mode failures must never block browsing.
  }
}

function freshEntries(entries: CacheEntry[], now: number, maxEntries: number): CacheEntry[] {
  return entries
    .filter((entry) => entry.expiresAt >= now)
    .sort((left, right) => right.accessedAt - left.accessedAt)
    .slice(0, maxEntries)
}

export function cachePosts(posts: DanbooruPost[], options: PostCacheOptions = {}): void {
  const storage = resolveStorage(options)
  if (!storage || posts.length === 0) return
  const now = options.now ?? Date.now()
  const ttlMs = options.ttlMs ?? DEFAULT_TTL_MS
  const maxEntries = options.maxEntries ?? DEFAULT_MAX_ENTRIES
  const entries = new Map(freshEntries(readEntries(storage), now, maxEntries)
    .map((entry) => [entry.post.id, entry]))
  for (const post of posts) {
    entries.set(post.id, { post, accessedAt: now, expiresAt: now + ttlMs })
  }
  writeEntries(storage, freshEntries([...entries.values()], now, maxEntries))
}

export function getCachedPost(id: number, options: PostCacheOptions = {}): DanbooruPost | null {
  const storage = resolveStorage(options)
  if (!storage) return null
  const now = options.now ?? Date.now()
  const maxEntries = options.maxEntries ?? DEFAULT_MAX_ENTRIES
  const entries = freshEntries(readEntries(storage), now, maxEntries)
  const entry = entries.find((candidate) => candidate.post.id === id)
  if (entry) entry.accessedAt = now
  writeEntries(storage, freshEntries(entries, now, maxEntries))
  return entry?.post ?? null
}

export function prunePostCache(options: PostCacheOptions = {}): void {
  const storage = resolveStorage(options)
  if (!storage) return
  const now = options.now ?? Date.now()
  writeEntries(storage, freshEntries(
    readEntries(storage),
    now,
    options.maxEntries ?? DEFAULT_MAX_ENTRIES,
  ))
}
