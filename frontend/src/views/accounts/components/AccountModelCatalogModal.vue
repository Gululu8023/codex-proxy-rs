<script setup lang="ts">
import type { Account, AccountModelsResponse } from '@/api'

import { defineTableColumns, ZButton, ZDialog, ZInput, ZScrollbar, ZTable } from '@codex-proxy/ui'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getAccountModels, refreshAccountModels } from '@/api'
import { useRequestState } from '@/composables/useRequestState'

const props = defineProps<{ account: Account | null }>()
const open = defineModel<boolean>({ required: true })
const { t } = useI18n({ useScope: 'global' })
const query = useRequestState()
const { loading, error } = query
const catalog = shallowRef<AccountModelsResponse | null>(null)
const search = ref('')
const models = computed(() => {
  const term = search.value.trim().toLowerCase()
  return catalog.value?.models.filter(model => `${model.id} ${model.label}`.toLowerCase().includes(term)) ?? []
})
const columns = computed(() => defineTableColumns<AccountModelsResponse['models'][number]>([
  {
    key: 'id',
    label: t('accounts.modelCatalog.modelId'),
    kind: 'mono',
    size: '3xl',
  },
  {
    key: 'label',
    label: t('accounts.modelCatalog.modelName'),
    kind: 'identity',
    size: '3xl',
  },
]))

async function load(refresh = false) {
  if (!open.value || !props.account)
    return
  const id = query.start()
  try {
    // 查看只读取账号目录，刷新需显式触发；此处不调用连接诊断或推理。
    const result = await (refresh ? refreshAccountModels : getAccountModels)(
      { accountId: props.account.id },
      { signal: query.signal, silent: true },
    )
    if (query.isCurrent(id))
      catalog.value = result
  }
  catch (cause) {
    query.fail(id, cause)
  }
  finally {
    query.finish(id)
  }
}

watch([
  open,
  () => props.account?.id,
], () => {
  query.invalidate()
  catalog.value = null
  search.value = ''
  error.value = ''
  if (open.value)
    void load()
}, { immediate: true })
</script>

<template>
  <ZDialog v-model="open" :title="t('accounts.modelCatalog.title')" :description="account?.name" width="48rem">
    <div class="grid min-w-0 gap-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <span class="text-z-sm text-z-text-secondary">{{ t('accounts.modelCatalog.observedAt') }}: {{ catalog?.observedAt ?? t('common.values.unknown') }}</span>
        <ZButton :loading="loading" @click="load(true)">
          {{ t('accounts.modelAccess.refreshModel') }}
        </ZButton>
      </div>
      <ZInput v-model="search" :aria-label="t('accounts.modelCatalog.search')" :placeholder="t('accounts.modelCatalog.search')" />
      <p v-if="error" role="alert" class="m-0 text-z-error-text">
        {{ error }}
      </p>
      <ZScrollbar max-height="24rem">
        <ZTable :columns="columns" :data="models" :loading="loading" :empty-text="error ? t('accounts.modelAccess.retryHint') : search.trim() ? t('accounts.modelAccess.noMatchingModels') : t('accounts.modelAccess.noModels')" />
      </ZScrollbar>
    </div>
    <template #footer>
      <RouterLink to="/settings/pricing" class="text-z-primary">
        {{ t('settings.page.tabs.pricing') }}
      </RouterLink>
      <ZButton type="primary" @click="open = false">
        {{ t('common.actions.close') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
