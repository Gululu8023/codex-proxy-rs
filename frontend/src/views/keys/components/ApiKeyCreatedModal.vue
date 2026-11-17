<script setup lang="ts">
import { ZButton, ZDialog, ZIconButton } from '@codex-proxy/ui'

import { Copy } from '@lucide/vue'
import { useI18n } from 'vue-i18n'

defineProps<{ createdKey: string }>()

const emit = defineEmits<{
  copy: [text: string]
  afterLeave: []
}>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ default: false })
</script>

<template>
  <ZDialog
    v-model="open"
    :title="t('keys.created.apiKeyCreated')"
    :description="t('keys.created.description')"
    type="success"
    width="36rem"
    @closed="emit('afterLeave')"
  >
    <div class="flex flex-col gap-4">
      <div class="rounded-z bg-z-warning-container px-4 py-3">
        <p class="m-0 text-z font-semibold text-z-warning-on-container">
          {{ t('keys.created.accessNotice') }}
        </p>
      </div>
      <div>
        <p class="mb-2 text-z font-medium text-z-text-secondary">
          {{ t('common.fields.apiKey') }}
        </p>
        <div class="flex items-center gap-2">
          <code class="flex-1 rounded-z bg-z-fill-quaternary px-3 py-2.5 font-mono text-z break-all text-z-text">
            {{ createdKey }}
          </code>
          <ZIconButton size="default" :aria-label="t('common.actions.copy')" @click="emit('copy', createdKey)">
            <Copy class="size-4" />
          </ZIconButton>
        </div>
      </div>
    </div>

    <template #footer>
      <ZButton @click="emit('copy', createdKey)">
        <template #icon>
          <Copy class="size-4" />
        </template>
        {{ t('common.actions.copy') }}
      </ZButton>
      <ZButton type="primary" @click="open = false">
        {{ t('keys.created.iHaveSaved') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
