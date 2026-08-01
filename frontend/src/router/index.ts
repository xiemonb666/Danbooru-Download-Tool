import { createRouter, createWebHistory } from 'vue-router'

const router = createRouter({
  history: createWebHistory(),
  scrollBehavior: () => ({ top: 0 }),
  routes: [
    { path: '/', redirect: '/explore' },
    { path: '/explore', name: 'explore', component: () => import('../views/ExploreView.vue') },
    { path: '/tasks', name: 'tasks', component: () => import('../views/TasksView.vue') },
    { path: '/library', name: 'library', component: () => import('../views/LibraryView.vue') },
    { path: '/tools', name: 'tools', component: () => import('../views/ToolsView.vue') },
    { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
    { path: '/download', redirect: '/explore' },
    { path: '/process', redirect: '/tools' },
    { path: '/tags', redirect: '/tools' },
    { path: '/browse', redirect: '/library' },
    { path: '/vllm', redirect: '/tools' },
    { path: '/:pathMatch(.*)*', redirect: '/explore' },
  ],
})

export default router
