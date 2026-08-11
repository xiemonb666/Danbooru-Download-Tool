import { fireEvent, render, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  analyzeLoraSvd: vi.fn(),
  browseTrainingPath: vi.fn(),
  getTrainingArtifacts: vi.fn(),
}))

vi.mock('../api', () => ({
  analyzeLoraSvd: mocks.analyzeLoraSvd,
  browseTrainingPath: mocks.browseTrainingPath,
  getTrainingArtifacts: mocks.getTrainingArtifacts,
  loraSvdExportUrl: vi.fn((id: string) => `/api/training/lora-svd/analyses/${id}/export`),
}))

import LoraSvdAnalysis from './LoraSvdAnalysis.vue'

describe('LoraSvdAnalysis', () => {
  it('accepts a local safetensors file and submits an auto-device SVD request', async () => {
    mocks.analyzeLoraSvd.mockResolvedValue({
      id: 'analysis-1', expires_at: 1_700_000_000, reports: [], execution: { device: 'cpu', reason: 'test', duration_ms: 1 },
    })
    const view = render(LoraSvdAnalysis, {
      props: {
        profiles: [{ id: 'conda:lora', label: 'Conda LoRA', kind: 'conda', managed: false, installed: true, runtime_root: 'D:/runtime', python_path: 'D:/python.exe' }],
        trainingTasks: [],
      },
    })

    expect(view.getByRole('heading', { name: 'LoRA SVD 分析' })).toBeVisible()
    await fireEvent.update(view.getByLabelText('本地 LoRA 路径'), 'D:/models/epoch-0001.safetensors')
    await fireEvent.click(view.getByRole('button', { name: '加入分析列表' }))
    await fireEvent.click(view.getByRole('button', { name: '开始 SVD 分析' }))

    await waitFor(() => expect(mocks.analyzeLoraSvd).toHaveBeenCalledWith({
      runtime_profile_id: 'conda:lora',
      files: [{ path: 'D:/models/epoch-0001.safetensors', label: 'epoch-0001' }],
      device: 'auto',
    }))
  })

  it('loads task artifacts for explicit checkpoint selection instead of adding the first five', async () => {
    mocks.getTrainingArtifacts.mockResolvedValue({
      artifacts: Array.from({ length: 7 }, (_, index) => ({
        id: `artifact-${index + 1}`,
        kind: 'lora',
        name: `epoch-${String(index + 1).padStart(4, '0')}.safetensors`,
        path: `D:/outputs/epoch-${String(index + 1).padStart(4, '0')}.safetensors`,
        size_bytes: 1024,
        modified_at: 1_700_000_000 + index,
        url: `/artifacts/${index + 1}`,
      })),
    })
    const view = render(LoraSvdAnalysis, {
      props: {
        profiles: [{ id: 'conda:lora', label: 'Conda LoRA', kind: 'conda', managed: false, installed: true, runtime_root: 'D:/runtime', python_path: 'D:/python.exe' }],
        trainingTasks: [{ id: 'task-1', training: { output_name: '训练产物' } }] as never[],
      },
    })

    await fireEvent.update(view.getByLabelText('训练任务产物'), 'task-1')
    await fireEvent.click(view.getByRole('button', { name: '读取任务 LoRA' }))

    await waitFor(() => expect(view.getByLabelText('选择 epoch-0007.safetensors')).toBeVisible())
    expect(view.queryByText('epoch-0001.safetensors', { selector: '.svd-file-list strong' })).toBeNull()
    await fireEvent.click(view.getByLabelText('选择 epoch-0007.safetensors'))
    await fireEvent.click(view.getByRole('button', { name: '加入所选 1 个' }))

    expect(view.getByText('epoch-0007.safetensors', { selector: '.svd-file-list strong' })).toBeVisible()
    expect(view.queryByText('epoch-0001.safetensors', { selector: '.svd-file-list strong' })).toBeNull()
  })
})
