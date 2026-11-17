# Claude OAuth 与内嵌 veil

Claude 仅使用订阅 OAuth 或 Claude Code 凭据导入，公网入口与管理功能复用 CPRS，同进程 veil 处理所有 Claude 请求，不另起代理、管理 API 或监听端口

```text
codex.example.com  → Nginx:443 → CPRS:8080 → OpenAI
claude.example.com → Nginx:443 → CPRS:8080/claude
                                 → Guard → veil 库 → Anthropic
```

## 固定源码依赖

veil 固定在 `82f9290022144c82609aaad149d81693c4a563ec`，唯一生效补丁为 [veil-library.patch](../../scripts/patches/veil-library.patch)，许可见 [LICENSE](veil-LICENSE) 与 [NOTICE](veil-NOTICE)

Windows 在仓库根目录执行 `pwsh -NoProfile -File scripts/prepare-veil.ps1`，Linux 执行 `sh scripts/prepare-veil.sh`。脚本只取得固定源码并应用本地补丁，已有不匹配工作副本会报错，不创建提交或推送。Cargo 相对路径依赖使用 `.deps/veil`，编译设置 `VEIL_SKIP_UI_BUILD=1`，不构建 veil 管理界面。源码 Dockerfile 包含相同准备步骤，不能只复制 backend 目录单独编译

补丁使用 `rusqlite 0.41.1`，与宿主 SQLx 共用 `libsqlite3-sys 0.39.1`，避免同一程序链接两个 SQLite 版本

| 库内改动 | 行为与边界 |
| --- | --- |
| `veil/src/proxy/anthropic_sse.rs` 与原 `SseRestorer` | 由原恢复入口识别 Anthropic 消息流；按 content block index 隔离状态，文本尾部在对应 block_stop 前排空；thinking、签名、用量与未知事件不进入通用文本恢复 |
| 工具参数恢复 | 缓冲 input_json_delta，完整工具 JSON 解析后恢复并重新序列化，在 block_stop 前输出完整参数 delta；不插入未转义明文，不发送空 JSON delta；接受工具块级延迟 |
| `veil-engine` 严格恢复 | 新增 `RestoreWindow::new_strict`、`restore_json_strict`；对应 `SseRestorer::new_strict` 使用严格缺失语义。原非严格入口保留原有缺失行为，CPRS 显式采用严格入口 |
| `veil_engine::anthropic` 原生 JSON 入口 | 新增请求脱敏和严格响应恢复接口；按实际 messages/content/tools/schema 位置保护协议 ID、thinking/签名块、二进制 source 及控制参数；工具参数中的同名 `type` 不取得协议豁免。文本处理仍复用引擎，不依赖客户端自报跳过标记 |
| 严格规则执行 | Claude 原生入口将正则执行/预算错误作为独立故障；仅按协议位置保护二进制 source，不对任意 `data` 字段或长 base64 形状字符串提供扫描豁免 |
| 字面量占位符 | 原生请求中的合法占位符形状按普通文本保护为稳定 LITERAL 映射；与规则命中的重叠范围合并，避免破坏敏感文本匹配。恢复仅执行一次，不重新解释恢复后的字面量 |
| `SqliteStore::with_max_rows` | 全库新增映射上限，在同一写事务内检查与插入；已有映射仍可使用，满额返回独立容量错误，不淘汰其他映射；未配置时保持原库行为 |
| 存储主密钥检查 | 打开非空映射库时先验证一条已有密文能解密，防止错误密钥使 HMAC 查询全部未命中并写入第二套映射；其他条目的损坏仍在实际访问时报告，不宣称完成全库校验 |
| 错误与资源边界 | 缺失映射、容量耗尽、无效流与缓冲超限分别表达；适配新增错误枚举的原调用点。工具待恢复正文合计上限 32 MiB，在途块数上限 1024，SSE 输入缓冲上限 32 MiB |

容量的语义经源码确认：原版 `with_cap` 只影响 `purge()` 的淘汰行为，`with_ttl(Duration::ZERO)` 配合 purge 会删除映射，不表示关闭过期。不调用 purge 可以避免自动删除，但原版不具备新增写入的硬容量限制，因此补充了独立上限接口

运行时修订为 `82f9290022144c82609aaad149d81693c4a563ec+cprs-library-3`，参与规则指纹；旧指纹卷不能静默启用新规则或新遍历语义。
`initialized` 保存随机存储代次，完整写入后原子发布；重启保持代次，独立新卷产生新代次。
主密钥、映射库、`initialized` 和 `rules-revision` 必须配套，丢失不能靠重新初始化掩盖。
旧版空 `initialized` 或旧遍历指纹均停止 Claude 数据面，不自动迁移、删除或承认旧历史；
启用新版需保留原卷备份，明确配置新卷并开始新会话。Thread 保存规则/存储/密钥的不可逆状态指纹，跨独立映射卷不能续接。

veil 只在实际协议位置保护 Thread/diagnostics ID、inline tool definition 和经过语法校验的 billing；
`metadata.user_id` 仅保护经过校验的三个身份值，其他文本仍脱敏。工具参数不因同名字段获得豁免。
修复复用原版 `RestoreWindow`、MappingStore、加密和 SQLite 存储；CPRS 不添加恢复算法、映射缓存或内部 HTTP 代理，必要的非显然约束在库代码旁以注释说明

## 主密钥、规则与持久化

在 `deploy/config.yaml` 配置：

```yaml
veil:
  data_directory: veil
  rules_file: null
  max_mappings: 1000000
  max_parallel_operations: 32
  initialize_store: true
```

`data_directory` 相对 `host.runtime_data_dir`，默认部署对应 `.runtime/data/veil`，容器复用已有 runtime data 挂载。目录必须由 CPRS 运行身份可读写；首次启动自动创建目录与密钥，Linux 权限分别为 0700 和 0600

默认 `initialize_store: true`，全新映射卷在 CPRS 启动时自动生成随机 32 字节主密钥，保存为 `master.key`，并创建映射库与初始化清单。一键安装和直接使用 Compose 都复用这条流程，无需手动生成密钥或在首次启动后修改配置；后续重启、升级继续使用持久化目录中的原密钥和映射库

也可预先提供 `master.key`（随机 32 字节的十六进制或 Base64 表示），或向容器显式传入 veil 库支持的 `VEIL_MASTER_KEY` 环境变量，环境变量优先。不要输出密钥到终端、日志或源码，不使用数据库密码替代。设置 `initialize_store: false` 可禁止创建新卷，已有完整映射卷仍可正常加载；旧配置显式设置了 false 时，创建新卷前需改为 true

只有 `mappings.db`、`initialized` 和 `rules-revision` 均不存在时才允许初始化新卷。发现其中任一文件后，其余文件与原密钥必须齐全，否则拒绝启动 Claude，不自动补建或覆盖，不能删标记绕过恢复。密钥错误、规则非法或映射库不可用时，Claude 入口返回不可用，管理页面与独立 OpenAI 链路仍可使用

`rules_file: null` 使用内置 secrets、region 规则包。需要环境专属规则时，复制 `veil.rules.toml.example` 为自己的规则文件并挂载到配置目录；规则文件只读取 rules、allowlist 与 packs。生效指纹保存在 `rules-revision`，包括 `cprs-library-3` 运行时修订，同一映射卷禁止静默切换规则或遍历语义。变更规则需要独立的新映射卷及新会话，并保留旧卷以便回退。页内规则不可编辑，不支持清空映射或关闭 Claude 隐私处理。

`initialized` 现在保存原子发布的随机存储代次；旧空标记、缺失标记或库、规则修订不一致均拒绝 Claude 数据面。
本次不自动迁移或删除旧卷。需要启用新版遍历语义时，保留原卷、配置全新映射目录并开始新会话。
恢复备份须配套恢复主密钥、映射库、`initialized` 与 `rules-revision`；重建或回滚时须使旧 Thread 过期并重新创建，不能接着发送增量。

映射按认证后的内部 Client Key ID 隔离，上游账号轮换及 Key 密钥轮换不会改变映射空间。诊断和复审使用独立派生空间。全卷最多 100 万条映射，满额拒绝新增，不执行 TTL/LRU 自动清理。图片、二进制附件、thinking 和签名按原生协议保持不透明，文本规则不代表这些内容已经过敏感内容扫描

## 公网入口与客户端

CPRS 只监听本机或受限容器网络，PostgreSQL 和 Redis 不直接暴露公网。默认可使用同一域名：Codex 访问 `/v1/*`，Claude 访问 `/claude/v1/*`，Nginx 原样转发路径即可，由 CPRS 的原生协议路由分流。客户端配置中的 Claude Base URL 为 `https://网关域名/claude`，不追加 `/v1`。

需要两域名分流时，可使用 [Nginx 模板](nginx.conf.example)。该模板将 Codex 域名与 Claude 域名隔离，是独立域名部署方式，不用于同域名路径分流。

Claude 域名只开放 Messages、count_tokens 和模型目录，将 /v1 前缀转发为 CPRS 的 /claude/v1。CPRS 自己校验原始鉴权头、权限、限流和并发，内嵌处理失败无直连回退。保持流式缓冲与代理缓存关闭

无需在 CPRS 设置里配置公网入口。Key 配置弹窗默认按当前站点生成 `/claude` 地址；使用上述独立域名模板时，把复制出的 `ANTHROPIC_BASE_URL` 改为 `https://claude.example.com`，此时不加 `/claude` 或 `/v1`。这只影响客户端配置，不修改服务端监听或 Nginx 路由。已有保存的独立域名仍会作为导出的默认地址。

导出的是 Claude Code 的 `settings.json`，CLI 与 VS Code 扩展共用。配置启用网关模型发现和提示头，不固定模型，也不维护陈旧模型家族常量。开发环境支持本地 HTTP 地址。

模型可用性来自账号的可信原生目录及 Key 权限，未开放的模型明确拒绝，不猜测替换。OAuth、profile、额度和目录使用同一受管传输与 veil 就绪门禁。
Claude 账号、额度和模型目录与其他供应商共用管理页面，按供应商筛选即可查看；客户端通过 `/claude/v1/models` 获取该 Key 可用的模型。Claude 账号不参与 CPR 互联共享，账号预热和额度上限自动暂停是否可用，以界面上显示的供应商能力为准

客户端身份通过现有设置/Key 画像配置；自动发布同步需要访问 `downloads.claude.ai`，失败保留最后成功版本。
metadata.user_id 重建为所选账号 UUID、派生设备 ID 和显式/新生成会话；额外文本仍经过 veil。
Thread 在成功终态前提交 Redis 检查点，已有 Thread 使用冻结身份，新 Thread 使用当前发布版本；不提供透明 tunnel。

## Prompt 审查

「Prompt 审查」是独立页面，用于查看概览、审查记录、Key 画像和会话锁，并管理策略与学习候选。策略可配置：

- 模式：observe（只记录）或 enforce（执行处置）
- 本地命中的处置：告警或阻断
- 严格规则终止
- 可选的外部复审节点，支持 Chat Completions 或 Moderations 接口。复审节点使用独立的 Key 池，不占用订阅 OAuth 账号

计分时同一类别内去重；纯信号、历史和观察类别不会直接抬高当前用户消息的决策分。普通命中按本地策略处置；开启严格终止后，严格规则单独累计分数，达到严格阈值即直接终止，不再调用外部复审

Chat Completions 复审会附带内容来源标记；Moderations 只评估当前用户文本，并按分类阈值判断。Claude 请求送去外部复审前同样先经过 veil 脱敏。observe 模式只记录拟执行的处置并放行请求，veil 脱敏照常生效

被上游拒绝的请求会以脱敏证据进入学习队列，可据此生成规则草稿。草稿必须人工发布，生成后不会自动生效；Moderations 模式下生成草稿，还需配置同一服务商的对话模型作为学习模型。复审范围、故障处置、会话锁和字段约束见 [Prompt 审查接口](../../docs/api.md#prompt-审查)

审查记录附在根请求观察数据中，沿用请求保留期限与有界写队列。页面显示无摘要记录与进程内共享队列丢弃/写入失败计数，缺失不等于通过。摘要没有原始文本、匹配片段或审核模型自由文本理由，复审成本不计入 Claude 用量。来源与许可见 [Prompt 审查 NOTICE](../../backend/crates/gateway-host/src/prompt_guard/NOTICE.md)

## 备份与验收

管理端 S3/R2 业务备份会在数据库导出后自动生成映射卷的一致快照（`.veil.tar.gz`，含 mappings.db、initialized、rules-revision 与自定义规则），与数据库归档同属一条记录；快照失败时整条备份失败。主密钥**不进入**备份，需与部署配置一样单独加密保管，恢复步骤见 [Claude 隐私存储恢复](../README.md#claude-隐私存储恢复)。自行做文件级备份时，先停止 CPRS 或使用 SQLite 一致备份方式，不能只复制正在写入的主数据库文件

关闭 `host.logging.request_dump` 和 `host.logging.oauth_recovery`。恢复缺失、容量满或断流明确失败，不向客户端交付未恢复占位符

当前没有授权测试账号，OAuth、原生流式工具往返、signed thinking、多轮会话、取消及实际模型权限尚无真实运行验收。源码编译与类型检查不替代这些验收，Docker、Nginx 和数据库迁移也需在部署环境验证
