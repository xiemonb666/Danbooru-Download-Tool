<script setup lang="ts">
import { ref, watch } from 'vue'
import { FolderOpen, FolderPlus } from '@lucide/vue'
import { createMediaDirectory, getMediaDirectories, type MediaRoot } from '../api'

const props = withDefaults(defineProps<{
  roots: MediaRoot[]
  rootId: string
  directory: string
  compact?: boolean
}>(), {
  compact: false,
})

const emit = defineEmits<{
  'update:rootId': [value: string]
  'update:directory': [value: string]
}>()

const directories = ref<string[]>([])
const loading = ref(false)
const loadError = ref(false)
const creating = ref(false)
const createOpen = ref(false)
const newDirectory = ref('')
const createError = ref('')
let loadRevision = 0

function formatDirectory(path: string): string {
  return path.split('/').join(' / ')
}

function selectRoot(event: Event): void {
  const target = event.currentTarget
  if (!(target instanceof HTMLSelectElement)) return
  emit('update:rootId', target.value)
  emit('update:directory', '')
}

function selectDirectory(event: Event): void {
  const target = event.currentTarget
  if (!(target instanceof HTMLSelectElement)) return
  emit('update:directory', target.value)
}

function openCreate(): void {
  createError.value = ''
  newDirectory.value = props.directory
  createOpen.value = true
}

function closeCreate(): void {
  createOpen.value = false
  createError.value = ''
}

async function createDirectory(): Promise<void> {
  const relativePath = newDirectory.value.trim().replace(/\\/g, '/').replace(/\/{2,}/g, '/')
  if (!props.rootId || !relativePath) return
  creating.value = true
  createError.value = ''
  try {
    const created = await createMediaDirectory(props.rootId, relativePath)
    if (!directories.value.includes(created.relative_path)) {
      directories.value = [...directories.value, created.relative_path]
        .sort((left, right) => left.localeCompare(right, 'zh-CN'))
    }
    emit('update:directory', created.relative_path)
    createOpen.value = false
    newDirectory.value = ''
  } catch (reason: unknown) {
    createError.value = reason instanceof Error ? reason.message : '无法创建文件夹'
  } finally {
    creating.value = false
  }
}

watch(() => props.rootId, async (rootId) => {
  const revision = ++loadRevision
  directories.value = []
  loadError.value = false
  if (!rootId) return
  loading.value = true
  try {
    const result = await getMediaDirectories(rootId)
    if (revision === loadRevision) directories.value = result.directories
  } catch {
    if (revision === loadRevision) loadError.value = true
  } finally {
    if (revision === loadRevision) loading.value = false
  }
}, { immediate: true })
</script>

<template>
  <div class="destination-picker" :class="{ 'is-compact': compact }">
    <div class="destination-heading" v-if="!compact">
      <FolderOpen :size="17" />
      <span><strong>下载位置</strong><small>先选择媒体库，再选择库内分类文件夹</small></span>
    </div>
    <div class="destination-fields">
      <label class="destination-field">
        <span>媒体库</span>
        <select class="select" :value="rootId" aria-label="媒体库" @change="selectRoot">
          <option value="" disabled>{{ roots.length ? '选择媒体库' : '请先添加下载位置' }}</option>
          <option v-for="root in roots" :key="root.id" :value="root.id">{{ root.name }}</option>
        </select>
      </label>
      <label class="destination-field">
        <span>库内文件夹</span>
        <select class="select" :value="directory" aria-label="库内文件夹" :disabled="!rootId || loading" @change="selectDirectory">
          <option value="">{{ loading ? '正在读取文件夹…' : '媒体库顶层（不分类）' }}</option>
          <option v-for="path in directories" :key="path" :value="path">{{ formatDirectory(path) }}</option>
        </select>
      </label>
    </div>
    <button v-if="rootId && !createOpen" type="button" class="destination-create-button" @click="openCreate">
      <FolderPlus :size="14" /> 新建文件夹
    </button>
    <form v-if="createOpen" class="destination-create" @submit.prevent="createDirectory">
      <label class="destination-field destination-create-field">
        <span>新文件夹路径</span>
        <input v-model="newDirectory" class="input" aria-label="新文件夹路径" maxlength="4096" autocomplete="off" placeholder="例如：角色/爱丽丝">
      </label>
      <div class="destination-create-actions">
        <button type="button" class="button button-small button-quiet" :disabled="creating" @click="closeCreate">取消</button>
        <button type="submit" class="button button-small button-primary" :disabled="creating || !newDirectory.trim()">{{ creating ? '创建中' : '创建并选择' }}</button>
      </div>
      <small class="destination-hint">使用“/”建立多级分类，例如“项目名/角色名”。</small>
      <small v-if="createError" class="destination-error">{{ createError }}</small>
    </form>
    <small v-if="loadError" class="destination-error">无法读取文件夹，可重新选择媒体库后重试。</small>
  </div>
</template>

<style scoped>
.destination-picker { display: grid; gap: 11px; padding: 14px; border: 1px solid var(--border); border-radius: 11px; background: var(--surface-muted); }
.destination-heading { display: flex; align-items: center; gap: 10px; color: var(--text-secondary); }
.destination-heading > svg { flex: 0 0 auto; }
.destination-heading span, .destination-heading strong, .destination-heading small { display: block; }
.destination-heading strong { color: var(--text); font-size: 12px; }
.destination-heading small { margin-top: 2px; color: var(--text-tertiary); font-size: 10px; }
.destination-fields { display: grid; grid-template-columns: minmax(150px, 0.8fr) minmax(190px, 1.2fr); gap: 9px; }
.destination-field { display: grid; min-width: 0; gap: 5px; }
.destination-field > span { color: var(--text-secondary); font-size: 10px; font-weight: 600; }
.destination-error { color: var(--red); font-size: 10px; }
.destination-create-button { display: inline-flex; width: fit-content; align-items: center; gap: 6px; padding: 2px 0; border: 0; background: transparent; color: var(--blue); font-size: 11px; font-weight: 600; cursor: pointer; }
.destination-create { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: end; gap: 8px; padding-top: 2px; }
.destination-create-actions { display: flex; gap: 6px; }
.destination-hint, .destination-create .destination-error { grid-column: 1 / -1; color: var(--text-tertiary); font-size: 10px; }
.destination-create .destination-error { color: var(--red); }
.is-compact { min-width: min(420px, 48vw); padding: 8px; border-color: var(--border-strong); background: white; }
.is-compact .destination-field > span { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0, 0, 0, 0); }
.is-compact .select { min-height: 38px; }
@media (max-width: 700px) {
  .destination-fields { grid-template-columns: 1fr; }
  .destination-create { grid-template-columns: 1fr; }
  .destination-create-actions { justify-content: flex-end; }
  .is-compact { min-width: 260px; }
}
</style>
