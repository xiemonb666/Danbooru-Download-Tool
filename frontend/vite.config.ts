import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': 'http://127.0.0.1:8888',
    },
  },
  preview: {
    host: '127.0.0.1',
    port: 4173,
    strictPort: true,
  },
  build: {
    // Apache ECharts is isolated behind the asynchronously loaded monitor
    // component. Its 533 KiB renderer is intentional and does not affect the
    // initial application bundle; retain a small buffer for patch releases.
    chunkSizeWarningLimit: 550,
  },
})
