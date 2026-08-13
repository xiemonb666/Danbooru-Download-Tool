function parseLocalDate(value: string): Date | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return undefined

  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
  if (
    date.getFullYear() !== Number(match[1])
    || date.getMonth() !== Number(match[2]) - 1
    || date.getDate() !== Number(match[3])
  ) return undefined
  return date
}

export function localDateStartEpochSeconds(value: string): number | undefined {
  const date = parseLocalDate(value)
  return date ? Math.floor(date.getTime() / 1_000) : undefined
}

export function localDateEndEpochSeconds(value: string): number | undefined {
  const date = parseLocalDate(value)
  if (!date) return undefined
  date.setDate(date.getDate() + 1)
  return Math.floor(date.getTime() / 1_000) - 1
}

const postDateFormatter = new Intl.DateTimeFormat('zh-CN', {
  dateStyle: 'medium',
  timeStyle: 'short',
})

export function formatPostCreatedAt(value: string | null | undefined): string {
  if (!value) return '发布时间未知'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '发布时间未知' : postDateFormatter.format(date)
}
