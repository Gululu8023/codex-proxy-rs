<script setup lang="ts">
import type { FormRule } from '@codex-proxy/ui'
import type { SettingsProvider } from '../utils/scope'
import { ZButton, ZCard, ZFormItem, ZIconButton, ZInput } from '@codex-proxy/ui'

import { Plus, Trash2 } from '@lucide/vue'

import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = withDefaults(defineProps<{
  provider: SettingsProvider
  mappings: Array<{ requestedModel: string, upstreamModel: string }>
  anthropicMappings: Array<{ requestedModel: string, upstreamModel: string }>
  loading?: boolean
}>(), {
  loading: false,
})

const emit = defineEmits<{
  addMapping: [provider: SettingsProvider]
  updateMapping: [provider: SettingsProvider, index: number, key: 'requestedModel' | 'upstreamModel', value: string]
  removeMapping: [provider: SettingsProvider, index: number]
}>()

const { t } = useI18n({ useScope: 'global' })
// 两组字段始终挂载，统一保存时也要校验隐藏供应商的完整性和重复映射。
const groups = computed(() => [
  { provider: 'openai' as const, fieldName: 'modelMappings', rows: props.mappings },
  { provider: 'anthropic' as const, fieldName: 'anthropicModelMappings', rows: props.anthropicMappings },
])
const requiredRule = computed<FormRule>(() => ({
  required: true,
  whitespace: true,
  trigger: ['blur', 'change'],
  message: t('settings.form.fillInModelMappingCompletely'),
}))
function uniqueModelRule(rows: typeof props.mappings): FormRule {
  return {
    trigger: ['blur', 'change'],
    validator: (_rule, value: string) => {
      const requested = value.trim()
      return !requested || rows.filter(row => row.requestedModel.trim() === requested).length < 2
        || new Error(t('settings.form.duplicateClientModel', { requested }))
    },
  }
}
</script>

<template>
  <ZCard
    :title="t('settings.modelMapping')"
    :description="t('settings.routing.modelMapHelp')"
  >
    <template #actions>
      <slot name="actions" />
    </template>
    <slot name="scope" />
    <div v-for="group in groups" v-show="provider === group.provider" :key="group.provider" class="grid gap-4">
      <div class="flex flex-wrap items-center gap-3">
        <ZButton :disabled="loading" @click="emit('addMapping', group.provider)">
          <template #icon>
            <Plus class="size-4" />
          </template>
          {{ t('settings.addMapping') }}
        </ZButton>
      </div>

      <div
        v-if="loading"
        class="rounded-z bg-z-fill-quaternary px-4 py-4 text-z font-emphasis text-z-text-quaternary"
      >
        {{ t('settings.loadingModelMapping') }}
      </div>
      <div v-else-if="group.rows.length > 0" class="grid max-w-6xl gap-3">
        <div
          v-for="(row, index) in group.rows"
          :key="index"
          class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-4.5 rounded-z-card bg-z-fill-quaternary p-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
        >
          <ZFormItem :prop="`${group.fieldName}.${index}.requestedModel`" :rules="[requiredRule, uniqueModelRule(group.rows)]" :show-message="Boolean(row.requestedModel.trim())">
            <ZInput
              :model-value="row.requestedModel"
              :placeholder="t('settings.requestModel')"
              :aria-label="t('settings.requestModel')"
              @update:model-value="emit('updateMapping', group.provider, index, 'requestedModel', $event)"
            />
          </ZFormItem>
          <span class="hidden text-z-text-quaternary sm:block" aria-hidden="true">→</span>
          <ZFormItem :prop="`${group.fieldName}.${index}.upstreamModel`" :rules="requiredRule" :show-message="false" class="col-start-1 row-start-2 sm:col-start-auto sm:row-start-auto">
            <ZInput
              :model-value="row.upstreamModel"
              :placeholder="t('settings.upstreamModel')"
              :aria-label="t('settings.upstreamModelName')"
              @update:model-value="emit('updateMapping', group.provider, index, 'upstreamModel', $event)"
            />
          </ZFormItem>
          <ZIconButton
            class="col-start-2 row-span-2 row-start-1 sm:col-start-auto sm:row-span-1 sm:row-start-auto"
            :aria-label="t('settings.deleteMapping')"
            @click="emit('removeMapping', group.provider, index)"
          >
            <Trash2 class="size-4 text-z-error" />
          </ZIconButton>
        </div>
      </div>
    </div>
    <slot name="footer" />
  </ZCard>
</template>
