import type { UsageBilling } from '@/api'

interface BillingItem {
  label: string
  value: string
  accent?: boolean
  tone?: string
}

function object(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
}

function tokenPrice(ticks: unknown, multiplier = 1): string | null {
  if (typeof ticks !== 'number' || !Number.isSafeInteger(ticks) || ticks < 0)
    return null
  // 保存值是每 Token 的 USD ticks（1 USD = 10^10 ticks），展示为每百万 Token 的价格。
  return `$${Math.floor(ticks * multiplier) / 10_000} / 1M`
}

export function recordedBillingItems(billing: UsageBilling | null, t: (key: string) => string) {
  const saved = billing?.recorded
  if (!billing || saved?.kind !== 'api_equivalent_estimate')
    return null
  const costs = object(saved.costBreakdown)
  if (costs.currency !== 'USD')
    return null
  const pricing = object(saved.pricing)
  const override = object(pricing.override)
  const amount = (key: string) => typeof costs[key] === 'string'
    ? `$${costs[key]}`
    : '—'
  const price = (key: string, ticks: unknown) => typeof override[key] === 'string'
    ? `$${override[key]} / 1M`
    : tokenPrice(ticks) ?? '—'
  const amounts: BillingItem[] = [
    {
      label: t('usage.billing.inputCost'),
      value: amount('input'),
    },
    {
      label: t('usage.billing.outputFee'),
      value: amount('output'),
    },
    {
      label: t('usage.billing.inputUnitPrice'),
      value: price('input', pricing.inputTicksPerToken),
      accent: true,
    },
    {
      label: t('usage.billing.outputUnitPrice'),
      value: price('output', pricing.outputTicksPerToken),
      accent: true,
    },
    {
      label: t('usage.billing.cacheReadCost'),
      value: amount('cacheRead'),
    },
    {
      label: t('usage.request.cacheUnitPrice'),
      value: price('cacheRead', pricing.cacheReadTicksPerToken),
      accent: true,
    },
    {
      label: t('usage.billing.cacheWriteCost'),
      value: amount('cacheWrite'),
    },
  ]
  // 两种 TTL 有独立价格；混合写入不能伪装成一个统一单价。
  // 翻译键使用字面量，供 i18n 静态检查收集。
  for (const [key, suffix, multiplier, label] of [
    [
      'cacheWrite5m',
      '5m',
      pricing.cacheWrite5mMultiplier,
      t('usage.billing.cacheWrite5m'),
    ],
    [
      'cacheWrite1h',
      '1h',
      pricing.cacheWrite1hMultiplier,
      t('usage.billing.cacheWrite1h'),
    ],
  ] as const) {
    if (typeof costs[key] !== 'string')
      continue
    amounts.push({
      label,
      value: amount(key),
    })
    const rate = typeof multiplier === 'string' && Number.isFinite(Number(multiplier)) && Number(multiplier) >= 0
      ? tokenPrice(pricing.inputTicksPerToken, Number(multiplier))
      : null
    if (rate !== null) {
      amounts.push({
        label: `${t('usage.billing.cacheWriteUnitPrice')} · ${suffix}`,
        value: rate,
        accent: true,
      })
    }
  }
  if (typeof override.cacheWrite === 'string') {
    amounts.push({
      label: t('usage.billing.cacheWriteUnitPrice'),
      value: `$${override.cacheWrite} / 1M`,
      accent: true,
    })
  }
  // Claude 的容量档位与速度模式是独立字段；历史观测缺失时不从价格或请求意图反推。
  const serviceTier = pricing.serviceTier === 'standard'
    ? 'Standard'
    : pricing.serviceTier === 'priority'
      ? 'Priority'
      : pricing.serviceTier === 'batch'
        ? 'Batch'
        : '—'
  const fastMode = pricing.speed === 'fast'
  const summary: BillingItem[] = [
    {
      label: t('usage.billing.basis'),
      value: t('usage.billing.apiEquivalent'),
    },
    {
      label: t('usage.billing.serviceLevel'),
      value: serviceTier,
      tone: 'info',
    },
    {
      label: t('usage.billing.speedMode'),
      value: fastMode
        ? 'Fast'
        : pricing.speed === 'standard'
          ? 'Standard'
          : '—',
      tone: 'info',
    },
    {
      label: t('usage.billing.speedMultiplier'),
      value: typeof pricing.speedMultiplierBps === 'number' && Number.isSafeInteger(pricing.speedMultiplierBps)
        ? `${pricing.speedMultiplierBps / 10_000}x`
        : '—',
      tone: 'info',
    },
    {
      label: t('usage.billing.totalCost'),
      value: billing.totalAmountDisplay,
      tone: 'success',
    },
    {
      label: t('usage.billing.standardFee'),
      value: typeof costs.standard === 'string'
        ? `≈ ${amount('standard')}`
        : '—',
    },
  ]
  if (typeof pricing.multiplierBps === 'number') {
    summary.push({
      label: t('settings.pricing.multiplier'),
      value: `${pricing.multiplierBps / 10_000}x`,
      tone: 'info',
    })
  }
  return {
    amounts,
    summary,
    fastMode,
    longContextBillingApplied: pricing.band === 'long_standard' || pricing.band === 'long_fast',
  }
}
