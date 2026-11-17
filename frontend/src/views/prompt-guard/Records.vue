<script setup lang="ts">
import type { GuardPage, GuardRecord, GuardSettings } from '@/api/modules/claude'
import { defineTableColumns, ZButton, ZCard, ZDialog, ZInput, ZPagination, ZSelect, ZTable } from '@codex-proxy/ui'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { getGuardRecords, getGuardSettings } from '@/api/modules/claude'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { useRequestState } from '@/composables/useRequestState'
import { useGuardCategoryCatalog } from './category'

const { t } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const query = useRequestState()
const { loading, error } = query
const data = shallowRef<GuardPage | null>(null)
const settings = shallowRef<GuardSettings | null>(null)
const selected = shallowRef<GuardRecord | null>(null)
const showDetail = ref(false)
const requestId = ref('')
const protocol = ref('')
const action = ref('')
const hours = ref('24')
const category = ref('')
const model = ref('')
const keyId = ref('')
const mode = ref('')
const policyVersion = ref('')
const reviewResult = ref('')
const origin = ref('')
const minScore = ref('')
const maxScore = ref('')
const advancedOpen = ref(false)
const pageSize = computed(() => Number(route.query.pageSize) || 25)
const range = shallowRef<{ start: string, end: string } | null>(null)
const policyModes = computed(() => ['', 'observe', 'enforce'].map(value => ({
  value,
  label: value
    ? t(`guard.mode.${value}`)
    : t('common.status.all'),
})))
const actions = computed(() => [
  '',
  'allow',
  'warn',
  'block',
  'coverage_error',
  'unavailable',
  'cancelled',
  'disabled',
  'missing',
].map(value => ({
  value,
  label: value
    ? t(`guard.action.${value}`)
    : t('common.status.all'),
})))
const { error: catalogError, options: categoryCatalog } = useGuardCategoryCatalog()
const categoryOptions = computed(() => [
  {
    value: '',
    label: t('guard.layout.allCategories'),
  },
  ...categoryCatalog.value,
])
const periods = computed(() => ['24', '168', '720'].map(value => ({
  value,
  label: t('guard.hours', { hours: value }),
})))
const columns = computed(() => defineTableColumns<GuardRecord>([
  {
    key: 'startedAt',
    label: t('guard.time'),
    kind: 'datetime',
  },
  {
    key: 'model',
    label: t('guard.model'),
    kind: 'custom',
    size: '3xl',
  },
  {
    key: 'clientKeyId',
    label: t('guard.key'),
    kind: 'mono',
    size: 'xl',
  },
  {
    key: 'action',
    label: t('guard.actionLabel'),
    kind: 'custom',
    size: 'xl',
  },
  {
    key: 'score',
    label: t('guard.decisionScore'),
    kind: 'numeric',
    size: 'sm',
  },
  {
    key: 'review',
    label: t('guard.reviewScore'),
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'details',
    label: t('guard.details'),
    kind: 'actions',
    size: 'sm',
  },
]))
const hitColumns = computed(() => defineTableColumns<NonNullable<GuardRecord['assessment']>['hits'][number]>([
  {
    key: 'ruleId',
    label: t('guard.rule'),
    kind: 'mono',
    size: '2xl',
  },
  {
    key: 'source',
    label: t('guard.source'),
    kind: 'mono',
    size: '2xl',
  },
  {
    key: 'origin',
    label: t('guard.origin'),
    kind: 'identity',
    size: 'lg',
  },
  {
    key: 'representation',
    label: t('guard.representation'),
    kind: 'identity',
    size: 'lg',
  },
  {
    key: 'weight',
    label: t('guard.weight'),
    kind: 'numeric',
    size: 'sm',
  },
  {
    key: 'effectiveWeight',
    label: t('guard.effectiveWeight'),
    kind: 'numeric',
    size: 'md',
  },
]))
const summaryKeys = [
  'recorded',
  'blocked',
  'observedHits',
  'unavailable',
  'coverageError',
  'missing',
] as const
const assessment = computed(() => selected.value?.assessment)
function reviewScore(record: GuardRecord) {
  const review = record.assessment?.review
  if (!review)
    return t('guard.notReviewed')
  return review.confidenceBps === null
    ? t('guard.inconclusive')
    : `${review.confidenceBps / 100}% / ${review.thresholdBps / 100}%`
}
async function load() {
  const id = query.start()
  data.value = null
  try {
    const end = typeof route.query.end === 'string'
      ? route.query.end
      : new Date().toISOString()
    const start = typeof route.query.start === 'string'
      ? route.query.start
      : new Date(Date.parse(end) - Number(hours.value) * 3600000).toISOString()
    range.value = { start, end }
    const [page, config] = await Promise.allSettled([
      getGuardRecords({
        start,
        end,
        page: Number(route.query.page) || 1,
        pageSize: pageSize.value,
        requestId: requestId.value || undefined,
        protocol: protocol.value || undefined,
        action: action.value || undefined,
        category: category.value || undefined,
        model: model.value || undefined,
        clientKeyId: keyId.value || undefined,
        mode: mode.value || undefined,
        policyVersion: policyVersion.value || undefined,
        reviewResult: reviewResult.value || undefined,
        origin: origin.value || undefined,
        minScore: minScore.value
          ? Number(minScore.value)
          : undefined,
        maxScore: maxScore.value
          ? Number(maxScore.value)
          : undefined,
      }, { signal: query.signal }),
      getGuardSettings({ signal: query.signal }),
    ])
    if (!query.isCurrent(id))
      return
    data.value = page.status === 'fulfilled'
      ? page.value
      : null
    if (page.status === 'fulfilled') {
      const last = Math.max(1, Math.ceil(page.value.summary.total / page.value.pageSize))
      if (page.value.page > last)
        paginate(last)
    }
    settings.value = config.status === 'fulfilled'
      ? config.value
      : null
    for (const result of [page, config]) {
      if (result.status === 'rejected')
        query.fail(id, result.reason)
    }
  }
  catch (cause) { query.fail(id, cause) }
  finally { query.finish(id) }
}
function filter() {
  const end = new Date().toISOString()
  const start = new Date(Date.parse(end) - Number(hours.value) * 3600000).toISOString()
  const next = {
    requestId: requestId.value || undefined,
    protocol: protocol.value || undefined,
    action: action.value || undefined,
    category: category.value || undefined,
    model: model.value || undefined,
    clientKeyId: keyId.value || undefined,
    hours: hours.value,
    page: '1',
    pageSize: String(pageSize.value),
    start,
    end,
    mode: mode.value || undefined,
    policyVersion: policyVersion.value || undefined,
    reviewResult: reviewResult.value || undefined,
    origin: origin.value || undefined,
    minScore: minScore.value || undefined,
    maxScore: maxScore.value || undefined,
  }
  // 相同筛选显式刷新，路由变化由 watch 统一加载，避免一次操作发两组请求。
  if (router.resolve({ query: next }).fullPath === route.fullPath)
    void load()
  else void router.replace({ query: next })
}
function reset() {
  for (const field of [
    requestId,
    protocol,
    action,
    category,
    model,
    keyId,
    mode,
    policyVersion,
    reviewResult,
    origin,
    minScore,
    maxScore,
  ]) field.value = ''
  hours.value = '24'
  filter()
}
function paginate(page: number, size = pageSize.value) {
  if (!range.value)
    return
  // 翻页和调整页容量沿用已提交的筛选与时间快照，不混入尚未查询的表单值。
  void router.replace({
    query: {
      ...route.query,
      ...range.value,
      page: String(page),
      pageSize: String(size),
    },
  })
}
watch(() => route.query, () => {
  requestId.value = String(route.query.requestId ?? '')
  protocol.value = String(route.query.protocol ?? '')
  action.value = String(route.query.action ?? '')
  hours.value = String(route.query.hours ?? '24')
  category.value = String(route.query.category ?? '')
  model.value = String(route.query.model ?? '')
  keyId.value = String(route.query.clientKeyId ?? '')
  mode.value = String(route.query.mode ?? '')
  policyVersion.value = String(route.query.policyVersion ?? '')
  reviewResult.value = String(route.query.reviewResult ?? '')
  origin.value = String(route.query.origin ?? '')
  minScore.value = String(route.query.minScore ?? '')
  maxScore.value = String(route.query.maxScore ?? '')
  if ([
    model.value,
    category.value,
    keyId.value,
    mode.value,
    policyVersion.value,
    reviewResult.value,
    origin.value,
    minScore.value,
    maxScore.value,
  ].some(Boolean)) {
    advancedOpen.value = true
  }
  void load()
}, { immediate: true })
</script>

<template>
  <div class="grid min-w-0 gap-5">
    <!-- 队列健康是全网关统计，与配置状态一起放在页首，不混入审查记录表。 -->
    <!-- 队列健康是全网关统计，只在出现丢弃或写入失败时提示，正常时不占用页面。 -->
    <div v-if="settings || data" class="space-y-1 text-z-sm text-z-text-secondary" role="status">
      <div class="flex flex-wrap gap-3">
        <span v-if="settings">{{ t(`settings.promptGuard.${settings.config.state}`) }} · {{ t(`guard.mode.${settings.config.mode}`) }}</span>
        <span v-if="settings">{{ t('guard.retention', { days: settings.retentionDays }) }}</span>
        <span v-if="data">{{ t('guard.averageReviewMs') }}: {{ data.summary.averageReviewMs === null ? '—' : Math.round(data.summary.averageReviewMs) }}</span>
      </div>
      <p v-if="data?.recordingHealth && (data.recordingHealth.droppedTotal || data.recordingHealth.writeFailureTotal)" class="text-z-warning-text">
        {{ t('guard.queueHealth', data.recordingHealth) }}
      </p>
    </div>
    <ZCard>
      <form @submit.prevent="filter()">
        <div class="flex min-w-0 flex-wrap items-end gap-3">
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('common.providers.filter') }}</span><ProviderFilter v-model="protocol" @update:model-value="filter()" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.time') }}</span><ZSelect v-model="hours" :options="periods" :aria-label="t('guard.time')" class="w-40" @update:model-value="filter()" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.actionLabel') }}</span><ZSelect v-model="action" :options="actions" :aria-label="t('guard.actionLabel')" class="w-44" @update:model-value="filter()" />
          </div>
          <div class="grid min-w-48 flex-1 gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.requestId') }}</span><ZInput v-model="requestId" :aria-label="t('guard.requestId')" />
          </div>
          <ZButton type="primary" :loading="loading" @click="filter()">
            {{ t('guard.layout.query') }}
          </ZButton>
          <ZButton :disabled="loading" @click="reset()">
            {{ t('common.actions.reset') }}
          </ZButton>
          <ZButton :aria-expanded="advancedOpen" @click="advancedOpen = !advancedOpen">
            {{ t('guard.layout.moreFilters') }}
          </ZButton>
        </div>
        <!-- 高级筛选与首行保持“标签在上”，修改后需点击查询或回车才会提交。 -->
        <div v-show="advancedOpen" class="mt-4 grid gap-3 border-t border-z-border pt-4 sm:grid-cols-2 xl:grid-cols-3">
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.model') }}</span><ZInput v-model="model" :aria-label="t('guard.model')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.category') }}</span><ZSelect v-if="categoryCatalog.length" v-model="category" filterable :options="categoryOptions" :aria-label="t('guard.category')" /><ZInput v-else v-model="category" :aria-label="t('guard.category')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.key') }}</span><ZInput v-model="keyId" :aria-label="t('guard.key')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.modeLabel') }}</span><ZSelect v-model="mode" :options="policyModes" :aria-label="t('guard.modeLabel')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.policyVersion') }}</span><ZInput v-model="policyVersion" :aria-label="t('guard.policyVersion')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.reviewResult') }}</span><ZInput v-model="reviewResult" :aria-label="t('guard.reviewResult')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.originFilter') }}</span><ZInput v-model="origin" :aria-label="t('guard.originFilter')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.minScore') }}</span><ZInput v-model="minScore" type="number" min="0" :aria-label="t('guard.minScore')" />
          </div>
          <div class="grid gap-1.5">
            <span class="text-z-sm text-z-text-secondary">{{ t('guard.maxScore') }}</span><ZInput v-model="maxScore" type="number" min="0" :aria-label="t('guard.maxScore')" />
          </div>
        </div>
      </form>
    </ZCard>
    <p v-if="error || catalogError" role="alert" class="text-z-error-text">
      {{ error || catalogError }}
    </p>
    <!-- 统计块沿用仪表盘 MetricCard 的层级：标签次要，数值为视觉重心。 -->
    <div v-if="data" class="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-6">
      <ZCard v-for="key in summaryKeys" :key="key" as="article" padding="compact">
        <span class="text-z leading-[1.15] font-emphasis text-z-text-secondary">{{ t(`guard.summary.${key}`) }}</span>
        <strong class="mt-3 block font-mono text-[28px] leading-[1.05] font-heavy tabular-nums text-z-text">{{ data.summary[key] }}</strong>
      </ZCard>
    </div>
    <ZCard :title="t('guard.tabs.records')" :description="t('guard.recordHelp')">
      <ZTable :key="String(route.query.protocol ?? '')" :columns="columns" :data="data?.items ?? []" :loading="loading" :empty-text="error || t('integration.empty')">
        <template #startedAt="{ row }">
          {{ row.startedAtDisplay ?? '—' }}
        </template>
        <template #model="{ row }">
          <span class="block truncate" :title="row.model">{{ row.model }}</span><span class="text-z-xs text-z-text-secondary">{{ row.protocol }}</span>
        </template>
        <template #action="{ row }">
          {{ t(`guard.action.${row.assessment?.action ?? 'missing'}`) }}<span v-if="row.assessment?.upstreamPolicy" class="block text-z-xs text-z-warning-text">{{ t('guard.flow.upstreamRejected') }}</span><span v-if="row.assessment?.mode === 'observe'" class="block text-z-xs">{{ t('guard.would', { action: t(`guard.action.${row.assessment.wouldAction}`) }) }}</span>
        </template>
        <template #score="{ row }">
          {{ row.assessment?.decisionScore ?? '—' }}
        </template>
        <template #review="{ row }">
          {{ reviewScore(row) }}
        </template>
        <template #details="{ row }">
          <ZButton size="small" @click="selected = row; showDetail = true">
            {{ t('guard.details') }}
          </ZButton>
        </template>
      </ZTable>
      <ZPagination v-if="data" :current-page="data.page" :page-size="data.pageSize" :total="data.summary.total" :page-sizes="[25, 50, 100]" :disabled="loading" @current-change="paginate($event)" @size-change="paginate(1, $event)" />
    </ZCard>
    <ZDialog v-model="showDetail" :title="t('guard.details')" width="68rem">
      <div v-if="selected" class="space-y-4">
        <p class="break-all font-mono text-z-sm">
          {{ selected.requestId }}
        </p>
        <template v-if="assessment">
          <p>{{ t(`guard.action.${assessment.action}`) }} · {{ t(`guard.coverage.${assessment.coverage}`) }} · {{ assessment.reasonCode ?? '—' }}</p>
          <p v-if="assessment.upstreamPolicy" class="text-z-warning-text">
            {{ t('guard.flow.upstreamRejected') }}: {{ assessment.upstreamPolicy.code }} · {{ assessment.upstreamPolicy.recorded ? t('guard.flow.feedbackRecorded') : t('guard.flow.feedbackFailed') }} · {{ !assessment.upstreamPolicy.recorded ? t('guard.flow.lockUnknown') : assessment.upstreamPolicy.lockApplied ? t('guard.flow.lockApplied') : t('guard.flow.noLock') }}
          </p>
          <p v-if="assessment.mode === 'observe'">
            {{ t('guard.would', { action: t(`guard.action.${assessment.wouldAction}`) }) }}
          </p>
          <dl class="grid grid-cols-2 gap-3 text-z-sm sm:grid-cols-4">
            <div v-for="key in ['rawScore', 'signalScore', 'auditScore', 'decisionScore', 'localMs', 'totalMs', 'evaluationCount', 'reuseCount'] as const" :key="key">
              <dt class="text-z-text-secondary">
                {{ t(`guard.${key}`) }}
              </dt><dd>{{ assessment[key] }}</dd>
            </div>
          </dl>
          <p>{{ t('guard.reviewScore') }}: {{ reviewScore(selected) }}</p>
          <p v-if="assessment.review" class="text-z-sm">
            {{ t('guard.projection', { selected: assessment.review.selectedBlocks, blocks: assessment.review.omittedBlocks, chars: assessment.review.omittedChars }) }} · {{ assessment.review.projection }} · {{ assessment.review.redacted ? t('guard.redacted') : t('guard.originalPolicy') }}
          </p>
          <p class="break-all font-mono text-z-xs">
            {{ assessment.policyVersion }} / {{ assessment.rulesVersion }} / {{ assessment.normalizationVersion }}
          </p>
          <ZTable :columns="hitColumns" :data="assessment.hits" :empty-text="t('integration.empty')" />
          <p v-if="assessment.omittedHits" class="text-z-sm">
            {{ t('guard.omittedHits', { count: assessment.omittedHits }) }}
          </p>
        </template>
        <p v-else>
          {{ t('guard.missingHelp') }}
        </p>
        <RouterLink class="text-z-primary" :to="{ path: '/usage', query: { requestId: selected.requestId } }">
          {{ t('guard.usageLink') }}
        </RouterLink>
      </div>
    </ZDialog>
  </div>
</template>
