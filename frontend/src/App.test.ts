import { render } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import App from './App.vue'

vi.mock('./stores/config', () => ({
  useConfigStore: () => ({
    config: { background_image: '', background_opacity: 18 },
    loaded: true,
    load: vi.fn(),
  }),
}))

vi.mock('./stores/tasks', () => ({
  useTasksStore: () => ({ activeCount: 0, connect: vi.fn(), disconnect: vi.fn() }),
}))

vi.mock('./stores/health', () => ({
  useHealthStore: () => ({
    status: 'online',
    message: '本地服务正常',
    vllmStatus: 'online',
    vllmMessage: 'vLLM 可用，发现 1 个模型',
    start: vi.fn(),
    stop: vi.fn(),
  }),
}))

describe('App service status', () => {
  it('shows vLLM health next to the local backend status', () => {
    const view = render(App, {
      global: {
        stubs: {
          RouterLink: { template: '<a><slot /></a>' },
          RouterView: { template: '<div />' },
          Transition: false,
          ToastContainer: true,
        },
      },
    })

    expect(view.getByText('本地服务正常')).toBeVisible()
    expect(view.getByText('vLLM 正常')).toBeVisible()
    expect(view.getByText('vLLM 可用，发现 1 个模型')).toBeVisible()
  })
})
