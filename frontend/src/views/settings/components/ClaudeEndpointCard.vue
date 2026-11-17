<script setup lang="ts">
import type { ClaudeSettings } from '@/api/modules/claude'
import { ZButton, ZCard, ZConfirmDialog, ZFormItem, ZInput } from '@codex-proxy/ui'
import { computed, onMounted, onScopeDispose, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { onBeforeRouteLeave } from 'vue-router'
import { getClaudeSettings, saveClaudeSettings } from '@/api/modules/claude'
import { ApiError } from '@/api/request'
import { useRequestState } from '@/composables/useRequestState'

const { t } = useI18n({ useScope: 'global' })
const query = useRequestState()
const mutation = useRequestState()
const { loading, error } = query
const { loading: saving, error: saveError } = mutation
const settings = shallowRef<ClaudeSettings | null>(null)
const publicUrl = ref('')
const conflict = ref(false)
const serverSettings = shallowRef<ClaudeSettings | null>(null)
const dirty = computed(() => settings.value !== null && publicUrl.value !== settings.value.publicUrl)
const discardOpen = shallowRef(false)
let resolveDiscard: ((confirmed: boolean) => void) | undefined

function confirmDiscard() {
  resolveDiscard?.(false)
  discardOpen.value = true
  return new Promise<boolean>((resolve) => {
    resolveDiscard = resolve
  })
}

function finishDiscard(confirmed: boolean) {
  resolveDiscard?.(confirmed)
  resolveDiscard = undefined
  discardOpen.value = false
}

// 关闭确认框或卸载组件也必须结束等待，避免路由守卫一直挂起。
watch(discardOpen, (visible) => {
  if (!visible)
    finishDiscard(false)
})
onScopeDispose(() => finishDiscard(false))

async function load() {
  const id = query.start()
  try {
    const configuration = await getClaudeSettings({ signal: query.signal })
    if (!query.isCurrent(id))
      return
    settings.value = configuration
    publicUrl.value = configuration.publicUrl
    conflict.value = false
    serverSettings.value = null
    saveError.value = ''
  }
  catch (cause) {
    query.fail(id, cause)
  }
  finally {
    query.finish(id)
  }
}

async function reload() {
  if (!dirty.value || await confirmDiscard())
    await load()
}

async function compare() {
  const id = query.start()
  try {
    const result = await getClaudeSettings({ signal: query.signal })
    if (query.isCurrent(id))
      serverSettings.value = result
  }
  catch (cause) {
    query.fail(id, cause)
  }
  finally {
    query.finish(id)
  }
}

async function save() {
  if (!settings.value || saving.value || loading.value || !dirty.value || conflict.value)
    return
  const id = mutation.start()
  try {
    // 与基础设置同页展示，但仍通过领域命令和读取时的版本保存，避免覆盖其他配置。
    const result = await saveClaudeSettings({
      configRevision: settings.value.configRevision,
      publicUrl: publicUrl.value.trim(),
    })
    if (mutation.isCurrent(id)) {
      settings.value = result
      publicUrl.value = result.publicUrl
      conflict.value = false
    }
  }
  catch (cause) {
    conflict.value = cause instanceof ApiError && cause.status === 409
    mutation.fail(id, cause)
  }
  finally {
    mutation.finish(id)
  }
}

onBeforeRouteLeave(async () => {
  const baseline = settings.value
  if (!baseline || !dirty.value)
    return true
  if (!await confirmDiscard())
    return false
  // 设置子路由复用同一组件，确认放弃后不能把旧草稿带到下一节。
  publicUrl.value = baseline.publicUrl
  return true
})
onMounted(load)
</script>

<template>
  <ZCard id="claude-public-url" :title="t('claude.publicUrl')">
    <template #actions>
      <ZButton size="small" :loading="loading" :disabled="saving" @click="reload">
        {{ t('common.actions.refresh') }}
      </ZButton>
    </template>
    <template #body>
      <p v-if="error || saveError" role="alert" class="text-z-error-text">
        {{ error || saveError }}
      </p>
      <ZFormItem :label="t('claude.publicUrl')" :description="t('claude.publicUrlHelp')">
        <ZInput v-model="publicUrl" :disabled="!settings || saving || loading" :aria-label="t('claude.publicUrl')" placeholder="https://claude.example.com" />
      </ZFormItem>
      <div class="mt-3 flex flex-wrap items-center justify-between gap-3">
        <span class="text-z-sm text-z-text-secondary">{{ settings ? t('integration.revision', { revision: settings.configRevision }) : '—' }} <span v-if="dirty">· {{ t('integration.unsaved') }}</span></span>
        <ZButton type="primary" :loading="saving" :disabled="!dirty || conflict || loading" @click="save">
          {{ t('claude.saveUrl') }}
        </ZButton>
      </div>
      <div v-if="conflict" class="mt-3 space-y-3">
        <p role="alert" class="text-z-warning-text">
          {{ t('integration.conflict') }}
        </p>
        <ZButton :loading="loading" @click="compare">
          {{ t('integration.compare') }}
        </ZButton>
        <div v-if="serverSettings" class="grid gap-3 sm:grid-cols-2">
          <div><p>{{ t('integration.localDraft') }}</p><pre class="whitespace-pre-wrap break-all">{{ publicUrl }}</pre></div>
          <div><p>{{ t('integration.serverVersion', { revision: serverSettings.configRevision }) }}</p><pre class="whitespace-pre-wrap break-all">{{ serverSettings.publicUrl }}</pre></div>
        </div>
      </div>
    </template>
  </ZCard>
  <ZConfirmDialog v-model="discardOpen" :title="t('integration.unsaved')" :description="t('integration.discardDraft')" @confirm="finishDiscard(true)" />
</template>
