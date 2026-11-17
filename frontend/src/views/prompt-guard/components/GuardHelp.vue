<script setup lang="ts">
import { ZIconButton, ZPopover } from '@codex-proxy/ui'
import { CircleAlert } from '@lucide/vue'

// 与系统设置页一致：较长的说明收进图标气泡，避免说明文字压过字段本身。
defineProps<{ label: string, text: string }>()
</script>

<template>
  <ZPopover class="-my-1" trigger="hover-click" placement="top-start">
    <template #reference="{ open }">
      <ZIconButton variant="link" :aria-label="label" :aria-expanded="open" class="size-6!">
        <CircleAlert class="size-3.5" aria-hidden="true" />
      </ZIconButton>
    </template>
    <p class="max-w-72 px-3 py-2 text-z-sm leading-relaxed text-z-text-secondary">
      {{ text }}
    </p>
  </ZPopover>
</template>
