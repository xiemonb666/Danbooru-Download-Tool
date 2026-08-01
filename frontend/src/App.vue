<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue'
import { Images, ListTodo, Search, Settings, Wrench } from '@lucide/vue'
import ToastContainer from './components/ToastContainer.vue'
import { useConfigStore } from './stores/config'
import { useHealthStore } from './stores/health'
import { useTasksStore } from './stores/tasks'

const config = useConfigStore()
const health = useHealthStore()
const tasks = useTasksStore()

const navItems = [
  { to: '/explore', label: '探索', icon: Search },
  { to: '/tasks', label: '任务', icon: ListTodo },
  { to: '/library', label: '图库', icon: Images },
  { to: '/tools', label: '工具', icon: Wrench },
  { to: '/settings', label: '设置', icon: Settings },
]

const healthLabel = computed(() => ({
  checking: '正在连接',
  online: '本地服务正常',
  offline: '本地服务离线',
})[health.status])
const vllmHealthLabel = computed(() => ({
  checking: '正在检查 vLLM',
  online: 'vLLM 正常',
  offline: 'vLLM 离线',
})[health.vllmStatus])

onMounted(() => {
  void config.load()
  health.start()
  void tasks.connect()
})

onUnmounted(() => {
  health.stop()
  tasks.disconnect()
})
</script>

<template>
  <div class="app-shell">
    <a class="skip-link" href="#main-content">跳至主要内容</a>

    <header class="mobile-header">
      <RouterLink to="/explore" class="wordmark" aria-label="Danbooru Tool Pro 首页">
        <span class="wordmark-mark">D</span>
        <span>Danbooru Tool</span>
      </RouterLink>
      <span class="mobile-health" :title="`${health.message}；${health.vllmMessage}`">
        <span class="health-dot" :class="health.status" role="img" aria-label="本地服务状态" />
        <span class="health-dot" :class="health.vllmStatus" role="img" aria-label="vLLM 状态" />
      </span>
    </header>

    <aside class="app-sidebar">
      <RouterLink to="/explore" class="wordmark" aria-label="Danbooru Tool Pro 首页">
        <span class="wordmark-mark">D</span>
        <span>
          <strong>Danbooru Tool</strong>
          <small>Local media workspace</small>
        </span>
      </RouterLink>

      <nav class="primary-nav" aria-label="主要导航">
        <RouterLink v-for="item in navItems" :key="item.to" :to="item.to" class="nav-link">
          <component :is="item.icon" :size="19" :stroke-width="1.8" />
          <span>{{ item.label }}</span>
          <span v-if="item.to === '/tasks' && tasks.activeCount" class="nav-count">{{ tasks.activeCount }}</span>
        </RouterLink>
      </nav>

      <div class="service-status" :title="`${health.message}；${health.vllmMessage}`">
        <div class="service-status-row">
          <span class="health-dot" :class="health.status" />
          <span>
            <strong>{{ healthLabel }}</strong>
            <small>{{ health.status === 'online' ? '127.0.0.1:8888' : health.message }}</small>
          </span>
        </div>
        <div class="service-status-row">
          <span class="health-dot" :class="health.vllmStatus" />
          <span>
            <strong>{{ vllmHealthLabel }}</strong>
            <small>{{ health.vllmMessage }}</small>
          </span>
        </div>
      </div>
    </aside>

    <main id="main-content" class="app-main" tabindex="-1">
      <RouterView v-slot="{ Component }">
        <Transition name="page" mode="out-in">
          <component :is="Component" />
        </Transition>
      </RouterView>
    </main>

    <nav class="mobile-nav" aria-label="移动端导航">
      <RouterLink v-for="item in navItems" :key="item.to" :to="item.to" class="mobile-nav-link">
        <component :is="item.icon" :size="20" :stroke-width="1.8" />
        <span>{{ item.label }}</span>
        <span v-if="item.to === '/tasks' && tasks.activeCount" class="mobile-count">{{ tasks.activeCount }}</span>
      </RouterLink>
    </nav>

    <ToastContainer />
  </div>
</template>
