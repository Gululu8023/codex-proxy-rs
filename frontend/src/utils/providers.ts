import type { Component } from 'vue'
import { Key, LinkAlt, Openai } from '@boxicons/vue'
import { Fingerprint, Plug } from '@lucide/vue'
import ClaudeIcon from '@/components/icons/ClaudeIcon.vue'
import { t } from '@/i18n'

const PROVIDER_DISPLAY_NAMES = {
  openai: 'Codex',
  anthropic: 'Claude',
} as const

// 供应商切换统一按 Codex、Claude 展示，避免接口顺序和字母排序改变按钮位置。
export const PROVIDER_IDS = ['openai', 'anthropic'] as const

type SupportedProvider = keyof typeof PROVIDER_DISPLAY_NAMES

export function isSupportedProvider(value: unknown): value is SupportedProvider {
  return typeof value === 'string' && Object.hasOwn(PROVIDER_DISPLAY_NAMES, value)
}

export function compareProviders(left: string, right: string) {
  const leftOrder = isSupportedProvider(left)
    ? PROVIDER_IDS.indexOf(left)
    : PROVIDER_IDS.length
  const rightOrder = isSupportedProvider(right)
    ? PROVIDER_IDS.indexOf(right)
    : PROVIDER_IDS.length
  return leftOrder - rightOrder || left.localeCompare(right)
}

export function formatProviderLabel(value?: string | null, fallback = '—') {
  const normalized = value?.trim()
  const provider = normalized?.toLowerCase()
  return isSupportedProvider(provider)
    ? PROVIDER_DISPLAY_NAMES[provider]
    : (normalized || fallback)
}

export function providerIcon(value: string): Component {
  switch (value.trim().toLowerCase()) {
    case 'openai': return Openai
    case 'anthropic': return ClaudeIcon
    default: return Plug
  }
}

export function formatAuthenticationLabel(value?: string | null) {
  const normalized = value?.trim()
  switch (normalized?.toLowerCase()) {
    case 'oauth': return 'OAuth'
    case 'api_key': return t('common.fields.apiKey')
    default: return normalized
      ? t('common.providers.customAuth', { normalized })
      : t('common.providers.unknownAuth')
  }
}

export function authenticationIcon(value?: string | null): Component | undefined {
  switch (value?.trim().toLowerCase()) {
    case 'oauth': return LinkAlt
    case 'api_key': return Key
    case undefined:
    case '': return undefined
    default: return Fingerprint
  }
}
