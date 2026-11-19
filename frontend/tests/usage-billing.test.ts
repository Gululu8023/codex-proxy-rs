/* eslint test/no-import-node-test: off -- 使用 Node 内置测试，不引入额外测试运行器。 */
import type { UsageBilling } from '../src/api/modules/usage.ts'
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { recordedBillingItems } from '../src/components/usage/shared/billing.ts'

const t = (key: string) => key

function billing(): UsageBilling {
  return {
    longContextBillingApplied: false,
    inputAmountDisplay: '—',
    outputAmountDisplay: '—',
    cacheReadAmountDisplay: '—',
    cacheWriteAmountDisplay: '—',
    standardAmountDisplay: '—',
    totalAmountDisplay: '$0.0007665',
    inputPriceDisplay: '—',
    outputPriceDisplay: '—',
    cacheReadPriceDisplay: '—',
    cacheWritePriceDisplay: '—',
    serviceTierDisplay: '—',
    multiplierDisplay: '—',
    recorded: {
      kind: 'api_equivalent_estimate',
      costBreakdown: {
        currency: 'USD',
        input: '0.0003',
        output: '0.0003',
        cacheRead: '0.000009',
        cacheWrite: '0.0001575',
        cacheWrite5m: '0.0000375',
        cacheWrite1h: '0.00012',
        total: '0.0007665',
      },
      pricing: {
        inputTicksPerToken: 30000,
        outputTicksPerToken: 150000,
        cacheReadTicksPerToken: 3000,
        cacheWrite5mMultiplier: '1.25',
        cacheWrite1hMultiplier: '2',
        multiplierBps: 10000,
        band: 'standard',
      },
    },
  }
}

test('recorded billing displays saved amounts and both cache TTL prices', () => {
  const items = recordedBillingItems(billing(), t)!
  const values = new Map(items.amounts.map(item => [item.label, item.value]))
  assert.equal(values.get('usage.billing.inputCost'), '$0.0003')
  assert.equal(values.get('usage.billing.inputUnitPrice'), '$3 / 1M')
  assert.equal(values.get('usage.billing.outputUnitPrice'), '$15 / 1M')
  assert.equal(values.get('usage.request.cacheUnitPrice'), '$0.3 / 1M')
  assert.equal(values.get('usage.billing.cacheWriteCost'), '$0.0001575')
  assert.equal(values.get('usage.billing.cacheWriteUnitPrice · 5m'), '$3.75 / 1M')
  assert.equal(values.get('usage.billing.cacheWriteUnitPrice · 1h'), '$6 / 1M')
  assert.equal(items.summary.find(item => item.label === 'usage.billing.totalCost')?.value, '$0.0007665')
  assert.equal(items.summary[0]?.value, 'usage.billing.apiEquivalent')
})

test('override pricing preserves explicit zero and does not invent TTL rates', () => {
  const value = billing()
  value.recorded = {
    ...value.recorded,
    costBreakdown: {
      currency: 'USD',
      input: '0',
      output: '0',
      cacheRead: '0',
      cacheWrite: '0',
      total: '0',
    },
    pricing: {
      override: {
        input: '0',
        output: '2',
        cacheRead: '0',
        cacheWrite: '4',
      },
      multiplierBps: 0,
    },
  }
  value.totalAmountDisplay = '$0'
  const items = recordedBillingItems(value, t)!
  assert.equal(items.amounts.find(item => item.label === 'usage.billing.inputUnitPrice')?.value, '$0 / 1M')
  assert.equal(items.amounts.find(item => item.label === 'usage.billing.cacheWriteUnitPrice')?.value, '$4 / 1M')
  assert.equal(items.summary.find(item => item.label === 'settings.pricing.multiplier')?.value, '0x')
  assert.equal(items.amounts.some(item => item.label.endsWith(' · 5m') || item.label.endsWith(' · 1h')), false)
})

test('missing prices and amounts remain unknown', () => {
  const value = billing()
  value.recorded = {
    kind: 'api_equivalent_estimate',
    costBreakdown: { currency: 'USD' },
  }
  const items = recordedBillingItems(value, t)!
  assert.ok(items.amounts.every(item => item.value === '—'))
  assert.equal(items.summary.find(item => item.label === 'usage.billing.totalCost')?.value, value.totalAmountDisplay)
})

test('historical amounts are not recalculated from a multiplier or long context price', () => {
  const value = billing()
  value.recorded = {
    ...value.recorded,
    pricing: { inputTicksPerToken: 60000, multiplierBps: 5000, band: 'long_standard' },
  }
  const items = recordedBillingItems(value, t)!
  assert.equal(items.longContextBillingApplied, true)
  assert.equal(items.summary.find(item => item.label === 'settings.pricing.multiplier')?.value, '0.5x')
  assert.equal(items.summary.find(item => item.label === 'usage.billing.totalCost')?.value, value.totalAmountDisplay)
  assert.equal(items.amounts.find(item => item.label === 'usage.billing.inputCost')?.value, '$0.0003')
})

test('ordinary billing keeps the existing presenter', () => {
  assert.equal(recordedBillingItems(null, t), null)
  const value = billing()
  delete value.recorded
  assert.equal(recordedBillingItems(value, t), null)
  value.recorded = {
    kind: 'other',
    costBreakdown: { currency: 'USD' },
  }
  assert.equal(recordedBillingItems(value, t), null)
})
