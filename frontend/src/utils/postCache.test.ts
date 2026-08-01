import { beforeEach, describe, expect, it } from 'vitest'
import type { DanbooruPost } from '../api'
import { cachePosts, getCachedPost } from './postCache'

function makePost(id: number): DanbooruPost {
  return {
    id,
    rating: 's',
    score: id,
    fav_count: 0,
    image_width: 100,
    image_height: 100,
    file_ext: 'jpg',
    file_size: 100,
    is_video: false,
    is_ugoira: false,
    restricted: false,
    downloaded: false,
    tags: { general: [`tag_${id}`], artist: [], copyright: [], character: [], meta: [] },
  }
}

describe('post cache', () => {
  beforeEach(() => localStorage.clear())

  it('returns fresh posts and removes expired entries', () => {
    cachePosts([makePost(1)], { now: 1_000, ttlMs: 500 })

    expect(getCachedPost(1, { now: 1_499, ttlMs: 500 })?.id).toBe(1)
    expect(getCachedPost(1, { now: 1_501, ttlMs: 500 })).toBeNull()
  })

  it('evicts the least recently used post when the entry limit is reached', () => {
    cachePosts([makePost(1)], { now: 1_000, maxEntries: 2 })
    cachePosts([makePost(2)], { now: 1_001, maxEntries: 2 })
    cachePosts([makePost(3)], { now: 1_002, maxEntries: 2 })

    expect(getCachedPost(1, { now: 1_003, maxEntries: 2 })).toBeNull()
    expect(getCachedPost(2, { now: 1_003, maxEntries: 2 })?.id).toBe(2)
    expect(getCachedPost(3, { now: 1_003, maxEntries: 2 })?.id).toBe(3)
  })
})
