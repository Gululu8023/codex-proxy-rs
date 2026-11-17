import type { UsageListRecord, UsageRecordDetail } from '@/api'
import { computed, shallowRef, watch } from 'vue'
import { getUsageRecordDetail } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { isErrorUsageRecord } from '../utils/detail'

export function useUsageRecordDetail() {
  const showDetailModal = shallowRef(false)
  const selectedUsageRecord = shallowRef<UsageRecordDetail | null>(null)
  const selectedRecord = shallowRef<Pick<UsageListRecord, 'id'> & Partial<Pick<UsageListRecord, 'logicalOutcome'>> | null>(null)
  const request = useRequestState()
  const detailIsError = computed(() => {
    const record = selectedUsageRecord.value ?? selectedRecord.value
    return isErrorUsageRecord(record?.logicalOutcome === undefined
      ? null
      : { logicalOutcome: record.logicalOutcome })
  })

  async function loadDetail() {
    if (!selectedRecord.value || !showDetailModal.value)
      return
    const sequence = request.start()
    try {
      const detail = await getUsageRecordDetail({ id: selectedRecord.value.id }, { signal: request.signal, silent: true })
      if (request.isCurrent(sequence))
        selectedUsageRecord.value = detail
    }
    catch (cause) {
      request.fail(sequence, cause)
    }
    finally {
      request.finish(sequence)
    }
  }

  function handleViewDetail(record: UsageListRecord) {
    selectedRecord.value = { id: record.id, logicalOutcome: record.logicalOutcome }
    selectedUsageRecord.value = null
    showDetailModal.value = true
    void loadDetail()
  }

  function openRequest(id: string) {
    // 审查记录深链只有请求 ID，错误类型等详情返回后再确定
    selectedRecord.value = { id }
    selectedUsageRecord.value = null
    showDetailModal.value = true
    void loadDetail()
  }

  watch(showDetailModal, (open) => {
    if (!open)
      request.invalidate({ resetLoading: false })
  }, { flush: 'sync' })

  return {
    showDetailModal,
    selectedUsageRecord,
    detailIsError,
    detailLoading: request.loading,
    detailError: request.error,
    handleViewDetail,
    reloadDetail: loadDetail,
    openRequest,
  }
}
