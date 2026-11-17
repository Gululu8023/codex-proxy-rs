<script setup lang="ts">
import type { PromptGuardConfig } from '@/api'
import { ZButton, ZCard, ZForm, ZFormItem, ZInput, ZSegmented, ZSwitch } from '@codex-proxy/ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import GuardHelp from './GuardHelp.vue'

defineProps<{ disabled?: boolean }>()
const { t, te } = useI18n({ useScope: 'global' })
// 类别名来自服务端配置；已知类别显示本地化名称，原始 API 名称保留在说明里便于对照上游文档。
function categoryLabel(category: string) {
  const key = `guard.flow.moderationCategory.${category}`
  return te(key)
    ? t(key)
    : category
}
const model = defineModel<PromptGuardConfig>({ required: true })
const key = computed({
  get: () => model.value.reviewApiKey ?? '',
  set: (value: string) => { model.value.reviewApiKey = value || undefined },
})
const modes = computed(() => ['enforce', 'observe'].map(value => ({
  value,
  label: t(`guard.mode.${value}`),
})))
const localActions = computed(() => ['warn', 'block'].map(value => ({
  value,
  label: t(`guard.flow.local.${value}`),
})))
const scopes = computed(() => ['local_candidates', 'all'].map(value => ({
  value,
  label: t(`guard.flow.scope.${value}`),
})))
const requestModes = [
  { value: 'chat_completions', label: 'Chat Completions' },
  { value: 'moderations', label: 'Moderations' },
]
</script>

<template>
  <!-- 与系统设置页一致：每个分组一张卡片；分组自身的开关放在卡片标题右侧，较长说明收进图标气泡。 -->
  <ZForm class="grid min-w-0 max-w-6xl gap-5">
    <ZCard :title="t('guard.layout.local')">
      <div class="grid min-w-0 gap-4.5 sm:grid-cols-2 sm:items-end">
        <ZSwitch v-model="model.enabled" class="col-span-full justify-self-start" :disabled="disabled" :label="t('settings.promptGuard.enabled')" />
        <ZFormItem :label="t('guard.modeLabel')">
          <ZSegmented v-model="model.mode" class="w-full" :options="modes" :aria-label="t('guard.modeLabel')" :disabled="disabled" />
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.localAction')">
          <ZSegmented v-model="model.localAction" class="w-full" :options="localActions" :aria-label="t('guard.flow.localAction')" :disabled="disabled" />
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.threshold')">
          <ZInput :model-value="String(model.threshold)" type="number" min="1" max="1000" :disabled="disabled" @update:model-value="model.threshold = Number($event)" />
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.strictThreshold')">
          <ZInput :model-value="String(model.strictThreshold)" type="number" :min="model.threshold" max="1000" :disabled="disabled" @update:model-value="model.strictThreshold = Number($event)" />
        </ZFormItem>
        <div class="col-span-full flex items-center gap-1">
          <ZSwitch v-model="model.strictTerminalEnabled" :disabled="disabled" :label="t('guard.flow.strictTerminal')" />
          <GuardHelp :label="t('guard.flow.strictTerminal')" :text="t('guard.flow.strictHelp')" />
        </div>
      </div>
    </ZCard>

    <ZCard :title="t('guard.layout.review')" :description="t('guard.flow.reviewHelp')">
      <template #actions>
        <ZSwitch v-model="model.reviewEnabled" :aria-label="t('guard.flow.reviewEnabled')" :active-text="t('common.values.on')" :inactive-text="t('common.values.off')" inline-prompt :width="56" :disabled="disabled" />
      </template>
      <div class="grid min-w-0 gap-4.5 sm:grid-cols-2 sm:items-end">
        <ZFormItem :label="t('guard.flow.reviewScope')">
          <ZSegmented v-model="model.scope" class="w-full" :options="scopes" :aria-label="t('guard.flow.reviewScope')" :disabled="disabled" />
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.requestMode')">
          <ZSegmented v-model="model.reviewRequestMode" class="w-full" :options="requestModes" :aria-label="t('guard.flow.requestMode')" :disabled="disabled" />
        </ZFormItem>
        <ZFormItem :label="t('settings.promptGuard.url')" :description="t('settings.promptGuard.egress')">
          <ZInput v-model="model.reviewUrl" :disabled="disabled" :placeholder="`https://review.example.com/v1/${model.reviewRequestMode === 'moderations' ? 'moderations' : 'chat/completions'}`" />
        </ZFormItem>
        <ZFormItem :label="t('settings.promptGuard.model')">
          <ZInput v-model="model.reviewModel" :disabled="disabled" />
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.keyPool')" :description="t('guard.flow.keyPoolHelp', { count: model.apiKeyCount })" class="col-span-full">
          <div class="flex flex-wrap gap-2">
            <ZInput v-model="key" type="password" autocomplete="new-password" :disabled="disabled" :aria-label="t('guard.flow.keyPool')" class="min-w-48 flex-1" />
            <ZButton :disabled="disabled || !model.hasApiKey" @click="model.reviewApiKey = ''; model.hasApiKey = false">
              {{ t('settings.promptGuard.clearKey') }}
            </ZButton>
          </div>
        </ZFormItem>
        <ZFormItem :label="t('guard.flow.maxConcurrent')">
          <ZInput :model-value="String(model.reviewMaxConcurrent)" type="number" min="1" max="64" :disabled="disabled" @update:model-value="model.reviewMaxConcurrent = Number($event)" />
        </ZFormItem>
        <ZFormItem :label="t('settings.promptGuard.timeout')">
          <ZInput :model-value="String(model.timeoutSeconds)" type="number" min="1" max="60" :disabled="disabled" @update:model-value="model.timeoutSeconds = Number($event)">
            <template #suffix>
              {{ t('settings.units.seconds') }}
            </template>
          </ZInput>
        </ZFormItem>
        <ZFormItem :label="t('settings.promptGuard.budget')">
          <ZInput :model-value="String(model.maxTextChars)" type="number" min="1" max="262144" :disabled="disabled" @update:model-value="model.maxTextChars = Number($event)" />
        </ZFormItem>
        <ZFormItem v-if="model.reviewRequestMode === 'chat_completions'" :label="t('settings.promptGuard.threshold')">
          <ZInput :model-value="String(model.confidenceThresholdBps / 100)" type="number" min="0.01" max="100" step="0.01" :disabled="disabled" @update:model-value="model.confidenceThresholdBps = Math.round(Number($event) * 100)">
            <template #suffix>
              %
            </template>
          </ZInput>
        </ZFormItem>
        <div class="col-span-full flex items-center gap-1">
          <ZSwitch v-model="model.reviewFailClosed" :disabled="disabled" :label="t('guard.flow.failClosed')" />
          <GuardHelp :label="t('guard.flow.failClosed')" :text="t('guard.flow.failHelp')" />
        </div>
        <details v-if="model.reviewRequestMode === 'moderations'" class="col-span-full">
          <summary class="cursor-pointer text-z-sm text-z-text-secondary">
            {{ t('guard.flow.moderationThresholds') }}
          </summary>
          <div class="mt-3 grid gap-4 sm:grid-cols-3">
            <ZFormItem v-for="(_, category) in model.moderationThresholds" :key="category" :label="categoryLabel(category)" :description="category">
              <ZInput :model-value="String((model.moderationThresholds[category] ?? 0) / 100)" type="number" min="0.01" max="100" step="0.01" :disabled="disabled" @update:model-value="model.moderationThresholds[category] = Math.round(Number($event) * 100)">
                <template #suffix>
                  %
                </template>
              </ZInput>
            </ZFormItem>
          </div>
        </details>
      </div>
    </ZCard>

    <ZCard :title="t('guard.layout.locks')" :description="t('guard.flow.lockHelp')">
      <template #actions>
        <ZSwitch v-model="model.conversationLockEnabled" :aria-label="t('guard.flow.lockEnabled')" :active-text="t('common.values.on')" :inactive-text="t('common.values.off')" inline-prompt :width="56" :disabled="disabled" />
      </template>
      <div class="grid min-w-0 gap-4.5 sm:grid-cols-2 sm:items-end">
        <ZFormItem :label="t('guard.flow.lockHours')">
          <ZInput :model-value="String(model.conversationLockTtlHours)" type="number" min="1" max="720" :disabled="disabled || !model.conversationLockEnabled" @update:model-value="model.conversationLockTtlHours = Number($event)">
            <template #suffix>
              {{ t('settings.affinity.hours') }}
            </template>
          </ZInput>
        </ZFormItem>
      </div>
    </ZCard>

    <ZCard :title="t('guard.layout.learning')" :description="t('guard.flow.learningHelp')">
      <template #actions>
        <ZSwitch v-model="model.learningEnabled" :aria-label="t('guard.flow.learningEnabled')" :active-text="t('common.values.on')" :inactive-text="t('common.values.off')" inline-prompt :width="56" :disabled="disabled" />
      </template>
      <div v-if="model.reviewRequestMode === 'moderations'" class="grid min-w-0 gap-4.5 sm:grid-cols-2 sm:items-end">
        <ZFormItem :label="t('guard.flow.learningModel')" :description="t('guard.flow.learningModelHelp')">
          <ZInput v-model="model.learningModel" :disabled="disabled" />
        </ZFormItem>
      </div>
    </ZCard>
  </ZForm>
</template>
