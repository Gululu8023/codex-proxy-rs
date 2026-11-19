# Claude 接入与隐私保护

CPRS 用同一个网关同时服务 Codex 和 Claude Code。Claude 账号使用订阅 OAuth 登录，或直接导入 Claude Code 的凭据；不需要另起代理服务、管理后台或监听端口。

和 Codex 不同的是，**所有发往 Anthropic 的请求都必须经过内嵌的 [veil](https://github.com/Mouseww/veil)**。veil 在请求离开网关前把密钥、令牌等敏感文本换成占位符，响应回来时再换回原文。veil 不可用时 Claude 入口直接拒绝请求，不会退回发送原文。

```text
Codex  客户端 → Nginx:443 → CPRS:8080/v1      ───────────────→ OpenAI
Claude 客户端 → Nginx:443 → CPRS:8080/claude → Prompt 审查 → veil → Anthropic
```

## 接入步骤

1. 正常启动 CPRS。veil 的主密钥和映射库在首次启动时自动生成，在管理端确认 Claude 隐私状态为"就绪"
2. 在「账号管理」中选择 Claude，用 OAuth 登录或导入 Claude Code 凭据
3. 在「API 密钥」中创建密钥，勾选 Anthropic 权限
4. 点「使用密钥」，选 Claude Code 和操作系统，把配置合并到 `~/.claude/settings.json`

客户端配置细节见[部署文档：Claude Code](../README.md#claude-code)。Claude 账号的订阅、额度和重置权益见 [Claude 账号管理](../../docs/claude-account-management.md)。

## 公网入口

**同一个域名**是默认做法：Codex 走 `/v1/*`，Claude 走 `/claude/v1/*`，Nginx 原样转发全部路径即可，由 CPRS 自己分流。Claude Code 的 `ANTHROPIC_BASE_URL` 填 `https://网关域名/claude`，**不要加 `/v1`**，客户端会自己拼接。

**两个域名分开**时，使用 [nginx.conf.example](nginx.conf.example)。模板里 Codex 域名屏蔽了 Claude 路径；Claude 域名只开放 Messages、count_tokens 和模型列表，并把 `/v1/...` 转发到 CPRS 的 `/claude/v1/...`。这时客户端的 `ANTHROPIC_BASE_URL` 改为 `https://claude.example.com`，同样不加 `/claude` 或 `/v1`。管理端导出的配置默认按当前站点生成 `/claude` 地址，复制后手动改一下即可，不需要修改服务端设置。

如果前面还有一台只做四层转发的中转机，可参考 [dmit-stream.conf.example](dmit-stream.conf.example)，用 Nginx `stream` 原样透传 TLS。

反向代理需要关闭响应缓冲和代理缓存，否则流式输出会卡住。鉴权、权限、限流和并发都由 CPRS 自己检查，Nginx 不需要处理 Key。

### 模型与客户端身份

- 客户端通过 `/claude/v1/models` 获取当前密钥能用的模型，列表来自 Claude 账号的实际模型目录。导出的 `settings.json` 默认启用网关模型发现，不写死模型名
- 请求了未开放的模型会直接报错，网关不会偷偷换成别的模型
- CPRS 会定期从 `downloads.claude.ai` 同步 Claude Code 的最新版本号，用于上游请求身份；同步失败时沿用上次成功的版本。身份可在「系统设置」或密钥上调整
- Claude 账号不参与 [CPR 互联](../README.md#cpr-互联)共享。账号预热、额度阈值暂停等功能是否可用，以管理端显示的供应商能力为准

## veil 能保护什么

veil 按 Anthropic 协议结构处理正文里的普通文本：消息、工具参数、工具结果等。以下内容**不会**被扫描或改写：

- thinking 及其签名、`metadata.user_id` 中的身份值、协议 ID 等控制字段（改了会破坏会话）
- 图片、文档等二进制附件
- Claude Code 绕开网关直接发出的流量，例如自带的 MCP、WebFetch、联网工具。设置 `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` 也不能保证所有流量都经过网关

另外要清楚边界：veil 的目标是**减少发往 Anthropic 的敏感内容**，不是隔离 CPRS 自己。CPRS 进程能看到脱敏前和恢复后的明文，网关、TLS 入口和客户端都属于你的可信范围。规则命中也不代表任何格式的敏感数据都能识别出来。

### 映射如何隔离

占位符与原文的对应关系（映射）按**客户端密钥**隔离：同一密钥的不同请求、不同上游账号共用一套映射，所以多轮对话能正确还原；不同密钥之间互相看不到。管理端的诊断请求和 Prompt 审查的复核请求使用独立的临时空间。

映射库只增不删，没有过期清理，默认上限 100 万条。满了以后，需要新增映射的请求会被拒绝，已有映射不受影响。

发送前如果发现请求引用的映射已经丢失，或者响应里有占位符还原不了，CPRS 会明确报错（如 `veil_mapping_missing`），不会把半还原的内容交给客户端。

## veil 配置与数据

在 `deploy/config.yaml` 中配置，以下是默认值：

```yaml
veil:
  data_directory: veil          # 相对 host.runtime_data_dir，默认即 .runtime/data/veil
  rules_file: null              # null 表示使用内置规则包
  max_mappings: 1000000         # 映射库条数上限
  max_parallel_operations: 32   # 同时进行的脱敏/还原任务数
  initialize_store: true        # 数据目录为空时是否自动创建新卷
```

数据目录里有四样东西，**必须作为一组保存和恢复**：

| 文件 | 作用 |
| --- | --- |
| `master.key` | 32 字节主密钥，用于加密映射库。也可以用环境变量 `VEIL_MASTER_KEY` 提供（优先于文件） |
| `mappings.db` | 加密的映射库（SQLite） |
| `initialized` | 卷的身份标记 |
| `rules-revision` | 当前规则的指纹 |

### 首次启动

默认 `initialize_store: true`。数据目录为空时，CPRS 会自动生成随机主密钥、映射库和标记文件（Linux 下目录权限 0700、密钥 0600）。一键安装和手动 Compose 部署都走这条流程，不需要手动生成密钥。

也可以提前放好自己的 `master.key`（32 字节随机值的十六进制或 Base64），或者设置 `VEIL_MASTER_KEY`。不要把密钥打印到终端、写进日志或源码，也不要拿数据库密码凑数。

### 重启与升级

重启和升级会继续使用原来的主密钥和映射库，已有会话不受影响。

只要数据目录里存在 `mappings.db`、`initialized`、`rules-revision` 中的任何一个，CPRS 就认为这是已有卷，要求其余文件和正确的主密钥都在。缺了任何一样，Claude 入口会保持不可用，**不会**自动重建空库来掩盖问题，也不要删掉标记文件来强行启动。这时管理端和 Codex 链路照常工作。

如果不希望 CPRS 在任何情况下自动创建新卷，把 `initialize_store` 设为 `false`。

### 自定义规则

`rules_file: null` 使用内置的密钥类和地域类规则。需要自己的规则时，复制 [veil.rules.toml.example](veil.rules.toml.example)，挂载到配置目录，再把 `rules_file` 指向它。

规则会写进 `rules-revision` 指纹，**同一个卷不能直接换规则**。修改规则时：保留旧卷作为回退，把 `data_directory` 指向一个新的空目录，然后让客户端开新会话。升级 CPRS 时如果 veil 的处理逻辑发生变化（运行时修订号写在指纹里），也按同样方式处理。

管理端只能查看规则，不能在线编辑、清空映射或关闭 Claude 隐私处理。

### 日志

不要在生产环境长期开启 `host.logging.request_dump` 和 `host.logging.oauth_recovery`，它们会以明文记录请求和凭据，绕过了 veil 的保护范围。

## 备份与恢复

管理端的 S3/R2 业务备份会在导出数据库之后，自动打包一份映射库快照（`.veil.tar.gz`），和数据库归档属于同一条备份记录。快照失败时整条备份算失败，不会产生缺映射的备份。

**主密钥不进入任何备份**，请和 `config.yaml` 一样单独加密保管。主密钥丢失后映射库无法解密，只能换新卷、开新会话。

恢复步骤见[部署文档：Claude 隐私存储恢复](../README.md#claude-隐私存储恢复)。自己做文件级备份时，要先停止 CPRS 或使用 SQLite 的一致性备份方式，不能直接复制正在写入的 `mappings.db`。

## veil 源码依赖

veil 作为 Rust 库编译进 CPRS，固定在上游提交 `82f9290022144c82609aaad149d81693c4a563ec`（0.3.14），并打上本地补丁 [veil-library.patch](../../scripts/patches/veil-library.patch)。许可见 [LICENSE](veil-LICENSE) 和 [NOTICE](veil-NOTICE)。

编译前在仓库根目录准备源码：

```bash
sh scripts/prepare-veil.sh                                     # Linux
pwsh -NoProfile -File scripts/prepare-veil.ps1                 # Windows
```

脚本把固定版本拉到 `.deps/veil` 并应用补丁；已有目录版本不对时直接报错，不会改动它。编译时设置 `VEIL_SKIP_UI_BUILD=1`，跳过 veil 自带的管理界面。源码版 Dockerfile 已包含这一步，所以不能只复制 `backend/` 目录单独构建。

补丁主要做了这些事：

- 增加 Anthropic 原生 JSON / SSE 的脱敏和还原入口，按协议位置跳过 thinking、签名、二进制和控制字段
- 工具参数先缓冲成完整 JSON 再还原，避免把未转义的原文插进流里
- 增加"严格还原"模式：映射缺失时报错，而不是原样放过占位符
- 给映射库加上条数上限（满额拒绝新增，不淘汰旧映射），打开已有库时先校验主密钥
- 把 `rusqlite` 对齐到 0.41.1，和 CPRS 的 SQLx 共用同一个 SQLite

具体约束写在补丁代码的注释里。
