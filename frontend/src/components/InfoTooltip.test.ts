import { fireEvent, render, waitFor } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import InfoTooltip from './InfoTooltip.vue'

describe('InfoTooltip', () => {
  it('shows an accessible explanation outside the form flow on hover', async () => {
    const view = render(InfoTooltip, {
      props: { title: '网络 rank', description: '控制低秩适配器的容量。' },
    })

    const trigger = view.getByRole('button', { name: '查看说明' })
    await fireEvent.mouseEnter(trigger)

    expect(view.getByRole('tooltip')).toHaveTextContent('控制低秩适配器的容量。')
    expect(view.getByRole('tooltip').parentElement?.tagName).toBe('BODY')
  })

  it('opens on focus and preserves keyboard focus when dismissed', async () => {
    const view = render(InfoTooltip, {
      props: { title: '实际 rank', description: '从 LoRA 因子形状读取的真实 rank。' },
    })

    const trigger = view.getByRole('button', { name: '查看说明' })
    trigger.focus()
    await waitFor(() => expect(view.getByRole('tooltip')).toBeInTheDocument())
    expect(view.getByRole('tooltip')).toHaveTextContent('真实 rank')

    await fireEvent.keyDown(trigger, { key: 'Escape' })
    await waitFor(() => expect(view.queryByRole('tooltip')).toBeNull())
    expect(document.activeElement).toBe(trigger)
  })
})
