<script setup lang="ts">
import ClientVersionGuideModal from './components/ClientVersionGuideModal.vue'
import ClientVersionPolicyCard from './components/ClientVersionPolicyCard.vue'
import { useClientDownloads } from './composables/useClientDownloads'

defineOptions({ name: 'ClientVersionSettings' })

withDefaults(
  defineProps<{
    provider: 'openai' | 'anthropic'
    loading?: boolean
  }>(),
  {
    loading: false,
  },
)

const minCodexDesktopVersion = defineModel<string>('minCodexDesktopVersion', { required: true })
const minCodexCliVersion = defineModel<string>('minCodexCliVersion', { required: true })
const minClaudeCodeVersion = defineModel<string>('minClaudeCodeVersion', { required: true })

const {
  open,
  loading: downloadsLoading,
  downloads,
  load,
  showGuide,
} = useClientDownloads()
</script>

<template>
  <ClientVersionPolicyCard
    v-model:min-codex-desktop-version="minCodexDesktopVersion"
    v-model:min-codex-cli-version="minCodexCliVersion"
    v-model:min-claude-code-version="minClaudeCodeVersion"
    :provider="provider"
    :loading="loading"
    @help="showGuide"
  >
    <template #actions>
      <slot name="actions" />
    </template>
    <template #scope>
      <slot name="scope" />
    </template>
    <template #footer>
      <slot name="footer" />
    </template>
  </ClientVersionPolicyCard>

  <ClientVersionGuideModal
    v-model="open"
    :downloads="downloads"
    :loading="downloadsLoading"
    @refresh="load(true)"
  />
</template>
