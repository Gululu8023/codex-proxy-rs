export type SettingsProvider = 'openai' | 'anthropic'
export type ProviderSettingsCard = 'profile' | 'models' | 'affinity' | 'privacy' | 'versions'
export type SettingsScope = `${SettingsProvider}-${ProviderSettingsCard}`
  | 'scheduling' | 'concurrency' | 'history' | 'refresh' | 'location'
  | 'review' | 'warmup' | 'reset' | 'body-limit'

// 将校验错误定位到卡片和供应商；基础设置由页面顶部统一提交。
const FIELD_SCOPES: Record<string, SettingsScope> = {
  rotationStrategy: 'scheduling',
  smartScheduling: 'scheduling',
  quotaResetWindowWeights: 'scheduling',
  maxConcurrentPerAccount: 'concurrency',
  maxAccountRotations: 'concurrency',
  maxWaitingPerKey: 'concurrency',
  maxWaitingPerAccount: 'concurrency',
  keyQueueTimeoutSeconds: 'concurrency',
  accountQueueTimeoutSeconds: 'concurrency',
  switchAccountOnQueueFailure: 'concurrency',
  maxRetryAfterSeconds: 'concurrency',
  usageRetentionDays: 'history',
  refreshMarginSeconds: 'refresh',
  refreshConcurrency: 'refresh',
  openaiGuardianReservedConcurrency: 'review',
  accountWarmupEnabled: 'warmup',
  accountWarmupCron: 'warmup',
  accountWarmupModel: 'warmup',
  resetCreditLeadMinutes: 'reset',
  resetCreditMaxWeeklyRemainingPercent: 'reset',
  resetCreditOnLowWeeklyQuota: 'reset',
  minCodexDesktopVersion: 'openai-versions',
  minCodexCliVersion: 'openai-versions',
  minClaudeCodeVersion: 'anthropic-versions',
  responsesMaxDecompressedBodyMiB: 'body-limit',
  codexPrivacyPolicy: 'openai-privacy',
  requestLocationEnabled: 'location',
  requestLocation: 'location',
  modelMappings: 'openai-models',
  anthropicModelMappings: 'anthropic-models',
  openaiAccountAffinity: 'openai-affinity',
  openaiSessionAffinityTtlHours: 'openai-affinity',
  anthropicAccountAffinity: 'anthropic-affinity',
  anthropicSessionAffinityTtlHours: 'anthropic-affinity',
}

export function settingsFieldScope(path: string): SettingsScope | undefined {
  return FIELD_SCOPES[path.split('.')[0]!]
}
