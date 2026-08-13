import { describe, expect, it } from 'vitest'
import { formatPostCreatedAt, localDateEndEpochSeconds, localDateStartEpochSeconds } from './postDateRange'

describe('post date range', () => {
  it('converts local calendar dates into inclusive epoch-second boundaries', () => {
    const start = new Date(2024, 1, 29)
    const nextDay = new Date(2024, 1, 29)
    nextDay.setDate(nextDay.getDate() + 1)

    expect(localDateStartEpochSeconds('2024-02-29')).toBe(start.getTime() / 1_000)
    expect(localDateEndEpochSeconds('2024-02-29')).toBe(nextDay.getTime() / 1_000 - 1)
  })

  it('formats a stored UTC publication time for the local interface', () => {
    expect(formatPostCreatedAt('2024-02-29T08:30:00Z')).toBe(
      new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' }).format(
        new Date('2024-02-29T08:30:00Z'),
      ),
    )
    expect(formatPostCreatedAt(null)).toBe('发布时间未知')
  })
})
