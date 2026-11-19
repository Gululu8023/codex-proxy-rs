# 开发指南

本文介绍如何在本地跑起 CPRS 的前后端、改代码以及提交前要做的检查。命令都在仓库根目录执行。

相关文档：[系统架构](architecture.md) · [API](api.md) · [后端测试](backend-testing.md) · [贡献指南](../CONTRIBUTING.md) · [发行指南](releasing.md)

## 项目结构

| 目录 | 内容 |
| --- | --- |
| `backend/` | Rust 网关（Cargo workspace），入口在 `backend/apps/gateway` |
| `backend/migrations/` | 数据库迁移，规则见[迁移说明](../backend/migrations/README.md) |
| `frontend/` | Vue 管理端，独立管理 Node 依赖，根目录没有 Node 包 |
| `deploy/` | Compose、配置模板、Dockerfile 和安装脚本 |
| `scripts/` | veil 源码准备脚本及补丁 |
| `modules/ui` | 可选子模块：UI 组件库 [codex-proxy-ui](https://github.com/zyycn/codex-proxy-ui)，只在同时修改组件库时需要 |

后端各 crate 的职责和依赖方向见[架构：Workspace 边界](architecture.md#3-workspace-边界)，前端目录分工见[前端模块职责](architecture.md#33-前端模块职责)。

## 环境与依赖

| 工具 | 版本 |
| --- | --- |
| Node.js | 26.13.1 |
| pnpm | 以 `frontend/package.json` 的 `packageManager` 为准 |
| Rust | 以 `backend/rust-toolchain.toml` 为准（当前 1.99.1） |
| PostgreSQL / Redis | 18 / 8，可以直接用 Compose 启动 |

```bash
git clone https://github.com/Gululu8023/codex-proxy-rs.git
cd codex-proxy-rs
pnpm --dir frontend install --frozen-lockfile
sh scripts/prepare-veil.sh          # Windows: pwsh -NoProfile -File scripts/prepare-veil.ps1
```

几点说明：

- **Rust 工具链**：`rust-toolchain.toml` 在 `backend/` 下，而从仓库根目录用 `--manifest-path` 运行 Cargo 时，rustup 不会读到它，所以命令里要写 `cargo +1.99.1`
- **veil**：Claude 的隐私处理以 Rust 库的形式编译进网关，编译前必须先运行准备脚本，把固定版本源码拉到 `.deps/veil` 并打补丁。编译时设置 `VEIL_SKIP_UI_BUILD=1`，跳过 veil 自带的管理界面。详见 [veil 源码依赖](../deploy/claude/README.md#veil-源码依赖)
- **UI 组件库**：`@codex-proxy/ui` 通过固定的 Git 标签安装，pnpm 安装时会自动编译，平常不需要初始化子模块
- **编译配置**：`dev` profile 关闭了调试符号和增量编译（`backend/Cargo.toml`），用来控制本地产物体积，不要随手改回去

## 启动

### 准备配置和数据库

按[手动安装](../deploy/README.md#手动安装)创建 `deploy/config.yaml` 和 `.runtime/` 目录，然后用 Compose 只启动数据库：

```bash
docker compose -f deploy/compose.yaml up -d postgres redis
```

本机运行时，`config.yaml` 里的数据库和 Redis 地址要指向本机端口。网关从当前目录向上查找 `deploy/config.yaml`，日志和数据目录的相对路径以这个文件所在目录为准。

> [!IMPORTANT]
> 数据库必须是全新的。CPRS 使用独立的初始化基线 `0001_cprs_initial.sql`，源仓库或旧版本的数据库无法直接使用，也不要改 checksum 绕过检查。

### 启动前后端

Windows 上可以用脚本一次启动前后端（需要 PowerShell 7）：

```powershell
pwsh -NoLogo -NoProfile -NonInteractive -File ./dev.ps1
```

脚本会自动设置 `VEIL_SKIP_UI_BUILD=1`，先启动后端（只监听 `127.0.0.1:8080`），健康检查通过后再启动前端。看到 Vite 就绪后访问 `http://localhost:5173`。前端支持热更新，改了 Rust 代码需要 `Ctrl+C` 后重新运行。退出时只关闭本次启动的进程，数据库继续运行。端口被占用时会直接报错，先关掉之前的开发服务。

其他平台分两个终端手动启动：

```bash
# 终端 1：后端
VEIL_SKIP_UI_BUILD=1 cargo +1.99.1 run --manifest-path backend/Cargo.toml -p codex-proxy-rs --locked

# 终端 2：管理端
pnpm --dir frontend dev
```

Vite 开发服务会代理后端请求：管理端 API 走 `/dev` 前缀，Claude 客户端可以直接用 `http://localhost:5173/claude` 作为 Base URL。开发代理没有开启 WebSocket 转发，调试 WebSocket 时请直连后端 `8080` 端口。

## 源码联调

需要同时修改 UI 组件库时，把它作为子模块检出到 `modules/ui`：

```bash
git submodule update --init --recursive      # 新克隆可用 git clone --recurse-submodules
pnpm --dir modules/ui install --frozen-lockfile
```

子模块固定在某个提交上，拉取主仓库更新后再执行一次上面的命令对齐。执行前先保存子模块里的改动。

```bash
pnpm --dir frontend dev:source      # 管理端，直接引用 modules/ui 的源码，支持热更新
pnpm --dir modules/ui dev           # 组件库自己的示例页面
pnpm --dir frontend build:source    # 用本地组件库源码做类型检查和构建，产物不覆盖正式 dist
```

`source` 模式下，组件库的入口由 `@codex-proxy/ui/vite` 提供的适配器映射到 `modules/ui` 源码，不会改动正式依赖和锁文件。组件命名和目录约定见组件库的[架构说明](../modules/ui/playground/guide/architecture.md)，主题见[主题文档](theme.md)。

### 提交跨仓库改动

主仓库和组件库是两个独立的仓库，要分别提交：

1. 在子模块里先建分支，例如 `git -C modules/ui switch -c feat/select`（刚初始化的子模块处于 detached HEAD）
2. 在组件库里跑它自己的检查，再回到主仓库跑联调检查
3. 组件库的改动单独提交、推送、合并 PR
4. 组件库发版后，主仓库把 `frontend/package.json` 里的 `@codex-proxy/ui` 升级到新标签并更新锁文件

更新子模块指针不会改变管理端实际使用的组件库版本。正式依赖必须是固定的标签或提交，不能写本地路径或分支名。

## 提交前检查

前端：

```bash
pnpm --dir frontend format:check    # ESLint：代码风格、Vue 模板、可访问性、翻译键
pnpm --dir frontend typecheck       # vue-tsc 类型检查
pnpm --dir frontend test:unit       # 单元测试（frontend/tests/）
```

Vite 开发页面能打开不代表这些检查能过。`pnpm --dir frontend build` 会同时做类型检查和生产构建。

后端（在 `backend/` 目录下执行）：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
```

后端测试需要专用的 PostgreSQL 和 Redis，准备方法见[后端测试](backend-testing.md)。CI 会按改动范围执行前端检查、后端检查和容器验证，定义在 `.github/workflows/`。

## 发布

- 推送到 `main` 并且 CI 通过后，会自动构建 `latest` 镜像（仅 amd64）
- 正式版本通过 GitHub Actions 的 Release 工作流发布，步骤和失败恢复见[发行指南](releasing.md)
