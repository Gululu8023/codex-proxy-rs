/* eslint test/no-import-node-test: off -- 使用 Node 内置测试，不引入额外测试运行器。 */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { formatReasoning } from '../src/components/usage/shared/reasoning.ts'

const labels = {
  unspecified: 'Not specified',
  modes: {
    adaptive: 'Adaptive',
    enabled: 'Enabled',
    disabled: 'Disabled',
    between_tools: 'Between tools',
  },
  budget: (tokens: number) => `Budget ${tokens}`,
}

test('Claude effort remains visible alongside thinking mode and budget', () => {
  assert.equal(formatReasoning({
    reasoningEffort: 'high',
    reasoningPreset: 'adaptive',
    thinkingMode: 'adaptive',
    reasoningObserved: true,
  }, labels), 'high · Adaptive')
  assert.equal(formatReasoning({
    reasoningEffort: null,
    thinkingMode: 'enabled',
    thinkingBudgetTokens: 32000,
    reasoningObserved: true,
  }, labels), 'Enabled · Budget 32000')
  assert.equal(formatReasoning({ reasoningEffort: null, thinkingMode: 'disabled', reasoningObserved: true }, labels), 'Disabled')
})

test('missing historical observations are not presented as a known default', () => {
  assert.equal(formatReasoning({ reasoningEffort: null }, labels), '—')
  assert.equal(formatReasoning({ reasoningEffort: null, reasoningObserved: true }, labels), 'Not specified')
})

test('Codex presets and subagent display retain their meaning', () => {
  assert.equal(formatReasoning({ reasoningEffort: 'high', reasoningPreset: 'ultra' }, labels), 'ultra')
  assert.equal(formatReasoning({ reasoningEffort: 'high', reasoningPreset: 'ultra', subagentKind: 'review' }, labels), 'high')
})
