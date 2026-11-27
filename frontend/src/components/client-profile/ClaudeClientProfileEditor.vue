<script setup lang="ts">
import type { ClaudeProfileOptions, ClaudeProfilePreview, ClaudeProfileSelection } from '@/api/modules/settings/profiles'
import { ZFormItem, ZInput, ZSegmented, ZSelect, ZTextarea } from '@codex-proxy/ui'
import { computed, onMounted, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getClaudeProfileOptions, previewClaudeProfile } from '@/api/modules/settings/profiles'
import { errorMessage } from '@/utils/operation'

const props = withDefaults(defineProps<{ active?: boolean, disabled?: boolean, allowInherit?: boolean, allowPreserve?: boolean }>(), {
  active: true,
  disabled: false,
  allowInherit: false,
  allowPreserve: false,
})
const { t } = useI18n({ useScope: 'global' })
const model = defineModel<ClaudeProfileSelection | null | undefined>({ required: true })
const options = shallowRef<ClaudeProfileOptions>()
const preview = shallowRef<ClaudeProfilePreview>()
const failure = shallowRef('')
const loading = shallowRef(true)
const effective = computed(() => {
  const selection = model.value ?? options.value?.globalConfiguration
  if (!selection)
    return
  // 只回写公开配置字段，避免旧配置中的 SDK/runtime 覆盖继续随表单提交。
  return {
    mode: selection.mode,
    client: selection.client,
    platform: selection.platform,
    arch: selection.arch,
    versionMode: selection.versionMode,
    version: selection.version,
    userAgent: selection.userAgent,
    installationSeed: selection.installationSeed,
  }
})
const inherited = computed(() => props.allowInherit && model.value === null)
const locked = computed(() => props.disabled || loading.value || inherited.value)
const source = computed({
  get: () => model.value === undefined
    ? 'keep'
    : model.value === null
      ? 'global'
      : 'independent',
  set: (value: string) => {
    model.value = value === 'keep'
      ? undefined
      : value === 'global'
        ? null
        : { ...options.value!.globalConfiguration }
  },
})
const platforms = { macos: 'macOS', linux: 'Linux', windows: 'Windows' }
const presetOptions = computed(() => [
  ...(options.value?.presets ?? []).map(selection => ({
    value: `${selection.platform}-${selection.client}`,
    label: `${platforms[selection.platform]} · ${selection.client === 'cli'
      ? 'CLI'
      : 'VS Code'}`,
  })),
  {
    label: t('claude.clientProfile.custom'),
    value: 'custom',
  },
  {
    label: t('claude.clientProfile.follow'),
    value: 'follow',
  },
])
const selectedPreset = computed({
  get: () => effective.value?.mode === 'preset'
    ? `${effective.value.platform}-${effective.value.client}`
    : effective.value?.mode,
  set: (value: string | undefined) => {
    const selection = effective.value
    if (!selection)
      return
    if (value === 'follow') {
      model.value = {
        ...selection,
        mode: 'follow',
        versionMode: 'latest',
        version: null,
        userAgent: null,
      }
    }
    else if (value === 'custom') {
      const identity = preview.value?.requiresClientIdentity ? undefined : preview.value?.effective
      model.value = {
        ...selection,
        mode: 'custom',
        versionMode: 'fixed',
        version: identity?.version ?? null,
        userAgent: identity?.userAgent ?? '',
      }
    }
    else {
      const preset = options.value?.presets.find(item => `${item.platform}-${item.client}` === value)
      if (preset) {
        model.value = {
          ...preset,
          versionMode: selection.mode === 'preset'
            ? selection.versionMode
            : 'latest',
          version: selection.mode === 'preset' && selection.versionMode === 'fixed'
            ? selection.version
            : null,
        }
      }
    }
  },
})

function field<K extends keyof ClaudeProfileSelection>(key: K) {
  return computed({
    get: () => effective.value?.[key],
    set: (value: ClaudeProfileSelection[K] | undefined) => {
      if (!effective.value || value === undefined)
        return
      model.value = { ...effective.value, [key]: value }
    },
  })
}
const platform = field('platform')
const arch = field('arch')
const versionMode = computed({
  get: () => effective.value?.versionMode,
  set: (value: ClaudeProfileSelection['versionMode'] | undefined) => {
    if (!effective.value || !value)
      return
    model.value = {
      ...effective.value,
      versionMode: value,
      version: value === 'fixed'
        ? effective.value.version ?? preview.value?.effective.version ?? null
        : null,
    }
  },
})
const versionField = field('version')
const version = computed({
  get: () => versionField.value ?? '',
  set: (value: string) => { versionField.value = value },
})
const userAgent = computed({
  get: () => effective.value?.userAgent ?? '',
  set: (value: string) => {
    if (!effective.value)
      return
    // 自定义 UA 同时决定版本和入口，避免多个输入项生成互相矛盾的身份。
    const match = /^claude-cli\/(\d+\.\d+\.\d+) \(external, (cli|vscode)\)$/.exec(value)
    model.value = {
      ...effective.value,
      userAgent: value,
      version: match?.[1] ?? null,
      client: (match?.[2] as ClaudeProfileSelection['client'] | undefined) ?? effective.value.client,
    }
  },
})
const needsInput = computed(() => effective.value?.mode === 'custom'
  ? !effective.value.userAgent?.trim()
  : effective.value?.mode === 'preset' && effective.value.versionMode === 'fixed' && !effective.value.version?.trim())

onMounted(async () => {
  try {
    options.value = await getClaudeProfileOptions()
  }
  catch (error) { failure.value = errorMessage(error) }
  finally { loading.value = false }
})
watch([
  effective,
  () => props.active,
], ([configuration, active], _, onCleanup) => {
  if (!active || model.value === undefined)
    return
  let cancelled = false
  preview.value = undefined
  if (needsInput.value) {
    failure.value = ''
    return
  }
  const timer = setTimeout(async () => {
    try {
      const result = await previewClaudeProfile(model.value === null
        ? null
        : configuration!)
      if (!cancelled) {
        preview.value = result
        failure.value = ''
      }
    }
    catch (error) {
      if (!cancelled)
        failure.value = errorMessage(error)
    }
  }, 300)
  onCleanup(() => {
    cancelled = true
    clearTimeout(timer)
  })
}, { immediate: true })
</script>

<template>
  <div class="grid min-w-0 gap-4">
    <div v-if="allowInherit" class="flex flex-wrap items-center justify-between gap-3">
      <ZSegmented
        v-model="source" :disabled="disabled || loading || !options" :options="[
          ...(allowPreserve ? [{ label: t('accounts.modelAccess.reserve'), value: 'keep' }] : []),
          { label: t('settings.clientProfile.globalConfiguration'), value: 'global' },
          { label: t('settings.clientProfile.customConfiguration'), value: 'independent' },
        ]"
      />
      <slot name="source-extra" />
    </div>
    <div v-if="model !== undefined && effective && !inherited" class="grid gap-4 sm:grid-cols-2">
      <ZFormItem :label="t('settings.clientProfile.clientDefault')">
        <ZSelect v-model="selectedPreset" :disabled="locked" :options="presetOptions" />
      </ZFormItem>
      <template v-if="effective.mode !== 'follow'">
        <ZFormItem v-if="effective.mode === 'preset'" :label="t('settings.clientProfile.versionStrategy')">
          <ZSelect
            v-model="versionMode"
            :disabled="locked"
            :options="[
              { label: t('settings.clientProfile.followLatestVersion'), value: 'latest' },
              { label: t('claude.clientProfile.stable'), value: 'stable' },
              { label: t('claude.clientProfile.fixed'), value: 'fixed' },
            ]"
          />
        </ZFormItem>
        <ZFormItem v-else :label="t('claude.clientProfile.platform')">
          <ZSelect v-model="platform" :disabled="locked" :options="[{ label: 'macOS', value: 'macos' }, { label: 'Linux', value: 'linux' }, { label: 'Windows', value: 'windows' }]" />
        </ZFormItem>
        <ZFormItem :label="t('claude.clientProfile.arch')">
          <ZSelect v-model="arch" :disabled="locked" :options="[{ label: 'arm64', value: 'arm64' }, { label: 'x64', value: 'x64' }]" />
        </ZFormItem>
        <ZFormItem v-if="effective.mode === 'preset' && versionMode === 'fixed'" :label="t('claude.clientProfile.version')">
          <ZInput v-model="version" :disabled="locked" class="font-mono" />
        </ZFormItem>
        <ZFormItem v-if="effective.mode === 'custom'" label="User-Agent" class="sm:col-span-2">
          <ZTextarea v-model="userAgent" :disabled="locked" :rows="3" :spellcheck="false" class="font-mono" :placeholder="t('settings.clientProfile.userAgentPlaceholder')" />
        </ZFormItem>
      </template>
    </div>
    <p v-if="failure" class="text-sm text-z-error" role="alert">
      {{ failure }}
    </p>
    <p v-if="needsInput" class="m-0 text-z-sm text-z-text-tertiary">
      {{ t(effective?.mode === 'custom' ? 'settings.clientProfile.userAgentPlaceholder' : 'settings.clientProfile.previewAfterFillingInVersion') }}
    </p>
    <div v-if="preview" class="grid min-h-20 min-w-0 content-center gap-2 break-all rounded-z bg-z-fill-quaternary p-4" role="status">
      <p v-if="preview.requiresClientIdentity" class="m-0 text-z-sm text-z-text-tertiary">
        {{ t('claude.clientProfile.followHelp') }}
      </p>
      <template v-else>
        <pre class="m-0 whitespace-pre-wrap break-all font-mono text-z-sm text-z-text"><code>User-Agent: {{ preview.effective.userAgent }}</code></pre>
        <p class="m-0 text-z-xs text-z-text-tertiary">
          {{ t(effective?.mode === 'custom' || effective?.versionMode === 'fixed' ? 'settings.clientProfile.fixedIdentity' : 'settings.clientProfile.automaticUpdates') }}
          <template v-if="effective?.mode === 'preset' && effective.versionMode !== 'fixed'">
            · {{ preview.checkedAtDisplay ? t('settings.clientProfile.checked', { checkedAtDisplay: preview.checkedAtDisplay }) : t('settings.clientProfile.toBeChecked') }}
          </template>
        </p>
      </template>
      <p v-if="preview.error" class="m-0 text-z-sm text-z-warning" role="alert">
        {{ preview.error }}
      </p>
    </div>
  </div>
</template>
