<script setup lang="ts">
import type { Account } from '@/api'

import { ZButton, ZDialog, ZEmpty, ZSkeleton } from '@codex-proxy/ui'

import { RefreshCw } from '@lucide/vue'

import { computed, toRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { useAccountPersonalInfo } from '../../composables/useAccountPersonalInfo'
import AccountProfileActivityInsights from './ActivityInsights.vue'
import AccountProfileMetrics from './Metrics.vue'
import AccountProfileHero from './ProfileHero.vue'
import AccountProfileSkeleton from './Skeleton.vue'
import AccountSubscription from './Subscription.vue'
import AccountProfileTokenActivity from './TokenActivity.vue'

const props = defineProps<{ account: Account }>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ required: true })
const accountId = toRef(() => props.account.id)
const { basicProfile, profile, subscription, loading, error, load } = useAccountPersonalInfo({
  accountId,
  open,
  capabilities: toRef(() => props.account.capabilities),
})
const identityFields = computed(() => {
  if (!basicProfile.value)
    return []
  const fields: Array<[string, string | null]> = [
    [
      t('accounts.profile.basic.name'),
      basicProfile.value.name,
    ],

    [
      t('accounts.profile.basic.email'),
      basicProfile.value.email,
    ],

    [
      t('accounts.profile.basic.userId'),
      basicProfile.value.upstreamUserId,
    ],

    [
      props.account.provider === 'anthropic'
        ? t('accounts.profile.basic.organizationId')
        : t('accounts.profile.basic.accountId'),
      basicProfile.value.upstreamAccountId,
    ],

    [
      t('accounts.profile.basic.plan'),
      basicProfile.value.planType,
    ],
  ]
  return fields.filter(([, value]) => value)
})
</script>

<template>
  <ZDialog v-model="open" :title="t('accounts.profile.personalInformation')" width="64rem">
    <div class="flex min-h-0 min-w-0 flex-col gap-6 pb-2">
      <div class="grid min-w-0 grid-cols-1 gap-4 rounded-z-lg bg-z-fill-alter p-4 sm:p-5 lg:items-center" :class="account.capabilities.profile ? 'lg:grid-cols-[minmax(310px,1.05fr)_minmax(0,1.95fr)]' : undefined">
        <AccountProfileHero :account="account" :profile="profile" />
        <AccountProfileMetrics v-if="account.capabilities.profile" :profile="profile" :loading="loading" />
      </div>

      <section v-if="basicProfile" aria-labelledby="profile-basic-title">
        <h3 id="profile-basic-title" class="m-0 text-z font-heavy text-z-text">
          {{ t('accounts.profile.basic.title') }}
        </h3>
        <dl class="mb-0 grid min-w-0 grid-cols-1 gap-4 rounded-z-lg bg-z-fill-alter p-4 sm:grid-cols-2">
          <div v-for="[label, value] in identityFields" :key="label" class="min-w-0">
            <dt class="text-z-xs text-z-text-secondary">
              {{ label }}
            </dt>
            <dd class="mx-0 mt-1 break-all text-z-sm text-z-text">
              {{ value }}
            </dd>
          </div>
        </dl>
      </section>

      <section v-if="subscription" class="grid min-w-0 grid-cols-1 gap-4" aria-labelledby="profile-subscription-title">
        <h3 id="profile-subscription-title" class="m-0 text-z font-heavy text-z-text">
          {{ t('accounts.profile.subscriptionInformation') }}
        </h3>
        <AccountSubscription :subscription="subscription" />
      </section>

      <template v-if="loading && !profile">
        <AccountProfileSkeleton v-if="account.capabilities.profile" />
        <div v-else role="status" aria-busy="true">
          <span class="sr-only">{{ account.capabilities.subscription ? t('accounts.profile.loadingSubscription') : t('accounts.profile.basic.loading') }}</span>
          <ZSkeleton class="h-24 w-full rounded-z-lg" />
        </div>
      </template>
      <template v-else-if="profile">
        <template v-if="!profile.hasStatsError">
          <AccountProfileTokenActivity :calendar="profile.activityCalendar" />
          <AccountProfileActivityInsights :insights="profile.activityInsights" />
        </template>
      </template>
      <ZEmpty
        v-else-if="!loading && !error && !account.capabilities.profile && !basicProfile && !subscription"
        :title="t('accounts.profile.noSubscriptionInformationYet')"
        surface="none"
      />
      <p v-if="error && !account.capabilities.profile" role="alert" class="m-0 text-z-sm text-z-danger">
        {{ error }}
      </p>
    </div>
    <template #footer>
      <ZButton @click="open = false">
        {{ t('common.actions.close') }}
      </ZButton>
      <ZButton :aria-busy="loading" :aria-disabled="loading" @click="load">
        <template #icon>
          <RefreshCw class="size-4" :class="loading ? 'animate-spin motion-reduce:animate-none' : undefined" />
        </template>
        {{ t('accounts.profile.refreshInformation') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
