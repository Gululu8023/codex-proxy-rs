<script setup lang="ts">
import { ZPageHeader, ZSegmented, ZSelect } from '@codex-proxy/ui'
import { BookOpen, Fingerprint, GraduationCap, ListChecks, ScrollText, SlidersHorizontal } from '@lucide/vue'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { GUARD_ACTIONS_TARGET } from './actions'

const { t } = useI18n({ useScope: 'global' })
const route = useRoute()
const router = useRouter()
const sections = computed(() => ([
  ['overview', SlidersHorizontal],
  ['records', ScrollText],
  ['profiles', Fingerprint],
  ['policy', ListChecks],
  ['learning', GraduationCap],
  ['help', BookOpen],
] as const).map(([value, icon]) => ({
  value,
  label: t(`guard.tabs.${value}`),
  icon,
})))
const section = computed(() => String(route.name).replace('prompt-guard-', '').replace('prompt-guard', 'overview'))
function navigate(value?: string) {
  void router.push({ path: `/prompt-guard${value && value !== 'overview'
    ? `/${value}`
    : ''}`, query: route.query })
}
</script>

<template>
  <div class="w-full">
    <ZPageHeader :title="t('guard.title')" :description="t('guard.description')" />
    <div class="mt-4 flex min-h-z-control flex-wrap items-center justify-between gap-3">
      <ZSegmented class="hidden! sm:inline-grid!" :model-value="section" :options="sections" :aria-label="t('guard.title')" @update:model-value="navigate" />
      <ZSelect class="sm:hidden!" :model-value="section" :options="sections" :aria-label="t('guard.title')" @update:model-value="navigate" />
      <div :id="GUARD_ACTIONS_TARGET.slice(1)" class="ml-auto flex flex-wrap items-center justify-end gap-2" />
    </div>
    <RouterView class="mt-5" />
  </div>
</template>
