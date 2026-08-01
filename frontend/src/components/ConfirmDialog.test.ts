import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import ConfirmDialog from './ConfirmDialog.vue'

describe('ConfirmDialog', () => {
  it('moves focus into the dialog and closes with Escape', async () => {
    const view = render(ConfirmDialog, {
      props: { open: true, title: '确认隔离', confirmLabel: '移入隔离区', destructive: true },
      slots: { default: '将移动 3 个文件。' },
    })

    const confirm = view.getByRole('button', { name: '移入隔离区' })
    expect(confirm).toHaveFocus()
    await fireEvent.keyDown(view.getByRole('dialog'), { key: 'Escape' })
    expect(view.emitted('cancel')).toHaveLength(1)
  })
})
