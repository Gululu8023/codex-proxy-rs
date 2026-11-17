import { onScopeDispose, shallowRef, watch } from 'vue'

/** 审查页面共用确认框；关闭、卸载或新的确认请求都会取消之前的等待。 */
export function useGuardConfirm() {
  const confirmOpen = shallowRef(false)
  const confirmDescription = shallowRef('')
  let resolveConfirm: ((confirmed: boolean) => void) | undefined

  function askConfirm(description: string) {
    resolveConfirm?.(false)
    confirmDescription.value = description
    confirmOpen.value = true
    return new Promise<boolean>((resolve) => {
      resolveConfirm = resolve
    })
  }

  function finishConfirm(confirmed: boolean) {
    resolveConfirm?.(confirmed)
    resolveConfirm = undefined
    confirmOpen.value = false
  }

  // 遮罩、Esc 和关闭按钮也必须结束等待，避免路由导航挂起。
  watch(confirmOpen, (open) => {
    if (!open)
      finishConfirm(false)
  })
  onScopeDispose(() => finishConfirm(false))

  return {
    confirmOpen,
    confirmDescription,
    askConfirm,
    finishConfirm,
  }
}
