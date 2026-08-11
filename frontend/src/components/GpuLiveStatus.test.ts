import { render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import GpuLiveStatus from './GpuLiveStatus.vue'

describe('GpuLiveStatus', () => {
  it('renders visual GPU utilization, memory, clocks, and power data', () => {
    const view = render(GpuLiveStatus, {
      props: {
        selectedGpuIds: ['0'],
        gpus: [{
          id: '0',
          name: 'NVIDIA GeForce RTX 5090',
          memory_total_mib: 32607,
          memory_used_mib: 12584,
          utilization_percent: 72,
          graphics_clock_mhz: 2840,
          memory_clock_mhz: 14001,
          power_draw_w: 356.4,
          power_limit_w: 575,
          temperature_c: 58,
          fan_speed_percent: 42,
        }],
      },
    })

    expect(view.getByText('GPU 实时状态')).toBeVisible()
    expect(view.getByText('NVIDIA GeForce RTX 5090')).toBeVisible()
    expect(view.getByText('72%')).toBeVisible()
    expect(view.getByText('12.3 / 31.8 GiB')).toBeVisible()
    expect(view.getByText('2840 MHz')).toBeVisible()
    expect(view.getByText('356 W')).toBeVisible()
    expect(view.getByRole('img', { name: 'GPU 0 利用率 72%' })).toBeVisible()
  })

  it('shows a clear empty state when GPU telemetry is unavailable', () => {
    const view = render(GpuLiveStatus, { props: { gpus: [], selectedGpuIds: [] } })

    expect(view.getByText('正在等待本机 GPU 遥测…')).toBeVisible()
  })
})
