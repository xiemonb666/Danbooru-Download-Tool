import { describe, expect, it } from 'vitest'
import { loadLibraryViewState, resolveLibraryViewState, saveLibraryViewState, type LibraryViewState } from './libraryViewState'

describe('library view session state', () => {
  it('treats an inaccessible session storage as an empty snapshot', () => {
    const storage = {
      getItem: () => { throw new Error('storage disabled') },
    }

    expect(loadLibraryViewState(storage)).toBeNull()
  })

  it('ignores a session storage write failure', () => {
    const storage = {
      setItem: () => { throw new Error('quota exceeded') },
    }

    expect(() => saveLibraryViewState({ version: 1 }, storage)).not.toThrow()
  })

  it('gives explicit URL parameters priority over the saved browsing context', () => {
    const saved: LibraryViewState = {
      version: 1,
      rootId: 'saved-root',
      directory: 'saved/folder',
      query: 'saved_tag',
      scoreRange: '0:9',
      resolutionRange: '512:1023',
      postCreatedFromDate: '',
      postCreatedToDate: '',
      cursor: 'saved-cursor',
      before: true,
      cursorDepth: 5,
      scrollY: 900,
      selectedIds: ['media-1'],
      selectedMedia: [],
      allMatchingSelected: false,
      allMatchingTotal: 0,
      selectedQuery: '',
      excludedMediaIds: [],
    }

    const resolved = resolveLibraryViewState(saved, {
      root: 'url-root',
      directory: 'url/folder',
      q: 'url_tag',
      score: '10:19',
      resolution: '1024:2047',
      cursor: 'url-cursor',
      before: '0',
      cursor_depth: '7',
    })

    expect(resolved.state).toMatchObject({
      rootId: 'url-root', directory: 'url/folder', query: 'url_tag',
      scoreRange: '10:19', resolutionRange: '1024:2047',
      cursor: 'url-cursor', before: false, cursorDepth: 7,
    })
    expect(resolved.restoreSelection).toBe(false)
    expect(resolved.restoreScroll).toBe(false)
  })

  it('does not carry a saved folder or cursor into an explicitly different root', () => {
    const saved = {
      ...resolveLibraryViewState(null, {}).state,
      rootId: 'saved-root',
      directory: 'saved/folder',
      cursor: 'saved-cursor',
      cursorDepth: 3,
    }

    const resolved = resolveLibraryViewState(saved, { root: 'other-root' })

    expect(resolved.state.directory).toBe('')
    expect(resolved.state.cursor).toBe('')
    expect(resolved.state.cursorDepth).toBe(1)
    expect(resolved.restoreSelection).toBe(false)
  })

  it('restores post publication dates while explicit URL dates take priority', () => {
    const saved = {
      ...resolveLibraryViewState(null, {}).state,
      postCreatedFromDate: '2025-01-01',
      postCreatedToDate: '2025-01-31',
    }

    const restored = resolveLibraryViewState(saved, {})
    const overridden = resolveLibraryViewState(saved, {
      post_created_from: '2026-02-01',
      post_created_to: '2026-02-28',
    })

    expect(restored.state).toMatchObject({
      postCreatedFromDate: '2025-01-01', postCreatedToDate: '2025-01-31',
    })
    expect(overridden.state).toMatchObject({
      postCreatedFromDate: '2026-02-01', postCreatedToDate: '2026-02-28',
    })
    expect(overridden.restoreSelection).toBe(false)
  })
})
