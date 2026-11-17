<!-- prettier-ignore -->
<div align="center">

<img src="frontend/public/favicon.svg" alt="Codex Proxy RS" width="80" height="80" />

# Codex Proxy RS

把多个 ChatGPT、Claude 订阅账号合成一个账号池，让 Codex 和 Claude Code 通过同一个网关使用

[![CI](https://github.com/Gululu8023/codex-proxy-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Gululu8023/codex-proxy-rs/actions/workflows/ci.yml)
[![Docker image](https://github.com/Gululu8023/codex-proxy-rs/actions/workflows/docker-image.yml/badge.svg)](https://github.com/Gululu8023/codex-proxy-rs/actions/workflows/docker-image.yml)
[![GHCR](https://img.shields.io/badge/GHCR-codex--proxy--rs-2496ED?logo=docker&logoColor=white&style=flat-square)](https://github.com/Gululu8023/codex-proxy-rs/pkgs/container/codex-proxy-rs)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square)](LICENSE)

[能做什么](#能做什么) · [快速开始](#快速开始) · [客户端接入](#客户端接入) · [文档](#文档)

</div>

Codex Proxy RS（CPRS）是一个自托管网关。你把手上的 ChatGPT 和 Claude 订阅账号登录进来，再给自己的电脑、同事或其他服务各发一个客户端密钥。Codex、Claude Code 只要连上网关，就由网关挑选合适的账号转发请求，同时记下每个密钥用了多少、花了多少钱。

> [!IMPORTANT]
> 本项目基于 [zyycn/codex-proxy-rs](https://github.com/zyycn/codex-proxy-rs) 二次开发，从 v4.0.0 起独立编号。数据库结构和源仓库**不兼容**，首次部署请使用全新的 PostgreSQL 和空的 Redis，原因见[初始化基线](backend/migrations/README.md#二开初始化基线)。

## 能做什么

**账号池**
- ChatGPT（Codex）和 Claude 账号用 OAuth 登录，也可以直接导入已有凭据
- 可以给每个账号指定独立的代理出口，并给账号分组
- 后台自动同步各账号的额度和可用模型，账号快用完或出错时会冷却，暂时不再分到请求
- 请求按额度、会话亲和和并发情况分配到账号，账号都忙时自动排队

**客户端密钥**
- 每个密钥可以单独限定：能用哪些供应商、哪些账号分组，日/周花费上限，并发数和每分钟请求数
- 密钥持有人用自己的 Key 登录管理端，就能查看自己的用量和额度，并复制现成的客户端配置

**客户端**
- **Codex**：支持 Responses 和 Chat Completions 接口，也支持生图和语音通话。配置可一键复制，也能导入 CC Switch
- **Claude Code**：走原生 Messages 接口，支持流式输出、token 计数和模型发现。CLI 和 VS Code 扩展共用一份 `settings.json`

**Claude 隐私保护**：发往 Anthropic 的文本先由内嵌的 [veil](deploy/claude/README.md) 替换掉密钥、令牌等敏感文本，响应回来时再还原。主密钥和映射库在首次启动时自动生成。

**Prompt 审查**：先用本地规则给请求打分，可以再交给一个独立的审核模型复核。可以只记录不拦截，先观察效果，再切到拦截模式。被上游拒绝的请求会进入学习队列，帮你整理出新规则，规则需要人工确认才会发布。

**运维**
- 按模型定价统计用量和费用，能查看请求日志、定位失败原因
- 支持备份数据库和 Claude 隐私存储，官方构建可以在管理端直接在线更新
- 通过 [CPR 互联](deploy/README.md#cpr-互联)，可以把另一台 CPRS 发出的密钥当作账号接入，共享它的账号池

## 快速开始

用 Docker Compose 部署，PostgreSQL 和 Redis 会一起启动。需要 Linux amd64，并已安装 Docker Engine、Docker Compose Plugin、curl 和 OpenSSL。

> [!TIP]
> `main` 分支自动构建的 `latest` 镜像只有 linux/amd64 版本。arm64 镜像，以及 Linux、macOS、Windows 的二进制包，请从 [Releases](https://github.com/Gululu8023/codex-proxy-rs/releases) 获取。

### 一键安装

当前用户要能使用 Docker，并能通过 `sudo` 或 root 设置目录权限：

```bash
curl -fsSL https://raw.githubusercontent.com/Gululu8023/codex-proxy-rs/main/deploy/install.sh -o install.sh && bash install.sh
```

脚本会在当前目录下创建 `codex-proxy-rs/`，下载部署文件、生成密码并启动服务。结束时会显示访问地址和管理员密码，请记下来。

可以用环境变量调整安装行为：

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `INSTALL_DIR` | 安装目录，建议写绝对路径 | 当前目录下的 `codex-proxy-rs/` |
| `CPR_RELEASE_TAG` | 安装指定 Release 版本 | 跟随 `main` 分支 |
| `ADMIN_PASSWORD` | 管理员初始密码，至少 12 位，不能包含 `$`，也不能是常见弱口令 | 随机生成 |

```bash
INSTALL_DIR="$HOME/services/codex-proxy-rs" bash install.sh
```

在已有安装目录里再次运行脚本，不会改动现有配置，也不会升级版本。想自己一步步部署，请看[手动安装](deploy/README.md#手动安装)。

### 升级

在安装目录执行：

```bash
docker compose -f deploy/compose.yaml pull
docker compose -f deploy/compose.yaml up -d
```

请保留 `.runtime/data` 目录。Claude 隐私处理的主密钥和映射库都存在这里，丢失后之前的 Claude 会话将无法继续。跨版本升级前先看[升级说明](deploy/README.md#镜像升级与源码构建)。

### 开始使用

1. 打开 `http://127.0.0.1:8080`，用 `admin@cpr.local` 和管理员密码登录。默认只能在服务器本机访问，要从其他设备使用，请配置 [HTTPS 反向代理](deploy/README.md#公网访问)
2. 在「账号管理」中选择供应商，登录或导入账号
3. 在「API密钥」中创建客户端密钥，勾选它能用的供应商。不选分组，就能使用权限范围内的全部账号
4. 点密钥的「使用密钥」，复制配置到客户端

## 客户端接入

| 客户端 | 地址 | 配置文件 |
| --- | --- | --- |
| Codex CLI / 桌面端 | `http://127.0.0.1:8080/v1` | `~/.codex/config.toml` |
| Claude Code CLI / VS Code | `http://127.0.0.1:8080/claude`（**不要加 `/v1`**） | `~/.claude/settings.json` |
| 其他 Responses / Chat Completions 客户端 | `http://127.0.0.1:8080/v1` | 按客户端填写 |

远程使用时，把 `http://127.0.0.1:8080` 换成网关的 HTTPS 地址。API Key 一律填管理端创建的客户端密钥；Claude Code 使用的密钥需要开通 Anthropic 权限。

查询某个密钥能用哪些模型：

```bash
curl http://127.0.0.1:8080/v1/models \
  -H 'Authorization: Bearer <client-api-key>'
```

具体配置、生图、语音和排障见[客户端配置](deploy/README.md#客户端配置)。

## 文档

| 想做什么 | 看这里 |
| --- | --- |
| 部署、备份、恢复、升级 | [部署文档](deploy/README.md) |
| 接入 Claude、了解隐私处理 | [Claude 接入与 veil](deploy/claude/README.md) |
| 对接接口、配置 Prompt 审查 | [API 参考](docs/api.md) |
| 二次开发 | [开发指南](docs/development.md) · [系统架构](docs/architecture.md) · [数据库迁移](backend/migrations/README.md) · [UI 组件库](https://github.com/zyycn/codex-proxy-ui) |
| 提交代码 | [贡献指南](CONTRIBUTING.md) |

## 致谢

感谢 [zyycn/codex-proxy-rs](https://github.com/zyycn/codex-proxy-rs) 原作者打下的基础，也感谢 [LINUX DO](https://linux.do) 社区。

## 许可证

[Apache License 2.0](LICENSE)
