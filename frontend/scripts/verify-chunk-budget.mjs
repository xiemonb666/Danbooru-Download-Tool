import { readdir, stat } from 'node:fs/promises'
import { join } from 'node:path'

const assetsDir = new URL('../dist/assets/', import.meta.url)
// Keep the explicit budget aligned with Vite's threshold rather than merely
// silencing its warning. The chart runtime is lazy-loaded by TrainingView.
const maxBytes = 550 * 1024
const oversized = []

for (const name of await readdir(assetsDir)) {
  if (!name.endsWith('.js')) continue
  const file = new URL(name, assetsDir)
  const size = (await stat(file)).size
  if (size > maxBytes) oversized.push(`${name} (${(size / 1024).toFixed(1)} KiB)`)
}

if (oversized.length > 0) {
  throw new Error(`Production chunk budget exceeded: ${oversized.join(', ')}`)
}
