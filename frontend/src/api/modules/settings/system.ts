import type { RequestOptions } from '../../request'
import request from '../../request'

export type SystemUpdateChannel = 'stable' | 'rc' | 'beta' | 'alpha' | 'exp'

export interface SystemUpdatePolicy {
  channel: SystemUpdateChannel
  availableChannels: SystemUpdateChannel[]
}

export interface SystemVersion {
  version: string
  gitSha: string
  buildTime: string
  deploymentMode: string
  deploymentModeLabel: string
  updateChannel: string
  latestVersion: string
  hasUpdate: boolean
  updateCached: boolean
  updateWarning: string | null
  timezone: string
}

export interface SystemUpdateDetail {
  policy: SystemUpdatePolicy
  currentVersion: string
  latestVersion: string
  hasUpdate: boolean
  deploymentMode: string
  deploymentModeLabel: string
  buildType: string
  buildTypeLabel: string
  releaseUrl: string | null
  notes: string | null
  cached: boolean
  updateSupported: boolean
  unsupportedReason: string | null
  warning: string | null
  retryAt: string | null
  retryAtDisplay: string | null
}

export interface SystemUpdateAccepted {
  operationId: string
  deploymentMode: string
  message: string
  targetVersion: string
}

export interface SystemUpdateStatus {
  previousVersion: string | null
  currentVersion: string | null
  needRestart: boolean
  operation: {
    operationId: string | null
    kind: 'update' | 'rollback' | 'restart' | null
    status: 'idle' | 'running' | 'succeeded' | 'failed'
    targetVersion: string | null
    message: string | null
    error: string | null
    startedAt: string | null
    startedAtDisplay: string | null
    finishedAt: string | null
    finishedAtDisplay: string | null
  }
}

export interface SystemRestartAccepted {
  message: string
  operationId: string
}

export function getSystemVersion(options: RequestOptions = {}) {
  return request<SystemVersion>({
    url: '/api/admin/system/version',
    method: 'GET',
    ...options,
  })
}

interface SystemUpdateDetailQuery {
  summaryOnly?: boolean
  refresh?: boolean
  channel?: SystemUpdateChannel
}

interface SystemUpdateTarget {
  targetVersion: string
  channel: SystemUpdateChannel
}

export function getSystemUpdateDetail(data: SystemUpdateDetailQuery, options: RequestOptions = {}) {
  return request<SystemUpdateDetail>({
    url: '/api/admin/system/update/detail',
    method: 'GET',
    params: data,
    ...options,
  })
}

export function performSystemUpdate(data: SystemUpdateTarget, options: RequestOptions = {}) {
  return request<SystemUpdateAccepted>({
    url: '/api/admin/system/update',
    method: 'POST',
    data,
    ...options,
  })
}

export function restartSystem(options: RequestOptions = {}) {
  return request<SystemRestartAccepted>({
    url: '/api/admin/system/restart',
    method: 'POST',
    ...options,
  })
}

export function getSystemUpdateStatus(options: RequestOptions = {}) {
  return request<SystemUpdateStatus>({
    url: '/api/admin/system/update/status',
    method: 'GET',
    ...options,
  })
}
