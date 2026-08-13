import { fireEvent, render, waitFor } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import TrainingMonitor from './TrainingMonitor.vue'

const mocks = vi.hoisted(() => ({
  getTrainingMetrics: vi.fn(),
  getTrainingMetricsOverview: vi.fn(),
  getTrainingArtifacts: vi.fn(),
  getTrainingLogs: vi.fn(),
  trainingMetricEventsUrl: vi.fn(),
  chartInit: vi.fn(),
  chart: {
    setOption: vi.fn(),
    dispatchAction: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
  },
}))

vi.mock('../api', () => ({
  getTrainingMetrics: mocks.getTrainingMetrics,
  getTrainingMetricsOverview: mocks.getTrainingMetricsOverview,
  getTrainingArtifacts: mocks.getTrainingArtifacts,
  getTrainingLogs: mocks.getTrainingLogs,
  trainingMetricEventsUrl: mocks.trainingMetricEventsUrl,
}))

vi.mock('echarts/core', () => {
  class LinearGradient {
    constructor(..._args: unknown[]) {}
  }
  return {
    use: vi.fn(),
    init: (...args: unknown[]) => {
      mocks.chartInit(...args)
      return mocks.chart
    },
    graphic: { LinearGradient },
  }
})

vi.mock('echarts/charts', () => ({ LineChart: {} }))
vi.mock('echarts/components', () => ({ DataZoomComponent: {}, GridComponent: {}, TooltipComponent: {} }))
vi.mock('echarts/renderers', () => ({ CanvasRenderer: {} }))

function resetMonitorMocks(): void {
  const metrics = [
    { series: 'loss', step: 10, timestamp: 1_700_000_000, value: 0.92 },
    { series: 'epoch', step: 10, timestamp: 1_700_000_000, value: 1 },
    { series: 'train.max_steps', step: 0, timestamp: 1_700_000_000, value: 100 },
    { series: 'loss', step: 20, timestamp: 1_700_000_020, value: 0.71 },
    { series: 'epoch', step: 20, timestamp: 1_700_000_020, value: 2 },
  ]
  mocks.getTrainingMetrics.mockResolvedValue({ metrics, cursor: 118 })
  mocks.getTrainingMetricsOverview.mockResolvedValue({
    cursor: 118,
    series: [
      { series: 'loss', count: 2, first: metrics[0], latest: metrics[3], minimum: metrics[3], maximum: metrics[0] },
      { series: 'epoch', count: 2, first: metrics[1], latest: metrics[4], minimum: metrics[1], maximum: metrics[4] },
      { series: 'train.max_steps', count: 1, first: metrics[2], latest: metrics[2], minimum: metrics[2], maximum: metrics[2] },
    ],
  })
  mocks.getTrainingArtifacts.mockResolvedValue({ artifacts: [
    ...Array.from({ length: 10 }, (_, index) => ({
      id: `sample-${index}`,
      kind: 'sample',
      name: `epoch-${index}.png`,
      path: `C:/output/samples/epoch-${index}.png`,
      size_bytes: 1024,
      modified_at: 1_700_000_000 + index,
      url: `/sample-${index}`,
    })),
    { id: 'lora', kind: 'lora', name: 'odette.safetensors', path: 'C:/output/odette.safetensors', size_bytes: 1024, modified_at: 1_700_000_020, url: '/lora' },
    { id: 'config', kind: 'config', name: 'config.toml', path: 'C:/run/config.toml', size_bytes: 512, modified_at: 1_700_000_020, url: '/config' },
  ] })
  mocks.getTrainingLogs.mockResolvedValue({ text: '', cursor: 0, truncated: false })
  mocks.trainingMetricEventsUrl.mockReturnValue('/api/training/tasks/test/events')
}

describe('TrainingMonitor', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      return window.setTimeout(() => callback(performance.now()), 0)
    })
    vi.stubGlobal('cancelAnimationFrame', (id: number) => window.clearTimeout(id))
    resetMonitorMocks()
  })

  afterEach(() => vi.unstubAllGlobals())

  it('renders a real interactive scalar chart instead of a static chart image', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    expect(await view.findByLabelText('交互式训练曲线')).toBeVisible()
    expect(view.getByRole('button', { name: '放大曲线' })).toBeVisible()
    expect(view.getByRole('button', { name: '缩小曲线' })).toBeVisible()
    expect(view.getByText(/滚轮缩放、拖拽平移/)).toBeVisible()
  })

  it('renders a step progress bar when telemetry provides the effective training-step target', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    const progress = await view.findByLabelText('训练步骤进度')
    expect(progress).toHaveAttribute('aria-valuenow', '20')
    expect(progress).toHaveAttribute('aria-valuemax', '100')
    expect(view.getByText('Step 20 / 100')).toBeVisible()
  })

  it('keeps every generated sample visible instead of truncating the gallery to eight cards', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    await view.findByAltText('epoch-0.png')
    expect(view.container.querySelectorAll('.training-sample-thumb')).toHaveLength(10)
  })

  it('keeps the artifact list focused on exported LoRA weights', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    expect(await view.findByText('odette.safetensors')).toBeInTheDocument()
    expect(view.queryByText('config.toml')).not.toBeInTheDocument()
  })

  it('uses Canvas rendering and passes original data points to the chart tooltip', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })
    expect(await view.findByLabelText('训练曲线画布')).toBeVisible()
    expect(mocks.chartInit).toHaveBeenCalled()
    await waitFor(() => expect(mocks.chart.setOption).toHaveBeenCalled())
    const option = mocks.chart.setOption.mock.calls[mocks.chart.setOption.mock.calls.length - 1]?.[0] as { series: Array<{ smooth: false | number; clip: boolean; lineStyle: { width: number } }> }
    expect(option.series[0].smooth).toBe(false)
    expect(option.series[0].clip).toBe(true)
    expect(option.series[0].lineStyle.width).toBe(3.5)
  })

  it('uses statistical smoothing without enabling chart interpolation or a locally emphasized segment', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    expect(await view.findByLabelText('曲线平滑程度')).toHaveValue('0')
    await fireEvent.update(view.getByLabelText('曲线平滑程度'), '100')
    await waitFor(() => expect(mocks.chart.setOption).toHaveBeenCalled())
    const option = mocks.chart.setOption.mock.calls[mocks.chart.setOption.mock.calls.length - 1]?.[0] as { series: Array<{ smooth: false | number }> }
    expect(option.series[0].smooth).toBe(false)
    expect(view.container.querySelector('.training-chart-line-emphasis')).toBeNull()
  })

  it('uses a uniform canvas curve with a research summary instead of a locally emphasized SVG path', async () => {
    const view = render(TrainingMonitor, { props: { taskId: 'task-1', active: false } })

    expect(await view.findByText('研究摘要')).toBeVisible()
    expect(view.container.querySelector('.training-chart-line-emphasis')).toBeNull()
  })

  it('does not fetch or poll while its retained monitor pane is hidden', async () => {
    render(TrainingMonitor, { props: { taskId: 'task-1', active: true, visible: false } })

    await Promise.resolve()

    expect(mocks.getTrainingMetrics).not.toHaveBeenCalled()
    expect(mocks.getTrainingMetricsOverview).not.toHaveBeenCalled()
    expect(mocks.getTrainingArtifacts).not.toHaveBeenCalled()
    expect(mocks.getTrainingLogs).not.toHaveBeenCalled()
  })
})
