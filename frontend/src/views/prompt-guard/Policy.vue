<script setup lang="ts">
import type { GuardPolicy, GuardSettings } from '@/api/modules/claude'
import { defineTableColumns, ZButton, ZCard, ZConfirmDialog, ZDialog, ZIconButton, ZInput, ZPagination, ZSelect, ZSwitch, ZTable } from '@codex-proxy/ui'
import { Save, Undo2 } from '@lucide/vue'
import { computed, onMounted, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave } from 'vue-router'
import { getGuardPolicy, getGuardSettings, saveGuardSettings } from '@/api/modules/claude'
import { ApiError } from '@/api/request'
import { useRequestState } from '@/composables/useRequestState'
import { GUARD_ACTIONS_TARGET } from './actions'
import { useRuleCategoryLabel } from './category'
import Evaluation from './components/Evaluation.vue'
import PromptGuardCard from './components/PromptGuardCard.vue'
import { useGuardConfirm } from './useGuardConfirm'

defineProps<{ settingsOnly?: boolean, rulesOnly?: boolean }>()

const { t } = useI18n({ useScope: 'global' })
const { confirmOpen, confirmDescription, askConfirm, finishConfirm } = useGuardConfirm()
const categoryLabel = useRuleCategoryLabel()
const query = useRequestState()
const mutation = useRequestState()
const { loading, error } = query
const { loading: saving, error: saveError } = mutation
const draft = ref<GuardSettings | null>(null)
const baseline = shallowRef('')
const policy = shallowRef<GuardPolicy | null>(null)
const conflict = shallowRef(false)
const serverSettings = shallowRef<GuardSettings | null>(null)
const search = ref('')
const category = ref('')
const ruleState = ref('')
const source = ref('')
const page = ref(1)
const pageSize = ref(25)
const selectedRuleId = ref('')
const detailOpen = ref(false)
const selectedRule = computed(() => policy.value?.rules.find(rule => rule.id === selectedRuleId.value))
const categories = computed(() => [
  {
    value: '',
    label: t('guard.layout.allCategories'),
  },
  ...[
    ...new Set(policy.value?.rules.map(rule => rule.category) ?? []),
  ].sort().map(value => ({
    value,
    label: categoryLabel(value),
    description: value,
  })),
])
const states = computed(() => ['', 'enabled', 'disabled'].map(value => ({
  value,
  label: value
    ? t(`guard.layout.${value}`)
    : t('common.status.all'),
})))
const sources = computed(() => ['', 'builtin', 'custom'].map(value => ({
  value,
  label: value
    ? t(`guard.layout.${value}`)
    : t('guard.layout.allSources'),
})))
const dirty = computed(() => draft.value !== null && JSON.stringify(draft.value.config) !== baseline.value)
function isEnabled(id: string) {
  return !draft.value?.config.disabledRules.includes(id)
}
const rows = computed(() => policy.value?.rules.filter(rule =>
  `${rule.id} ${rule.category} ${categoryLabel(rule.category)} ${rule.pattern}`.toLowerCase().includes(search.value.trim().toLowerCase())
  && (!category.value || rule.category === category.value)
  && (!source.value || rule.source === source.value)
  && (!ruleState.value || isEnabled(rule.id) === (ruleState.value === 'enabled')),
) ?? [])
// 分页只改变可见行；所有页面仍编辑同一份规则草稿，并由同一个配置版本统一保存。
const visibleRows = computed(() => rows.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value))
watch([
  search,
  category,
  source,
  ruleState,
  pageSize,
], () => { page.value = 1 })
watch(() => rows.value.length, (count) => {
  page.value = Math.min(page.value, Math.max(1, Math.ceil(count / pageSize.value)))
})
const columns = computed(() => defineTableColumns<GuardPolicy['rules'][number]>([
  {
    key: 'id',
    label: t('guard.rule'),
    kind: 'custom',
    size: '3xl',
  },
  {
    key: 'category',
    label: t('guard.category'),
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'source',
    label: t('guard.layout.ruleSource'),
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'weight',
    label: t('guard.weight'),
    kind: 'numeric',
    size: 'sm',
    align: 'center',
  },
  {
    key: 'purpose',
    label: t('guard.purpose'),
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'enabled',
    label: t('guard.flow.ruleEnabled'),
    kind: 'custom',
    size: 'sm',
    align: 'center',
  },
]))
function setRule(id: string, enabled: boolean) {
  if (!draft.value)
    return
  draft.value.config.disabledRules = enabled
    ? draft.value.config.disabledRules.filter(value => value !== id)
    : [
        ...new Set([...draft.value.config.disabledRules, id]),
      ]
}
async function load() {
  const id = query.start()
  try {
    const [settings, catalog] = await Promise.all([
      getGuardSettings({ signal: query.signal }),
      getGuardPolicy({ signal: query.signal }),
    ])
    if (!query.isCurrent(id))
      return
    draft.value = settings
    baseline.value = JSON.stringify(settings.config)
    policy.value = catalog
    conflict.value = false
    serverSettings.value = null
  }
  catch (cause) { query.fail(id, cause) }
  finally { query.finish(id) }
}
async function save() {
  if (!draft.value || saving.value)
    return
  const id = mutation.start()
  try {
    const result = await saveGuardSettings({ configRevision: draft.value.configRevision, config: draft.value.config })
    if (!mutation.isCurrent(id))
      return
    draft.value = result
    baseline.value = JSON.stringify(result.config)
    conflict.value = false
    policy.value = await getGuardPolicy({ signal: mutation.signal })
  }
  catch (cause) {
    conflict.value = cause instanceof ApiError && cause.status === 409
    mutation.fail(id, cause)
  }
  finally { mutation.finish(id) }
}
async function reload() {
  if (!dirty.value || await askConfirm(t('integration.discardDraft')))
    await load()
}
// 撤销只回到最近一次加载或保存的配置，不请求服务端。
function undo() {
  if (draft.value)
    draft.value.config = JSON.parse(baseline.value) as GuardSettings['config']
}
async function compare() {
  const id = query.start()
  try {
    const result = await getGuardSettings({ signal: query.signal })
    if (query.isCurrent(id))
      serverSettings.value = result
  }
  catch (cause) { query.fail(id, cause) }
  finally { query.finish(id) }
}
function comparison(settings: GuardSettings) {
  // 冲突对照不展示草稿密钥或服务端秘密，只展示是否请求保留、清除或替换。
  const { reviewApiKey, ...config } = settings.config
  return JSON.stringify({
    ...config,
    keyChange: reviewApiKey === undefined
      ? 'keep'
      : reviewApiKey === ''
        ? 'clear'
        : 'replace',
  }, null, 2)
}
// 草稿只保存在当前组件，离开前提示，密钥不会进入 URL 或浏览器持久化。
onBeforeRouteLeave(async () => !dirty.value || await askConfirm(t('integration.leaveDraft')))
onScopeDispose(() => {
  if (draft.value)
    draft.value.config.reviewApiKey = undefined
})
onMounted(load)
</script>

<template>
  <div class="grid min-w-0 max-w-6xl gap-5">
    <!-- 页面级操作放到页签行右侧，形式与系统设置页的保存区一致。 -->
    <Teleport defer :to="GUARD_ACTIONS_TARGET">
      <span v-if="dirty" class="mr-1 size-1.5 shrink-0 rounded-full bg-z-warning" aria-hidden="true" />
      <ZIconButton v-if="dirty" :aria-label="t('guard.layout.undoChanges')" variant="solid" :disabled="loading || saving" @click="undo">
        <Undo2 class="size-4" />
      </ZIconButton>
      <ZButton :disabled="loading || saving" @click="reload">
        {{ t('common.actions.refresh') }}
      </ZButton>
      <ZButton type="primary" :loading="saving" :disabled="!dirty || loading || conflict" @click="save">
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ rulesOnly ? t('guard.layout.saveRules') : t('guard.save') }}
      </ZButton>
    </Teleport>
    <p v-if="error || saveError" role="alert" class="text-z-error-text">
      {{ error || saveError }}
    </p>
    <ZCard v-if="conflict" :title="t('integration.compare')">
      <p role="alert" class="mb-3 text-z-warning-text">
        {{ t('integration.conflict') }}
      </p>
      <ZButton :loading="loading" @click="compare">
        {{ t('integration.compare') }}
      </ZButton>
      <div v-if="serverSettings && draft" class="mt-3 grid gap-3 sm:grid-cols-2">
        <div><p>{{ t('integration.localDraft') }}</p><pre class="whitespace-pre-wrap break-all text-z-xs">{{ comparison(draft) }}</pre></div>
        <div><p>{{ t('integration.serverVersion', { revision: serverSettings.configRevision }) }}</p><pre class="whitespace-pre-wrap break-all text-z-xs">{{ comparison(serverSettings) }}</pre></div>
      </div>
    </ZCard>
    <PromptGuardCard v-if="draft && !rulesOnly" v-model="draft.config" :disabled="saving || loading" />
    <!-- 评估只使用已保存规则，放在规则表之前，无需滚过整张表即可试测。 -->
    <Evaluation v-if="!settingsOnly" />
    <ZCard v-if="policy && !settingsOnly" :title="t('guard.catalog')" :description="t('guard.localPolicy', { threshold: policy.candidateThreshold })">
      <p class="mb-4 text-z-sm text-z-warning-text">
        {{ t('guard.layout.sharedRules') }}
      </p>
      <div class="mb-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-[minmax(15rem,2fr)_repeat(3,minmax(9rem,1fr))]">
        <div class="grid gap-1.5">
          <span class="text-z-sm text-z-text-secondary">{{ t('guard.ruleSearch') }}</span><ZInput v-model="search" :aria-label="t('guard.ruleSearch')" />
        </div>
        <div class="grid gap-1.5">
          <span class="text-z-sm text-z-text-secondary">{{ t('guard.category') }}</span><ZSelect v-model="category" filterable :options="categories" :aria-label="t('guard.category')" />
        </div>
        <div class="grid gap-1.5">
          <span class="text-z-sm text-z-text-secondary">{{ t('guard.layout.ruleSource') }}</span><ZSelect v-model="source" :options="sources" :aria-label="t('guard.layout.ruleSource')" />
        </div>
        <div class="grid gap-1.5">
          <span class="text-z-sm text-z-text-secondary">{{ t('guard.flow.ruleEnabled') }}</span><ZSelect v-model="ruleState" :options="states" :aria-label="t('guard.flow.ruleEnabled')" />
        </div>
      </div>
      <ZTable :columns="columns" :data="visibleRows" :loading="loading" :empty-text="t('integration.empty')">
        <template #id="{ row }">
          <button type="button" class="cursor-pointer break-all text-left font-mono text-z-primary-text hover:underline" @click="selectedRuleId = row.id; detailOpen = true">
            {{ row.id }}
          </button>
        </template>
        <template #category="{ row }">
          <span :title="row.category">{{ categoryLabel(row.category) }}</span>
        </template>
        <template #source="{ row }">
          {{ t(`guard.layout.${row.source}`) }}
        </template>
        <template #purpose="{ row }">
          {{ row.signalOnly ? t('guard.signalOnly') : row.observationOnly ? t('guard.observationOnly') : t('guard.candidate') }} <span v-if="row.intentRequired">· {{ t('guard.intentRequired') }}</span><span v-if="row.localTermination"> · {{ t('guard.flow.strictRule') }}</span>
        </template>
        <template #enabled="{ row }">
          <ZSwitch :model-value="isEnabled(row.id)" :disabled="saving || loading" :aria-label="row.id" @update:model-value="setRule(row.id, $event)" />
        </template>
      </ZTable>
      <ZPagination :current-page="page" :page-size="pageSize" :total="rows.length" :page-sizes="[25, 50, 100]" :disabled="loading" @current-change="page = $event" @size-change="pageSize = $event" />
      <details class="mt-4 text-z-xs text-z-text-secondary">
        <summary class="cursor-pointer">
          {{ t('guard.policyVersion') }}
        </summary><p class="mt-2 break-all font-mono">
          {{ policy.policyVersion }} / {{ policy.rulesVersion }} / {{ policy.normalizationVersion }}
        </p>
      </details>
    </ZCard>
    <ZDialog v-model="detailOpen" :title="t('guard.layout.ruleDetails')" width="64rem">
      <div v-if="selectedRule" class="space-y-4">
        <p class="break-all font-mono text-z-sm">
          {{ selectedRule.id }}
        </p>
        <p class="text-z-sm">
          {{ categoryLabel(selectedRule.category) }} · {{ t(`guard.layout.${selectedRule.source}`) }} · {{ t('guard.weight') }} {{ selectedRule.weight }} · {{ isEnabled(selectedRule.id) ? t('guard.layout.enabled') : t('guard.layout.disabled') }}
        </p>
        <p class="text-z-sm text-z-text-secondary">
          {{ t('guard.layout.sharedRules') }}
        </p>
        <div v-for="field in ['pattern', 'allPatterns', 'anyPatterns', 'excludePatterns', 'authorizationExcludePatterns'] as const" :key="field">
          <p class="mb-2 text-z-sm font-medium">
            {{ field === 'authorizationExcludePatterns' ? t('guard.layout.authorizationExclusions') : t(`guard.flow.${field}`) }}
          </p>
          <pre class="max-h-52 overflow-auto whitespace-pre-wrap break-all rounded-z bg-z-fill-quaternary p-3 text-z-xs">{{ typeof selectedRule[field] === 'string' ? selectedRule[field] : selectedRule[field].join('\n') || '—' }}</pre>
        </div>
      </div>
    </ZDialog>
    <ZConfirmDialog v-model="confirmOpen" :title="t('common.actions.confirm')" :description="confirmDescription" @confirm="finishConfirm(true)" />
  </div>
</template>
