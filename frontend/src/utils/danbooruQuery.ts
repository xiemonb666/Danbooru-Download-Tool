export interface DanbooruQuickFilters {
  rating?: '' | 'g' | 's' | 'q' | 'e'
  order?: '' | 'id' | 'score' | 'favcount' | 'random'
  format?: '' | 'jpg' | 'png' | 'webp' | 'gif' | 'avif' | 'mp4' | 'webm'
  minimumMegapixels?: '' | '1' | '2' | '4' | '8'
}

export function composeDanbooruQuery(query: string, filters: DanbooruQuickFilters): string {
  const nativeQuery = query.trim()
  const customOrder = filters.order && filters.order !== 'id'
  const parts = [nativeQuery]
  if (!nativeQuery && customOrder) parts.push('age:<1month')
  if (filters.rating) parts.push(`rating:${filters.rating}`)
  if (customOrder) parts.push(`order:${filters.order}`)
  if (filters.format) parts.push(`filetype:${filters.format}`)
  if (filters.minimumMegapixels) parts.push(`mpixels:>=${filters.minimumMegapixels}`)
  return parts.filter(Boolean).join(' ')
}

export function composeTagDownloadQuery(includeQuery: string, excludedTags: string): string {
  const exclusions = splitBatchTags(excludedTags)
    .map((tag) => tag.replace(/^-+/, ''))
    .filter(Boolean)
    .map((tag) => `-${tag}`)
  return [includeQuery.trim(), ...exclusions].filter(Boolean).join(' ')
}

export function splitBatchTags(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((tag) => tag.trim())
    .filter(Boolean)
}

export interface BatchDownloadQueryOptions {
  tags: string
  excludedTags: string
  minimumScore: number
  minimumResolution?: number
  prioritizeScore: boolean
  prioritizeResolution?: boolean
}

export function composeBatchDownloadQuery(options: BatchDownloadQueryOptions): string {
  const base = composeTagDownloadQuery(options.tags, options.excludedTags)
  const minimumScore = Number.isFinite(options.minimumScore) ? Math.trunc(options.minimumScore) : 0
  const minimumResolution = Number.isFinite(options.minimumResolution)
    ? Math.max(0, Math.trunc(options.minimumResolution ?? 0))
    : 0
  // The downloader applies requested priorities locally. Remote order metatags can time out on
  // broad searches, even when the rest of the query is valid.
  return [
    base,
    `score:>=${minimumScore}`,
    minimumResolution > 0 ? `width:>=${minimumResolution}` : '',
    minimumResolution > 0 ? `height:>=${minimumResolution}` : '',
  ].filter(Boolean).join(' ')
}
