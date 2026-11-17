<script setup lang="ts">
import type { GuardCandidate, GuardCandidatePage, GuardRule } from '@/api/modules/claude'
import { defineTableColumns, ZButton, ZCard, ZConfirmDialog, ZDialog, ZForm, ZFormItem, ZInput, ZPagination, ZSelect, ZSwitch, ZTable, ZTextarea } from '@codex-proxy/ui'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave } from 'vue-router'
import { actOnGuardCandidate, getGuardCandidates, saveGuardCandidate } from '@/api/modules/claude'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { useRequestState } from '@/composables/useRequestState'
import { GUARD_ACTIONS_TARGET } from './actions'
import { useGuardCategoryCatalog } from './category'
import { useGuardConfirm } from './useGuardConfirm'

const { t } = useI18n({ useScope: 'global' })
const { confirmOpen, confirmDescription, askConfirm, finishConfirm } = useGuardConfirm()
const request = useRequestState()
const mutation = useRequestState()
const { loading, error } = request
const { loading: saving, error: mutationError } = mutation
const { error: catalogError, customRuleOptions: categoryOptions } = useGuardCategoryCatalog()
const data = shallowRef<GuardCandidatePage | null>(null)
const provider = ref('')
const status = ref('pending')
const page = ref(1)
const pageSize = ref(25)
const open = ref(false)
const selected = shallowRef<GuardCandidate | null>(null)
const draft = ref<GuardRule | null>(null)
const rationale = ref('')
const baseline = ref('')
const dirty = computed(() => open.value && JSON.stringify({ rule: draft.value, rationale: rationale.value }) !== baseline.value)
const readonly = computed(() => selected.value?.status === 'published' || selected.value?.status === 'dismissed')
const statuses = computed(() => [
  '',
  'pending',
  'published',
  'dismissed',
  'disabled',
].map(value => ({
  value,
  label: value
    ? t(`guard.flow.status.${value}`)
    : t('common.status.all'),
})))
const columns = computed(() => defineTableColumns<GuardCandidate>([
  {
    key: 'id',
    label: 'ID',
    kind: 'numeric',
    size: 'sm',
  },
  {
    key: 'rule',
    label: t('guard.rule'),
    kind: 'custom',
    size: '3xl',
  },
  {
    key: 'protocol',
    label: t('guard.layout.evidenceProvider'),
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'status',
    label: t('guard.flow.candidateStatus'),
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'evidenceCount',
    label: t('guard.flow.evidenceCount'),
    kind: 'numeric',
    size: 'md',
  },
  {
    key: 'details',
    label: t('guard.details'),
    kind: 'actions',
    size: 'md',
  },
]))
const patternFields = ['allPatterns', 'anyPatterns', 'excludePatterns'] as const
async function load() {
  const id = request.start()
  data.value = null
  try {
    const value = await getGuardCandidates({
      page: page.value,
      pageSize: pageSize.value,
      protocol: provider.value || undefined,
      status: status.value || undefined,
    }, { signal: request.signal })
    if (!request.isCurrent(id))
      return
    const last = Math.max(1, Math.ceil(value.total / pageSize.value))
    if (page.value > last) {
      page.value = last
      return
    }
    data.value = value
  }
  catch (cause) { request.fail(id, cause) }
  finally { request.finish(id) }
}
function edit(candidate: GuardCandidate | null) {
  selected.value = candidate
  draft.value = candidate?.rule
    ? JSON.parse(JSON.stringify(candidate.rule)) as GuardRule
    : {
        id: '',
        category: 'malicious',
        pattern: '',
        weight: 50,
        strict: false,
        signalOnly: false,
        allPatterns: [],
        anyPatterns: [],
        excludePatterns: [],
      }
  rationale.value = candidate?.rationale ?? ''
  baseline.value = JSON.stringify({ rule: draft.value, rationale: rationale.value })
  open.value = true
}
async function save() {
  if (!draft.value || saving.value)
    return
  const id = mutation.start()
  try {
    const result = await saveGuardCandidate({
      id: selected.value?.id ?? 0,
      revision: selected.value?.revision ?? 0,
      rule: draft.value,
      rationale: rationale.value,
    })
    edit(result)
    await load()
  }
  catch (cause) { mutation.fail(id, cause) }
  finally { mutation.finish(id) }
}
async function act(action: string) {
  if (!selected.value || saving.value || dirty.value)
    return
  if (action === 'publish' && !await askConfirm(t('guard.flow.publishConfirm')))
    return
  if (!selected.value || saving.value || dirty.value)
    return
  const id = mutation.start()
  try {
    const result = await actOnGuardCandidate({ id: selected.value.id, revision: selected.value.revision, action })
    edit(result)
    await load()
  }
  catch (cause) { mutation.fail(id, cause) }
  finally { mutation.finish(id) }
}
async function setOpen(value: boolean) {
  if (saving.value || (!value && dirty.value && !await askConfirm(t('integration.leaveDraft'))))
    return
  open.value = value
  if (!value) {
    draft.value = null
    selected.value = null
  }
}
onBeforeRouteLeave(async () => !saving.value && (!dirty.value || await askConfirm(t('integration.leaveDraft'))))
watch([provider, status, pageSize], () => {
  if (page.value === 1)
    void load()
  else page.value = 1
})
watch(page, load, { immediate: true })
</script>

<template>
  <div class="grid min-w-0 gap-5">
    <Teleport defer :to="GUARD_ACTIONS_TARGET">
      <ZButton :loading="loading" @click="load">
        {{ t('common.actions.refresh') }}
      </ZButton>
      <ZButton type="primary" :disabled="saving || open" @click="edit(null)">
        {{ t('guard.flow.newRule') }}
      </ZButton>
    </Teleport>
    <div class="flex min-w-0 flex-wrap items-center gap-3">
      <span class="text-z-sm text-z-text-secondary">{{ t('guard.layout.evidenceProvider') }}</span><ProviderFilter v-model="provider" :disabled="saving" />
      <span class="text-z-sm text-z-text-secondary sm:ml-3">{{ t('guard.flow.candidateStatus') }}</span><ZSelect v-model="status" :options="statuses" class="w-40" :aria-label="t('guard.flow.candidateStatus')" />
    </div>
    <p v-if="error || catalogError || (!open && mutationError)" role="alert" class="text-z-error-text">
      {{ error || catalogError || mutationError }}
    </p>
    <ZCard :title="t('guard.tabs.learning')" :description="t('guard.flow.learningQueueHelp')">
      <ZTable :key="provider" :columns="columns" :data="data?.items ?? []" :loading="loading" :empty-text="error || t('integration.empty')">
        <template #rule="{ row }">
          {{ row.rule?.id || t('guard.flow.evidenceOnly') }}
        </template>
        <template #protocol="{ row }">
          {{ row.protocol === 'anthropic' ? 'Claude' : row.protocol === 'openai' ? 'Codex' : t('common.providers.all') }}
        </template>
        <template #status="{ row }">
          {{ t(`guard.flow.status.${row.status}`) }}
        </template>
        <template #details="{ row }">
          <ZButton size="small" @click="edit(row)">
            {{ t('guard.details') }}
          </ZButton>
        </template>
      </ZTable>
      <ZPagination v-if="data" :current-page="page" :page-size="pageSize" :total="data.total" :page-sizes="[25, 50, 100]" :disabled="loading" @current-change="page = $event" @size-change="pageSize = $event" />
    </ZCard>
    <ZDialog :model-value="open" :title="t('guard.flow.reviewCandidate')" width="64rem" @update:model-value="setOpen">
      <div v-if="draft" class="space-y-4">
        <p v-if="mutationError" role="alert" class="text-z-error-text">
          {{ mutationError }}
        </p>
        <p v-if="selected" class="text-z-sm">
          {{ t(`guard.flow.status.${selected.status}`) }} · {{ t('guard.flow.evidenceQuality') }}: {{ selected.evidenceQuality }}
        </p>
        <details v-if="selected?.evidence">
          <summary class="cursor-pointer text-z-sm">
            {{ t('guard.flow.redactedEvidence') }}
          </summary><pre class="mt-2 max-h-60 overflow-auto whitespace-pre-wrap break-all rounded-z bg-z-fill-quaternary p-3 text-z-xs">{{ selected.evidence }}</pre>
        </details>
        <fieldset :disabled="saving || readonly" class="m-0 min-w-0 border-0 p-0">
          <ZForm class="sm:grid-cols-2">
            <ZFormItem :label="t('guard.rule')">
              <ZInput v-model="draft.id" placeholder="custom_rule_name" />
            </ZFormItem>
            <!-- 类别目录加载失败时退回手工输入，服务端仍会校验类别。 -->
            <ZFormItem :label="t('guard.category')">
              <ZSelect v-if="categoryOptions.length" v-model="draft.category" filterable :options="categoryOptions" :aria-label="t('guard.category')" /><ZInput v-else v-model="draft.category" />
            </ZFormItem>
            <!-- 权重与两个开关同排、三类条件同排，两列网格中不留下孤立的单项。 -->
            <div class="grid min-w-0 gap-4 sm:col-span-2 sm:grid-cols-3">
              <ZFormItem :label="t('guard.weight')">
                <ZInput :model-value="String(draft.weight)" type="number" min="1" max="1000" @update:model-value="draft.weight = Number($event)" />
              </ZFormItem>
              <ZFormItem :label="t('guard.flow.strictRule')">
                <ZSwitch v-model="draft.strict" :disabled="saving || readonly" />
              </ZFormItem>
              <ZFormItem :label="t('guard.signalOnly')">
                <ZSwitch v-model="draft.signalOnly" :disabled="saving || readonly" />
              </ZFormItem>
            </div>
            <ZFormItem :label="t('guard.flow.pattern')" class="sm:col-span-2">
              <ZTextarea v-model="draft.pattern" :rows="3" maxlength="4096" :spellcheck="false" class="font-mono" />
            </ZFormItem>
            <div class="grid min-w-0 gap-4 sm:col-span-2 sm:grid-cols-3">
              <ZFormItem v-for="field in patternFields" :key="field" :label="t(`guard.flow.${field}`)" :description="t('guard.flow.onePatternPerLine')">
                <ZTextarea :model-value="draft[field].join('\n')" :rows="3" :spellcheck="false" class="font-mono" @update:model-value="draft[field] = $event.split('\n').filter(value => value.trim())" />
              </ZFormItem>
            </div>
            <ZFormItem :label="t('guard.flow.rationale')" class="sm:col-span-2">
              <ZTextarea v-model="rationale" :rows="3" maxlength="2048" />
            </ZFormItem>
          </ZForm>
        </fieldset>
      </div>
      <!-- 按对话框惯例放在 footer：次要与状态操作在左，保存草稿作为主操作在最右。 -->
      <template v-if="draft" #footer>
        <ZButton :disabled="saving" @click="setOpen(false)">
          {{ t('common.actions.close') }}
        </ZButton>
        <template v-if="selected">
          <ZButton v-if="selected.status === 'pending'" :disabled="saving || dirty" @click="act('dismiss')">
            {{ t('guard.flow.dismiss') }}
          </ZButton>
          <ZButton v-if="selected.status === 'published'" :disabled="saving" @click="act('disable')">
            {{ t('guard.flow.disable') }}
          </ZButton>
          <ZButton v-if="selected.status === 'dismissed'" :disabled="saving" @click="act('reopen')">
            {{ t('guard.flow.reopen') }}
          </ZButton>
          <ZButton v-if="!readonly && selected.evidenceQuality === 'complete'" :disabled="saving || dirty" @click="act('generate')">
            {{ t('guard.flow.generate') }}
          </ZButton>
          <ZButton v-if="!readonly && selected.rule" type="success" :disabled="saving || dirty" @click="act('publish')">
            {{ t('guard.flow.publish') }}
          </ZButton>
        </template>
        <ZButton v-if="!readonly" type="primary" :loading="saving" :disabled="!dirty || !draft.id || !draft.pattern" @click="save">
          {{ t('guard.flow.saveDraft') }}
        </ZButton>
      </template>
    </ZDialog>
    <ZConfirmDialog v-model="confirmOpen" :title="t('common.actions.confirm')" :description="confirmDescription" @confirm="finishConfirm(true)" />
  </div>
</template>
