<script setup lang="ts">
import type { ClaudeProfileSelection, ClientProfileSelection, ProviderRequestProfile, ProviderRequestProfiles, ProviderRequestProfileUpdates } from '@/api/modules/settings/profiles'

import { ZSegmented } from '@codex-proxy/ui'
import { isEqual } from 'es-toolkit'
import { computed, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { formatProviderLabel, PROVIDER_IDS, providerIcon } from '@/utils/providers'
import ClaudeClientProfileEditor from './ClaudeClientProfileEditor.vue'
import ClientProfileEditor from './ClientProfileEditor.vue'

const props = withDefaults(defineProps<{
  active?: boolean
  disabled?: boolean
  allowInherit?: boolean
  initialProfiles?: ProviderRequestProfileUpdates
  provider?: (typeof PROVIDER_IDS)[number]
}>(), {
  active: true,
  disabled: false,
  allowInherit: false,
})

const { t } = useI18n({ useScope: 'global' })

const model = defineModel<ProviderRequestProfiles>({
  default: () => ({}),
})
const updates = defineModel<ProviderRequestProfileUpdates>('updates')
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
  return computed<T | null | undefined>({
    get: () => {
      const source = updates.value === undefined
        ? model.value
        : Object.hasOwn(updates.value, providerId)
          ? updates.value
          : props.initialProfiles
      const profile = source?.[providerId]
      if (updates.value !== undefined && profile === undefined)
        return undefined
      if (!profile || profile.inherit)
        return null
      // The profile preview endpoint accepts client fields only; seeds remain in the Key override.
      const configuration = { ...profile }
      delete configuration.installationSeed
      return configuration as T
    },
    set: (value) => {
      if (updates.value === undefined) {
        updateProfile(providerId, value as ProviderRequestProfile | null)
        return
      }
      const next = { ...updates.value }
      const configuration = value == null
        ? value
        : { ...value } as ProviderRequestProfile
      // 批量编辑只提交画像字段，安装种子由独立字段控制并在后端按每把 Key 合并。
      if (configuration)
        delete configuration.installationSeed
      if (configuration === undefined || isEqual(configuration, props.initialProfiles?.[providerId]))
        delete next[providerId]
      else
        next[providerId] = configuration
      updates.value = next
    },
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
      :allow-preserve="updates !== undefined && initialProfiles?.[provider] === undefined"
      :active="active"
      :disabled="disabled"
    />
    <ClaudeClientProfileEditor v-else-if="provider === 'anthropic'" v-model="anthropic" :allow-inherit="allowInherit" :allow-preserve="updates !== undefined && initialProfiles?.[provider] === undefined" :active="active" :disabled="disabled" />
  </div>
</template>
