/* eslint test/no-import-node-test: off -- 使用 Node 内置测试，不引入额外测试运行器。 */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { formatReasoning, formatThinking } from '../src/components/usage/shared/reasoning.ts'

const labels = {
  modes: {
    adaptive: 'Adaptive',
    enabled: 'Enabled',
  },
  budget: (tokens: number) => `Budget ${tokens}`,
}

test('Claude effort is displayed independently of thinking mode and budget', () => {
  assert.equal(formatReasoning({
    reasoningEffort: 'high',
    reasoningPreset: 'adaptive',
    thinkingMode: 'adaptive',
    reasoningObserved: true,
  }), 'high')
  assert.equal(formatReasoning({
    reasoningEffort: 'medium',
    thinkingMode: 'enabled',
    thinkingBudgetTokens: 32000,
    reasoningObserved: true,
  }), 'medium')
})

test('thinking can be displayed without a reasoning effort', () => {
  for (const [mode, description] of [
    ['adaptive', 'Adaptive'],
    ['enabled', 'Enabled'],
  ] as const) {
    const record = {
      reasoningEffort: null,
      thinkingMode: mode,
      thinkingBudgetTokens: null,
      reasoningObserved: true,
    }
    assert.equal(formatReasoning(record), '—')
    assert.equal(formatThinking(record, labels), description)
  }
})

test('thinking budget is retained in the tooltip description', () => {
  const record = {
    reasoningEffort: null,
    thinkingMode: 'enabled',
    thinkingBudgetTokens: 8192,
    reasoningObserved: true,
  }
  assert.equal(formatReasoning(record), '—')
  assert.equal(formatThinking(record, labels), 'Enabled · Budget 8192')
})

test('disabled, between-tools and unspecified modes do not display thinking', () => {
  for (const mode of [
    'disabled',
    'between_tools',
    null,
    undefined,
  ]) {
    const record = {
      reasoningEffort: 'high',
      thinkingMode: mode,
      reasoningObserved: true,
    }
    assert.equal(formatReasoning(record), 'high')
    assert.equal(formatThinking(record, labels), null)
  }
})

test('a budget alone does not imply that thinking is enabled', () => {
  assert.equal(formatThinking({
    reasoningEffort: 'medium',
    thinkingBudgetTokens: 8192,
    reasoningObserved: true,
  }, labels), null)
})

test('missing historical observations are not presented as a known default', () => {
  assert.equal(formatReasoning({ reasoningEffort: null }), '—')
  assert.equal(formatReasoning({ reasoningEffort: null, reasoningObserved: true }), '—')
  for (const observed of [undefined, null, false]) {
    assert.equal(formatThinking({
      reasoningEffort: 'high',
      thinkingMode: 'adaptive',
      reasoningObserved: observed,
    }, labels), null)
  }
})

test('Codex presets and subagent display retain their meaning', () => {
  assert.equal(formatReasoning({ reasoningEffort: 'high', reasoningPreset: 'ultra' }), 'ultra')
  assert.equal(formatReasoning({ reasoningEffort: 'high', reasoningPreset: 'ultra', subagentKind: 'review' }), 'high')
})
