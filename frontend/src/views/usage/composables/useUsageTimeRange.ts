import type { DatePickerValue } from '@codex-proxy/ui'
import { computed, shallowRef } from 'vue'

export interface UsageTimeRangeParams {
  startTime: string
  endTime: string
}

function dateStart(date: Date, offset = 0) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + offset)
}

export function useUsageTimeRange(timezone: string) {
  const dateFormatter = new Intl.DateTimeFormat('en-US', {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })

  function businessDate(asOf: number) {
    const parts = Object.fromEntries(dateFormatter.formatToParts(asOf).map(part => [part.type, part.value]))
    // Date 仅承载选择器上的年月日，不能用其浏览器零点作为查询时间点。
    return new Date(Number(parts.year), Number(parts.month) - 1, Number(parts.day))
  }

  function dateValue(date: Date) {
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
  }

  const now = new Date()
  const selectedDates = shallowRef<[Date, Date]>([
    businessDate(now.getTime()),
    businessDate(now.getTime()),
  ])
  const timeRange = computed<DatePickerValue>({
    get: () => selectedDates.value,
    set: (value) => {
      if (!Array.isArray(value) || value.length !== 2)
        return
      const [start, end] = value
      if (start instanceof Date && end instanceof Date && !disabledDate(start) && !disabledDate(end) && start <= end)
        selectedDates.value = [start, end]
    },
  })
  const rangeEnd = shallowRef(now.getTime())
  const timeRangeParams = computed(() => rangeParams(rangeEnd.value))

  function disabledDate(date: Date) {
    const today = businessDate(Date.now())
    const value = dateStart(date).getTime()
    return !Number.isFinite(value) || value < dateStart(today, -30).getTime() || value > today.getTime()
  }

  function rangeParams(asOf: number): UsageTimeRangeParams {
    const [start, end] = selectedDates.value
    // 历史范围传排他的次日日期，由后端按业务时区换算；当天截到实际查询时间。
    return {
      startTime: dateValue(start),
      endTime: dateStart(end) >= businessDate(asOf)
        ? new Date(asOf).toISOString()
        : dateValue(dateStart(end, 1)),
    }
  }

  function refreshTimeRangeEnd() {
    rangeEnd.value = Date.now()
  }

  function latestTimeRangeParams() {
    return rangeParams(Date.now())
  }

  return {
    timeRange,
    timeRangeParams,
    disabledDate,
    refreshTimeRangeEnd,
    latestTimeRangeParams,
  }
}
