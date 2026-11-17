# 开发与源码联调

本文说明开发环境、服务启动与跨仓库联调，命令均从宿主仓库根目录执行

协作与检查要求见 [贡献与审查](../CONTRIBUTING.md)，部署实例的配置与数据维护见 [部署与运维](../deploy/README.md)

| 任务 | 入口 |
| --- | --- |
| 修改宿主前后端 | [环境与依赖](#环境与依赖) → [启动宿主](#启动宿主) |
| 定位模块与共享实现 | [源码入口](#源码入口) |
| 同时修改 UI | [源码联调](#源码联调) → [启动与检查](#启动与检查) |
| 验证本地 SDK | [本地 SDK 验证](#本地-sdk-验证) |
| 提交跨仓库改动 | [编辑与提交](#编辑与提交) |
| 验证与发行 | [检查与发布](#检查与发布) |

## 环境与依赖

需要 Node.js 26.13.1、pnpm、[Rust 工具链](architecture.md#验证命令)、PostgreSQL 和 Redis

各前端的 `packageManager` 固定 pnpm 版本，宿主 CI 和 Docker 从 `frontend/package.json` 读取
Docker 前端构建阶段固定为 `node:26.13.1-bookworm-slim@sha256:5df79989ed685e09f0a98019db4cebfb22fb9df1a53c0161664a011f7ff6b0f1`；veil 准备、主密钥初始化、运行镜像及发布流程沿用现有配置。

Rust 工具链声明位于 `backend/rust-toolchain.toml`，从仓库根目录运行 Cargo 时显式选择该工具链；
`--manifest-path` 只选择 manifest，不切换 rustup 按当前目录选定的工具链

```bash
git clone https://github.com/Gululu8023/codex-proxy-rs.git
cd codex-proxy-rs
pnpm --dir frontend install --frozen-lockfile
```

前端依赖、锁文件与 ESLint 配置由 `frontend/` 管理，根目录不维护 Node 包

- **版本固定**：`@codex-proxy/ui` 使用固定 GitHub 标签或提交，锁文件记录实际提交、源码归档地址及完整性摘要
- **安装与构建**：pnpm 按 UI 自身锁文件准备依赖，执行 `prepack` 生成 JS、CSS 和类型声明，普通 `dev`、`build`、发行 CI 和 Docker 使用编译产物，无需初始化子模块
- **构建许可**：`frontend/pnpm-workspace.yaml` 的 `allowBuilds` 按仓库放行自有 UI 包脚本，升级时核对源码并更新锁文件，无需逐次修改许可，不开放全部依赖脚本

## 启动宿主

使用源码仓库自带的 Compose 与配置模板，按[手动安装](../deploy/README.md#手动安装)中的配置与权限要求准备 `deploy/config.yaml` 和 `.runtime/` 目录

数据库及 Redis 可由 Compose 启动：

```bash
docker compose -f deploy/compose.yaml up -d postgres redis
sh scripts/prepare-veil.sh
VEIL_SKIP_UI_BUILD=1 cargo +1.99.1 run --manifest-path backend/Cargo.toml -p codex-proxy-rs --locked
```

后端从当前目录向上查找 `deploy/config.yaml`，本机运行时数据库与 Redis 地址应指向可访问的本机端口

`host.runtime_data_dir` 和日志的相对路径以该配置文件所在目录解析

另开终端启动管理端：

```bash
pnpm --dir frontend dev
```

后端代理由 `frontend/vite.config.ts` 配置：管理端 API 使用 `/dev` 前缀，Claude 客户端的 Base URL 直接使用 `http://localhost:5173/claude`。验证 WebSocket 时直接连接后端

Windows 本地联调可在仓库根目录用 PowerShell 7 一次启动前后端：

```powershell
pwsh -NoLogo -NoProfile -NonInteractive -File ./dev.ps1
```

先准备上述配置和前端依赖，并启动 PostgreSQL、Redis。脚本沿用仓库根目录的当前 Rust 工具链与环境变量，需提前安装满足项目版本要求的工具链。后端仅监听 `127.0.0.1:8080`，健康检查通过后才启动前端；等待 Vite 输出就绪信息后访问 `http://localhost:5173`。前端保留 Vite 热更新，Rust 修改后需停止并重新运行命令

按 `Ctrl+C` 或任一服务退出时，会关闭本次启动的前后端进程树，数据库和 Redis 继续运行。端口被占用时直接报错，请先关闭原有开发服务。子进程共享当前终端，编译错误与启动日志直接输出，启动失败时查看退出提示之前的原始错误

Claude 使用同进程 veil，编译前运行 `scripts/prepare-veil.ps1`（Windows）或 `sh scripts/prepare-veil.sh`（Linux）。`dev.ps1` 自动设置 `VEIL_SKIP_UI_BUILD=1`；直接调用 Cargo 时需自行设置，避免构建 veil 的独立管理界面。默认配置在首次启动时自动生成主密钥和映射卷，已有数据的持久化与恢复要求见[内嵌 veil 配置](../deploy/claude/README.md)

数据库使用独立的 `0001_cprs_initial.sql` 初始化基线，已包含 Claude / Prompt Guard 及当前存储结构。源仓库和旧定制版数据库不能原地升级到该基线；开发环境须使用全新数据库及空 Redis，不能修改 checksum 绕过检查。后续结构变更追加迁移，边界见[二开初始化基线](../backend/migrations/README.md#二开初始化基线)

前后端检查见 [贡献与审查](../CONTRIBUTING.md#验证)，后端单元／集成测试及专用服务准备见 [后端测试](backend-testing.md)。

## 源码入口

后端职责和依赖方向见 [Workspace 边界](architecture.md#3-workspace-边界)，页面与共享逻辑见 [前端模块职责](architecture.md#34-前端模块职责)。
先按所属模块定位，再跟踪请求、状态与展示的调用关系

共享 UI 的命名与目录见[组件架构](../modules/ui/playground/guide/architecture.md)

## 源码联调

宿主与组件库保留独立仓库，通过 Git 子模块在同一目录开发：

| 目录 | 仓库与职责 |
| --- | --- |
| `frontend` / `backend` | 宿主管理端与 Rust 网关 |
| `modules/ui` | `codex-proxy-ui`，共用组件与主题 |

各仓库的常规 CI 验证自身代码和锁定依赖。源码模式用于按需联调，运行前确认所选版本或提交组合及其公开合同兼容

### 检出与安装

首次检出可同时下载子模块：

```bash
git clone --recurse-submodules https://github.com/Gululu8023/codex-proxy-rs.git
cd codex-proxy-rs
```

已有仓库运行 `git submodule update --init --recursive`。子模块固定到具体提交；拉取宿主更新后，同样用此命令对齐。执行前先保存子模块中的改动，避免切换正在开发的分支

在宿主根目录安装各项目依赖：

```bash
pnpm --dir modules/ui install --frozen-lockfile
pnpm --dir frontend install --frozen-lockfile
```

### 启动与检查

在各自终端启动所需服务：

```bash
pnpm --dir frontend dev:source                            # 管理端，引用本地 UI
pnpm --dir modules/ui dev                                 # 组件库交互示例
```

管理端需要[已启动的网关](#启动宿主)。

`dev:source` 使用 Vite 的 `source` 模式，由 `@codex-proxy/ui/vite` 的 `CodexProxyUI` 适配器将公开入口解析到 `modules/ui`，并统一处理共享依赖、源码预构建排除与开发服务的文件访问范围。入口映射以 UI 包 `exports` 中的 `codex-proxy-source` 条件为准，由 Vite 热更新；`source` 模式需要上述子模块目录结构

```bash
pnpm --dir frontend build:source
```

`build:source` 使用 `tsconfig.source.json` 检查源码类型，管理端构建到 `frontend/node_modules/.vite/source-dist`，不改写正式依赖、锁文件或 `dist`。

## 编辑与提交

直接打开宿主目录即可开发，分别在对应仓库目录操作 Git

1. 在要修改的子模块内先创建分支，例如 `git -C modules/ui switch -c feat/select`。初次初始化的子模块通常处于 detached HEAD
2. 在子模块运行自身检查，并在宿主根目录运行所需联调检查
3. 分别提交、推送并合并组件库 PR。宿主只记录子模块提交号，不能替代子仓库的提交与推送
4. 管理端按需升级 UI 依赖，固定标签或提交并维护锁文件，无需与 `modules/ui` 同步版本

更新子模块指针不会自动升级消费方依赖。正式依赖不能写成开发机路径或浮动分支

## 检查与发布

前端改动提交前，分别检查 ESLint 和 TypeScript；Vite 开发页面正常显示不能代替这两项检查：

```bash
pnpm --dir frontend format:check
pnpm --dir frontend typecheck
```

`format:check` 调用 ESLint，包含代码风格、Vue 模板、可访问性及翻译键检查。`build` 还会执行类型检查和生产资源构建。CI 按改动范围执行前端检查、后端质量检查和容器验证，工作流定义位于 `.github/workflows/`。

### main 镜像

推送 `main` 后先运行 CI；同一提交的 CI 成功且仍是当前 `main` 时，Docker image 工作流才发布 `ghcr.io/gululu8023/codex-proxy-rs:latest` 和 `sha-<提交前 12 位>`，平台为 linux/amd64。手动触发 Docker image 也要求当前 `main` 的同一提交已有成功 CI。失败时先处理对应检查，不能用手动构建绕过。

### 手动 Release

发行入口为 `release/publish`，需要 Git、Ruby、已登录的 GitHub CLI，以及对当前分支上游仓库的推送和工作流执行权限。先提交其他代码和界面改动，将目标版本的说明写入 `release/notes.md` 并保持该文件未提交，首行使用 `# vX.Y.Z`。版本规则以 `.github/scripts/release-metadata.rb` 为准。

```bash
# X.Y.Z 替换为尚未使用的目标版本；预检不改文件或远端状态。
bash release/publish --check X.Y.Z
# 提交版本与说明、创建 tag、推送并触发 Release 工作流。
bash release/publish X.Y.Z
```

Release 验证各平台产物后发布版本号镜像与安装归档；正式版本可提升 `stable` 和 GitHub 最新 Release，预发行不提升 `stable`。Release 不修改 `latest`。部署文件、镜像标签和升级方式见 [镜像升级与源码构建](../deploy/README.md#镜像升级与源码构建)。
