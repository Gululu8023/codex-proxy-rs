<script setup lang="ts">
import type { UsageReasoningRecord } from './shared/presenter'

import { Bot, SquareDashedBottomCode } from '@lucide/vue'

import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { usageReasoningEffort, usageThinkingDescription } from './shared/presenter'

const props = defineProps<{
  record: UsageReasoningRecord & { route?: string | null }
}>()

const { t } = useI18n({ useScope: 'global' })
const thinkingDescription = computed(() => usageThinkingDescription(props.record))
</script>

<template>
  <span
    class="inline-flex items-center gap-1 whitespace-nowrap text-z-sm font-bold text-z-text"
  >
    <span
      v-if="record.route === '/v1/messages/count_tokens'"
      class="inline-flex h-6 shrink-0 items-center rounded-full bg-z-blue-container px-2 font-mono text-z-xs leading-none font-bold text-z-blue-on-container"
      tabindex="0"
      :title="t('usage.route.tokenCountRequest')"
      :aria-label="t('usage.route.tokenCountRequest')"
    >
      {{ t('usage.route.tokenCount') }}
    </span>
    <span v-else>{{ usageReasoningEffort(record) }}</span>
    <span
      v-if="thinkingDescription"
      class="inline-flex h-6 shrink-0 items-center rounded-full bg-z-blue-container px-2 font-mono text-z-xs leading-none font-bold text-z-blue-on-container"
      tabindex="0"
      :title="`thinking · ${thinkingDescription}`"
      :aria-label="`thinking · ${thinkingDescription}`"
    >
      thinking
    </span>
    <span
      v-if="record.subagentKind"
      class="inline-flex shrink-0 text-z-purple-text"
      :title="t('usage.reasoning.subagentRequest')"
      :aria-label="t('usage.reasoning.subagentRequest')"
    >
      <Bot class="size-3.5" stroke-width="2.2" />
    </span>
    <span
      v-if="record.subagentKind === 'review'"
      class="inline-flex shrink-0 text-z-purple-text"
      :title="t('usage.reasoning.reviewSubagentRequest')"
      :aria-label="t('usage.reasoning.reviewSubagentRequest')"
    >
      <SquareDashedBottomCode class="size-3.25" stroke-width="2.2" />
    </span>
  </span>
</template>
