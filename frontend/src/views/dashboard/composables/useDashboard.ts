import type { DashboardSummaryResponse, DashboardTrendResponse } from '@/api'
import { useIntervalFn } from '@vueuse/core'
import { computed, onMounted, onScopeDispose, shallowRef, watch } from 'vue'

import { getDashboardSummary, getDashboardTrend } from '@/api'
import { errorMessage, withMinimumDuration } from '@/utils/operation'
import { dashboardSnapshotView, dashboardTrendView, normalizeDashboardTrendKind } from '../presenter'

export function useDashboard() {
  const provider = shallowRef('')
  let viewRevision = 0
  let summaryRequestId = 0
  const activeTrendKind = shallowRef(normalizeDashboardTrendKind('usage'))
  const summary = shallowRef<DashboardSummaryResponse | null>(null)
  const snapshot = computed(() => dashboardSnapshotView(summary.value))
  const trend = shallowRef<DashboardTrendResponse | null>(null)
  const trendLoading = shallowRef(false)
  const trendError = shallowRef('')
  const analyticsError = shallowRef('')
  const loading = shallowRef(false)
  const refreshing = shallowRef(false)
  const lastRefreshedAt = shallowRef('')
  let rangeAsOf = Date.now()
  let trendRequestId = 0
  let disposed = false
  let summaryController: AbortController | undefined
  let trendController: AbortController | undefined

  const analytics = computed(() => snapshot.value.analytics)
  const metrics = computed(() => snapshot.value.metrics)
  const healthTimeline = computed(() => snapshot.value.healthTimeline)
  const accountUsage = computed(() => snapshot.value.accountUsage)
  const wireProfiles = computed(() => snapshot.value.wireProfiles)
  const usageRecords = computed(() => snapshot.value.usageRecords)
  const poolSummary = computed(() => snapshot.value.poolSummary)
  const capacityInfo = computed(() => snapshot.value.capacityInfo)
  const rotationStrategy = computed(() => snapshot.value.rotationStrategy)
  const trendView = computed(() => dashboardTrendView(
    trend.value?.kind === activeTrendKind.value
      ? trend.value
      : null,
  ))
  const trendPoints = computed(() => trendView.value.points)
  const trendSummary = computed(() => trendView.value.summary)

  const { resume: startAutoRefresh } = useIntervalFn(
    () => {
      void loadDashboardData(true)
    },
    30_000,
    { immediate: false },
  )

  async function loadDashboardData(silent = false) {
    if (summaryController || refreshing.value)
      return
    const revision = viewRevision
    try {
      loading.value = !silent
      await loadDashboardSnapshot(silent)
    }
    catch {
      // 自动刷新会继续重试，保留最后一次成功快照。
    }
    finally {
      if (revision === viewRevision)
        loading.value = false
    }
  }

  async function refreshDashboardData() {
    if (summaryController || refreshing.value)
      return
    refreshing.value = true
    const revision = viewRevision
    try {
      await withMinimumDuration(loadDashboardSnapshot)
    }
    catch {
      // 手动刷新失败时保留当前数据，不打断概览操作。
    }
    finally {
      if (revision === viewRevision)
        refreshing.value = false
    }
  }

  async function loadTrend(kind: string) {
    const trendKind = normalizeDashboardTrendKind(kind)
    activeTrendKind.value = trendKind
    const requestId = ++trendRequestId
    trendController?.abort()
    trendController = new AbortController()
    trendLoading.value = true
    trendError.value = ''
    try {
      const result = await getDashboardTrend({
        kind: trendKind,
        period: 'today',
        asOf: rangeAsOf,
        provider: provider.value || undefined,
      }, { signal: trendController.signal })
      if (isCurrentTrendRequest(requestId, trendKind))
        trend.value = result
    }
    catch (error: unknown) {
      if (isCurrentTrendRequest(requestId, trendKind))
        trendError.value = errorMessage(error)
    }
    finally {
      if (isCurrentTrendRequest(requestId, trendKind))
        trendLoading.value = false
    }
  }

  async function loadDashboardSnapshot(silent = false) {
    const trendKind = activeTrendKind.value
    const summaryId = ++summaryRequestId
    const selectedProvider = provider.value
    const asOf = Date.now()
    const requestId = ++trendRequestId
    trendController?.abort()
    summaryController = new AbortController()
    // 自动刷新保留当前空态和错误，等待成功快照后再更新展示。
    if (!silent) {
      trendLoading.value = true
      trendError.value = ''
    }
    try {
      const response = await getDashboardSummary({
        kind: trendKind,
        period: 'today',
        asOf,
        provider: selectedProvider || undefined,
      }, { silent, signal: summaryController.signal })
      if (disposed || summaryId !== summaryRequestId)
        return
      rangeAsOf = asOf
      summary.value = response
      analyticsError.value = ''
      lastRefreshedAt.value = response.asOfDisplay
      if (isCurrentTrendRequest(requestId, trendKind)) {
        trend.value = response.trend
        trendError.value = ''
      }
      else {
        // 快照加载期间切换指标时，用已提交快照的同一锚点重新查询趋势。
        void loadTrend(activeTrendKind.value)
      }
    }
    catch (error: unknown) {
      if (disposed || summaryId !== summaryRequestId)
        return
      if (!disposed)
        analyticsError.value = errorMessage(error)
      if (!silent && isCurrentTrendRequest(requestId, trendKind))
        trendError.value = errorMessage(error)
      throw error
    }
    finally {
      if (summaryId === summaryRequestId)
        summaryController = undefined
      if (isCurrentTrendRequest(requestId, trendKind))
        trendLoading.value = false
    }
  }

  function isCurrentTrendRequest(
    requestId: number,
    kind: ReturnType<typeof normalizeDashboardTrendKind>,
  ) {
    return !disposed && requestId === trendRequestId && activeTrendKind.value === kind
  }

  watch(provider, () => {
    // 切换范围立即丢弃旧快照；取消请求之外再用序号拦截晚到的结果。
    viewRevision += 1
    summaryRequestId += 1
    trendRequestId += 1
    summaryController?.abort()
    trendController?.abort()
    summaryController = undefined
    trendController = undefined
    summary.value = null
    trend.value = null
    analyticsError.value = ''
    trendError.value = ''
    lastRefreshedAt.value = ''
    refreshing.value = false
    rangeAsOf = Date.now()
    void loadDashboardData()
  })

  onMounted(() => {
    void loadDashboardData()
    startAutoRefresh()
  })

  onScopeDispose(() => {
    disposed = true
    trendRequestId += 1
    summaryController?.abort()
    trendController?.abort()
  })

  return {
    provider,
    loading,
    refreshing,
    activeTrendKind,
    lastRefreshedAt,
    metrics,
    analytics,
    analyticsError,
    trendPoints,
    trendSummary,
    trendLoading,
    trendError,
    healthTimeline,
    accountUsage,
    wireProfiles,
    usageRecords,
    poolSummary,
    capacityInfo,
    rotationStrategy,
    refresh: refreshDashboardData,
    loadTrend,
  }
}
