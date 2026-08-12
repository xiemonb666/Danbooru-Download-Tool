<script setup lang="ts">
import { nextTick, onMounted, ref } from 'vue'
import { Check, FolderPlus, FolderTree, KeyRound, Pencil, Plus, Save, Server, Trash2 } from '@lucide/vue'
import DownloadDestinationPicker from '../components/DownloadDestinationPicker.vue'
import {
  createMediaRoot,
  deleteMediaRoot,
  deleteSecret,
  getMediaRoots,
  loadVllmModel,
  saveSecret,
  unloadVllmModel,
  updateMediaRoot,
  type MediaRoot,
  type SaveMediaRootRequest,
  type SecretKind,
} from '../api'
import { useConfigStore } from '../stores/config'
import { useHealthStore } from '../stores/health'
import { useToastStore } from '../stores/toast'

const config = useConfigStore()
const health = useHealthStore()
const toast = useToastStore()
const roots = ref<MediaRoot[]>([])
const saving = ref(false)
const rootSaving = ref(false)
const editingRootId = ref<string | null>(null)
const rootFormOpen = ref(false)
const rootForm = ref<SaveMediaRootRequest>({ name: '', windows_path: null, linux_path: null })
const managedRootId = ref('')
const managedDirectory = ref('')
const danbooruSecret = ref('')
const vllmSecret = ref('')
const allowedHosts = ref('')
const credentialSaving = ref<SecretKind | null>(null)
const vllmLoading = ref(false)
const vllmUnloading = ref(false)

const vllmPromptPresets = {
  danbooru: 'You are a Danbooru image tagging assistant. Return concise, canonical Danbooru tags inside exactly one <tag>...</tag> block. Use lowercase tags separated by commas, replace spaces inside tags with underscores, and do not include prose or explanations.',
  zh: '你是图像描述助手。请使用简洁、客观、自然的中文描述画面中可见的内容，并且只在一个 <tag>...</tag> 块中返回描述；不要添加解释或无关内容。',
  en: 'You are an image description assistant. Describe the visible content in concise, objective, natural English and return only the description inside exactly one <tag>...</tag> block. Do not add explanations or unrelated content.',
} as const

function applyVllmPromptPreset(event: Event): void {
  const target = event.currentTarget
  if (!(target instanceof HTMLSelectElement)) return
  const language = target.value as keyof typeof vllmPromptPresets
  config.config.vllm_language = language
  config.config.vllm_system_prompt = vllmPromptPresets[language]
}

async function load(): Promise<void> {
  await config.load()
  allowedHosts.value = config.config.vllm_allowed_hosts.join('\n')
  roots.value = await getMediaRoots()
  managedRootId.value = roots.value[0]?.id ?? ''
}

async function saveSettings(): Promise<boolean> {
  if (/[\\/]/.test(config.config.filename_template)) {
    toast.warning('文件名模板不能包含路径分隔符')
    return false
  }
  saving.value = true
  try {
    config.config.download_concurrency = Math.max(1, Math.min(32, config.config.download_concurrency))
    config.config.vllm_concurrency = Math.max(1, Math.min(32, config.config.vllm_concurrency))
    config.config.vllm_max_tags = Math.max(1, Math.min(200, config.config.vllm_max_tags))
    config.config.vllm_max_length = Math.max(1, Math.min(4000, config.config.vllm_max_length))
    config.config.proxy_url = config.config.proxy_url?.trim() || null
    config.config.vllm_base_url = config.config.vllm_base_url.trim()
    config.config.vllm_model = config.config.vllm_model.trim()
    config.config.vllm_system_prompt = config.config.vllm_system_prompt.trim()
    config.config.vllm_allowed_hosts = allowedHosts.value.split(/[,\n]/).map((host) => host.trim()).filter(Boolean)
    await config.save()
    toast.success('设置已保存')
    return true
  } catch (reason: unknown) {
    toast.error('无法保存设置', reason instanceof Error ? reason.message : '未知错误')
    return false
  } finally {
    saving.value = false
  }
}

async function requestVllmModelLoad(): Promise<void> {
  if (!(await saveSettings())) return
  vllmLoading.value = true
  try {
    const result = await loadVllmModel()
    toast.success(result.message)
    await health.check()
  } catch (reason: unknown) {
    toast.error('无法加载 vLLM 模型', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    vllmLoading.value = false
  }
}

async function requestVllmModelUnload(): Promise<void> {
  vllmUnloading.value = true
  try {
    const result = await unloadVllmModel()
    toast.success(result.message)
    await health.check()
  } catch (reason: unknown) {
    toast.error('无法卸载 vLLM 模型', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    vllmUnloading.value = false
  }
}

async function showRootForm(): Promise<void> {
  rootFormOpen.value = true
  await nextTick()
  document.getElementById('root-form')?.scrollIntoView?.({ behavior: 'smooth', block: 'center' })
}

function addRoot(): void {
  editingRootId.value = null
  rootForm.value = { name: '', windows_path: null, linux_path: null }
  void showRootForm()
}

function editRoot(root: MediaRoot): void {
  editingRootId.value = root.id
  rootForm.value = { name: root.name, windows_path: root.windows_path, linux_path: root.linux_path }
  void showRootForm()
}

function resetRootForm(): void {
  editingRootId.value = null
  rootFormOpen.value = false
  rootForm.value = { name: '', windows_path: null, linux_path: null }
}

function manageRoot(root: MediaRoot): void {
  managedRootId.value = root.id
  managedDirectory.value = ''
  void nextTick(() => document.getElementById('folder-manager')?.scrollIntoView?.({ behavior: 'smooth', block: 'center' }))
}

async function removeRoot(root: MediaRoot): Promise<void> {
  const accepted = window.confirm(`移除“${root.name}”这个下载位置？只会移除应用中的位置和图库记录，磁盘上的文件不会被删除。`)
  if (!accepted) return
  rootSaving.value = true
  try {
    await deleteMediaRoot(root.id)
    roots.value = await getMediaRoots()
    if (managedRootId.value === root.id) {
      managedRootId.value = roots.value[0]?.id ?? ''
      managedDirectory.value = ''
    }
    toast.success('下载位置已移除', '磁盘上的文件没有被删除。')
  } catch (reason: unknown) {
    toast.error('无法移除下载位置', reason instanceof Error ? reason.message : '请先完成该位置中的任务')
  } finally {
    rootSaving.value = false
  }
}

async function saveRoot(): Promise<void> {
  rootSaving.value = true
  try {
    const request: SaveMediaRootRequest = {
      name: rootForm.value.name.trim(),
      windows_path: rootForm.value.windows_path?.trim() || null,
      linux_path: rootForm.value.linux_path?.trim() || null,
    }
    if (!request.windows_path && !request.linux_path) {
      toast.warning('至少填写一个平台路径')
      return
    }
    const saved = editingRootId.value
      ? await updateMediaRoot(editingRootId.value, request)
      : await createMediaRoot(request)
    roots.value = await getMediaRoots()
    managedRootId.value = saved.id
    managedDirectory.value = ''
    resetRootForm()
    toast.success('下载位置已保存', '现在可以在下载时选择或新建库内分类文件夹。')
  } catch (reason: unknown) {
    toast.error('无法保存下载位置', reason instanceof Error ? reason.message : '请检查文件夹路径')
  } finally {
    rootSaving.value = false
  }
}

async function storeCredential(kind: SecretKind): Promise<void> {
  const value = kind === 'danbooru' ? danbooruSecret.value : vllmSecret.value
  if (!value) return
  credentialSaving.value = kind
  try {
    const stored = await saveSecret(kind, value)
    if (kind === 'danbooru') {
      config.config.danbooru_api_key_configured = true
      danbooruSecret.value = ''
    } else {
      config.config.vllm_api_key_configured = true
      vllmSecret.value = ''
    }
    if (stored.storage === 'system') {
      toast.success('凭据已保存到系统凭据库')
    } else {
      toast.warning('凭据仅保留本次会话', '系统凭据库不可用；应用退出后需要重新填写。')
    }
  } catch (reason: unknown) {
    toast.error('无法保存凭据', reason instanceof Error ? reason.message : '系统凭据库不可用；服务器可能仅保留本次会话。')
  } finally {
    credentialSaving.value = null
  }
}

async function removeCredential(kind: SecretKind): Promise<void> {
  credentialSaving.value = kind
  try {
    await deleteSecret(kind)
    if (kind === 'danbooru') config.config.danbooru_api_key_configured = false
    else config.config.vllm_api_key_configured = false
    toast.success('凭据已移除')
  } catch (reason: unknown) {
    toast.error('无法移除凭据', reason instanceof Error ? reason.message : '未知错误')
  } finally {
    credentialSaving.value = null
  }
}

onMounted(() => {
  void load().catch((reason: unknown) => toast.error('设置加载失败', reason instanceof Error ? reason.message : '未知错误'))
})
</script>

<template>
  <div class="page-shell">
    <header class="page-header">
      <div>
        <p class="eyebrow">Local settings</p>
        <h1 class="page-title">设置</h1>
        <p class="page-description">管理下载位置、库内分类、安全凭据和运行策略。密钥永远不会在配置响应中回显。</p>
      </div>
      <button type="button" class="button button-primary" :disabled="saving || !config.loaded" @click="saveSettings"><Save :size="16" /> {{ saving ? '保存中' : '保存设置' }}</button>
    </header>

    <div class="settings-layout">
      <div class="stack">
        <section class="surface">
          <header class="surface-header">
            <div><h2 class="section-title">下载位置</h2><p class="section-copy">每个位置是一个顶层媒体库；具体分类在下载时选择库内文件夹。</p></div>
            <button type="button" class="button button-small" @click="addRoot"><Plus :size="14" /> 添加下载位置</button>
          </header>
          <div class="surface-body">
            <div v-if="roots.length">
              <article v-for="root in roots" :key="root.id" class="root-card">
                <div class="root-card-header">
                  <span class="root-card-title"><FolderTree :size="17" /><span><strong>{{ root.name }}</strong><small>{{ root.media_count ? `${root.media_count.toLocaleString()} 个图库媒体` : '暂无图库媒体，可在图库中刷新' }}</small></span></span>
                  <span class="inline"><button type="button" class="button button-small" @click="manageRoot(root)">管理分类</button><button type="button" class="button button-small button-quiet" @click="editRoot(root)"><Pencil :size="13" /> 编辑</button><button type="button" class="button button-small button-danger" :disabled="rootSaving" :aria-label="`移除下载位置 ${root.name}`" @click="removeRoot(root)"><Trash2 :size="13" /> 移除</button></span>
                </div>
                <div class="path-row"><span>Windows</span><code>{{ root.windows_path || '未映射' }}</code></div>
                <div class="path-row"><span>Linux/WSL</span><code>{{ root.linux_path || '未映射' }}</code></div>
              </article>
            </div>
            <div v-else class="root-onboarding">
              <FolderPlus :size="24" />
              <div><strong>先添加一个顶层下载位置</strong><p>例如选择 <code>D:\Danbooru</code> 作为媒体库；下载时再选择或新建库内分类文件夹，例如 <code>角色/爱丽丝</code>。</p></div>
              <ol class="root-steps"><li>添加顶层下载位置</li><li>在探索页选择分类文件夹</li><li>需要浏览新放入的文件时刷新图库</li></ol>
            </div>

            <form v-if="rootFormOpen" id="root-form" class="form-grid root-form-panel" @submit.prevent="saveRoot">
              <div class="span-full"><h3 class="root-form-title">{{ editingRootId ? '编辑下载位置' : '添加下载位置' }}</h3><p class="section-copy">填写后端实际运行平台的文件夹；如果 Windows 和 WSL 共用同一批文件，可同时填写对应路径。</p></div>
              <div class="field span-full"><label class="field-label" for="root-name">位置名称</label><input id="root-name" v-model="rootForm.name" class="input" required maxlength="80" placeholder="例如：Danbooru 主图库"></div>
              <div class="field"><label class="field-label" for="windows-path">Windows 文件夹</label><input id="windows-path" v-model="rootForm.windows_path" class="input" placeholder="D:\\Danbooru"><span class="field-help">后端在 Windows 运行时使用</span></div>
              <div class="field"><label class="field-label" for="linux-path">Linux / WSL 文件夹</label><input id="linux-path" v-model="rootForm.linux_path" class="input" placeholder="/mnt/d/Danbooru"><span class="field-help">后端在 Linux 或 WSL 运行时使用</span></div>
              <div class="inline span-full">
                <button type="submit" class="button button-primary" :disabled="rootSaving || !rootForm.name.trim()"><FolderPlus :size="15" /> {{ rootSaving ? '保存中' : editingRootId ? '保存修改' : '保存下载位置' }}</button>
                <button type="button" class="button" @click="resetRootForm">取消</button>
              </div>
            </form>

            <div v-if="roots.length" id="folder-manager" class="root-folder-manager">
              <div><h3 class="root-form-title">库内分类文件夹</h3><p class="section-copy">可在这里预先创建分类；探索页下载时也能随时创建并自动选中。</p></div>
              <DownloadDestinationPicker v-model:root-id="managedRootId" v-model:directory="managedDirectory" :roots="roots" />
            </div>
          </div>
        </section>

        <section class="surface">
          <header class="surface-header"><div><h2 class="section-title">下载策略</h2><p class="section-copy">并发范围 1–32；默认保存原始媒体并按帖子 ID 去重。</p></div></header>
          <div class="surface-body form-grid">
            <div class="field"><label class="field-label" for="concurrency">并发下载数</label><input id="concurrency" v-model.number="config.config.download_concurrency" class="input" type="number" min="1" max="32"></div>
            <div class="field"><label class="field-label" for="ugoira">Ugoira 策略</label><select id="ugoira" v-model="config.config.ugoira_policy" class="select"><option value="webm_and_zip">WebM + 原始 ZIP</option><option value="webm_only">仅 WebM</option><option value="zip_only">仅原始 ZIP</option></select></div>
            <div class="field span-full"><label class="field-label" for="filename">文件名模板 <span class="field-help">允许 {id} {score} {rating} {ext}</span></label><input id="filename" v-model="config.config.filename_template" class="input" required></div>
            <div class="field span-full">
              <label class="field-label" for="blur-sensitive">默认模糊敏感分级</label>
              <div class="inline">
                <input id="blur-sensitive" v-model="config.config.blur_sensitive_media" type="checkbox">
                <span class="field-help">开启后，Questionable、Explicit 与未知分级需手动揭示；关闭后直接显示。</span>
              </div>
            </div>
          </div>
        </section>

        <section class="surface">
          <header class="surface-header"><div><h2 class="section-title">网络与 vLLM</h2><p class="section-copy">代理失败时不会静默直连。vLLM 默认只允许 loopback 地址。</p></div></header>
          <div class="surface-body form-grid">
            <div class="field span-full"><label class="field-label" for="proxy">代理 URL</label><input id="proxy" v-model="config.config.proxy_url" class="input" placeholder="socks5://127.0.0.1:1080"></div>
            <div class="field span-full"><label class="field-label" for="vllm-url">vLLM Base URL</label><input id="vllm-url" v-model="config.config.vllm_base_url" class="input" placeholder="http://127.0.0.1:8000/v1"></div>
            <div class="field span-full"><label class="field-label" for="vllm-model">vLLM 模型</label><input id="vllm-model" v-model="config.config.vllm_model" class="input" required placeholder="model/name"></div>
            <div class="field"><label class="field-label" for="vllm-tag-mode">标签写入模式</label><select id="vllm-tag-mode" v-model="config.config.vllm_tag_mode" class="select"><option value="overwrite">覆盖现有标签</option><option value="append">追加到现有标签</option></select></div>
            <div class="field"><label class="field-label" for="vllm-concurrency">vLLM 并发数</label><input id="vllm-concurrency" v-model.number="config.config.vllm_concurrency" class="input" type="number" min="1" max="32"></div>
            <div class="field"><label class="field-label" for="vllm-language">输出格式</label><select id="vllm-language" v-model="config.config.vllm_language" class="select" @change="applyVllmPromptPreset"><option value="danbooru">Danbooru 标签</option><option value="zh">中文描述</option><option value="en">英文描述</option></select></div>
            <div class="field"><label class="field-label" for="vllm-max-tags">最大标签数</label><input id="vllm-max-tags" v-model.number="config.config.vllm_max_tags" class="input" type="number" min="1" max="200"></div>
            <div class="field"><label class="field-label" for="vllm-max-length">最大输出长度</label><input id="vllm-max-length" v-model.number="config.config.vllm_max_length" class="input" type="number" min="1" max="4000"></div>
            <div class="setting-options span-full" role="group" aria-label="视觉模型输出选项">
              <label class="setting-check" for="vllm-verify">
                <input id="vllm-verify" v-model="config.config.vllm_verify_danbooru" type="checkbox" aria-label="联网校验 Danbooru 标签" aria-describedby="vllm-verify-help">
                <span><strong>联网校验 Danbooru 标签</strong><small id="vllm-verify-help">Danbooru 格式下核对标签是否真实存在。</small></span>
              </label>
              <label class="setting-check" for="vllm-reference">
                <input id="vllm-reference" v-model="config.config.vllm_reference_existing" type="checkbox" aria-label="参考现有标签文件" aria-describedby="vllm-reference-help">
                <span><strong>参考现有标签文件</strong><small id="vllm-reference-help">读取同名 .txt，作为生成与修正标签的上下文。</small></span>
              </label>
            </div>
            <div class="field span-full"><label class="field-label" for="vllm-prompt">系统提示词</label><span id="vllm-prompt-help" class="field-help">切换输出格式会载入匹配模板，载入后仍可编辑</span><textarea id="vllm-prompt" v-model="config.config.vllm_system_prompt" class="textarea" required aria-describedby="vllm-prompt-help"></textarea></div>
            <div class="field span-full"><label class="field-label" for="allowed-hosts">额外地址 allowlist <span class="field-help">每行一个 host:port；外部地址仅 HTTPS，默认端口也需写 :443</span></label><textarea id="allowed-hosts" v-model="allowedHosts" class="textarea" placeholder="vision.example.com:443"></textarea></div>
          </div>
        </section>
      </div>

      <aside class="stack">
        <section class="surface">
          <header class="surface-header"><div><h2 class="section-title">本地服务</h2><p class="section-copy">来自 /api/health 的实时状态。</p></div></header>
          <div class="surface-body">
            <div class="credential-row">
              <Server :size="20" :class="health.status === 'online' ? 'configured' : 'not-configured'" />
              <span><strong>{{ health.status === 'online' ? '服务正常' : health.status === 'offline' ? '服务离线' : '正在检查' }}</strong><small>{{ health.message }}</small></span>
              <button type="button" class="button button-small" @click="health.check">重新检查</button>
            </div>
            <div class="credential-row">
              <Server :size="20" :class="health.vllmStatus === 'online' ? 'configured' : 'not-configured'" />
              <span><strong>{{ health.vllmStatus === 'online' ? 'vLLM 正常' : health.vllmStatus === 'offline' ? 'vLLM 离线' : '正在检查 vLLM' }}</strong><small>{{ health.vllmMessage }}</small></span>
              <span class="inline">
                <button type="button" class="button button-small" :disabled="vllmLoading || vllmUnloading || health.vllmStatus === 'online'" @click="requestVllmModelLoad">{{ vllmLoading ? '正在请求加载' : health.vllmStatus === 'online' ? '模型已加载' : '加载 vLLM 模型' }}</button>
                <button type="button" class="button button-small button-danger" :disabled="vllmLoading || vllmUnloading" @click="requestVllmModelUnload">{{ vllmUnloading ? '正在卸载' : '卸载 vLLM 模型' }}</button>
              </span>
            </div>
          </div>
        </section>

        <section class="surface">
          <header class="surface-header"><div><h2 class="section-title">系统凭据</h2><p class="section-copy">优先存入 Windows Credential Manager 或 Linux Secret Service。</p></div></header>
          <div class="surface-body">
            <div class="credential-row">
              <KeyRound :size="20" />
              <span><strong>Danbooru API Key</strong><small :class="config.config.danbooru_api_key_configured ? 'configured' : 'not-configured'">{{ config.config.danbooru_api_key_configured ? '已配置，不回显' : '未配置' }}</small></span>
              <Check v-if="config.config.danbooru_api_key_configured" :size="17" class="configured" />
            </div>
            <div class="field" style="margin: 12px 0 18px"><label class="field-label" for="danbooru-user">Danbooru 用户名</label><input id="danbooru-user" v-model="config.config.danbooru_username" class="input" autocomplete="username"></div>
            <div class="field"><label class="field-label" for="danbooru-secret">更新 API Key</label><input id="danbooru-secret" v-model="danbooruSecret" class="input" type="password" autocomplete="new-password" placeholder="输入后安全保存"></div>
            <div class="inline" style="margin-top: 10px"><button type="button" class="button button-primary button-small" :disabled="!danbooruSecret || credentialSaving !== null" @click="storeCredential('danbooru')">保存凭据</button><button v-if="config.config.danbooru_api_key_configured" type="button" class="button button-danger button-small" :disabled="credentialSaving !== null" @click="removeCredential('danbooru')">移除</button></div>

            <div class="credential-row" style="margin-top: 20px">
              <KeyRound :size="20" />
              <span><strong>vLLM API Key</strong><small :class="config.config.vllm_api_key_configured ? 'configured' : 'not-configured'">{{ config.config.vllm_api_key_configured ? '已配置，不回显' : '未配置或无需认证' }}</small></span>
              <Check v-if="config.config.vllm_api_key_configured" :size="17" class="configured" />
            </div>
            <div class="field" style="margin-top: 12px"><label class="field-label" for="vllm-secret">更新 API Key</label><input id="vllm-secret" v-model="vllmSecret" class="input" type="password" autocomplete="new-password" placeholder="输入后安全保存"></div>
            <div class="inline" style="margin-top: 10px"><button type="button" class="button button-primary button-small" :disabled="!vllmSecret || credentialSaving !== null" @click="storeCredential('vllm')">保存凭据</button><button v-if="config.config.vllm_api_key_configured" type="button" class="button button-danger button-small" :disabled="credentialSaving !== null" @click="removeCredential('vllm')">移除</button></div>
          </div>
        </section>

        <div class="notice">若系统凭据库不可用，服务器只允许会话密钥，不会降级写入明文配置。旧密钥只有在凭据库写入并回读成功后才会从旧配置移除。</div>
      </aside>
    </div>
  </div>
</template>
