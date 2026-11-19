export interface UsageReasoningRecord {
  reasoningEffort: string | null
  reasoningPreset?: string | null
  subagentKind?: string | null
  thinkingMode?: string | null
  thinkingBudgetTokens?: number | null
  reasoningObserved?: boolean | null
}

export interface ThinkingLabels {
  modes: Record<'adaptive' | 'enabled', string>
  budget: (tokens: number) => string
}

export function formatReasoning(record: UsageReasoningRecord): string {
  // 新 Claude 记录明确区分强度、模式和预算；旧记录不猜默认值，Codex preset 保持原合同。
  if (record.reasoningObserved !== true)
    return (!record.subagentKind && record.reasoningPreset) || record.reasoningEffort || '—'

  return record.reasoningEffort || '—'
}

export function formatThinking(record: UsageReasoningRecord, labels: ThinkingLabels): string | null {
  // 胶囊只表示请求显式启用 thinking，不代表实际产生思考；工具间进度不视为启用。
  if (record.reasoningObserved !== true || (record.thinkingMode !== 'adaptive' && record.thinkingMode !== 'enabled'))
    return null

  const parts = [labels.modes[record.thinkingMode]]
  if (record.thinkingBudgetTokens != null)
    parts.push(labels.budget(record.thinkingBudgetTokens))
  return parts.join(' · ')
}
