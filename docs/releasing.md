# 发行指南

本文面向仓库维护者，说明如何发布 CPRS 的正式版本，以及发布中途失败时如何恢复。

CPRS 从 v4.0.0 起独立编号。版本号写在 `release/version.yaml`，发行平台只有 Linux amd64 和 arm64（`release/platforms.yaml`）。

## 两种构建

| | 触发方式 | 产物 |
| --- | --- | --- |
| **main 构建** | 推送到 `main` 且对应源码的 CI 通过 | 复用 CI 已验证的 amd64 镜像，发布 `sha-<提交前 12 位>`；提升前核对 main 和 CI 后更新 `latest` |
| **正式发行** | 手动运行 Release 工作流或推送符合条件的 `v*` tag | 两个架构的二进制包、校验文件、部署模板、版本号镜像和 GitHub Release |

Release 不会改动 `latest` 镜像，`latest` 跟随通过 CI 的 main 源码。发布机器人写回 `published` 不会触发新 CI；若 main 只多出这种索引变更，自动构建和手动重建可复用原提交的成功结果，镜像仍记录原源码 SHA。源码或构建版本变化则必须有对应提交的成功 CI。

CI 的容器检查先构建候选镜像并保存到 GHCR，候选标签不更新 `latest` 或 `stable`。通过扫描与启动验证后，从该镜像导出二进制和前端，连同镜像 digest、源码 SHA、版本、构建类型、实际构建时间、工具链和文件 SHA-256 保存为 `verified-build-linux-amd64` artifact，保留 7 天。PR 仍只在 runner 本机验证，不推送候选镜像。

`Docker image` 只下载成功 main push CI 的明确 artifact ID，验证合同后按 digest 发布提交标签，不再编译或构建镜像。提升 `latest` 前重新核对 main、最新 CI 和 artifact ID；源码变化或 CI 重跑产生新产物时，旧任务不能覆盖当前入口。发布机器人仅写回 `published` 的例外仍沿用原源码 SHA。

main push 的候选镜像统一由 CI 生成，PR 继续按变更范围选择容器检查。artifact 缺失或过期时，`Docker image` 会跳过提升并提示重跑对应提交的 CI，不自行重复构建。依赖审计仍重新获取漏洞信息；每周容器安全检查直接扫描 `latest` 和 `stable` 的已有 amd64 镜像 digest，不重建后再扫描。

## 发布步骤

### 1. 准备版本提交

把以下内容随待发布的代码一起提交到 `main`：

- `release/version.yaml` 中的 `version`。格式为 `X.Y.Z`，或带 `-alpha.N`、`-beta.N`、`-rc.N`、`-exp.N` 后缀（`N` 从 1 开始），可以附带 SemVer 构建元数据
- `release/notes.md`，第一行必须是 `# v<version>`，正文写本次的实际变化和升级注意事项

**不要手动修改 `published` 字段**，它由发布成功后的工作流自动维护。所有发行提交都必须在 `main` 上，实验版本也不例外。

### 2. 触发发布

**方式一：GitHub 页面（推荐）**

打开 [Release 工作流](https://github.com/Gululu8023/codex-proxy-rs/actions/workflows/release.yml)，点 **Run workflow**，分支选 `main`，填写 `v<version>`，不要勾选 `index_only`。

- tag 不存在：工作流核对 `main` 的提交、版本号和发布说明后自动创建 tag，并在同一次运行里继续构建。不需要再手动触发一次
- tag 已存在：使用 tag 指向的提交，核对它在 `main` 历史上且版本和说明一致，不会移动 tag
- Release 已经公开：停止构建，按下方[失败恢复](#失败恢复)处理

**方式二：本地脚本**

需要 Bash、Git、Ruby、已登录的 GitHub CLI 和推送 tag 的权限。当前分支必须是 `main`，与远端一致，且没有未提交的改动。

```bash
bash release/publish --check X.Y.Z   # 只读预检
bash release/publish X.Y.Z           # 创建并推送 tag，由 tag 推送触发工作流
```

脚本不会修改版本号，也不会自动提交。直接手动推送符合条件的 `v*` tag 效果相同。

tag 一旦创建就不再移动。发现代码需要修改，就发一个新版本。

### 3. 工作流做了什么

工作流先解析发行提交，再安排质量检查、依赖审计和产物准备；全部通过归档校验与容器验证后，生成产物证明、签名镜像、公开 Release 并更新发布通道。

同一提交的 main CI 正在运行时，Release 先等待它完成。若存在成功 CI 的有效 artifact，复用其中的前端、amd64 二进制和完整镜像 digest，仅补建 arm64；复用产物保留实际编译时的时间，新架构记录自己的构建时间。若没有成功 CI 产物或 artifact 已过期，则在 Release 内构建所需产物。下载后的源码 SHA、版本、构建类型、工具链或文件哈希不匹配时直接失败，不用重新构建掩盖错误。发行仍重新审计依赖和扫描、验证最终镜像，复用不省略这些检查。

如果这次只改了版本号和发布说明，且父提交已有成功的 `main` 后端 CI，会直接复用那次后端检查结果。

| 发布内容 | 结果 |
| --- | --- |
| 比当前 stable 更高的正式版 | 同时更新 `stable` 镜像和 GitHub Latest |
| alpha、beta、rc、exp | 标记为 Pre-release，不动 `stable` 和 GitHub Latest |
| 所有版本 | 公开后把版本写入 `main` 上 `release/version.yaml` 的 `published`，管理端的在线更新据此发现新版本 |

### 4. 发布后核对

- Release 的 tag、全部附件和 SHA-256
- 版本号镜像包含两个架构
- `stable` 指向是否符合预期
- `main` 上的 `published` 已更新

只改了 `version` 或者 `latest` 构建成功，都不等于正式发行成功。从未发布过任何版本时，`published: {}` 是正常状态。

## 失败恢复

工作流在不同阶段失败，恢复方式不同：

| 失败时已完成到 | 怎么恢复 |
| --- | --- |
| tag 已创建，构建或校验失败 | 修复外部问题后，重跑原任务中失败的 job；也可以从 `main` 对同一个 tag 重新触发。需要改代码就发新版本 |
| 草稿 Release 已上传，版本镜像还没推送 | 重跑失败的 job，草稿可以补齐附件 |
| 版本镜像已推送，Release 还没公开 | **只**重跑失败的 publish job，它会复用已验证的二进制和镜像。不能重新构建去覆盖已有的版本镜像 |
| Release 已公开，但 stable / GitHub Latest / published 没更新 | 从 `main` 手动运行 Release，填原 tag，勾选 `index_only` |

`index_only` 只补做发布通道的更新：先核对已公开的 Release、附件和版本镜像，再更新 `stable` 和 `published`，不会构建或上传任何东西。它同样会防止旧版本把 stable 回退。如果多个版本同时写 `published` 产生冲突，重新运行一次即可。

几条通用规则：

- 已存在的版本镜像只允许同一 digest 的恢复，不会被重新构建的结果覆盖
- 已公开 Release 的附件不会重新上传
- 本地推送 tag 失败时，先确认远端是否已经收到，不要把 tag 改指到别的提交
- 已公开的版本发现问题，发新版本修复

## 仓库保护

仓库里提供了两份可审阅的规则集配置，但**提交文件本身不会改变 GitHub 设置**，需要管理员手动应用：

| 配置 | 效果 |
| --- | --- |
| [main.json](../.github/rulesets/main.json) | `main` 禁止删除和强制推送，允许正常提交和发布机器人更新 `published` |
| [release-tags.json](../.github/rulesets/release-tags.json) | `v*` tag 允许创建，禁止删除和修改，没有任何绕过账号 |

可以在 Settings → Rules → Rulesets 中导入，或用 [Rulesets API](https://docs.github.com/en/rest/repos/rules)。已有同名规则时按 ID 更新，避免重复创建：

```bash
gh api repos/Gululu8023/codex-proxy-rs/rulesets
gh api --method POST repos/Gululu8023/codex-proxy-rs/rulesets --input .github/rulesets/main.json
gh api --method POST repos/Gululu8023/codex-proxy-rs/rulesets --input .github/rulesets/release-tags.json
```

权限相关：

- Release 工作流需要 `contents: write`（创建 tag、发布 Release、更新 `published`）和 `packages: write`（推送镜像）
- 镜像发布到 GHCR 使用工作流自带的 `GITHUB_TOKEN`，不需要额外的 PAT。首次发布后，需要在 GitHub Packages 里把镜像设为 Public，服务器才能匿名拉取
- 开启分支保护时必须保留 `main` 的直接写入能力，否则工作流无法提交 `published`。如果以后要求所有改动都走 PR，需要同时改造这一步

其他：保留 CI、Dependabot 和安全扫描。Dependabot 的 `npm` 生态同时覆盖 pnpm，不要改成不存在的 `pnpm` 生态。
