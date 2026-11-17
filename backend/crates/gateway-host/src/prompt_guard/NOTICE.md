# Prompt 审查来源

规则、来源分类、候选判定及 cyber 复审提示词参考 [james-6-23/codex2api](https://github.com/james-6-23/codex2api/tree/811ad451e29d752cc01a13acaacc25074b515034) 的 `security/promptfilter`，本次核对提交为 `811ad451e29d752cc01a13acaacc25074b515034`。130 条内置规则及四组上下文规则按 Go 字面量和常量拼接解析后与现有资产逐字段一致。对应原作者归属保持不变，未引入 sub2api 的 Prompt 策略。

CPRS 使用同一个 Rust 评分器执行本地告警／阻断、严格终止和人工发布规则；外部 Chat Completions 或 Moderations 复审可独立启停。上游明确的内容策略拒绝进入脱敏证据、会话锁、人工学习审核流程。来源提取、编码预算、终止阈值、无效响应处理及协议错误投影按 CPRS 架构实现，不承诺与原 Go 判定器逐行或逐请求等价。

规则版本为 `codex2api-811ad45-cprs-3`，复审合同为 `cprs-guard-v2`。默认本地阈值 50、严格终止阈值 90；外部复审默认关闭，启用时默认超时 10 秒、置信度阈值 0.7、输入预算 32768 个 Unicode 字符，来源信封计入预算。具体采用范围和差异见[接口文档](../../../../../docs/api.md#prompt-审查)与[系统架构](../../../../../docs/architecture.md#prompt-审查)。
