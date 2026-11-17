import request from '../../request'

export type ProviderRequestProfile = Record<string, unknown>
export type ProviderRequestProfiles = Record<string, ProviderRequestProfile>
export type ProviderRequestProfileUpdates = Record<string, ProviderRequestProfile | null>

export interface ClaudeProfileSelection {
  mode: 'preset' | 'custom' | 'follow'
  client: 'cli' | 'vscode'
  platform: 'macos' | 'linux' | 'windows'
  arch: 'arm64' | 'x64'
  versionMode: 'latest' | 'stable' | 'fixed'
  version: string | null
  userAgent: string | null
  installationSeed?: string | null
}

export interface ClaudeProfileOptions {
  globalConfiguration: ClaudeProfileSelection
  presets: ClaudeProfileSelection[]
}

export interface ClaudeProfilePreview {
  configuration: ClaudeProfileSelection
  effective: {
    userAgent: string
    version: string
    entry: string
    os: string
    arch: string
    sdkVersion: string
    runtimeVersion: string
    template: string
    versionSource: string
    verifiedAt: string | null
  }
  checkedAt: string | null
  checkedAtDisplay: string | null
  error: string | null
  requiresClientIdentity: boolean
}

export function getClaudeProfileOptions() {
  return request<ClaudeProfileOptions>({
    url: '/api/admin/settings/client-profiles/anthropic',
    method: 'GET',
  })
}

export function previewClaudeProfile(configuration: ClaudeProfileSelection | null) {
  return request<ClaudeProfilePreview>({
    url: '/api/admin/settings/client-profiles/anthropic/preview',
    method: 'POST',
    data: { configuration },
    silent: true,
  })
}

export interface PresetClientProfileSelection {
  mode?: undefined
  client: 'desktop' | 'cli'
  platform: 'macos' | 'linux' | 'windows'
  versionMode: 'latest' | 'fixed'
  versionLag?: number | null
  cliEntry?: 'tui' | 'exec' | null
  osType?: string | null
  originator: string | null
  osVersion: string | null
  arch: string | null
  terminal: string | null
  codexVersion: string | null
  desktopVersion: string | null
  desktopBuild: string | null
}

export interface CustomClientProfileSelection {
  mode: 'custom'
  userAgent: string
  originator?: string | null
  codexVersion?: string | null
}

export type ClientProfileSelection = PresetClientProfileSelection | CustomClientProfileSelection

export function isValidInstallationSeed(value: unknown) {
  return value == null || value === '' || (typeof value === 'string' && value.length === 36 && /^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(value))
}

export interface ClientProfileOptions {
  presets: ClientProfilePreset[]
  globalConfiguration: ClientProfileSelection
  maxVersionLag: number
}

export interface ClientProfilePreview {
  configuration: ClientProfileSelection
  source: 'global' | 'override'
  originator: string
  osType: string
  osVersion: string
  arch: string
  terminal: string
  codexVersion: string
  desktopVersion: string | null
  desktopBuild: string | null
  userAgent: string
  versionSource: 'official' | 'custom'
  versionLag: number | null
  recognized?: boolean
  verifiedAt: string | null
  verifiedAtDisplay: string | null
  checkedAt: string | null
  checkedAtDisplay: string | null
  error: string | null
}

export interface ClientProfilePreset {
  configuration: PresetClientProfileSelection
  automaticAvailable: boolean
  reason: string | null
  defaults: Pick<PresetClientProfileSelection, 'originator' | 'osType' | 'osVersion' | 'arch' | 'terminal'>
}

export function getClientProfileOptions() {
  return request<ClientProfileOptions>({
    url: '/api/admin/settings/client-profiles/openai',
    method: 'GET',
  })
}

interface ClientProfilePreviewParam {
  configuration: ClientProfileSelection | null
}

export function previewClientProfile(data: ClientProfilePreviewParam) {
  return request<ClientProfilePreview>({
    url: '/api/admin/settings/client-profiles/openai/preview',
    method: 'POST',
    data,
    silent: true,
  })
}
