export interface UsageReasoningRecord {
  reasoningEffort: string | null
  reasoningPreset?: string | null
  subagentKind?: string | null
  thinkingMode?: string | null
  thinkingBudgetTokens?: number | null
  reasoningObserved?: boolean | null
}

export interface ReasoningLabels {
  unspecified: string
  modes: Record<string, string>
  budget: (tokens: number) => string
}

export function formatReasoning(record: UsageReasoningRecord, labels: ReasoningLabels): string {
  // 新 Claude 记录明确区分强度、模式和预算；旧记录不猜默认值，Codex preset 保持原合同。
  if (record.reasoningObserved !== true)
    return (!record.subagentKind && record.reasoningPreset) || record.reasoningEffort || '—'

  const parts: string[] = []
  if (record.reasoningEffort)
    parts.push(record.reasoningEffort)
  if (record.thinkingMode && labels.modes[record.thinkingMode])
    parts.push(labels.modes[record.thinkingMode])
  if (record.thinkingBudgetTokens != null)
    parts.push(labels.budget(record.thinkingBudgetTokens))
  return parts.join(' · ') || labels.unspecified
}
