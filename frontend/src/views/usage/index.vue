<script setup lang="ts">
import type { TableColumn } from '@codex-proxy/ui'

import type { UsageListRecord, UsageRecordStatus } from '@/api'
import { useTableColumns, ZCard, ZDatePicker, ZIconButton, ZPageHeader, ZPagination, ZTableColumnSettings } from '@codex-proxy/ui'
import { Eye } from '@lucide/vue'

import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute } from 'vue-router'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { usageRecordColumns } from '@/components/usage/shared/columns'
import UsageRecordsTable from '@/components/usage/UsageRecordsTable.vue'
import UsageRecordStatusFilter from '@/components/usage/UsageRecordStatusFilter.vue'
import UsageErrorDetailModal from './components/UsageErrorDetailModal.vue'
import UsageFilters from './components/UsageFilters.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageRecordDetailModal from './components/UsageRecordDetailModal.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageRecordDetail } from './composables/useUsageRecordDetail'
import { useUsageRecordsTable } from './composables/useUsageRecordsTable'
import { useUsageTimeRange } from './composables/useUsageTimeRange'
import { isErrorUsageRecord } from './utils/detail'

const { timezone } = defineProps<{ timezone: string }>()

const { t } = useI18n({ useScope: 'global' })

const recordView = shallowRef<UsageRecordStatus>('success')
const recordColumns = computed<TableColumn<UsageListRecord>[]>(() => recordView.value === 'error'
  ? usageRecordColumns.value.flatMap(column => column.key === 'route'
      ? [
          column,
          {
            key: 'errorMessage',
            label: t('usage.filters.error'),
            kind: 'custom',
            size: '4xl',
            hideable: false,
          },
        ]
      : [column])
  : usageRecordColumns.value)
const { visibleColumns, columnOptions, setColumnVisible, setColumnOrder, resetColumns } = useTableColumns(recordColumns, 'usage-records')
const { timeRange, timeRangeParams, disabledDate, refreshTimeRangeEnd, latestTimeRangeParams }
  = useUsageTimeRange(timezone)

const {
  filterField,
  filterValue,
  providerQuery,
  usagePagination,
  loading,
  records,
  summary,
  health,
  performance,
  cost,
  diagnostics,
  performanceMetric,
  costMetric,
  refreshingList,
  diagnosticDimension,
  loadUsageRecords,
  refreshUsageRecords,
  handlePageChange,
  handlePageSizeChange,
} = useUsageRecordsTable({
  timeRangeParams,
  latestTimeRangeParams,
  status: recordView,
})

const { showDetailModal, selectedUsageRecord, detailIsError, detailLoading, detailError, handleViewDetail, reloadDetail, openRequest } = useUsageRecordDetail()
const route = useRoute()
watch(() => route.query.requestId, (id) => {
  if (typeof id === 'string' && id)
    void openRequest(id)
}, { immediate: true })
const detailComponent = computed(() => detailIsError.value
  ? UsageErrorDetailModal
  : UsageRecordDetailModal)

watch(timeRange, () => {
  refreshTimeRangeEnd()
  void loadUsageRecords()
})
</script>

<template>
  <div class="@container w-full">
    <ZPageHeader :title="t('navigation.usage')" :description="t('usage.page.description')" />
    <div class="mt-4 mb-5 flex min-w-0 flex-wrap items-center gap-3">
      <ProviderFilter v-model="providerQuery" :disabled="refreshingList" />
      <ZDatePicker
        v-model="timeRange"
        type="daterange"
        :clearable="false"
        :editable="false"
        :disabled-date="disabledDate"
        single-panel
        :aria-label="t('usage.page.dateRangeLabel')"
        class="min-w-0 flex-1 basis-full @min-[21rem]:basis-auto @min-[28rem]:w-60 @min-[28rem]:flex-none @max-[28rem]:[&>div]:gap-1.5 @max-[28rem]:[&>div]:px-2"
      />
      <span class="text-xs text-z-text-secondary">{{ timezone }}</span>
    </div>

    <UsageSummaryCards :summary="summary.data" :error="summary.error" />
    <UsageInsightsGrid
      v-model:diagnostic-dimension="diagnosticDimension"
      v-model:performance-metric="performanceMetric"
      v-model:cost-metric="costMetric"
      :health="health"
      :performance="performance"
      :cost="cost"
      :diagnostics="diagnostics"
    />

    <ZCard
      class="mt-5 flex flex-col"
    >
      <template #header>
        <div>
          <h2 class="m-0 text-xl leading-[1.15] font-heavy text-z-text">
            {{ t('usage.page.requestDetails') }}
          </h2>
          <p
            class="mt-1.75 mb-0 text-z leading-[1.15] font-emphasis text-z-text-secondary"
          >
            {{ t('usage.page.recordsDescription') }}
          </p>
        </div>
      </template>

      <template #body>
        <div
          class="grid min-h-130 min-w-0 flex-1 grid-rows-[auto_minmax(0,1fr)] gap-3"
        >
          <UsageFilters
            v-model:field="filterField"
            v-model:value="filterValue"
            :loading="loading"
            :refreshing="refreshingList"
            @refresh="refreshUsageRecords"
          >
            <template #actions>
              <UsageRecordStatusFilter v-model="recordView" />
              <ZTableColumnSettings
                :options="columnOptions"
                @change="setColumnVisible"
                @reorder="setColumnOrder"
                @reset="resetColumns"
              />
            </template>
          </UsageFilters>

          <div class="flex min-h-0 min-w-0 flex-col">
            <UsageRecordsTable
              class="min-h-0 flex-1"
              :columns="visibleColumns"
              :rows="records"
              :loading="loading"
              :empty-text="t('usage.page.noRequestsMatchTheseFilters')"
            >
              <template #actions="{ row }">
                <div class="flex items-center justify-start">
                  <ZIconButton
                    size="small"
                    :aria-label="isErrorUsageRecord(row) ? t('usage.page.viewErrorDetails') : t('usage.page.viewUsageRecordDetails')"
                    @click="handleViewDetail(row)"
                  >
                    <Eye class="size-3.5" />
                  </ZIconButton>
                </div>
              </template>
            </UsageRecordsTable>
            <ZPagination
              v-bind="usagePagination"
              :disabled="loading"
              @current-change="handlePageChange"
              @size-change="handlePageSizeChange"
            />
          </div>
        </div>
      </template>
    </ZCard>

    <component :is="detailComponent" v-model="showDetailModal" :record="selectedUsageRecord" :loading="detailLoading" :error="detailError" @retry="reloadDetail" />
  </div>
</template>
