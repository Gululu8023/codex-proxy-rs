import type { GuardPolicy } from '@/api/modules/claude'
import { computed, onMounted, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { getGuardPolicy } from '@/api/modules/claude'
import { useRequestState } from '@/composables/useRequestState'

/** 规则类别显示名：已知类别本地化，服务端新增而前端未收录的类别回退为原始标识。 */
export function useRuleCategoryLabel() {
  const { t, te } = useI18n({ useScope: 'global' })
  return (category: string) => te(`guard.ruleCategory.${category}`)
    ? t(`guard.ruleCategory.${category}`)
    : category
}

/**
 * 从策略目录加载服务端认可的类别，避免前端另抄一份名单。
 * `options` 覆盖全部类别（含内置隐私类别），用于筛选；
 * `customRuleOptions` 只含自定义规则可用的执行类与观察类——privacy 仅供内置规则按名称启用，服务端会拒绝。
 */
export function useGuardCategoryCatalog() {
  const { t } = useI18n({ useScope: 'global' })
  const label = useRuleCategoryLabel()
  const request = useRequestState()
  const catalog = shallowRef<GuardPolicy | null>(null)
  const observing = computed(() => (catalog.value?.observingCategories ?? []).map(value => ({
    value,
    label: label(value),
    description: `${value} · ${t('guard.observationOnly')}`,
  })))
  const executing = computed(() => (catalog.value?.executingCategories ?? []).map(value => ({
    value,
    label: label(value),
    description: value,
  })))
  async function load() {
    const id = request.start()
    try {
      const value = await getGuardPolicy({ signal: request.signal })
      if (request.isCurrent(id))
        catalog.value = value
    }
    catch (cause) { request.fail(id, cause) }
    finally { request.finish(id) }
  }
  onMounted(load)
  return {
    error: request.error,
    options: computed(() => [
      ...executing.value,
      ...observing.value,
    ]),
    customRuleOptions: computed(() => [
      ...executing.value.filter(option => option.value !== 'privacy'),
      ...observing.value,
    ]),
  }
}
