<script setup lang="ts">
import type { VeilHealth } from '@/api/modules/claude'
import { ZButton } from '@codex-proxy/ui'
import { ChevronDown, Lock, RefreshCw } from '@lucide/vue'
import { computed, onMounted, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { getVeilHealth } from '@/api/modules/claude'
import { useCopyText } from '@/composables/useCopyText'
import { useRequestState } from '@/composables/useRequestState'
import VeilRulesTable from './VeilRulesTable.vue'

const { t } = useI18n({ useScope: 'global' })
const copyText = useCopyText()
const query = useRequestState()
const { loading, error } = query
const health = shallowRef<VeilHealth | null>(null)
const expanded = shallowRef(false)
const enabledCount = computed(() => health.value?.rules?.items.filter(rule => rule.enabled).length ?? 0)
const fingerprints = computed(() => [
  {
    label: t('claude.privacyRulesFingerprint'),
    value: health.value?.rulesRevision,
    title: t('claude.privacyCopyFingerprint'),
  },
  {
    label: t('claude.privacyKeyFingerprint'),
    value: health.value?.keyFingerprint,
    title: t('claude.privacyKeyFingerprintHelp'),
  },
].filter((item): item is { label: string, value: string, title: string } => !!item.value))

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
// 指纹只显示首尾，完整值通过复制获取。
function shortFingerprint(value: string) {
  return value.length > 12
    ? `${value.slice(0, 8)}…${value.slice(-4)}`
    : value
}
onMounted(load)
</script>

<template>
  <div class="grid gap-4">
    <div class="grid gap-2.5 rounded-z-lg bg-z-fill-tertiary px-4 py-3.5">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <p v-if="health" role="status" class="m-0 flex items-center gap-2 font-bold" :class="health.ready ? 'text-z-text' : 'text-z-error-text'">
          <span class="size-2 shrink-0 rounded-full" :class="health.ready ? 'bg-z-success' : 'bg-z-error'" aria-hidden="true" />
          {{ health.ready ? t('claude.privacyReady') : t('claude.privacyUnavailable') }}
        </p>
        <p v-else-if="loading" role="status" class="m-0">
          {{ t('common.status.loading') }}
        </p>
        <span v-else />
        <div class="flex items-center gap-3">
          <span v-if="health" class="text-z-xs text-z-text-tertiary">{{ t('claude.privacyObservedAt', { time: health.observedAt }) }}</span>
          <ZButton size="small" :loading="loading" @click="load">
            <template #icon>
              <RefreshCw class="size-3.5" />
            </template>{{ t('common.actions.refresh') }}
          </ZButton>
        </div>
      </div>
      <p v-if="error" role="alert" class="m-0 text-z-sm text-z-error-text">
        {{ error }}
      </p>
      <p v-if="health && !health.ready && health.lastError" role="alert" class="m-0 text-z-sm text-z-error-text">
        {{ health.lastError }}<template v-if="health.lastErrorAt">
          · {{ health.lastErrorAt }}
        </template>
      </p>
      <div v-if="health?.rules" class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-z-sm text-z-text-secondary">
          <span class="inline-flex h-6 items-center rounded-full px-2 text-z-xs font-bold" :class="health.rules.source === 'custom' ? 'bg-z-blue-container text-z-blue-on-container' : 'bg-z-bg-container text-z-text-secondary'">
            {{ health.rules.source === 'custom' ? t('claude.privacySourceCustom') : t('claude.privacySourceBuiltin') }}
          </span>
          <span>{{ t('claude.privacyEnabledRules', { enabled: enabledCount, total: health.rules.items.length }) }}</span>
          <span v-for="item in fingerprints" :key="item.label" class="inline-flex items-center gap-1.5">
            {{ item.label }}
            <button
              type="button"
              class="cursor-copy border-0 border-b border-dashed border-z-text-quaternary bg-transparent p-0 font-mono text-z-xs text-z-text-secondary"
              :title="item.title"
              @click="copyText(item.value)"
            >
              {{ shortFingerprint(item.value) }}
            </button>
          </span>
        </div>
        <ZButton size="small" variant="text" :aria-expanded="expanded" @click="expanded = !expanded">
          {{ expanded ? t('claude.privacyHideRules') : t('claude.privacyShowRules') }}
          <ChevronDown class="ml-1 size-3.5 transition-transform" :class="{ 'rotate-180': expanded }" />
        </ZButton>
      </div>
      <p class="m-0 flex items-start gap-1.5 border-t border-z-border-secondary pt-2.5 text-z-xs text-z-text-tertiary">
        <Lock class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
        <span>{{ t('claude.privacyReadonly') }}</span>
      </p>
    </div>
    <VeilRulesTable v-if="expanded && health?.rules" :rules="health.rules" :packs="health.packs" />
  </div>
</template>
