import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/vue'
import { afterEach, vi } from 'vitest'

afterEach(cleanup)

Object.defineProperty(window, 'scrollTo', { value: vi.fn(), writable: true })
