import type { RequestOptions } from '../request'
import type { PromptGuardConfig } from './settings'
import request from '../request'

export interface ClaudeSettings { configRevision: number, publicUrl: string }
export interface GuardSettings { configRevision: number, config: PromptGuardConfig, retentionDays: number }
export interface VeilHealth {
  ready: boolean
  revision: string
  rulesRevision: string | null
  ruleCount: number
  maxMappings: number
  /** 主密钥的域分离指纹，用于与备份清单 `keyFingerprint` 核对 */
  keyFingerprint: string | null
  availableSlots: number
  mappingCount: number | null
  lastError: string | null
  lastErrorAt: string | null
  observedAt: string
  automaticEviction: boolean
  packs: { secrets: boolean, region: boolean } | null
  categories: Record<string, number>
  redactionOperations: number
  restorationOperations: number
  failedOperations: number
  processingMs: number
}
export type GuardAction = 'disabled' | 'allow' | 'warn' | 'block' | 'coverage_error' | 'unavailable' | 'cancelled'
export interface GuardHit {
  ruleId: string
  category: string
  origin: string
  source: string
  representation: string
  weight: number
  effectiveWeight: number
  signalOnly: boolean
  observationOnly: boolean
}
export interface GuardAssessment {
  policyVersion: string
  rulesVersion: string
  normalizationVersion: string
  mode: 'observe' | 'enforce'
  action: GuardAction
  wouldAction: GuardAction
  rawScore: number
  signalScore: number
  auditScore: number
  decisionScore: number
  categories: { category: string, rawScore: number, decisionScore: number, observationOnly: boolean }[]
  hits: GuardHit[]
  omittedHits: number
  coverage: 'complete' | 'text_only' | 'incomplete'
  reasonCode: string | null
  review: null | { result: string, confidenceBps: number | null, thresholdBps: number, evaluatedCategories: string[], inputChars: number, selectedBlocks: number, omittedBlocks: number, omittedChars: number, projection: string, redacted: boolean, requestMode: string, categoryScores: Record<string, number> }
  localMs: number
  totalMs: number
  evaluationCount: number
  reuseCount: number
  terminal: boolean
  upstreamPolicy: { code: string, recorded: boolean, lockApplied: boolean } | null
}
export interface GuardRecord {
  requestId: string
  startedAt: string
  startedAtDisplay: string | null
  clientKeyId: string
  protocol: string | null
  model: string
  outcome: string
  assessment: GuardAssessment | null
}
export interface GuardPage {
  recordingHealth: { queuedItems: number, queuedBytes: number, droppedTotal: number, writeFailureTotal: number, persistedTotal: number } | null
  items: GuardRecord[]
  summary: { total: number, recorded: number, missing: number, allowed: number, blocked: number, unavailable: number, coverageError: number, observed: number, observedHits: number, averageReviewMs: number | null, cancelled: number, disabled: number }
  page: number
  pageSize: number
}
export interface GuardQuery {
  start: string
  end: string
  page: number
  pageSize: number
  requestId?: string
  protocol?: string
  action?: string
  clientKeyId?: string
  model?: string
  mode?: string
  category?: string
  policyVersion?: string
  reviewResult?: string
  minScore?: number
  maxScore?: number
  origin?: string
}
export interface GuardPolicy {
  rulesRevision: number
  policyVersion: string
  rulesVersion: string
  normalizationVersion: string
  candidateThreshold: number
  localTerminationEnabled: boolean
  executingCategories: string[]
  observingCategories: string[]
  rules: { id: string, category: string, pattern: string, weight: number, signalOnly: boolean, observationOnly: boolean, localTermination: boolean, sources: string[], intentRequired: boolean, enabled: boolean, source: 'builtin' | 'custom', allPatterns: string[], anyPatterns: string[], excludePatterns: string[], authorizationExcludePatterns: string[] }[]
}
export interface GuardRule {
  id: string
  pattern: string
  category: string
  weight: number
  strict: boolean
  signalOnly: boolean
  allPatterns: string[]
  anyPatterns: string[]
  excludePatterns: string[]
}
export interface GuardCandidate {
  id: number
  revision: number
  status: 'pending' | 'published' | 'dismissed' | 'disabled'
  source: string
  protocol: string
  evidenceCount: number
  evidence: string | null
  evidenceQuality: string
  rule: GuardRule | null
  rationale: string
  lastSeenAt: string
}
export interface GuardListQuery { page: number, pageSize: number, protocol?: string, status?: string }
export interface GuardCandidatePage { items: GuardCandidate[], total: number, page: number, pageSize: number }
export interface GuardProfiles {
  items: { key: string, protocol: string, requests: number, blocked: number, warned: number, upstream_rejections: number, maximum_score: number | null, last_seen_at: string }[]
  total: number
  page: number
  pageSize: number
  windowDays: number
}
export interface GuardLocks {
  items: { id: number, revision: number, client_key_id: string, protocol: string, request_id: string, expires_at: string }[]
  total: number
  page: number
  pageSize: number
}
export function evaluateGuard(data: { text: string, origin: string }, options: RequestOptions = {}) {
  return request<GuardAssessment>({
    url: '/api/admin/prompt-guard/evaluate',
    method: 'POST',
    data,
    ...options,
  })
}
export const getGuardCandidates = (params: GuardListQuery, options: RequestOptions = {}) => request<GuardCandidatePage>({ url: '/api/admin/prompt-guard/candidates', params, ...options })
export const saveGuardCandidate = (data: { id: number, revision: number, rule: GuardRule, rationale: string }) => request<GuardCandidate>({ url: '/api/admin/prompt-guard/candidates/save', method: 'POST', data })
export const actOnGuardCandidate = (data: { id: number, revision: number, action: string }) => request<GuardCandidate>({ url: '/api/admin/prompt-guard/candidates/action', method: 'POST', data })
export const getGuardProfiles = (params: GuardListQuery, options: RequestOptions = {}) => request<GuardProfiles>({ url: '/api/admin/prompt-guard/profiles', params, ...options })
export const getGuardLocks = (params: GuardListQuery, options: RequestOptions = {}) => request<GuardLocks>({ url: '/api/admin/prompt-guard/locks', params, ...options })
export const releaseGuardLock = (data: { id: number, revision: number }) => request<{ released: boolean }>({ url: '/api/admin/prompt-guard/locks/release', method: 'POST', data })
export const getClaudeSettings = (options: RequestOptions = {}) => request<ClaudeSettings>({ url: '/api/admin/claude/settings', ...options })
export const saveClaudeSettings = (data: ClaudeSettings) => request<ClaudeSettings>({ url: '/api/admin/claude/settings', method: 'POST', data })
export const getVeilHealth = (options: RequestOptions = {}) => request<VeilHealth>({ url: '/api/admin/claude/privacy', ...options })
export const getGuardSettings = (options: RequestOptions = {}) => request<GuardSettings>({ url: '/api/admin/prompt-guard/settings', ...options })
export const saveGuardSettings = (data: Pick<GuardSettings, 'configRevision' | 'config'>) => request<GuardSettings>({ url: '/api/admin/prompt-guard/settings/update', method: 'POST', data })
export const getGuardRecords = (params: GuardQuery, options: RequestOptions = {}) => request<GuardPage>({ url: '/api/admin/prompt-guard/records', params, ...options })
export const getGuardPolicy = (options: RequestOptions = {}) => request<GuardPolicy>({ url: '/api/admin/prompt-guard/policy', ...options })
