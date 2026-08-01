const ratingNames: Readonly<Record<string, string>> = {
  g: 'General',
  s: 'Sensitive',
  q: 'Questionable',
  e: 'Explicit',
}

export function requiresContentReveal(rating: unknown): boolean {
  return rating !== 'g' && rating !== 's'
}

export function contentRatingName(rating: unknown): string {
  return typeof rating === 'string' ? (ratingNames[rating] ?? 'Unknown') : 'Unknown'
}
