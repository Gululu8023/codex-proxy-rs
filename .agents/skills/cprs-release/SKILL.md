---
name: cprs-release
description: CPRS 的版本准备、发布说明、GitHub Release 工作流维护和发行失败恢复。用于核对版本、tag、Linux 产物、镜像与网页更新索引；不包含运行实例部署。
---

# CPRS 发行

先确定任务是准备、审查、发布还是恢复。准备版本或修改工作流不授权创建 tag、推送、发布 Release、应用仓库规则或升级实例；沿用用户已给出的具体授权。

## 唯一入口与事实源

- 操作与恢复说明：[发行指南](../../../docs/releasing.md)。执行发布前读相关阶段并检查所用脚本的实际副作用。
- 版本与通道：`release/version.yaml`；发布说明：`release/notes.md`；平台：`release/platforms.yaml`。
- 手动按钮与 tag push 共用 `.github/workflows/release.yml`，不要另建发布流程。
- 本地 `bash release/publish --check VERSION` 只读；不带 `--check` 创建并推送 tag，不修改版本或自动提交。

## 发布前

确认版本、说明与代码已经提交到 main，说明首行与版本一致。所有发行提交必须属于 main 历史；平台只保留 Linux amd64、arm64。普通备版保留 `published`，不预写未公开版本。

检查工作区、当前提交、已有 tag／Release 和相关 CI 证据。不能绕过质量门禁，也不能把语法检查称为 Linux 构建或真实发布验收。

## 发布与恢复

`latest` 由 main 构建维护。Release 公开版本镜像与附件；更高正式版推进 `stable`、GitHub Latest，公开附件核对后更新 main 的 `published`。预发行和旧版本不能回退稳定通道。

tag 保持不变，版本镜像仅允许相同 digest 恢复。发布失败先按指南判断所处阶段：未公开时续跑原失败任务；公开后用 `index_only` 恢复稳定通道与索引。源码变更使用新版本，不删除 tag 或覆盖已公开产物。

完成后回读 Release、附件、镜像架构及 `published`。报告版本、提交、工作流／Release 链接和实际状态；构建中、索引未完成或未验收时如实说明。仓库规则 JSON 只供审阅，未经授权不应用远端设置。
