<script setup lang="ts">
import { ZCard } from '@codex-proxy/ui'
import { useI18n } from 'vue-i18n'

const { t } = useI18n({ useScope: 'global' })
</script>

<template>
  <ZCard :title="t('guard.tabs.help')" class="max-w-6xl">
    <div class="space-y-5 text-z-sm leading-6 text-z-text-secondary">
      <section v-for="key in ['local', 'review', 'upstream', 'learning', 'coverage', 'retention']" :key="key">
        <h3 class="mb-1.5 text-z font-emphasis text-z-text">
          {{ t(`guard.layout.${key}`) }}
        </h3>
        <p>{{ t(`guard.flow.help.${key}`) }}</p>
      </section>
    </div>
  </ZCard>
</template>
