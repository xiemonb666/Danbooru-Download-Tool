import { nextTick } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useTagAutocomplete } from './useTagAutocomplete'

describe('useTagAutocomplete', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('waits 200ms and aborts the stale request when the query changes', async () => {
    vi.useFakeTimers()
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ data: [] }), { status: 200 }),
    )
    vi.stubGlobal('fetch', fetchMock)
    const autocomplete = useTagAutocomplete()

    autocomplete.query.value = 'cat'
    await nextTick()
    await vi.advanceTimersByTimeAsync(200)
    const firstSignal = fetchMock.mock.calls[0]?.[1]?.signal

    autocomplete.query.value = 'dog'
    await nextTick()
    await vi.advanceTimersByTimeAsync(200)

    expect(firstSignal?.aborted).toBe(true)
    expect(fetchMock).toHaveBeenCalledTimes(2)
    autocomplete.dispose()
  })
})
