import { describe, expect, it } from 'vitest'
import { contentRatingName, requiresContentReveal } from './contentRating'

describe('content rating policy', () => {
  it('shows only known general and sensitive ratings without an explicit reveal', () => {
    expect(requiresContentReveal('g')).toBe(false)
    expect(requiresContentReveal('s')).toBe(false)
    expect(requiresContentReveal('q')).toBe(true)
    expect(requiresContentReveal('e')).toBe(true)
    expect(requiresContentReveal('unknown')).toBe(true)
    expect(requiresContentReveal(undefined)).toBe(true)
    expect(contentRatingName('unexpected')).toBe('Unknown')
  })
})
