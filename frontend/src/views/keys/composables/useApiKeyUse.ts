import type { ApiKey } from '@/api'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { revealApiKey } from '@/api'
import { getSettings } from '@/api/modules/settings'
import { useCopyText } from '@/composables/useCopyText'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { buildCodexCcSwitchImportDeeplink, resolveServiceRootUrl } from '@/utils/client'
import { notification as ZNotification } from '@/utils/notification'

type KeyRevealAction = 'copy' | 'use' | 'import'

// 明文读取、复制、展示与导入共用生命周期，列表数据不保存明文。
export function useApiKeyUse() {
  const copyText = useCopyText()
  const revealingKeys = shallowRef(new Map<string, KeyRevealAction>())
  const revealRequest = useRequestState()
  const showUseKeyModal = shallowRef(false)
  const selectedUseKey = shallowRef<(ApiKey & { key: string, claudePublicUrl: string }) | null>(null)
  const showCreatedKeyModal = shallowRef(false)
  const createdKey = shallowRef('')
  const showImportKeyModal = shallowRef(false)
  const selectedImportKey = shallowRef<ApiKey | null>(null)
  watch(showUseKeyModal, (open) => {
    if (!open)
      selectedUseKey.value = null
  })

  function showCreatedKey(key: string) {
    createdKey.value = key
    showCreatedKeyModal.value = true
  }

  const openAiBaseUrl = `${resolveServiceRootUrl()}/v1`

  function openUseKeyModal(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'use', async (key) => {
      const signal = revealRequest.signal
      const settings = await getSettings({ signal })
      if (signal?.aborted)
        return
      selectedUseKey.value = { ...apiKey, key, claudePublicUrl: settings.claudePublicUrl }
      showUseKeyModal.value = true
    })
  }

  function openImportKeyModal(apiKey: ApiKey) {
    // Claude 专用密钥沿用原生配置入口，不能导入为 Codex 配置。
    if (apiKey.providerKinds.length && !apiKey.providerKinds.includes('openai'))
      return openUseKeyModal(apiKey)
    revealRequest.invalidate()
    selectedImportKey.value = apiKey
    showImportKeyModal.value = true
  }

  function importSelectedKey() {
    const apiKey = selectedImportKey.value
    if (!apiKey)
      return
    return withPlaintextKey(apiKey, 'import', (key) => {
      window.location.href = buildCodexCcSwitchImportDeeplink({
        apiKey: key,
        baseUrl: openAiBaseUrl,
        providerName: apiKey.name || apiKey.prefix || 'codex-proxy-rs',
      })
    })
  }

  async function copyToClipboard(text: string) {
    await copyText(text, {
      notifyWhenEmpty: true,
    })
  }

  async function withPlaintextKey(apiKey: ApiKey, action: KeyRevealAction, use: (key: string) => void | Promise<void>) {
    if (revealingKeys.value.has(apiKey.id))
      return
    // 明文操作共用一个当前意图，跨行点击和页面离开都使旧结果失效
    const requestId = revealRequest.start()
    revealingKeys.value = new Map(revealingKeys.value).set(apiKey.id, action)
    try {
      const result = await revealApiKey({ id: apiKey.id }, { signal: revealRequest.signal })
      if (!revealRequest.isCurrent(requestId))
        return
      if (!result.plaintextKey) {
        ZNotification.error({
          message: t('keys.configuration.fullApiKeyNotAvailable'),
        })
        return
      }
      await use(result.plaintextKey)
    }
    catch (error) {
      revealRequest.fail(requestId, error)
    }
    finally {
      const pendingKeys = new Map(revealingKeys.value)
      pendingKeys.delete(apiKey.id)
      revealingKeys.value = pendingKeys
      revealRequest.finish(requestId)
    }
  }

  function copyApiKey(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'copy', copyToClipboard)
  }

  function clearCreatedKey() {
    createdKey.value = ''
  }

  onScopeDispose(() => {
    selectedUseKey.value = null
    selectedImportKey.value = null
    clearCreatedKey()
  })

  return {
    showUseKeyModal,
    selectedUseKey,
    showCreatedKeyModal,
    createdKey,
    showImportKeyModal,
    selectedImportKey,
    revealingKeys,
    openAiBaseUrl,
    showCreatedKey,
    clearCreatedKey,
    copyToClipboard,
    copyApiKey,
    openUseKeyModal,
    openImportKeyModal,
    importSelectedKey,
  }
}
