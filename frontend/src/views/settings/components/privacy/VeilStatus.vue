<script setup lang="ts">
import type { VeilHealth } from '@/api/modules/claude'
import { ZButton } from '@codex-proxy/ui'
import { onMounted, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { getVeilHealth } from '@/api/modules/claude'
import { useRequestState } from '@/composables/useRequestState'

const { t } = useI18n({ useScope: 'global' })
const query = useRequestState()
const { loading, error } = query
const health = shallowRef<VeilHealth | null>(null)
async function load() {
  const id = query.start()
  health.value = null
  try {
    const result = await getVeilHealth({ signal: query.signal })
    if (query.isCurrent(id))
      health.value = result
  }
  catch (cause) {
    query.fail(id, cause)
  }
  finally {
    query.finish(id)
  }
}
onMounted(load)
</script>

<template>
  <div class="space-y-3">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <p v-if="health" role="status" :class="health.ready ? 'text-z-success-text' : 'text-z-error-text'">
        {{ health.ready ? t('claude.privacyReady') : t('claude.privacyUnavailable') }}
      </p>
      <p v-else-if="loading" role="status">
        {{ t('common.status.loading') }}
      </p>
      <ZButton size="small" :loading="loading" @click="load">
        {{ t('common.actions.refresh') }}
      </ZButton>
    </div>
    <p v-if="error" role="alert" class="text-z-error-text">
      {{ error }}
    </p>
    <p v-if="health && !health.ready && health.lastError" role="alert" class="text-z-sm text-z-error-text">
      {{ health.lastError }}
    </p>
    <p v-if="health?.keyFingerprint" class="text-z-sm text-z-text-secondary">
      {{ t('claude.privacyKeyFingerprint') }}
      <code class="break-all font-mono text-z-xs">{{ health.keyFingerprint }}</code>
    </p>
    <p class="text-z-sm text-z-text-secondary">
      {{ t('claude.privacyHelp') }}
    </p>
  </div>
</template>
