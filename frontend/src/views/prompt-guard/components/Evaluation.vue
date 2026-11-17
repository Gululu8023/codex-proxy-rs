<script setup lang="ts">
import type { GuardAssessment } from '@/api/modules/claude'
import { ZButton, ZCard, ZSelect, ZTextarea } from '@codex-proxy/ui'
import { computed, ref, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { evaluateGuard } from '@/api/modules/claude'
import { useRequestState } from '@/composables/useRequestState'

const { t } = useI18n({ useScope: 'global' })
const text = ref('')
const origin = ref('current_user')
const result = shallowRef<GuardAssessment | null>(null)
const request = useRequestState()
const { loading, error } = request
const origins = computed(() => [
  'current_user',
  'system',
  'history',
  'tool_output',
].map(value => ({
  value,
  label: t(`guard.flow.origin.${value}`),
})))
async function evaluate() {
  const id = request.start()
  result.value = null
  try {
    const value = await evaluateGuard({ text: text.value, origin: origin.value }, { signal: request.signal })
    if (request.isCurrent(id))
      result.value = value
  }
  catch (cause) { request.fail(id, cause) }
  finally { request.finish(id) }
}
</script>

<template>
  <ZCard :title="t('guard.flow.evaluate')" :description="t('guard.flow.evaluateHelp')">
    <div class="space-y-3">
      <div class="grid max-w-xs gap-1.5">
        <span class="text-z-sm text-z-text-secondary">{{ t('guard.origin') }}</span><ZSelect v-model="origin" :options="origins" :aria-label="t('guard.origin')" />
      </div>
      <ZTextarea v-model="text" :autosize="{ minRows: 3, maxRows: 12 }" maxlength="262144" :spellcheck="false" :aria-label="t('guard.flow.evaluationText')" class="font-mono" />
      <ZButton :loading="loading" :disabled="!text.trim()" @click="evaluate">
        {{ t('guard.flow.evaluate') }}
      </ZButton>
      <p v-if="error" role="alert" class="text-z-error-text">
        {{ error }}
      </p>
      <div v-if="result" class="space-y-2 text-z-sm">
        <p>{{ t(`guard.action.${result.action}`) }} · {{ t('guard.decisionScore') }} {{ result.decisionScore }} · {{ t('guard.auditScore') }} {{ result.auditScore }}</p>
        <p>{{ t('guard.would', { action: t(`guard.action.${result.wouldAction}`) }) }}</p>
        <pre class="max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-z bg-z-fill-quaternary p-3 text-z-xs">{{ JSON.stringify(result.hits, null, 2) }}</pre>
      </div>
    </div>
  </ZCard>
</template>
