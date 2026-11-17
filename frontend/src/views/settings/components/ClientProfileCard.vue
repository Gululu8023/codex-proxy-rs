<script setup lang="ts">
import type { ProviderRequestProfiles } from '@/api/modules/settings/profiles'

import { ZCard } from '@codex-proxy/ui'
import { useI18n } from 'vue-i18n'
import ProviderRequestProfilesEditor from '@/components/client-profile/ProviderRequestProfilesEditor.vue'

withDefaults(defineProps<{ provider: 'openai' | 'anthropic', active?: boolean, disabled?: boolean }>(), {
  active: true,
  disabled: false,
})

const { t } = useI18n({ useScope: 'global' })

const model = defineModel<ProviderRequestProfiles>({ required: true })
</script>

<template>
  <ZCard :title="t('settings.clientProfile.clientIdentity')" :description="t('settings.clientProfile.description')">
    <template #actions>
      <slot name="actions" />
    </template>
    <slot name="scope" />
    <ProviderRequestProfilesEditor
      v-model="model"
      :provider="provider"
      :active="active"
      :disabled="disabled"
      class="max-w-6xl"
    />
    <slot name="footer" />
  </ZCard>
</template>
