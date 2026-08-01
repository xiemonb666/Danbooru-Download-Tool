import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getHealth, getVllmHealth, type HealthStatus, type VllmHealthStatus } from '../api'

export const useHealthStore = defineStore('health', () => {
  const status = ref<'checking' | 'online' | 'offline'>('checking')
  const details = ref<HealthStatus | null>(null)
  const message = ref('正在连接本地服务')
  const vllmStatus = ref<'checking' | 'online' | 'offline'>('checking')
  const vllmDetails = ref<VllmHealthStatus | null>(null)
  const vllmMessage = ref('正在检查 vLLM')
  let timer: ReturnType<typeof setInterval> | null = null

  async function check(): Promise<void> {
    status.value = details.value ? status.value : 'checking'
    vllmStatus.value = vllmDetails.value ? vllmStatus.value : 'checking'
    const [localResult, vllmResult] = await Promise.allSettled([getHealth(), getVllmHealth()])
    if (localResult.status === 'fulfilled') {
      details.value = localResult.value
      status.value = 'online'
      message.value = details.value.database === 'ok' ? '本地服务正常' : '本地服务可用，数据库降级'
    } else {
      status.value = 'offline'
      message.value = '无法连接本地服务'
    }
    if (vllmResult.status === 'fulfilled') {
      vllmDetails.value = vllmResult.value
      vllmStatus.value = vllmResult.value.available ? 'online' : 'offline'
      vllmMessage.value = vllmResult.value.message
    } else {
      vllmDetails.value = null
      vllmStatus.value = 'offline'
      vllmMessage.value = status.value === 'offline' ? '本地服务离线，无法检查 vLLM' : '无法检查 vLLM 状态'
    }
  }

  function start(): void {
    if (timer) return
    void check()
    timer = setInterval(() => { void check() }, 15_000)
  }

  function stop(): void {
    if (timer) clearInterval(timer)
    timer = null
  }

  return {
    status,
    details,
    message,
    vllmStatus,
    vllmDetails,
    vllmMessage,
    check,
    start,
    stop,
  }
})
