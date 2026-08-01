<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Check, Eye, ImageOff, Play } from '@lucide/vue'
import { danbooruMediaUrl, type DanbooruPost } from '../api'
import { contentRatingName, requiresContentReveal } from '../utils/contentRating'

const props = withDefaults(defineProps<{
  post: DanbooruPost
  selected: boolean
  blurSensitive?: boolean
}>(), {
  blurSensitive: true,
})
const emit = defineEmits<{
  select: [post: DanbooruPost]
  open: [post: DanbooruPost]
}>()

const revealed = ref(false)
const mediaVariant = ref<'sample' | 'preview' | 'large' | 'original'>('sample')
const previewUnavailable = ref(false)
const obscured = computed(() => props.blurSensitive
  && requiresContentReveal(props.post.rating)
  && !revealed.value)
const ratio = computed(() => `${Math.max(props.post.image_width, 1)} / ${Math.max(props.post.image_height, 1)}`)
const rating = computed(() => contentRatingName(props.post.rating))

watch(() => props.post.id, () => {
  revealed.value = false
  mediaVariant.value = 'sample'
  previewUnavailable.value = false
})

function useFallbackPreview(): void {
  if (mediaVariant.value === 'sample') mediaVariant.value = 'preview'
  else if (mediaVariant.value === 'preview') mediaVariant.value = 'large'
  else if (mediaVariant.value === 'large' && !props.post.is_video && !props.post.is_ugoira) {
    mediaVariant.value = 'original'
  } else {
    previewUnavailable.value = true
  }
}
</script>

<template>
  <article class="post-card" :class="{ 'is-selected': selected }">
    <button
      type="button"
      class="post-media"
      :style="{ aspectRatio: ratio }"
      :aria-label="`打开帖子 ${post.id}`"
      @click="emit('open', post)"
    >
      <img
        v-if="!previewUnavailable"
        :src="danbooruMediaUrl(post.id, mediaVariant)"
        :alt="`Danbooru 帖子 ${post.id}`"
        :width="Math.max(post.image_width, 1)"
        :height="Math.max(post.image_height, 1)"
        :style="{ width: '100%', height: 'auto', objectFit: 'contain' }"
        loading="lazy"
        decoding="async"
        :class="{ 'media-obscured': obscured }"
        @error="useFallbackPreview"
      >
      <span v-else class="preview-unavailable"><ImageOff :size="22" />暂无可访问的预览</span>
      <span v-if="post.is_video || post.is_ugoira" class="media-kind" aria-label="视频">
        <Play :size="15" fill="currentColor" />
      </span>
    </button>

    <button
      v-if="obscured && !previewUnavailable"
      type="button"
      class="reveal-button"
      aria-label="显示敏感内容"
      @click="revealed = true"
    >
      <Eye :size="16" />
      显示敏感内容
    </button>

    <button
      type="button"
      class="select-button"
      :class="{ selected }"
      :aria-label="selected ? `取消选择帖子 ${post.id}` : `选择帖子 ${post.id}`"
      :aria-pressed="selected"
      @click="emit('select', post)"
    >
      <Check v-if="selected" :size="14" :stroke-width="3" />
    </button>

    <footer class="post-meta">
      <span>#{{ post.id }}</span>
      <span>{{ rating }}</span>
      <span class="post-score">{{ post.score >= 0 ? '+' : '' }}{{ post.score }}</span>
    </footer>
  </article>
</template>
