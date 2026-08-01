import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './e2e-real',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'line',
  timeout: 30_000,
  use: {
    baseURL: process.env.REAL_E2E_BASE_URL ?? 'http://127.0.0.1:18991',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
})
