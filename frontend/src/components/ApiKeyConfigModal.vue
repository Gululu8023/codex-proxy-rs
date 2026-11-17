<script setup lang="ts">
import { ZButton, ZDialog, ZIconButton, ZScrollbar, ZSegmented, ZSwitch } from '@codex-proxy/ui'

import { Apple, Copy, Monitor, Upload } from '@lucide/vue'

import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { buildClaudeConfig, buildCodexCcSwitchImportDeeplink, buildCodexConfig, CODEX_WEBSOCKET_ENABLED_BY_DEFAULT } from '@/utils/client'
import ClientImportModal from './ClientImportModal.vue'

const props = defineProps<{
  title?: string
  apiKey: {
    id?: string
    name?: string
    providerKinds?: string[]
    key?: string
  } | null
  apiBaseUrl: string
  claudePublicUrl?: string
}>()

const emit = defineEmits<{
  copy: [text: string]
  afterLeave: []
}>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ default: false })
const showImport = shallowRef(false)

const activeClient = shallowRef('codex')
// Claude 对外使用站点根路径，不带管理 API 的开发代理前缀 /dev。
const claudeBaseUrl = computed(() => props.claudePublicUrl?.trim() || `${window.location.origin}/claude`)
const activePlatform = shallowRef('unix')
const clientOptions = computed(() => [
  { label: 'Codex', value: 'codex', provider: 'openai' },
  { label: 'Claude Code', value: 'claude', provider: 'anthropic' },
].filter(option => !props.apiKey?.providerKinds?.length || props.apiKey.providerKinds.includes(option.provider)))
watch(clientOptions, (options) => {
  if (!options.some(option => option.value === activeClient.value))
    activeClient.value = options[0]?.value ?? ''
}, { immediate: true })
const isClaude = computed(() => activeClient.value === 'claude')
const websocketEnabled = shallowRef(CODEX_WEBSOCKET_ENABLED_BY_DEFAULT)

const platformOptions = [
  { label: 'macOS / Linux', value: 'unix', icon: Apple },
  { label: 'Windows', value: 'windows', icon: Monitor },
]

const keyValue = computed(() => props.apiKey?.key ?? '')
const configPath = computed(() => {
  const [directory, file] = isClaude.value
    ? ['.claude', 'settings.json']
    : ['.codex', 'config.toml']
  return activePlatform.value === 'windows'
    ? `%userprofile%\\${directory}\\${file}`
    : `~/${directory}/${file}`
})
const config = computed(() => isClaude.value
  ? buildClaudeConfig({ apiKey: keyValue.value, baseUrl: claudeBaseUrl.value })
  : buildCodexConfig({
      apiKey: keyValue.value,
      baseUrl: props.apiBaseUrl,
      websocketEnabled: websocketEnabled.value,
    }))

function importToClient() {
  if (!keyValue.value)
    return
  window.location.href = buildCodexCcSwitchImportDeeplink({
    apiKey: keyValue.value,
    baseUrl: props.apiBaseUrl,
    providerName: props.apiKey?.name || 'codex-proxy-rs',
  })
}
</script>

<template>
  <ZDialog
    v-model="open"
    :title="title ?? t('keys.configuration.title')"
    :description="t('keys.configuration.description')"
    width="48rem"
    @closed="showImport = false; emit('afterLeave')"
  >
    <div class="flex flex-col gap-5">
      <ZSegmented v-model="activeClient" :aria-label="t('keys.configuration.client')" :options="clientOptions" />
      <div class="flex flex-wrap items-center justify-between gap-3">
        <ZSegmented v-model="activePlatform" :aria-label="t('keys.configuration.platform')" :options="platformOptions" />
        <ZSwitch
          v-if="!isClaude"
          v-model="websocketEnabled"
          :aria-label="t('keys.configuration.toggleWebSocket')"
          active-text="WS"
          inactive-text="WS"
          inline-prompt
          :width="56"
        />
      </div>

      <section class="overflow-hidden rounded-z-card bg-z-fill-quaternary shadow-z-tertiary">
        <div class="flex items-center justify-between gap-3 px-4 py-2.5">
          <span
            class="min-w-0 truncate font-mono text-z-sm font-emphasis text-z-text-secondary"
          >
            {{ configPath }}
          </span>
          <ZIconButton
            variant="solid"
            size="small"
            :aria-label="t('common.actions.copy')"
            :disabled="!config"
            @click="emit('copy', config)"
          >
            <Copy class="size-3.5" />
          </ZIconButton>
        </div>
        <ZScrollbar max-height="calc(100dvh - 21rem)">
          <div class="mx-3 mb-3 rounded-z bg-z-bg-container px-3.5 py-3 shadow-z-tertiary">
            <pre
              class="m-0 whitespace-pre-wrap wrap-break-word font-mono text-z-sm leading-[1.65] font-emphasis text-z-text"
              v-text="config"
            />
          </div>
        </ZScrollbar>
      </section>
    </div>

    <template #footer>
      <ZButton v-if="!isClaude" :disabled="!keyValue" @click="showImport = true">
        <Upload class="size-4" />
        {{ t('keys.configuration.importTo') }}
      </ZButton>
      <ZButton type="primary" @click="open = false">
        {{ t('common.actions.close') }}
      </ZButton>
    </template>
  </ZDialog>
  <ClientImportModal v-if="open && !isClaude" v-model="showImport" @select="importToClient" />
</template>
