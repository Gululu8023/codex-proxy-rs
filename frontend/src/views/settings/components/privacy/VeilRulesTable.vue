<script setup lang="ts">
import type { VeilHealth, VeilRule, VeilRules } from '@/api/modules/claude'
import { defineTableColumns, ZIconButton, ZPagination, ZTable } from '@codex-proxy/ui'
import { Copy } from '@lucide/vue'
import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useCopyText } from '@/composables/useCopyText'

const props = defineProps<{ rules: VeilRules, packs: VeilHealth['packs'] }>()

// 内置规则包为 11 条，单页即可完整展示；只有较长的自定义规则才分页。
const PAGE_SIZE = 15
const { t } = useI18n({ useScope: 'global' })
const copyText = useCopyText()
const page = shallowRef(1)
// 启用规则在前，组内按优先级降序，与引擎的匹配优先顺序一致。
const sorted = computed(() => [...props.rules.items].sort((a, b) =>
  Number(b.enabled) - Number(a.enabled) || b.priority - a.priority))
const rows = computed(() => sorted.value.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE))
const packNames = computed(() => props.packs
  ? (['secrets', 'region'] as const).filter(name => props.packs?.[name])
  : [])
const matcherLabels = computed<Record<VeilRule['matcher'], string>>(() => ({
  regex: t('claude.privacyRule.matchers.regex'),
  dictionary: t('claude.privacyRule.matchers.dictionary'),
  ip: t('claude.privacyRule.matchers.ip'),
}))
const columns = computed(() => defineTableColumns<VeilRule>([
  {
    key: 'priority',
    label: t('claude.privacyRule.priority'),
    kind: 'numeric',
    size: 'xs',
    align: 'right',
  },
  {
    key: 'typePrefix',
    label: t('claude.privacyRule.type'),
    kind: 'custom',
    size: 'sm',
  },
  {
    key: 'id',
    label: t('claude.privacyRule.id'),
    kind: 'custom',
    size: 'sm',
  },
  {
    key: 'matcher',
    label: t('claude.privacyRule.matcher'),
    kind: 'custom',
    size: 'xs',
  },
  {
    key: 'pattern',
    label: t('claude.privacyRule.content'),
    kind: 'custom',
    size: '4xl',
  },
  {
    key: 'enabled',
    label: t('common.fields.status'),
    kind: 'status',
    size: 'xs',
  },
]))
watch(() => props.rules, () => {
  page.value = 1
})
</script>

<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-x-2 gap-y-1.5 text-z-xs text-z-text-tertiary">
      <span>{{ t('claude.privacyRule.source') }}</span>
      <span class="inline-flex h-6 items-center rounded-full px-2 font-bold" :class="rules.source === 'custom' ? 'bg-z-blue-container text-z-blue-on-container' : 'bg-z-fill-tertiary text-z-text-secondary'">
        {{ rules.source === 'custom' ? t('claude.privacySourceCustom') : t('claude.privacySourceBuiltin') }}
      </span>
      <template v-if="packNames.length">
        <span class="mx-1 h-3 w-px bg-z-border-secondary" aria-hidden="true" />
        <span>{{ t('claude.privacyRule.packs') }}</span>
        <span v-for="name in packNames" :key="name" class="inline-flex h-6 items-center rounded-full bg-z-green-container px-2 font-mono font-bold text-z-green-on-container">{{ name }}</span>
      </template>
      <template v-if="rules.allowlist.length">
        <span class="mx-1 h-3 w-px bg-z-border-secondary" aria-hidden="true" />
        <span :title="t('claude.privacyRule.allowlistHelp')">{{ t('claude.privacyRule.allowlist') }}</span>
        <span v-for="item in rules.allowlist" :key="item" class="inline-flex h-6 items-center rounded-full bg-z-fill-tertiary px-2 font-mono font-bold text-z-text-secondary">{{ item }}</span>
      </template>
    </div>
    <p v-if="rules.source === 'custom'" class="m-0 rounded-z bg-z-warning-container px-3 py-2 text-z-xs text-z-warning-on-container">
      {{ t('claude.privacyRule.customReplacesBuiltin') }}
    </p>
    <ZTable :columns="columns" :data="rows" row-key="id" :empty-text="t('claude.privacyRule.empty')">
      <template #typePrefix="{ row }">
        <span class="inline-flex h-6 items-center rounded-full bg-z-blue-container px-2 font-mono text-z-xs font-bold text-z-blue-on-container" :class="{ 'opacity-50': !row.enabled }">{{ row.typePrefix }}</span>
      </template>
      <template #id="{ row }">
        <span class="font-mono font-bold" :class="row.enabled ? 'text-z-text' : 'text-z-text-tertiary'">{{ row.id }}</span>
      </template>
      <template #matcher="{ row }">
        <span :class="{ 'text-z-text-tertiary': !row.enabled }">{{ matcherLabels[row.matcher] }}</span>
      </template>
      <template #pattern="{ row }">
        <div v-if="row.matcher === 'regex' && row.pattern" class="flex min-w-0 items-center gap-1">
          <span class="min-w-0 truncate font-mono text-z-xs" :class="row.enabled ? 'text-z-text-secondary' : 'text-z-text-tertiary'" :title="row.pattern">{{ row.pattern }}</span>
          <ZIconButton variant="link" size="small" :aria-label="t('claude.privacyRule.copyPattern')" :title="t('claude.privacyRule.copyPattern')" @click="copyText(row.pattern)">
            <Copy class="size-3.5" />
          </ZIconButton>
        </div>
        <span v-else-if="row.matcher === 'dictionary'" class="text-z-xs text-z-text-tertiary">{{ t('claude.privacyRule.wordCount', { count: row.wordCount ?? 0 }) }}</span>
        <span v-else class="text-z-xs text-z-text-tertiary">{{ t('claude.privacyRule.ipParse') }}</span>
      </template>
      <template #enabled="{ row }">
        <span class="font-bold" :class="row.enabled ? 'text-z-success-text' : 'text-z-text-tertiary'">
          {{ row.enabled ? t('common.status.enabled') : t('common.status.disabled') }}
        </span>
      </template>
    </ZTable>
    <ZPagination v-if="sorted.length > PAGE_SIZE" v-model:current-page="page" :page-size="PAGE_SIZE" :page-sizes="[PAGE_SIZE]" :total="sorted.length" />
  </div>
</template>
