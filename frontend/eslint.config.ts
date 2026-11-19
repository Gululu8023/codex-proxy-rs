// @env node
import type { ESLint, Rule } from 'eslint'
import antfu from '@antfu/eslint-config'
import vueI18n from '@intlify/eslint-plugin-vue-i18n'

type LocatedRuleNode = Rule.Node & {
  loc: NonNullable<Rule.Node['loc']>
}

interface TemplateExpressionContainer {
  type: 'VExpressionContainer'
  parent: { type: string }
  expression: LocatedRuleNode
  loc: NonNullable<Rule.Node['loc']>
  range: NonNullable<Rule.Node['range']>
}

const singleLineSimpleInterpolationRule = {
  meta: {
    type: 'layout',
    fixable: 'whitespace',
    schema: [],
    messages: {
      expectedSingleLine: 'Expected a simple mustache interpolation to stay on one line.',
    },
  },
  create(context) {
    const { parserServices } = context.sourceCode
    if (!parserServices.defineTemplateBodyVisitor)
      return {}

    return parserServices.defineTemplateBodyVisitor({
      'VExpressionContainer[expression!=null]': function (node: TemplateExpressionContainer) {
        if (node.parent.type !== 'VElement')
          return

        const expression = node.expression
        if (
          expression.loc.start.line !== expression.loc.end.line
          || node.loc.start.line === node.loc.end.line
        ) {
          return
        }

        context.report({
          node,
          messageId: 'expectedSingleLine',
          fix(fixer) {
            return fixer.replaceTextRange(
              node.range,
              `{{ ${context.sourceCode.getText(expression)} }}`,
            )
          },
        })
      },
    })
  },
} satisfies Rule.RuleModule

const templateStylePlugin = {
  rules: {
    'single-line-simple-interpolation': singleLineSimpleInterpolationRule,
  },
} satisfies ESLint.Plugin

export default antfu(
  {
    type: 'app',
    ignores: ['**/dist/**', '**/coverage/**', '**/pnpm-workspace.yaml'],
    formatters: {
      css: true,
      html: true,
      markdown: 'prettier',
    },
    stylistic: {
      indent: 2,
      quotes: 'single',
    },
    typescript: true,
    vue: {
      a11y: true,
    },
  },
  {
    files: ['**/*.vue'],
    plugins: {
      'template-style': templateStylePlugin,
    },
    rules: {
      'template-style/single-line-simple-interpolation': 'error',
      'vue/html-closing-bracket-newline': [
        'error',
        {
          singleline: 'never',
          multiline: 'always',
        },
      ],
      'vue/multiline-html-element-content-newline': [
        'error',
        {
          ignores: ['pre', 'textarea'],
          ignoreWhenEmpty: true,
          allowEmptyLines: false,
        },
      ],
      'vue/mustache-interpolation-spacing': ['error', 'always'],
    },
  },
  ...vueI18n.configs.base,
  {
    settings: {
      'vue-i18n': {
        localeDir: {
          pattern: './src/i18n/locales/*/*.json',
          localeKey: 'path',
          // ESLint 接收平台原生路径，Windows 与 Unix 都按目录名识别语言。
          localePattern: /[/\\]locales[/\\](?<locale>[^/\\]+)[/\\]/,
        },
        messageSyntaxVersion: '^11.0.0',
      },
    },
    rules: {
      '@intlify/vue-i18n/no-missing-keys': 'error',
      '@intlify/vue-i18n/no-missing-keys-in-other-locales': 'error',
      '@intlify/vue-i18n/no-duplicate-keys-in-locale': 'error',
      '@intlify/vue-i18n/valid-message-syntax': 'error',
      '@intlify/vue-i18n/valid-plural-forms': 'error',
      '@intlify/vue-i18n/no-unused-keys': [
        'error',
        {
          extensions: ['.ts', '.vue'],
          // 动态键无法被静态扫描识别；仅豁免已有调用对应的范围。
          ignores: [
            // 账号额度状态由服务端返回，额度面板按字段拼接键。
            '/^accounts\\.quota\\.states\\.(unsampled|stale|query_failed|exhausted|sampled)$/',
            // 审查页的页签、动作、覆盖范围和统计项按枚举或列表拼接键。
            '/^guard\\.tabs\\.(overview|records|profiles|policy|learning|help)$/',
            '/^guard\\.mode\\.(enforce|observe)$/',
            '/^guard\\.action\\.[^.]+$/',
            '/^guard\\.coverage\\.[^.]+$/',
            '/^guard\\.summary\\.[^.]+$/',
            // 规则类别和 Moderations 类别来自服务端目录，保留本地化显示名。
            '/^guard\\.ruleCategory\\.[^.]+$/',
            '/^guard\\.flow\\.moderationCategory\\.[^.]+$/',
            // 审查详情、策略选项与帮助章节在 v-for 或选项列表中拼接键。
            '/^guard\\.(rawScore|signalScore|auditScore|decisionScore|localMs|totalMs|evaluationCount|reuseCount)$/',
            '/^guard\\.layout\\.(enabled|disabled|builtin|custom|local|review|upstream|learning|coverage|retention)$/',
            '/^guard\\.flow\\.(allPatterns|anyPatterns|excludePatterns)$/',
            '/^guard\\.flow\\.(local|scope|origin|profile|status|help)\\.[^.]+$/',
            // 连接诊断阶段和审查状态使用服务端返回的阶段名、状态名。
            '/^claude\\.stages\\.[^.]+$/',
            '/^settings\\.promptGuard\\.(disabled|ready|candidate_review_unavailable)$/',
          ],
        },
      ],
    },
  },
)
