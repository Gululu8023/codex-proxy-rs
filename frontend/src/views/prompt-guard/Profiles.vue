<script setup lang="ts">
import type { GuardLocks, GuardProfiles } from '@/api/modules/claude'
import { defineTableColumns, ZButton, ZCard, ZConfirmDialog, ZPagination, ZTable } from '@codex-proxy/ui'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getGuardLocks, getGuardProfiles, releaseGuardLock } from '@/api/modules/claude'
import ProviderFilter from '@/components/ProviderFilter.vue'
import { useRequestState } from '@/composables/useRequestState'
import { GUARD_ACTIONS_TARGET } from './actions'
import { useGuardConfirm } from './useGuardConfirm'

const { t } = useI18n({ useScope: 'global' })
const { confirmOpen, confirmDescription, askConfirm, finishConfirm } = useGuardConfirm()
const request = useRequestState()
const locksRequest = useRequestState()
const mutation = useRequestState()
const { loading, error } = request
const { loading: locksLoading, error: locksError } = locksRequest
const { loading: saving, error: mutationError } = mutation
const data = shallowRef<GuardProfiles | null>(null)
const locks = shallowRef<GuardLocks | null>(null)
const provider = ref('')
const page = ref(1)
const pageSize = ref(25)
const lockPage = ref(1)
const lockPageSize = ref(25)
const columns = computed(() => defineTableColumns<GuardProfiles['items'][number]>([
  {
    key: 'key',
    label: t('guard.key'),
    kind: 'mono',
    size: '3xl',
  },
  {
    key: 'protocol',
    label: t('common.providers.filter'),
    kind: 'custom',
    size: 'lg',
  },
  ...([
    'requests',
    'blocked',
    'warned',
    'upstream_rejections',
    'maximum_score',
  ] as const).map(key => ({
    key,
    label: t(`guard.flow.profile.${key}`),
    kind: 'numeric' as const,
    size: 'md' as const,
  })),
]))
const lockColumns = computed(() => defineTableColumns<GuardLocks['items'][number]>([
  {
    key: 'client_key_id',
    label: t('guard.key'),
    kind: 'mono',
    size: '3xl',
  },
  {
    key: 'protocol',
    label: t('common.providers.filter'),
    kind: 'custom',
    size: 'lg',
  },
  {
    key: 'expires_at',
    label: t('guard.flow.expiresAt'),
    kind: 'custom',
    size: '2xl',
  },
  {
    key: 'request_id',
    label: t('guard.requestId'),
    kind: 'custom',
    size: '3xl',
  },
  {
    key: 'actions',
    label: t('guard.flow.release'),
    kind: 'actions',
    size: 'md',
  },
]))
async function load() {
  const id = request.start()
  data.value = null
  try {
    const value = await getGuardProfiles({ page: page.value, pageSize: pageSize.value, protocol: provider.value || undefined }, { signal: request.signal })
    if (!request.isCurrent(id))
      return
    const last = Math.max(1, Math.ceil(value.total / pageSize.value))
    if (page.value > last) {
      page.value = last
      return
    }
    data.value = value
  }
  catch (cause) { request.fail(id, cause) }
  finally { request.finish(id) }
}
async function loadLocks() {
  const id = locksRequest.start()
  locks.value = null
  try {
    const value = await getGuardLocks({ page: lockPage.value, pageSize: lockPageSize.value, protocol: provider.value || undefined }, { signal: locksRequest.signal })
    if (!locksRequest.isCurrent(id))
      return
    const last = Math.max(1, Math.ceil(value.total / lockPageSize.value))
    if (lockPage.value > last) {
      lockPage.value = last
      return
    }
    locks.value = value
  }
  catch (cause) { locksRequest.fail(id, cause) }
  finally { locksRequest.finish(id) }
}
function refresh() {
  void load()
  void loadLocks()
}
async function release(lock: GuardLocks['items'][number]) {
  if (saving.value)
    return
  if (!await askConfirm(t('guard.flow.releaseConfirm')) || saving.value)
    return
  const id = mutation.start()
  try {
    await releaseGuardLock({ id: lock.id, revision: lock.revision })
    await loadLocks()
  }
  catch (cause) { mutation.fail(id, cause) }
  finally { mutation.finish(id) }
}
watch([provider, pageSize], () => {
  if (page.value === 1)
    void load()
  else page.value = 1
})
watch(page, load, { immediate: true })
// 两张表共用供应商筛选，但页码、请求状态和刷新互相独立。
watch([provider, lockPageSize], () => {
  if (lockPage.value === 1)
    void loadLocks()
  else lockPage.value = 1
})
watch(lockPage, loadLocks, { immediate: true })
</script>

<template>
  <div class="grid min-w-0 gap-5">
    <Teleport defer :to="GUARD_ACTIONS_TARGET">
      <ZButton :loading="loading || locksLoading" @click="refresh">
        {{ t('common.actions.refresh') }}
      </ZButton>
    </Teleport>
    <div class="flex min-w-0 flex-wrap items-center gap-3">
      <span class="text-z-sm text-z-text-secondary">{{ t('common.providers.filter') }}</span><ProviderFilter v-model="provider" :disabled="saving" />
    </div>
    <p v-if="error || mutationError" role="alert" class="text-z-error-text">
      {{ error || mutationError }}
    </p>
    <ZCard :title="t('guard.tabs.profiles')" :description="t('guard.flow.profileHelp')">
      <ZTable :key="provider" :columns="columns" :data="data?.items ?? []" :loading="loading" :empty-text="error || t('integration.empty')">
        <template #protocol="{ row }">
          {{ row.protocol === 'anthropic' ? 'Claude' : 'Codex' }}
        </template>
      </ZTable>
      <ZPagination v-if="data" :current-page="page" :page-size="pageSize" :total="data.total" :page-sizes="[25, 50, 100]" :disabled="loading" @current-change="page = $event" @size-change="pageSize = $event" />
    </ZCard>
    <ZCard :title="t('guard.flow.locks')" :description="t('guard.flow.locksHelp')">
      <p v-if="locksError" role="alert" class="mb-3 text-z-error-text">
        {{ locksError }}
      </p>
      <ZTable :key="provider" :columns="lockColumns" :data="locks?.items ?? []" :loading="locksLoading" :empty-text="locksError || t('integration.empty')">
        <template #protocol="{ row }">
          {{ row.protocol === 'anthropic' ? 'Claude' : 'Codex' }}
        </template>
        <template #expires_at="{ row }">
          {{ new Date(row.expires_at).toLocaleString() }}
        </template>
        <template #request_id="{ row }">
          <RouterLink :to="{ path: '/prompt-guard/records', query: { requestId: row.request_id } }" class="text-z-primary">
            {{ row.request_id }}
          </RouterLink>
        </template>
        <template #actions="{ row }">
          <ZButton size="small" :disabled="saving" @click="release(row)">
            {{ t('guard.flow.release') }}
          </ZButton>
        </template>
      </ZTable>
      <ZPagination v-if="locks" :current-page="lockPage" :page-size="lockPageSize" :total="locks.total" :page-sizes="[25, 50, 100]" :disabled="locksLoading || saving" @current-change="lockPage = $event" @size-change="lockPageSize = $event" />
    </ZCard>
    <ZConfirmDialog v-model="confirmOpen" :title="t('common.actions.confirm')" :description="confirmDescription" @confirm="finishConfirm(true)" />
  </div>
</template>
