import { describe, expect, it } from 'vitest'
import router from './index'

describe('application routes', () => {
  it('uses the new information architecture and redirects legacy paths', async () => {
    await router.push('/download')
    await router.isReady()

    expect(router.currentRoute.value.path).toBe('/explore')
    expect(['/explore', '/tasks', '/library', '/tools', '/settings'].every((path) => router.resolve(path).matched.length > 0)).toBe(true)
  })
})
