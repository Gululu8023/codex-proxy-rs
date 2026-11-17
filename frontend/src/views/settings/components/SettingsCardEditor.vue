<script setup lang="ts">
import type { Component } from 'vue'
import type { useSettingsForm } from '../composables/useSettingsForm'
import type { ProviderSettingsCard, SettingsProvider, SettingsScope } from '../utils/scope'
import { ZSegmented } from '@codex-proxy/ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { formatProviderLabel, PROVIDER_IDS, providerIcon } from '@/utils/providers'

defineOptions({ inheritAttrs: false })
const props = defineProps<{
  component: Component
  state: ReturnType<typeof useSettingsForm>
  scope?: SettingsScope
  providerCard?: ProviderSettingsCard
  codexOnly?: boolean
}>()
const provider = defineModel<SettingsProvider>('provider', { default: 'openai' })
const { t } = useI18n({ useScope: 'global' })
const scope = computed<SettingsScope>(() => props.scope ?? `${provider.value}-${props.providerCard!}`)
const disabled = computed(() => props.state.saving.value || props.state.loading.value || props.state.detectingLocation.value || !!props.state.error.value)
const options = computed(() => PROVIDER_IDS.map(value => ({
  value,
  label: formatProviderLabel(value),
  icon: providerIcon(value),
})))
</script>

<template>
  <fieldset :disabled="disabled" :data-settings-scope="scope" class="m-0 min-w-0 w-full max-w-6xl border-0 p-0">
    <component :is="component" v-bind="{ ...$attrs, ...(providerCard ? { provider } : {}) }">
      <template #actions>
        <span v-if="codexOnly" class="rounded-z bg-z-fill-quaternary px-2 py-1 text-z-xs text-z-text-secondary">{{ t('settings.cards.codexOnly') }}</span>
      </template>
      <template #scope>
        <div v-if="providerCard" class="mb-5 flex min-w-0 flex-wrap items-center gap-3">
          <span class="text-z-sm text-z-text-secondary">{{ t('settings.scopes.label') }}</span>
          <ZSegmented v-model="provider" :options="options" :aria-label="t('settings.scopes.label')" :disabled="disabled" />
        </div>
      </template>
    </component>
  </fieldset>
</template>
