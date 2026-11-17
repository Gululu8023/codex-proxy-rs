import type { RequestOptions } from '../../request'
import type { RequestLocation, RequestLocationDetection } from '../../types/location'
import type { CodexPrivacyPolicy } from './privacy'
import type { ClientProfileSelection, ProviderRequestProfiles, ProviderRequestProfileUpdates } from './profiles'
import request from '../../request'

export * from './backups'
export * from './pricing'
export * from './privacy'
export * from './system'

export function detectRequestLocation() {
  return request<RequestLocationDetection>({
    url: '/api/admin/settings/location/detect',
    method: 'POST',
    timeout: 25000,
  })
}

export type AccountAffinity = 'relaxed' | 'preferred' | 'strict'

export type RotationStrategy = 'smart' | 'quota_reset_priority' | 'round_robin' | 'sticky'

export interface ResetWindowWeights {
  fiveHour: number
  weekly: number
  monthly: number
}

export interface SmartSchedulingConfig {
  loadWeight: number
  quotaWeight: number
  healthWeight: number
  latencyWeight: number
  resetWeight: number
  queueWeight: number
  resetWindowWeights: ResetWindowWeights
}

export interface PromptGuardConfig {
  enabled: boolean
  mode: 'observe' | 'enforce'
  localAction: 'warn' | 'block'
  threshold: number
  strictThreshold: number
  strictTerminalEnabled: boolean
  reviewEnabled: boolean
  reviewFailClosed: boolean
  reviewMaxConcurrent: number
  reviewRequestMode: 'chat_completions' | 'moderations'
  moderationThresholds: Record<string, number>
  scope: 'local_candidates' | 'all'
  reviewUrl: string
  reviewModel: string
  reviewApiKey?: string
  hasApiKey: boolean
  apiKeyCount: number
  state: 'disabled' | 'ready' | 'candidate_review_unavailable'
  timeoutSeconds: number
  confidenceThresholdBps: number
  maxTextChars: number
  disabledRules: string[]
  conversationLockEnabled: boolean
  conversationLockTtlHours: number
  learningEnabled: boolean
  learningModel: string
}

export interface RuntimeSettings {
  promptGuard: PromptGuardConfig
  claudePublicUrl: string
  configRevision: number
  smartScheduling: SmartSchedulingConfig
  smartSchedulingDefaults: SmartSchedulingConfig
  quotaResetWindowWeights: ResetWindowWeights
  quotaResetWindowWeightsDefaults: ResetWindowWeights
  providerRequestProfiles: ProviderRequestProfiles
  openaiClientProfile: ClientProfileSelection | null

  codexPrivacyPolicy: CodexPrivacyPolicy
  requestLocationEnabled: boolean
  requestLocation: RequestLocation
  modelMappings: Record<string, string>
  anthropicModelMappings: Record<string, string>
  refreshMarginSeconds: number
  refreshConcurrency: number
  maxConcurrentPerAccount: number
  openaiGuardianReservedConcurrency: number
  openaiAccountAffinity: AccountAffinity
  anthropicAccountAffinity: AccountAffinity
  maxAccountRotations: number
  openaiSessionAffinityTtlHours: number
  anthropicSessionAffinityTtlHours: number
  maxWaitingPerKey: number
  maxWaitingPerAccount: number
  keyQueueTimeoutSeconds: number
  accountQueueTimeoutSeconds: number
  switchAccountOnQueueFailure: boolean
  maxRetryAfterSeconds: number
  responsesMaxDecompressedBodyBytes: number
  rotationStrategy: RotationStrategy
  minCodexDesktopVersion: string | null
  minCodexCliVersion: string | null
  minClaudeCodeVersion: string | null
  usageRetentionDays: number
  opsEventRetentionDays: number
  auditRetentionDays: number
  accountWarmupEnabled: boolean
  accountWarmupCron: string
  accountWarmupModel: string | null
  resetCreditOnLowWeeklyQuota: boolean
  resetCreditLeadMinutes: number
  resetCreditMaxWeeklyRemainingPercent: number | null
  updatedAt: string
  updatedAtDisplay: string
}

export type ClientArchitecture = 'x64' | 'arm64'
export type ClientDownloadSource = 'microsoft_store' | 'official_openai'

export interface ClientDownloadPackage {
  architecture: ClientArchitecture
  source: ClientDownloadSource
  version: string | null
  fileName: string
  sizeBytes: number | null
  downloadUrl: string
  expiresAt: string | null
  expiresAtDisplay: string | null
}

export interface CodexDesktopWindowsDownloads {
  resolvedAt: string
  resolvedAtDisplay: string
  cached: boolean
  warning: string | null
  packages: ClientDownloadPackage[]
}

export interface AdminApiKeyStatus {
  exists: boolean
}

export interface RegeneratedAdminApiKey {
  key: string
}

export interface DeletedAdminApiKey {
  message: string
}

export function getSettings(options: RequestOptions = {}) {
  return request<RuntimeSettings>({
    url: '/api/admin/settings',
    method: 'GET',
    ...options,
  })
}

type UpdateSettingsParam = Omit<RuntimeSettings, 'promptGuard' | 'claudePublicUrl' | 'updatedAt' | 'updatedAtDisplay' | 'smartSchedulingDefaults' | 'quotaResetWindowWeightsDefaults' | 'openaiClientProfile' | 'providerRequestProfiles'> & {
  providerRequestProfiles: ProviderRequestProfileUpdates
}

export function updateSettings(data: UpdateSettingsParam) {
  return request<RuntimeSettings>({
    url: '/api/admin/settings/update',
    method: 'POST',
    data,
  })
}

export function getAdminApiKeyStatus() {
  return request<AdminApiKeyStatus>({
    url: '/api/admin/settings/admin-api-key',
    method: 'GET',
  })
}

export function regenerateAdminApiKey() {
  return request<RegeneratedAdminApiKey>({
    url: '/api/admin/settings/admin-api-key/regenerate',
    method: 'POST',
  })
}

export function deleteAdminApiKey() {
  return request<DeletedAdminApiKey>({
    url: '/api/admin/settings/admin-api-key/delete',
    method: 'POST',
  })
}

interface CodexDesktopWindowsDownloadsParams {
  refresh?: boolean
}

export function getCodexDesktopWindowsDownloads(data: CodexDesktopWindowsDownloadsParams = {}) {
  return request<CodexDesktopWindowsDownloads>({
    url: '/api/admin/settings/client-downloads/codex-desktop/windows',
    method: 'GET',
    params: data,
  })
}
