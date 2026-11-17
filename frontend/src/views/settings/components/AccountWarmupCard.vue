<script setup lang="ts">
import { ZCard, ZFormItem, ZIconButton, ZInput, ZPopover, ZSwitch } from '@codex-proxy/ui'

import { CircleAlert, Clock, Sparkles } from '@lucide/vue'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n({ useScope: 'global' })

const enabled = defineModel<boolean>('enabled', { required: true })
const cron = defineModel<string[]>('cron', { required: true })
const model = defineModel<string>('model', { required: true })
const cronText = computed({
  get: () => cron.value.join(';'),
  // 编辑时保留空格，提交时再裁剪，避免无法输入 Cron 字段间的分隔空格。
  set: (value: string) => {
    cron.value = value
      ? value.split(';')
      : []
  },
})
</script>

<template>
  <ZCard
    :description="t('settings.warmup.description')"
  >
    <template #actions>
      <slot name="actions" />
    </template>
    <template #title>
      <span class="inline-flex items-center gap-1">
        {{ t('settings.warmup.title') }}
        <ZPopover trigger="hover-click" placement="top-start">
          <template #reference="{ open }">
            <ZIconButton variant="link" :aria-label="t('settings.warmup.helpTitle')" :aria-expanded="open" class="size-6!">
              <CircleAlert class="size-3.5" aria-hidden="true" />
            </ZIconButton>
          </template>
          <div class="max-w-80 space-y-2 px-3 py-2 text-z-sm font-regular leading-relaxed text-z-text-secondary">
            <p>{{ t('settings.warmup.help') }}</p>
            <p>{{ t('settings.warmup.scheduleHelp') }}</p>
          </div>
        </ZPopover>
      </span>
    </template>
    <div class="grid min-w-0 gap-4.5 max-w-6xl sm:grid-cols-2 sm:items-end">
      <ZSwitch
        v-model="enabled"
        class="col-span-full justify-self-start"
        :label="t('settings.warmup.enableWarmup')"
      />

      <ZFormItem
        :show-message="enabled || cron.length > 0"
        prop="accountWarmupCron"
        :label="t('settings.warmup.cronExpression')"
        :description="t('settings.warmup.cronHelp')"
        :required="enabled"
      >
        <ZInput
          v-model="cronText"
          :disabled="!enabled"
          :aria-label="t('settings.warmup.cronExpression')"
          placeholder="0 8 * * *"
          maxlength="255"
          spellcheck="false"
        >
          <template #prefix>
            <Clock class="size-4" />
          </template>
        </ZInput>
      </ZFormItem>

      <ZFormItem
        :show-message="Boolean(model.trim())"
        prop="accountWarmupModel"
        :label="t('settings.warmup.modelLabel')"
        :description="t('settings.warmup.modelHelp')"
        :required="enabled"
      >
        <ZInput
          v-model="model"
          :disabled="!enabled"
          :aria-label="t('settings.warmup.modelLabel')"
          :placeholder="t('settings.warmup.modelPlaceholder')"
        >
          <template #prefix>
            <Sparkles class="size-4" />
          </template>
        </ZInput>
      </ZFormItem>
    </div>
    <slot name="footer" />
  </ZCard>
</template>
