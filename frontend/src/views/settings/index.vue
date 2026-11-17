<script setup lang="ts">
import type { SettingsProvider } from './utils/scope'
import { ZButton, ZConfirmDialog, ZForm, ZIconButton, ZPageHeader, ZSegmented } from '@codex-proxy/ui'

import { Save, Undo2 } from '@lucide/vue'

import { computed, nextTick, reactive, shallowRef, useTemplateRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import AccountAffinityCard from './components/AccountAffinityCard.vue'
import AccountWarmupCard from './components/AccountWarmupCard.vue'
import SettingsBackupSection from './components/backup/SettingsBackupSection.vue'
import ClientVersionSettings from './components/client-version/index.vue'
import ClientProfileCard from './components/ClientProfileCard.vue'
import CodexReviewConcurrencyCard from './components/CodexReviewConcurrencyCard.vue'
import ConcurrencyPolicyCard from './components/ConcurrencyPolicyCard.vue'
import ModelAliasesCard from './components/ModelAliasesCard.vue'
import PrivacyPolicyCard from './components/privacy/PrivacyPolicyCard.vue'
import RequestHistoryCard from './components/RequestHistoryCard.vue'
import RequestLocationCard from './components/RequestLocationCard.vue'
import ResetCreditCard from './components/ResetCreditCard.vue'
import ResponseBodyLimitCard from './components/ResponseBodyLimitCard.vue'
import RotationStrategyCard from './components/RotationStrategyCard.vue'
import SettingsAccessSection from './components/SettingsAccessSection.vue'
import SettingsCardEditor from './components/SettingsCardEditor.vue'
import TokenRefreshCard from './components/TokenRefreshCard.vue'
import { useSettingsForm } from './composables/useSettingsForm'
import { useSettingsFormRules } from './composables/useSettingsFormRules'
import { useSettingsLeaveGuard } from './composables/useSettingsLeaveGuard'
import { rotationOptions } from './constants'
import PricingSection from './pricing/index.vue'
import { settingsFieldScope } from './utils/scope'

const { t } = useI18n({ useScope: 'global' })

const route = useRoute()
const router = useRouter()
const sectionOptions = computed(() => ([
  {
    label: t('settings.page.tabs.runtime'),
    value: 'runtime',
  },
  {
    label: t('settings.page.tabs.upstream'),
    value: 'upstream',
  },
  {
    label: t('settings.page.tabs.pricing'),
    value: 'pricing',
  },
  {
    label: t('settings.page.tabs.access'),
    value: 'access',
  },
  {
    label: t('settings.page.tabs.backups'),
    value: 'backup',
  },
]))
const section = computed(() => {
  switch (route.name) {
    case 'settings-upstream': return 'upstream'
    case 'settings-access': return 'access'
    case 'settings-pricing': return 'pricing'
    case 'settings-backup': return 'backup'
    default: return 'runtime'
  }
})
const isBasicSection = computed(() => section.value !== 'pricing' && section.value !== 'backup')
const visited = reactive(new Set<string>())
const settingsVisited = shallowRef(false)
const providers = reactive<Record<'profile' | 'models' | 'affinity' | 'privacy' | 'versions', SettingsProvider>>({
  profile: 'openai',
  models: 'openai',
  affinity: 'openai',
  privacy: 'openai',
  versions: 'openai',
})
// 旧深链仅预选卡片中的供应商，不再创建第二层设置分区。
watch(() => [route.query.scope, route.query.privacy, route.hash], () => {
  const selected = route.query.scope
  if (selected === 'openai' || selected === 'anthropic') {
    for (const key of Object.keys(providers) as Array<keyof typeof providers>)
      providers[key] = selected
  }
  if (route.hash === '#privacy-policy') {
    providers.privacy = route.query.privacy === 'anthropic'
      ? 'anthropic'
      : 'openai'
  }
}, { immediate: true })

function switchSection(value = 'runtime') {
  return router.push({
    path: value === 'runtime'
      ? '/settings'
      : `/settings/${value}`,
  })
}

const state = useSettingsForm()
const {
  loading,
  saving,
  detectingLocation,
  detectLocation,
  hasChanges,
  resetSettings,
  error,
  form,
  smartSchedulingDefaults,
  quotaResetWindowWeightsDefaults,
  mappings,
  anthropicMappings,
  addMapping,
  updateMapping,
  removeMapping,
  refreshMarginSecondsValue,
  refreshConcurrencyValue,
  maxConcurrentPerAccountValue,
  openaiGuardianReservedConcurrencyValue,
  maxAccountRotationsValue,
  openaiSessionAffinityTtlHoursValue,
  anthropicSessionAffinityTtlHoursValue,
  maxWaitingPerKeyValue,
  maxWaitingPerAccountValue,
  keyQueueTimeoutSecondsValue,
  accountQueueTimeoutSecondsValue,
  maxRetryAfterSecondsValue,
  responsesMaxDecompressedBodyMiBValue,
  usageRetentionDaysValue,
  resetCreditLeadMinutesValue,
  resetCreditMaxWeeklyRemainingPercentValue,
  saveSettings,
  loadSettings,
} = state
const disabled = computed(() => saving.value || loading.value || detectingLocation.value || !!error.value)
const formRef = useTemplateRef<InstanceType<typeof ZForm>>('formElement')
const formModel = computed(() => ({ ...form, modelMappings: mappings.value, anthropicModelMappings: anthropicMappings.value }))
const rules = useSettingsFormRules(() => form)
const affinity = computed({
  get: () => providers.affinity === 'anthropic'
    ? form.anthropicAccountAffinity
    : form.openaiAccountAffinity,
  set: (value) => {
    if (providers.affinity === 'anthropic')
      form.anthropicAccountAffinity = value
    else form.openaiAccountAffinity = value
  },
})
const affinityTtl = computed({
  get: () => providers.affinity === 'anthropic'
    ? anthropicSessionAffinityTtlHoursValue.value
    : openaiSessionAffinityTtlHoursValue.value,
  set: (value) => {
    if (providers.affinity === 'anthropic')
      anthropicSessionAffinityTtlHoursValue.value = value
    else openaiSessionAffinityTtlHoursValue.value = value
  },
})

async function requestSave() {
  if (disabled.value || !hasChanges.value)
    return
  await formRef.value?.validate(async (valid, errors) => {
    if (valid) {
      await saveSettings()
      return
    }
    const path = Object.keys(errors ?? {})[0]
    if (!path)
      return
    const target = formRef.value?.getField(path)?.$el?.closest('[data-settings-section]')?.getAttribute('data-settings-section')
    if (target)
      await switchSection(target)
    // 统一校验包含隐藏的供应商字段，先显示对应表单再定位错误。
    const scope = settingsFieldScope(path)
    for (const card of Object.keys(providers) as Array<keyof typeof providers>) {
      if (scope === `openai-${card}`)
        providers[card] = 'openai'
      else if (scope === `anthropic-${card}`)
        providers[card] = 'anthropic'
    }
    await nextTick()
    const field = formRef.value?.getField(path)
    field?.scrollIntoView({ block: 'center' })
    field?.focus()
  })
}

function resetForm() {
  resetSettings()
  formRef.value?.clearValidate()
}

const backupSection = useTemplateRef<InstanceType<typeof SettingsBackupSection>>('backupSection')
const hasUnsavedChanges = computed(() => hasChanges.value || !!backupSection.value?.hasChanges)
const { showLeaveConfirm, confirmLeave } = useSettingsLeaveGuard(hasUnsavedChanges)

watch(() => loading.value ? '' : route.hash, async (hash) => {
  if (['#reset-credits', '#privacy-policy'].includes(hash)) {
    await nextTick()
    document.getElementById(hash.slice(1))?.scrollIntoView({ block: 'start' })
  }
}, { immediate: true })

watch(section, (value) => {
  visited.add(value)
  if (isBasicSection.value && !settingsVisited.value) {
    settingsVisited.value = true
    void loadSettings()
  }
}, { immediate: true })
</script>

<template>
  <div class="w-full" :class="section === 'pricing' ? 'flex h-[calc(100dvh-2rem)] flex-none! flex-col min-[961px]:h-[calc(100dvh-3rem)]' : undefined">
    <ZPageHeader :title="t('navigation.settings')" :description="t('settings.page.description')" />

    <div class="mt-4 flex min-h-z-control shrink-0 flex-wrap items-center justify-between gap-3">
      <ZSegmented
        :model-value="section"
        :aria-label="t('settings.page.sectionsLabel')"
        class="w-full sm:w-auto"
        :options="sectionOptions"
        @update:model-value="switchSection"
      />
      <div v-if="isBasicSection || hasChanges" class="ml-auto flex items-center justify-end gap-2">
        <span v-if="hasChanges" class="mr-1 size-1.5 shrink-0 rounded-full bg-z-warning" aria-hidden="true" />
        <ZIconButton v-if="hasChanges" :aria-label="t('settings.page.undoAllBasicSettingsChanges')" variant="solid" :disabled="saving || loading || detectingLocation" @click="resetForm">
          <Undo2 class="size-4" />
        </ZIconButton>
        <ZButton type="primary" :loading="saving" :disabled="disabled || !hasChanges" @click="requestSave">
          <template #icon>
            <Save class="size-4" />
          </template>
          {{ saving ? t('settings.backup.schedule.saving') : t('settings.page.saveChanges') }}
        </ZButton>
      </div>
    </div>

    <div v-if="isBasicSection && error" role="alert" class="mt-5 flex items-center justify-between gap-3 text-z-error">
      <span>{{ error }}</span><ZButton :disabled="saving || loading" @click="loadSettings()">
        {{ t('settings.cards.reload') }}
      </ZButton>
    </div>
    <ZForm v-if="settingsVisited" v-show="isBasicSection" ref="formElement" :model="formModel" :rules="rules" :validate-on-rule-change="false" novalidate class="mt-5 grid min-w-0 max-w-6xl gap-5" @submit="requestSave">
      <div v-if="visited.has('runtime')" v-show="section === 'runtime'" data-settings-section="runtime" class="grid min-w-0 gap-5">
        <SettingsCardEditor
          v-model="form.rotationStrategy" v-model:smart-scheduling="form.smartScheduling" v-model:quota-reset-window-weights="form.quotaResetWindowWeights"
          :component="RotationStrategyCard" :state="state"
          scope="scheduling"
          :smart-defaults="smartSchedulingDefaults" :quota-reset-defaults="quotaResetWindowWeightsDefaults"
          :disabled="disabled" :options="rotationOptions"
        />
        <SettingsCardEditor
          v-model:max-concurrent-per-account="maxConcurrentPerAccountValue" v-model:max-waiting-per-key="maxWaitingPerKeyValue" v-model:max-waiting-per-account="maxWaitingPerAccountValue"
          v-model:key-queue-timeout-seconds="keyQueueTimeoutSecondsValue"
          v-model:account-queue-timeout-seconds="accountQueueTimeoutSecondsValue" v-model:switch-account-on-queue-failure="form.switchAccountOnQueueFailure"
          v-model:max-retry-after-seconds="maxRetryAfterSecondsValue"
          v-model:max-account-rotations="maxAccountRotationsValue" :component="ConcurrencyPolicyCard"
          :state="state" scope="concurrency"
        />
        <SettingsCardEditor
          v-model:provider="providers.affinity" v-model="affinity" v-model:ttl-hours="affinityTtl" :component="AccountAffinityCard"
          :state="state" provider-card="affinity" :disabled="disabled"
        />
        <div class="grid min-w-0 gap-5 lg:grid-cols-2">
          <SettingsCardEditor v-model="usageRetentionDaysValue" :component="RequestHistoryCard" :state="state" scope="history" />
          <SettingsCardEditor v-model="openaiGuardianReservedConcurrencyValue" :component="CodexReviewConcurrencyCard" :state="state" scope="review" codex-only />
        </div>
        <SettingsCardEditor
          v-model:enabled="form.accountWarmupEnabled" v-model:cron="form.accountWarmupCron" v-model:model="form.accountWarmupModel" :component="AccountWarmupCard"
          :state="state" scope="warmup" codex-only
        />
        <SettingsCardEditor
          v-model:on-low-weekly-quota="form.resetCreditOnLowWeeklyQuota" v-model="resetCreditLeadMinutesValue" v-model:max-weekly-remaining-percent="resetCreditMaxWeeklyRemainingPercentValue" :component="ResetCreditCard"
          :state="state"
          scope="reset" codex-only
        />
      </div>
      <div v-if="visited.has('upstream')" v-show="section === 'upstream'" data-settings-section="upstream" class="grid min-w-0 gap-5">
        <SettingsCardEditor
          v-model:refresh-margin-seconds="refreshMarginSecondsValue" v-model:refresh-concurrency="refreshConcurrencyValue" :component="TokenRefreshCard"
          :state="state" scope="refresh"
        />
        <SettingsCardEditor
          v-model:provider="providers.profile" v-model="form.providerRequestProfiles" :component="ClientProfileCard" :state="state"
          provider-card="profile" :active="section === 'upstream'" :disabled="disabled"
        />
        <SettingsCardEditor
          v-model="form.requestLocation" v-model:enabled="form.requestLocationEnabled"
          :component="RequestLocationCard" :state="state" scope="location" :disabled="disabled" :detecting-location="detectingLocation"
          @detect-location="detectLocation"
        />
        <SettingsCardEditor
          v-model:provider="providers.privacy" v-model="form.codexPrivacyPolicy" :component="PrivacyPolicyCard" :state="state"
          provider-card="privacy" :disabled="disabled"
        />
        <SettingsCardEditor
          v-model:provider="providers.models" :component="ModelAliasesCard" :state="state" provider-card="models"
          :mappings="mappings" :anthropic-mappings="anthropicMappings" :loading="loading"
          @add-mapping="addMapping"
          @update-mapping="updateMapping"
          @remove-mapping="removeMapping"
        />
      </div>
      <div v-if="visited.has('access')" v-show="section === 'access'" data-settings-section="access" class="grid min-w-0 gap-5">
        <SettingsAccessSection />
        <SettingsCardEditor
          v-model:provider="providers.versions" v-model:min-codex-desktop-version="form.minCodexDesktopVersion" v-model:min-codex-cli-version="form.minCodexCliVersion" v-model:min-claude-code-version="form.minClaudeCodeVersion"
          :component="ClientVersionSettings" :state="state"
          provider-card="versions" :loading="loading"
        />
        <SettingsCardEditor v-model="responsesMaxDecompressedBodyMiBValue" :component="ResponseBodyLimitCard" :state="state" scope="body-limit" />
      </div>
    </ZForm>

    <SettingsBackupSection v-if="visited.has('backup')" v-show="section === 'backup'" ref="backupSection" class="mt-5" :active="section === 'backup'" />
    <PricingSection v-if="visited.has('pricing')" v-show="section === 'pricing'" class="mt-5 min-h-0 flex-1" />
    <ZConfirmDialog
      v-model="showLeaveConfirm"
      :title="t('settings.page.unsavedChanges.title')"
      :description="t('settings.page.unsavedChanges.description')"
      :confirm-text="t('settings.page.unsavedChanges.leave')"
      :cancel-text="t('settings.page.unsavedChanges.stay')"
      @confirm="confirmLeave()"
    />
  </div>
</template>
