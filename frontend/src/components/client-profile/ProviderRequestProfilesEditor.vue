<script setup lang="ts">
import type { ClaudeProfileSelection, ClientProfileSelection, ProviderRequestProfile, ProviderRequestProfiles } from '@/api/modules/settings/profiles'

import { ZSegmented } from '@codex-proxy/ui'
import { computed, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { formatProviderLabel, PROVIDER_IDS, providerIcon } from '@/utils/providers'
import ClaudeClientProfileEditor from './ClaudeClientProfileEditor.vue'
import ClientProfileEditor from './ClientProfileEditor.vue'

const props = withDefaults(defineProps<{
  active?: boolean
  disabled?: boolean
  allowInherit?: boolean
  provider?: (typeof PROVIDER_IDS)[number]
}>(), {
  active: true,
  disabled: false,
  allowInherit: false,
})

const { t } = useI18n({ useScope: 'global' })

const model = defineModel<ProviderRequestProfiles>({ required: true })
const selectedProvider = shallowRef<(typeof PROVIDER_IDS)[number]>('openai')
const provider = computed({
  get: () => props.provider ?? selectedProvider.value,
  set: (value) => { selectedProvider.value = value },
})
const providerOptions = PROVIDER_IDS.map(value => ({
  value,
  label: formatProviderLabel(value),
  icon: providerIcon(value),
}))
const openai = profileModel<ClientProfileSelection>('openai')
const anthropic = profileModel<ClaudeProfileSelection>('anthropic')

function profileModel<T extends object>(providerId: string) {
  return computed<T | null>({
    get: () => {
      const profile = model.value[providerId]
      if (!profile || profile.inherit)
        return null
      // The profile preview endpoint accepts client fields only; seeds remain in the Key override.
      const configuration = { ...profile }
      delete configuration.installationSeed
      return configuration as T
    },
    set: value => updateProfile(providerId, value as ProviderRequestProfile | null),
  })
}

function updateProfile(providerId: string, value: ProviderRequestProfile | null) {
  const profiles = { ...model.value }
  const previous = profiles[providerId]
  const identity = (previous?.inherit as Record<string, unknown> | undefined) ?? previous
  const installationSeed = identity?.installationSeed
  // Inheritance changes only the client profile; a provider's distinct device seed survives it.
  if (value === null) {
    if (installationSeed) {
      profiles[providerId] = {
        inherit: { installationSeed },
      }
    }
    else {
      delete profiles[providerId]
    }
  }
  else {
    profiles[providerId] = installationSeed
      ? { ...value, installationSeed }
      : value
  }
  model.value = profiles
}
</script>

<template>
  <div class="grid min-w-0 gap-4">
    <div v-if="!props.provider || $slots.heading" class="flex flex-wrap items-center gap-3">
      <slot name="heading" />
      <ZSegmented
        v-if="!props.provider"
        v-model="provider"
        class="max-w-full shrink-0"
        :aria-label="t('settings.clientProfile.clientIdentityProvider')"
        :options="providerOptions"
        :disabled="disabled"
      />
    </div>
    <ClientProfileEditor
      v-if="provider === 'openai'"
      v-model="openai"
      :allow-inherit="allowInherit"
      :active="active"
      :disabled="disabled"
    />
    <ClaudeClientProfileEditor v-else-if="provider === 'anthropic'" v-model="anthropic" :allow-inherit="allowInherit" :active="active" :disabled="disabled" />
  </div>
</template>
