export const LIBRARY_VIEW_STATE_KEY = 'danbooru.library-view-state.v1'

export interface LibraryMediaSnapshot {
  id: string
}

export interface LibraryViewState {
  version: 1
  rootId: string
  directory: string
  query: string
  scoreRange: string
  resolutionRange: string
  postCreatedFromDate: string
  postCreatedToDate: string
  cursor: string
  before: boolean
  cursorDepth: number
  scrollY: number
  selectedIds: string[]
  selectedMedia: LibraryMediaSnapshot[]
  allMatchingSelected: boolean
  allMatchingTotal: number
  selectedQuery: string
  selectedScoreMin?: number
  selectedScoreMax?: number
  selectedResolutionMin?: number
  selectedResolutionMax?: number
  selectedPostCreatedFrom?: number
  selectedPostCreatedTo?: number
  excludedMediaIds: string[]
}

type ReadableStorage = Pick<Storage, 'getItem'>
type WritableStorage = Pick<Storage, 'setItem'>
type LibraryRouteQuery = Record<string, string | Array<string | null> | null | undefined>

export interface ResolvedLibraryViewState {
  state: LibraryViewState
  restoreSelection: boolean
  restoreScroll: boolean
}

export function emptyLibraryViewState(): LibraryViewState {
  return {
    version: 1,
    rootId: '',
    directory: '',
    query: '',
    scoreRange: '',
    resolutionRange: '',
    postCreatedFromDate: '',
    postCreatedToDate: '',
    cursor: '',
    before: false,
    cursorDepth: 1,
    scrollY: 0,
    selectedIds: [],
    selectedMedia: [],
    allMatchingSelected: false,
    allMatchingTotal: 0,
    selectedQuery: '',
    excludedMediaIds: [],
  }
}

function browserSessionStorage(): Storage | null {
  try {
    return typeof window === 'undefined' ? null : window.sessionStorage
  } catch {
    return null
  }
}

export function loadLibraryViewState(storage: ReadableStorage | null = browserSessionStorage()): LibraryViewState | null {
  try {
    const raw = storage?.getItem(LIBRARY_VIEW_STATE_KEY)
    if (!raw) return null
    const value: unknown = JSON.parse(raw)
    if (typeof value !== 'object' || value === null || Reflect.get(value, 'version') !== 1) return null
    return sanitizeLibraryViewState(value)
  } catch {
    return null
  }
}

export function saveLibraryViewState(state: unknown, storage: WritableStorage | null = browserSessionStorage()): void {
  try {
    storage?.setItem(LIBRARY_VIEW_STATE_KEY, JSON.stringify(state))
  } catch {
    // Session persistence is best-effort (private browsing and quotas can reject writes).
  }
}

export function resolveLibraryViewState(
  snapshot: LibraryViewState | null,
  query: LibraryRouteQuery,
): ResolvedLibraryViewState {
  const saved = snapshot ?? emptyLibraryViewState()
  const rootId = routeString(query, 'root', saved.rootId)
  const explicitRootChanged = Object.prototype.hasOwnProperty.call(query, 'root') && rootId !== saved.rootId
  const directory = routeString(query, 'directory', explicitRootChanged ? '' : saved.directory)
  const resolvedQuery = routeString(query, 'q', saved.query)
  const scoreRange = routeString(query, 'score', saved.scoreRange)
  const resolutionRange = routeString(query, 'resolution', saved.resolutionRange)
  const postCreatedFromDate = routeString(query, 'post_created_from', saved.postCreatedFromDate)
  const postCreatedToDate = routeString(query, 'post_created_to', saved.postCreatedToDate)
  const sameContext = snapshot !== null
    && rootId === saved.rootId
    && directory === saved.directory
    && resolvedQuery === saved.query
    && scoreRange === saved.scoreRange
    && resolutionRange === saved.resolutionRange
    && postCreatedFromDate === saved.postCreatedFromDate
    && postCreatedToDate === saved.postCreatedToDate
  const cursor = routeString(query, 'cursor', sameContext ? saved.cursor : '')
  const before = routeBoolean(query, 'before', sameContext ? saved.before : false)
  const cursorDepth = routePositiveInteger(query, 'cursor_depth', sameContext ? saved.cursorDepth : 1)
  const samePosition = sameContext
    && cursor === saved.cursor
    && before === saved.before
    && cursorDepth === saved.cursorDepth

  return {
    state: {
      ...saved,
      rootId,
      directory,
      query: resolvedQuery,
      scoreRange,
      resolutionRange,
      postCreatedFromDate,
      postCreatedToDate,
      cursor,
      before,
      cursorDepth,
    },
    restoreSelection: samePosition,
    restoreScroll: samePosition,
  }
}

function sanitizeLibraryViewState(value: object): LibraryViewState {
  const fallback = emptyLibraryViewState()
  const stringArray = (key: string) => {
    const candidate = Reflect.get(value, key)
    return Array.isArray(candidate) ? candidate.filter((item): item is string => typeof item === 'string') : []
  }
  const selectedMedia = Reflect.get(value, 'selectedMedia')
  return {
    version: 1,
    rootId: safeString(value, 'rootId'),
    directory: safeString(value, 'directory'),
    query: safeString(value, 'query'),
    scoreRange: safeString(value, 'scoreRange'),
    resolutionRange: safeString(value, 'resolutionRange'),
    postCreatedFromDate: safeString(value, 'postCreatedFromDate'),
    postCreatedToDate: safeString(value, 'postCreatedToDate'),
    cursor: safeString(value, 'cursor'),
    before: safeBoolean(value, 'before'),
    cursorDepth: safePositiveInteger(value, 'cursorDepth', fallback.cursorDepth),
    scrollY: safeNonNegativeNumber(value, 'scrollY'),
    selectedIds: stringArray('selectedIds'),
    selectedMedia: Array.isArray(selectedMedia)
      ? selectedMedia.filter((item): item is LibraryMediaSnapshot => typeof item === 'object' && item !== null && typeof Reflect.get(item, 'id') === 'string')
      : [],
    allMatchingSelected: safeBoolean(value, 'allMatchingSelected'),
    allMatchingTotal: safeNonNegativeNumber(value, 'allMatchingTotal'),
    selectedQuery: safeString(value, 'selectedQuery'),
    ...optionalNumber(value, 'selectedScoreMin'),
    ...optionalNumber(value, 'selectedScoreMax'),
    ...optionalNumber(value, 'selectedResolutionMin'),
    ...optionalNumber(value, 'selectedResolutionMax'),
    ...optionalNumber(value, 'selectedPostCreatedFrom'),
    ...optionalNumber(value, 'selectedPostCreatedTo'),
    excludedMediaIds: stringArray('excludedMediaIds'),
  }
}

function safeString(value: object, key: string): string {
  const candidate = Reflect.get(value, key)
  return typeof candidate === 'string' ? candidate : ''
}

function safeBoolean(value: object, key: string): boolean {
  return Reflect.get(value, key) === true
}

function safeNonNegativeNumber(value: object, key: string): number {
  const candidate = Reflect.get(value, key)
  return typeof candidate === 'number' && Number.isFinite(candidate) && candidate >= 0 ? candidate : 0
}

function safePositiveInteger(value: object, key: string, fallback: number): number {
  const candidate = Reflect.get(value, key)
  return typeof candidate === 'number' && Number.isInteger(candidate) && candidate > 0 ? candidate : fallback
}

function optionalNumber(value: object, key: string): Partial<LibraryViewState> {
  const candidate = Reflect.get(value, key)
  return typeof candidate === 'number' && Number.isFinite(candidate) ? { [key]: candidate } : {}
}

function firstRouteValue(value: string | Array<string | null> | null | undefined): string | undefined {
  const candidate = Array.isArray(value) ? value[0] : value
  return typeof candidate === 'string' ? candidate : undefined
}

function routeString(query: LibraryRouteQuery, key: string, fallback: string): string {
  return Object.prototype.hasOwnProperty.call(query, key) ? (firstRouteValue(query[key]) ?? '') : fallback
}

function routeBoolean(query: LibraryRouteQuery, key: string, fallback: boolean): boolean {
  if (!Object.prototype.hasOwnProperty.call(query, key)) return fallback
  const value = firstRouteValue(query[key])
  return value === '1' || value === 'true'
}

function routePositiveInteger(query: LibraryRouteQuery, key: string, fallback: number): number {
  if (!Object.prototype.hasOwnProperty.call(query, key)) return fallback
  const value = Number(firstRouteValue(query[key]))
  return Number.isInteger(value) && value > 0 ? value : 1
}
