<script setup lang="ts">
import { ZCard, ZFormItem, ZIconButton, ZInput, ZPopover } from '@codex-proxy/ui'
import { CircleAlert } from '@lucide/vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n({ useScope: 'global' })
const model = defineModel<string>({ required: true })
</script>

<template>
  <ZCard :title="t('settings.concurrency.autoReview.label')" :description="t('settings.concurrency.autoReview.cardDescription')">
    <template #actions>
      <slot name="actions" />
    </template>
    <div class="grid min-w-0 gap-4.5 max-w-6xl sm:grid-cols-2 sm:items-end">
      <ZFormItem :show-message="Boolean(model.trim())" prop="openaiGuardianReservedConcurrency" :label="t('settings.concurrency.autoReview.slotsLabel')" :description="t('settings.concurrency.autoReview.description')">
        <template #label-extra>
          <ZPopover class="-my-1" trigger="hover-click" placement="top-start">
            <template #reference="{ open }">
              <ZIconButton variant="link" :aria-label="t('settings.concurrency.autoReview.helpTitle')" :aria-expanded="open" class="size-6!">
                <CircleAlert class="size-3.5" aria-hidden="true" />
              </ZIconButton>
            </template>
            <div class="max-w-72 space-y-2 px-3 py-2 text-z-sm leading-relaxed text-z-text-secondary">
              <p>{{ t('settings.scheduling.autoReviewSlotsHelp') }}</p>
              <p>{{ t('settings.scheduling.autoReviewQueueHelp') }}</p>
            </div>
          </ZPopover>
        </template>
        <ZInput v-model="model" :aria-label="t('settings.concurrency.autoReview.slotsLabel')" type="number" min="0" max="4294967295" step="1" />
      </ZFormItem>
    </div>
    <slot name="footer" />
  </ZCard>
</template>
