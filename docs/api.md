# Codex Proxy RS 接口

本文是 CPRS 的 HTTP 接口参考，路由以 `backend/crates/gateway-api/src` 中的 router 为准。按读者分两部分：

- **客户端与第三方集成**：用管理端创建的 Client Key 调用 `/v1/*`（Responses、Chat Completions、Images、模型列表、额度查询）、`/claude/v1/*`（Anthropic Messages）和 CPR 互联协议。看 [§1 数据面鉴权](#openai-数据面客户端接口)、[§2 健康检查](#2-健康检查)和 [§3 数据面接口](#3-openai-数据面与模型目录)。只想把 Codex 或 Claude Code 接上网关，直接看[部署文档：客户端配置](../deploy/README.md#客户端配置)即可
- **管理端接口**（§4 起）：`/api/admin/*`、`/api/auth/*` 等，供管理端前端和自动化脚本使用，按字段逐条描述行为合同，适合修改代码时核对

快速验证一个 Key 能用哪些模型：

```bash
curl https://gateway.example.com/v1/models -H 'Authorization: Bearer <client-api-key>'
```

内部职责见[系统架构](architecture.md)；部署实例的实际功能以其运行版本为准

| 查找内容 | 入口 |
| --- | --- |
| 鉴权与响应 | [公共约定](#1-鉴权与公共约定) · [浏览器会话与 Key 自助页](#4-浏览器认证) |
| 客户端协议 | [Responses、Chat、Images、模型与 Provider 操作](#3-openai-数据面与模型目录) · [额度查询](#api-key-额度查询) |
| 账号与授权范围 | [账号](#5-账号) · [代理](#独立代理管理--managed-proxies) · [分组](#6-账号分组) · [Client Key](#7-client-key) |
| 系统与运维 | [健康检查](#2-健康检查) · [运行设置](#8-运行设置) · [备份](#9-备份) · [统计与诊断](#10-dashboard用量与错误) · [版本与更新](#11-版本更新与重启) |

## 1. 鉴权与公共约定

### OpenAI 数据面客户端接口

默认情况下，`/v1/*` 使用管理端创建的 Client Key：

```http
Authorization: Bearer sk_...
```

自动生成的 Key 保持 `sk_` 格式；迁入的自定义 Key 使用保存时的原值，不限制前缀或固定长度。
无论格式如何，只有已保存且启用的 Client Key 能通过鉴权

Codex 原生生图配置还会携带 `X-OpenAI-Actor-Authorization: proxy-managed`。
它仅用于客户端识别服务端托管认证，不能代替 Client Key。网关和 OpenAI Provider 都会过滤该请求头，
上游账号身份只由服务端选中的账号提供；不要把真实账号 token 放进该标记

Client Key 通过账号分组限定路由范围：未绑定分组时可使用全部账号，绑定一个或多个分组时只能使用
已启用分组成员的并集。分组可以混合 `openai` 与 `anthropic` 账号；同一请求只会在模型能力明确匹配且满足
重放安全边界时跨 Provider fallback

运行设置可以分别配置 `minCodexDesktopVersion` 与 `minCodexCliVersion`。两者只接受 SemVer，`null`
表示不限制，比较版本时忽略 `+` 后的构建元数据。API 在 Client Key 鉴权成功后识别官方客户端请求头；
适用门禁的客户端没有合法版本，或版本低于对应门槛时，除只读 `/v1/usage`、`/v1/cpr/link` 外的 `/v1/*` HTTP 请求和新 WebSocket
握手在访问上游前返回 `400 Bad Request`，按不可重试的请求错误处理。
未知客户端保持兼容，不应用版本门禁

Claude 原生入口使用独立的 `minClaudeCodeVersion`，同样接受 SemVer，`null` 表示不限。
在 Client Key 鉴权后、出站身份伪装前，按原始 `User-Agent` 的 `claude-cli/<版本>` 产品段
校验 Claude Code；低版本或已识别但版本无效时返回 Anthropic 格式的 `400 invalid_request_error`。
CLI 和 IDE 共用该下限，普通 SDK 等未识别为 Claude Code 的客户端不受此门禁影响。
`anthropic-version` 是 API 协议版本，不作为客户端版本；上游身份自动同步也不改变此检查。

`minCodexCliVersion` 检查官方客户端 User-Agent 产品段中的 Core 版本，支持
`codex_cli_rs`、`codex-cli`、`codex-tui`、`codex_exec`、`codex_vscode`、`codex_sdk_ts`、
`codex_python_sdk`、`codex-app-server`、`codex-app-server-sdk` 与 `codex_mcp_server`
名称按完整标识匹配且不区分大小写，括号后缀中的应用或 SDK 版本不能替代 Core 版本

Desktop 应用版本优先取 `version` 头，未提供时取 User-Agent 中的 `(Codex Desktop; <版本>)`。
ChatGPT 远程控制使用 `(codex_chatgpt_<平台>_remote; <版本>)` 形式的 User-Agent 后缀，已知平台包括
`android` 和 `ios`。网关按该命名格式识别非空的平台名，后缀须完整，平台名和版本均不能含空白、括号或分号。
这类 Desktop 请求未提供应用版本时不应用版本门禁；Core 版本和
远程客户端版本不能替代 Desktop 应用版本，因此也无法保证其满足 Desktop 最低版本要求。
携带 `version` 头或 Desktop 应用版本后缀时仍按上述规则校验，非法版本不会因远程标记而放行

低版本响应使用 OpenAI 风格错误格式：

```json
{
  "error": {
    "message": "Codex CLI 0.151.0 is outdated. Upgrade to 0.152.0 or later.",
    "type": "invalid_request_error",
    "code": "client_version_too_old",
    "client": "codex_cli",
    "current_version": "0.151.0",
    "min_version": "0.152.0"
  }
}
```

因缺失或非法版本被门禁拒绝时，`code` 为 `client_version_unavailable`，`current_version` 为 `null`，
提示检查客户端版本及反向代理是否保留 `User-Agent` 和 `version` 头

### 管理接口

所有 `/api/admin/*` 请求都需要以下任一鉴权方式：

- 管理员登录后得到的 `cpr_session` Cookie（服务端会话身份必须为 `admin`）
- `x-api-key: <admin-api-key>`

同源管理端的登录和退出根据浏览器 `Origin` 自动设置会话 Cookie：HTTP 来源省略 `Secure`，
HTTPS 来源以及缺失、`null` 或非法来源保留 `Secure`。`HttpOnly`、`SameSite=Lax` 始终保留。
不根据 `X-Forwarded-Proto` 等转发头降级；HTTPS 反代使用 HTTP 回源不影响 Cookie。
部署要求见 [公网访问](../deploy/README.md#公网访问)

请求无需自带 `x-request-id`；缺失时服务端自动生成 UUID 并在响应头回传同一 request ID。
`api.request_id_header` 可改变注入与回传的 header 名，管理端鉴权不依赖该名字。
管理端 JSON 与动态响应统一带 `Cache-Control: no-store`

管理端长度已知且至少 1 KiB 的 `application/json` 响应按 `Accept-Encoding` 协商 gzip，
压缩时返回 `Content-Encoding: gzip`，可协商的响应带 `Vary: accept-encoding`。
小响应、SSE、其他流式正文及已有内容编码的响应不再压缩；模型执行接口不经过管理端压缩层

配置了 CORS 白名单 origin 时，跨域请求以凭据模式放行，仅允许 `GET`/`POST` 方法和
`authorization`、`content-type`、`x-api-key` 与 request ID 四个请求头，不使用通配符

普通成功响应使用以下信封：

```json
{
  "code": 200,
  "message": "OK",
  "data": {}
}
```

所有 `/api/admin/*` 错误（包括 JSON/Query rejection、未知路由和错误 HTTP method）统一返回
`application/json`：

```json
{
  "code": 40001,
  "message": "请求参数不合法",
  "data": null
}
```

管理端本地产生的 `message` 是可安全展示的中文文案，不包含内部异常或原始上游正文。稳定业务码如下：

| HTTP | `code` | 含义 |
| ---: | ---: | --- |
| 400 | `40000` | 请求体不是合法 JSON |
| 400 / 405 / 415 / 422 | `40001` | 通用请求、方法、Content-Type 或字段错误；HTTP 状态保留具体语义 |
| 400 | `40002` | 时间范围不合法 |
| 401 | `40101` / `40102` / `40103` | 会话缺失、过期或吊销 / 登录凭据错误 / 管理 API Key 错误 |
| 403 | `40301` | 有效会话的身份无权访问目标接口；不清除会话 |
| 404 | `40401` | 资源或管理接口不存在 |
| 409 | `40901` | 资源状态冲突 |
| 429 | `42901` | 登录尝试过多 |
| 429 | `42902` | 管理操作请求过于频繁；安全文案可说明具体受限操作 |
| 500 | `50001` | 服务内部错误 |
| 502 | `50201` | 上游服务请求失败 |
| 502 | `50202` | 不可逆上游操作的执行结果未知；刷新状态后再决定是否重试 |
| 503 | `50301` | 依赖服务暂不可用 |

未知 `/api/admin/*` 路径使用 `40401`，不会落入 SPA；已存在路径使用错误 method 时返回 `405`、
`40001`，并保留标准 `Allow` header。request ID 继续通过配置的响应 header 返回

`50201`、`50202` 和 `50301` 的 `message` 提供可安全展示的具体原因；缺少安全消息时使用通用提示。
认证错误和未知内部错误使用固定文案，不公开 Provider 内部 message、原始响应或凭据

手动刷新令牌时，按错误类型处理：

| `code` | 场景 | 调用方处理 |
| --- | --- | --- |
| `40901` | 刷新容量、账号租约占用或账号快照冲突 | 按提示等待，或重新查询账号后操作 |
| `40001` | 缺少或无效的凭据、已确认刷新令牌失效或账号停用 | 检查凭据或重新授权 |
| `50201` | 明确的上游失败，且未归类为凭据永久失效 | 根据安全消息处理上游拒绝、限流或服务异常 |
| `50202` | 无法确认令牌轮换结果，包括 Claude 刷新响应无法确认 | 先核对账号状态，不要自动重发可能已轮换的一次性令牌 |
| `50301` | 刷新服务或依赖暂不可用 | 检查出站连接与依赖服务后重试 |

OpenAI 上游 `401` 按 `50201` 返回，不代表管理员会话失效，也不要求管理端重新登录。
Codex PAT 验证服务不可用和身份响应无效分别返回 `50301`、`50201` 及对应的安全提示

### 页面时间合同

管理端与 Key 用量接口保留原始 RFC3339 时间点，并提供 `createdAtDisplay`、`updatedAtDisplay`、
`expiresAtDisplay` 等展示字段。展示文本由后端按部署 `host.timezone` 生成，完整时间通常为
`YYYY-MM-DD HH:mm:ss`，短时间、相对时间和图表标签按对应视图返回；缺失时间的展示字段为 `null` 或占位文本。
调用者直接显示文本，使用原始时间点进行排序、比较和输入，不把展示文本转回请求时间

账号、模型用量和 Key 列表的最近使用时间由前端根据原始 RFC3339 时间点，通过 VueUse `useTimeAgoIntl` 按界面语言显示短相对时间，并随时间更新；悬停的完整日期仍使用后端 `lastUsedAtFullDisplay`，不使用浏览器本地时区重算

Dashboard 的 `lastUsedRelative` 和账号额度的 `refreshedAtRelative` 由后端确定时间语义，
分别以查询锚点和服务端当前时间计算，随接口刷新更新。前端只翻译分类与单位，不使用浏览器时钟或时区重算

- 缺失时间返回 `null`，前端显示占位文本
- `kind=just_now` 表示不足 60 秒
- `kind=elapsed` 包含整数 `value` 和 `unit=minute|hour|day`，分钟不足 60、小时不足 24，之后始终用天；数值向下取整
- `kind=absolute` 的 `value` 是后端按部署 `host.timezone` 生成的完整日期文本，用于按整秒判断的未来时间点

原始 RFC3339 `lastUsedAt`、`quota.refreshedAt` 与兼容展示字段保持可用，不用于前端重新决定上述展示方式

账号冷却返回 RFC3339 `rateLimitedUntil` 和 `rateLimitRecoveryDisplay`，后者包含剩余时长或到期时间，
随账号数据刷新；前端不解析展示文本或自行计算恢复提示

`period=today|7d|30d` 表达自然日范围，`asOf` 是 Unix 毫秒查询锚点，省略时取服务端当前时刻。
近 7 天包含锚点所在日期及前 6 个自然日，近 30 天同理包含前 29 日；结束点为锚点。
同一组汇总、趋势与列表共享 `asOf`，分页保持锚点，刷新再更新。显式 `startTime` / `endTime`
支持带偏移的 RFC3339 时间点或 `YYYY-MM-DD` 日期，不能与 `period` 或 `asOf` 混用；
日期由服务端按 `host.timezone` 换算日界，范围始终为左闭右开 `[startTime, endTime)`，对应接口仍执行范围上限校验。
管理端日期选择器按版本接口返回的 `timezone` 显示业务日期；历史结束日传次日日期，当天结束点截到查询时刻，
不依赖浏览器或容器系统时区，也不以固定 24 小时推算夏令时日界。
自然日刚开始时允许空的今日快照，显式起止范围仍要求开始早于结束

图表的 `label`、日期提示和空桶由后端提供；日粒度按本地日界，小时及 15 分钟桶以 UTC 时间点定位。
账号请求柱覆盖截至锚点的最近 24 个 UTC 小时桶，包含当前未完整小时，独立于今日汇总的自然日范围。
健康时间线覆盖锚点所在自然日，每 15 分钟一个桶，夏令时日期可以为 92 或 100 个桶。
图表时间标签不显示时区偏移，重复本地时刻可以使用相同展示文本；唯一键、排序和去重使用原始时间点。
只有日历日期的上游统计保留日期语义，不当作 UTC 午夜换算

### 管理写入一致性

会改变路由快照或安全配置的写入由后端在事务内推进 `config_revision`，用于快照发布与审计。
账号、分组等响应中的 `configRevision` 表示已提交事实，不是这些接口的写入前置条件。
[运行设置整体更新](#8-运行设置)必须提交读取时的 `configRevision`，过期返回 `409`；
代理的 `revision` 属于该资源，按对应接口提交，不与全局版本混用

## 2. 健康检查

| 方法 | 路由 | 鉴权 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/readyz` | 无 | 流量就绪检查。运行时快照可用且 PostgreSQL、Redis 探针健康时返回 `204`，否则返回 `503` |
| `GET` | `/healthz` | 无 | 综合健康检查。在就绪依赖之外检查文件日志和关键后台任务，全部健康时返回 `204`，否则返回 `503` |

两个接口均返回空正文。探针并行执行，共用 2 秒截止时间

## 3. OpenAI 数据面与模型目录

除下述入站解压保护外，Responses、Chat Completions、Images 和 standalone Search HTTP body、
WebSocket message 和 frame 不设置网关私有长度上限；请求字段与协议限制见各入口说明

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `POST` | `/v1/responses` | OpenAI Responses JSON；`stream=true` 返回 SSE，否则返回完整 JSON |
| `GET` | `/v1/responses` | 通过 HTTP Upgrade 建立 Responses WebSocket |
| `POST` | `/v1/chat/completions` | Chat Completions 转 Responses 执行；返回单 choice 的 Chat JSON 或 SSE |
| `POST` | `/v1/alpha/search` | Codex standalone web search；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/generations` | 通过 OpenAI Provider 发起图像生成；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/edits` | 通过 OpenAI Provider 发起图像编辑；JSON 原样转发，multipart 上传转为原生 Images JSON |
| `GET` | `/v1/models` | 返回当前 Client Key 账号范围内各 Provider 的可用公开模型并集；有两种响应形态，见下 |
| `GET` | `/v1/models/{model_id}` | 返回 OpenAI 兼容的单模型详情 |
| `POST` | `/v1/providers/{provider}/models/{model}/count_tokens` | 调用指定 Provider/model 声明的精确 Token 计数；请求和响应均为 JSON |
| `GET` / `POST` | `/v1/providers/{provider}/http/{endpoint}` | 调用指定 Provider 预先声明的固定 HTTP 操作；不是任意 URL 代理 |
| `GET` | `/v1/usage` | 查询当前 Client Key 的日与周额度，仅使用网关已结算的 USD 账本 |
| `GET` | `/v1/cpr/link` | 返回当前 Key 获准公开的预算及直连账号池观测 |

### Chat Completions 请求

`POST /v1/chat/completions` 接受 Chat 消息历史，转换为 Responses 请求执行，再交付 Chat 格式结果。
每次请求须提供非空字符串 `model` 和非空数组 `messages`，多轮对话由客户端携带完整历史；
没有 Chat 查询、删除或服务端历史续接接口。`stream` 省略、`null` 或 `false` 时返回 JSON，`true` 时返回 SSE

官方 OpenAI SDK 可使用 Chat Completions 方法，将 `base_url` 指向网关的 `/v1`。
下面的 `CPR_CLIENT_KEY` 为管理端创建的 Client Key，`CPR_MODEL` 为当前 Key 可用的模型 ID，可通过 `/v1/models` 查询：

```python
import os
from openai import OpenAI

client = OpenAI(
    base_url="https://gateway.example.com/v1",
    api_key=os.environ["CPR_CLIENT_KEY"],
)
completion = client.chat.completions.create(
    model=os.environ["CPR_MODEL"],
    messages=[{"role": "user", "content": "你好"}],
)
print(completion.choices[0].message.content)
```

#### 请求字段

入口负责将 Chat 结构映射为 Responses；同语义字段的模型支持范围、取值范围与业务有效性由 Provider 和上游判断。
只有结构无法解码或不能正确表达的语义在本地返回 `400`，错误中的 `param` 指向字段，不回显字段值

| 字段 | 支持与映射 |
| --- | --- |
| `messages[].role` | `system`、`developer`、`user`、`assistant`、`tool`，保持顺序；assistant 文本历史转为 `output_text` |
| `messages[].content` | 字符串或内容块数组；user 和 tool 可包含图片、文件与音频，assistant 支持文本与拒绝内容 |
| `image_url` 内容块 | HTTP(S) URL 或非空 `data:image/...;base64,...`，保留 `detail`，网关不代取图片 |
| `file` 内容块 | `file.file_id`、`file.file_data`、`file.file_url` 至少提供一种，保留可选 `filename`；URL 和非 PDF 文件为网关的 Responses 扩展，实际支持由上游判断 |
| `input_audio` 内容块 | Chat 的 `data` 和 `format` 转为 Responses `audio_url` data URL；原生音频引用与 `input_text`、`input_image`、`input_file` 内容块也可携带 |
| `messages[].refusal`、assistant 的 `refusal` 内容块 | 保留拒绝历史；assistant 同时有工具调用或拒绝内容时，`content` 可省略或为 `null` |
| `messages[].tool_calls`、`messages[].tool_call_id` | function 和 custom 调用分别映射到对应 Responses 调用及结果；保留 ID、参数或输入字符串，不解析或执行工具；custom 结果须携带前序调用历史 |
| `messages[].name` | tool 消息的工具名随结果携带；其他角色的名称作为辅助信息忽略 |
| `tools` | 展开 function/custom 定义，保留 schema、format 及原生工具对象；function 的 `strict` 省略或 `null` 时使用 `false` |
| `tool_choice`、`parallel_tool_calls` | 展开指定 function/custom 和 `allowed_tools` 选择；原生选择与并行布尔值交给上游 |
| `web_search_options` | `{}` 即要求上游执行 `web_search`；支持 `search_context_size` 与 `user_location.approximate` |
| `response_format` | 映射到 `text.format`，保留 JSON Schema；未知格式交给上游 |
| `reasoning_effort`、`verbosity` | 覆盖 `reasoning.effort`、`text.verbosity`，不固定枚举档位 |
| `max_completion_tokens`、`max_tokens`、`max_output_tokens` | 映射到 `max_output_tokens`，按本行从左到右的顺序优先 |
| `temperature`、`top_p` | 原值进入 Provider；是否生效见下文正文兼容规则 |
| `user`、`safety_identifier`、`service_tier`、`prompt_cache_key`、`prompt_cache_retention` | 同语义字段保留原值 |
| `metadata`、`prompt_cache_options`、`moderation` | 同结构对象原样携带，不递归清理业务字段 |
| `instructions`、`reasoning`、`text`、`include`、`max_tool_calls`、`truncation`、`context_management` | Responses 原生扩展，保留原值；`reasoning`、`text` 须为对象 |
| `logprobs`、`top_logprobs` | `logprobs: true` 请求输出文本 logprobs，并携带 `top_logprobs`；返回值位于 Chat 的 `choices[].logprobs` |
| 内容块的 `prompt_cache_breakpoint` | 对象原样携带；assistant 输出消息不支持此选项 |

可选映射参数为 `null` 时按未提供处理。`stop`、`seed`、`frequency_penalty`、`presence_penalty`、
`logit_bias` 和 `prediction` 没有对应的 Responses 控制语义，接受后忽略，不保证生效。
消息的 `reasoning_content` 和 `reasoning` 作为辅助历史忽略，不转为用户正文

本地保留以下限制：`n` 只能为 `1`，`store` 和 `background` 只能为 `false`，
`modalities` 只能为 `["text"]`，旧式 `functions` 只能为空数组；这些字段也可省略或为 `null`。
非空旧式函数定义、`function_call`、旧式 `function` 角色、音频输出以及无法映射的内容类型会被拒绝。
入口不接收 `input`、`previous_response_id` 或 `conversation`；需要服务端续接时使用 `/v1/responses`

未知客户端附加字段在协议包装对象中忽略，不据名称自动转交 Responses；schema、metadata、工具参数和
已识别的原生对象内部不做递归过滤。新工具声明可交给上游，但返回的业务输出仍须具有明确的 Chat 映射

#### 响应与共用规则

非流式响应包含一个 `choices` 项，支持文本、拒绝及 function/custom 工具调用；流式响应交付对应的 Chat delta。
两种响应的 `id` 均由网关生成，`model` 保留客户端请求的模型 ID；流内各块使用同一身份。
正常完成的 `finish_reason` 为 `stop`，包含工具调用时为 `tool_calls`；上游明确因输出长度或内容过滤而未完成时，
分别为 `length` 或 `content_filter`

联网搜索使用请求指定的模型，能否搜索取决于上游。搜索请求不接受非空 `tools`，`tool_choice` 可省略或使用 `auto`。
需要由模型在搜索和其他工具之间选择时使用 `/v1/responses`。非流式结果的 URL 引用位于 `choices[0].message.annotations`，
索引对应合并后的消息正文。流式结果保留正文中的行内链接，并在结束前通过 `delta.annotations` 交付结构化引用，
索引对应实际交付的合并正文

`stream_options` 仅在 `stream: true` 时可提供，
`include_usage: true` 在完整上游用量可用时于结束前追加 `choices: []` 的用量块，其余块的 `usage` 为 `null`。
未提供完整用量时不补造 Token 数；缺省不启用流长度混淆，`include_obfuscation: true` 不受支持，`false` 可接受

响应对象的额外普通属性、非必要事件不影响已有内容映射；无法表达的业务输出、缺少必要终态或终态冲突会失败，
不补造成功终态。SSE 已开始后通过 `data: {"error": ...}` 交付错误，再以 `[DONE]` 结束，
因此 `[DONE]` 仅表示流结束，不能单独作为成功依据。
OpenAI 是此入口的兼容目标，混合分组沿用账号范围内的选路与 fallback 规则，
不能据此推导所有 Provider 都具备相同的 Chat 能力

公开的推理摘要和推理文本投影到 `message.reasoning_content` / `delta.reasoning_content`；
推理和上下文压缩的 `encrypted_content` 不作为可读内容返回，密文回放应使用原生 Responses。实际推理用量与上游 Token 明细保留

原生 `image_generation` 工具的图片结果通过网关扩展 `message.images` / `delta.images` 返回，
每项为 `type: "image_url"` 和包含 data URL 的 `image_url` 对象；流中带图片 `index`，预览与最终结果可能更新同一项。
这是 Chat 扩展，需要客户端支持；音频输出和需要客户端执行的其他未映射动作不受支持

Chat 沿用 [Client Key 鉴权](#openai-数据面客户端接口)、[请求期限](#请求期限)、[请求解压](#请求解压)、
[请求头与身份](#请求头与身份)及[隐私策略](#隐私策略)。隐私策略的 `request_body` 面向转换后的 Responses 正文，
例如 `$.input`，不需要另外维护一套针对 `messages` 的规则

转换后的请求按选中的 Provider 执行。OpenAI 的字段处理见[正文兼容](#正文兼容)；
输出上限原值交给上游校验，低于模型下限时会收到上游错误

### Images 请求

Images 使用原生非流式 JSON 端点，JSON 请求与响应正文原样转发。
`/v1/images/edits` 还接受 `multipart/form-data`，将上传转换为原生 Images JSON 请求

multipart 的 `image` 和 `image[]` 上传按出现顺序组成 `images` 数组，每项为
`{"image_url":"data:image/...;base64,..."}`；`mask` 上传转换为同结构的单个对象。
图片支持 PNG、JPEG、WebP，根据内容识别格式，原始文件字节直接编码，不缩放或重新编码

| 表单字段 | JSON 类型 |
| --- | --- |
| `n`、`output_compression`、`partial_images` | JSON 数字，不经浮点数中转 |
| `stream` | JSON 布尔值 `true` 或 `false`；转换不增加流式响应支持 |
| 其余文本字段，包括未知字段 | 字符串，保留字段名、空白与原值 |

未提供的参数保持缺省；除 `image` / `image[]` 聚合外，其余同名字段按出现顺序保留为重复 JSON 键，
解释与校验交给上游。
缺少图片、损坏的 multipart、无效的 UTF-8 或字段类型返回 `400`。
需要为扩展字段指定其他 JSON 类型，或传入 `file_id` 等图片引用时，使用 JSON 请求

### Provider 自有操作

这两类 `/v1/providers/*` 路由使用同一 Client Key 鉴权、账号组范围、Provider 账号资格、租约、
并发/频率准入、请求记录和发送事实。`provider`、`model` 与 `endpoint` 都是已发布目录中的稳定 ID；
宿主不会接受客户端提供的上游 URL，也不会把下游 Authorization、Cookie、API Key、Host、转发头或
`Connection` 声明的逐跳头交给目标 Provider。原生执行的上游账号认证来自宿主选中的账号

`POST /v1/providers/{provider}/models/{model}/count_tokens` 接受最多 8 MiB 的合法 JSON，且只路由到
为该模型明确声明 `count_tokens` 的 Provider。Core 不解释或重写请求/响应 JSON，响应必须是目标 Provider 按其
声明的本地精确 tokenizer 或上游精确计数合同返回的原始 JSON；没有准确实现时返回不支持，不从 usage
或字符数推算。响应固定使用 `application/json` 并重算 `Content-Length`

`GET|POST /v1/providers/{provider}/http/{endpoint}` 只允许 Provider descriptor 中该 endpoint 声明的
方法；`HEAD`、其他方法、未声明 endpoint 和未声明的方法在出站前拒绝。GET 不能携带正文，POST 正文
最多 8 MiB；query 作为不透明值交给目标 Provider 再按其固定合同校验，不能改变上游 origin 或路径。
成功响应由最多 64 KiB 的有序原始片段组成，聚合后最多 8 MiB，字节不经 JSON 转码。宿主保留允许的
`Content-Type` 和安全业务响应头，移除认证、`Set-Cookie`、`Location`、逐跳、压缩及 framing 头，
并按最终正文重算 `Content-Length`；缺少媒体类型时使用 `application/octet-stream`。Provider 的结构化
失败仍沿用数据面错误映射和已确认的上游状态/可见正文边界，不把非 2xx 成功封套当作成功

Codex 的 review 等子代理请求仍使用 `/v1/responses`，并通过 `x-openai-subagent` 请求头携带子代理类型；
网关不提供独立的子代理请求路径

### Responses 请求与传输

#### 请求期限

模型请求默认不限制总执行时长。排队受[并发等待设置](#8-运行设置)约束，上游传输仍有独立的连接、空闲与心跳超时；
持续有输出的请求不会仅因总时长达到固定值而结束。

`api.request_timeout_seconds` 控制 HTTP 路由处理到返回响应的等待，默认 `null`；不限制已返回响应的流式正文寿命，
也不是 WebSocket 每轮模型执行的总时限。客户端断开或取消仍会结束所属执行，反向代理和客户端可另设超时

#### 请求解压

`POST /v1/responses` 与 `POST /v1/chat/completions` 在鉴权后按 `Content-Encoding` 解压，再解析 JSON；支持单一 `gzip`、
`deflate`（zlib 封装）和 `zstd`，缺省、空值或 `identity` 直接使用原始正文。gzip 多成员与 zstd
多帧连续解码，整体展开结果受运行设置 `responsesMaxDecompressedBodyBytes` 约束（默认 64 MiB），
超限在继续展开前返回 `400 request_too_large`，错误信息包含当前请求的上限字节数。zstd
回溯窗口独立固定为最多 64 MiB，不能满足该限制的帧按解码失败处理。这个限制保护入站解压资源，不是
模型上下文或 Token 上限；未压缩正文同样受该大小限制，见[请求解压上限](#请求解压上限)。
不支持的编码、逗号分隔的叠加编码和重复 `Content-Encoding` 头返回
`400 unsupported_content_encoding`；压缩正文损坏、截断或解压后不是合法 JSON 返回
`400 invalid_json`。本地错误不包含原始正文或解压库细节。WebSocket 文本帧不经过这条解压路径

#### 请求头与身份

Responses 不透传下游的逐跳头、反代元数据（如 `cf-*`、`x-forwarded-*`、`forwarded`、`via`、
`cdn-loop`）以及 `Accept-Encoding` / `Content-Encoding`。链路元数据和编解码能力
由各段传输层独立管理；其余业务扩展头继续透传，不使用固定业务头白名单。
此规则同时适用于上游 HTTP 和 WebSocket，不影响上游响应的 `cf-ray` 等诊断信息。
API Key 与 OAuth 共用模拟客户端画像（`User-Agent`、`originator`、`version`）和业务头透传规则，
包括会话、线程、Lite 和其他业务扩展头。
上游认证只来自选中的账号；API Key 不携带 OAuth Cookie、ChatGPT 账号身份或下游的
`X-OpenAI-Actor-Authorization` 托管认证声明。
`x-codex-installation-id` 在 Key 配置种子时，由种子、Key ID 与选中账号 ID 稳定派生；未配置时使用账号已持久化的安装 ID，与正文 `client_metadata` 中的安装身份一致
`x-openai-internal-codex-residency` 和 `x-openai-account-routing-override` 分别由部署配置
`openai.residency`、`openai.account_routing_override` 控制，配置值原样发送并覆盖客户端同名头，
省略或 `null` 时删除对应请求头。配置说明见 [部署与运维](../deploy/README.md#启动后的运行设置)
`x-openai-fedramp` 默认按业务头透传并保留原始多值，Responses 明确换号时清理。

Responses 也不透传 `x-stainless-*`、`Origin`、`Referer`、`sec-ch-ua*` 和 `sec-fetch-*`
携带的下游 SDK/浏览器环境或页面来源。过滤规则适用于所有下游客户端，与 User-Agent 无关；
原始入站头仍供 CORS、鉴权和本地观测使用。
`session_id` 请求头仅作为入站会话别名，上游通过 `session-id` 表达；
两者同时存在时仍优先使用 `session-id`。正文中的 `client_metadata.session_id`、
`prompt_cache_key` 不受这条请求头规则影响。
`thread-id`、turn metadata 等 Codex 协议字段及未知业务扩展不受下游环境头过滤规则影响；
Responses 明确换号时清理 `traceparent`、`tracestate`、`sentry-trace`、`baggage` 和
`x-openai-fedramp`，同时清理正文、`client_metadata`、turn metadata 中的同名字段及
`ws_request_header_*` 副本。同账号及归属未知时保留原值，业务输入和工具结果保持原始内容

会话、线程、窗口及父子线程引用在出站时按本次安装身份稳定映射为 UUID；
默认使用账号安装身份；Key 配置种子时，每个 Key × 账号组合拥有独立的安装身份，换账号会改变映射。HTTP 请求头、正文 metadata、WS 副本及
已识别的 Desktop 自动 Git 上下文使用一致映射。
轮次及父子轮次引用保留客户端原值，包括输入项的 `internal_chat_message_metadata_passthrough.turn_id`、
请求 metadata 中的 `turn_id`、`parent_turn_id`、`root_turn_id`，以及 `x-client-turn-id`、
Images 的 `x-codex-image-turn-id` 和 Desktop 自动 Git 上下文的 `clientUserMessageId`。
输入项元数据保持不变。Search 的会话 `id` 按同一安装身份域映射，只替换该值的原始字节，其余正文保持不变。
`prompt_cache_key` 同样按安装身份映射，保留同一身份内的缓存共享关系；未提供时不补造。
续接 response ID、历史 item/call ID、业务正文和未知扩展不参与映射，
该处理减少显式追踪标识的跨账号复用，不保证会话内容或业务 metadata 无法关联

`x-codex-turn-state` 是上游返回的轮次路由 token，同一轮内应原样回传，开始新轮次时清除。
全量请求没有 `previous_response_id` 或缺少本地归属记录时，代理原样保留客户端回传值；
明确换账号或可信续接状态表明不属于同一轮时清除。该 token 不使其他账号状态字段或未知 `previous_response_id` 成为可信续接上下文

#### 正文兼容

OpenAI Provider 的 Responses 上游编码会移除 Codex 不接受的顶层 `temperature` 和 `prompt_cache_retention`，
并保留调用方指定的 `max_output_tokens`。缺少顶层 `store` 时补齐 `false`，与官方 Codex 客户端一致；
显式提供的值保持原样。顶层 `input` 为字符串时按公开 Responses API 的语义展开为一条
`user` 文本消息条目（`{"type": "message", "role": "user", "content": [{"type": "input_text", ...}]}`），
因为 Codex 后端只接受条目数组；数组及其他类型原样透传。`input` 数组中显式指定 `type: "message"`
且 `role: "system"` 的消息，其角色转换为 Codex 接受的 `developer`；消息内容、顺序、其他字段和顶层
`instructions` 保持不变。HTTP/SSE 与 WebSocket 共用这条正文兼容规则。
`prompt_cache_key`、`reasoning`、`include` 等 Codex 参数继续保留。上述参数过滤只作用于顶层，
不删除工具参数 schema、输入内容或 `client_metadata` 内的同名业务字段；其他未知字段继续透传。
`prompt_cache_key` 不用于补造会话或线程请求头；客户端未提供缓存键、会话或线程身份时保持缺省

发往 Codex/OAuth 的 Responses 请求另按下表处理；API Key 上游保留这些字段。规则只匹配表中列出的位置

| 请求字段 | Codex/OAuth 出站行为 |
| --- | --- |
| `top_p`、`frequency_penalty`、`presence_penalty`、`chat_template_kwargs`、`user`、`metadata`、`safety_identifier` | 省略；采样参数取默认值时也省略 |
| `stream_options.include_usage` | 省略该成员，保留其他流式选项 |
| `truncation`、`stop_sequences`、`prompt_cache_options` | 分别仅在值为 `"disabled"`、`[]`、`{}` 时省略；显式策略交给上游校验 |
| `max_completion_tokens`、`max_tokens` | 转为 `max_output_tokens`。原生字段优先，其次按左列顺序取首个非 `null` 值 |

Codex/OAuth 上游的历史回填按字段形状兼容，不以 User-Agent 品牌区分：显式 `type: "reasoning"`
的 `input` 项移除顶层 `status`；该项具有非空字符串 `encrypted_content` 时，还会移除非空数组
`content`。其他字段及顺序保持不变，缺少非空加密载荷的明文历史由上游判定。
普通消息、工具项及未知类型不受此规则影响，API Key 上游不应用此规则

请求头过滤不提供客户端匿名化；系统提示词、工具定义、工具结果、工作目录及其他业务 metadata
保持原有语义，可能包含客户端环境信息。
环境正文中的工作区路径、shell 和文件权限信息不会因位置覆盖而隐藏。
turn metadata 的 `workspaces` 也会保留仓库绝对路径、Git 远端地址、提交及工作区变更状态；
安装身份和账号绑定字段的处理不提供这些信息的脱敏

`client_metadata.parent_response_id` 是 Guardian 的账号内响应引用，只有归属可信且仍为同一账号时保留；
切号或归属未知时移除。`x-codex-guardian`、`guardian_credits_requested` 和序列化
`x-codex-turn-metadata` 内普通扩展的同名 `parent_response_id` 保持原样

同一 Client Key、同一 OAuth 账号的 Guardian 请求可利用网关记录的父响应 WS 握手信息，
尽力复用父请求的上游路由。路由 Cookie 仅在网关与上游之间使用，不传给客户端；
网关重启、记录淘汰或 Cookie 到期后仍可按普通路由请求，不保证上游路由亲和

#### WebSocket 与上游传输

Responses WebSocket 接受文本消息，`response.create` 在同一连接串行执行。当前响应期间收到的后续创建请求
留在有界接收队列中，待当前响应完成终结和写出后再逐条校验、准入与执行，不因请求提前到达而断开。
接收队列容量为 32 个事件，超载仍关闭连接；Ping/Pong、客户端关闭和服务关闭不等待队列中的请求执行

网关分别维护两段链路的心跳：上游 WS 定时发送 Ping 保活并回复 Pong，下游 WS 独立探活；Ping/Pong 不跨段转发，也不转换为 SSE 事件。
下游探活包含消息接收中的字节进度，完整消息收齐后才进入中间件与请求处理。
客户端断开时取消当前执行，并释放仍归该客户端所有的上游连接。`response.completed` 结束本轮响应。通常保留客户端 WS 等待下一轮请求，大包回退后的重连规则见下文

| 入站 → 上游 | 正常回答结束后的连接处理 |
| --- | --- |
| SSE → SSE | 结束本次响应流，底层 HTTP 连接由 HTTP 连接池管理 |
| SSE → WS | 结束 SSE，上游 WS 归池，按无客户端绑定的空闲期限回收 |
| WS → SSE | 上游本次响应结束，下游 WS 通常保留；大包回退可触发正常重连 |
| WS → WS | 保留当前绑定的上游 WS，支持下一轮续接 |

活跃客户端 WS 的绑定不受普通空闲回收期限约束；上游 Close（不限关闭码）、EOF、传输读写失败、总寿命到期或连接被回收仍可能使连接内续接失效，
网关在收到绑定上游的失效通知且下游空闲时，以关闭码 1000 要求客户端重连并发送完整输入。
终态后的结算先完成，已接收或正在处理的客户端消息先按原合同执行；启用 WS 帧中间件时保留连接。
续接请求与失效通知同时到达时，仍按 `previous_response_not_found` 合同完整重放。
具体心跳和回收参数见[部署与运维](../deploy/README.md#公网访问)

Codex OAuth 上游 WS 首包未声明搜索时，后续新增 `web_search` 声明需要新连接，非生成预热也属于首包。
HTTP/SSE 与 WS 入站共用此判断；新链在发送前重新建连，依赖原连接的增量请求返回
`400 previous_response_not_found`，由客户端携带完整历史重试。网关保留请求中的搜索配置；
已初始化搜索的连接可以继续复用，临时移除搜索或从文本搜索增加图片搜索不触发此回收。

其余文本消息发送到当前执行绑定的上游 WebSocket，未知类型、模式和扩展字段由上游校验。
`response.interrupt` 的 `discard_partial_items` 模式若指向当前逻辑请求已交付的响应 ID，
网关会保留中断意图；重试交付新的 `response.created` 后，以新 ID 转发中断，避免换号空档丢失控制消息。
跨账号缺少原连接上下文时，仍通过恢复错误要求客户端完整重放

控制帧不重新选号、不创建推理 attempt。响应结束后原连接仍可接收控制帧并返回上游事件；
下一轮执行使用新绑定的控制入口。没有绑定上游控制通道时（包括实际走 HTTP），有效的
`response.interrupt` / `discard_partial_items` 请求返回带原 `response_id` 的
`response.interrupt.failed`，当前模型响应继续交付真实事件与终态，客户端可收完后再续接。
该事件只表示网关无法转发中断，不表示上游已停止生成或丢弃部分输出。
未知控制消息、未知模式或无效响应 ID 字段在无控制通道时仍返回 `400`。
除上述重试期间保留中断的情况外，已绑定的原连接失效或发送失败仍返回 `400`，不能通过控制帧新建上游连接

中断后继续转发上游事件与终态；只有 `response.incomplete` 的 `incomplete_details.reason` 为
`interrupted` 时，才按官方中断语义保留原连接续接能力。部分输出是否被丢弃以上游终态为准。
控制帧发送成功不等于上游已确认中断；若上游仍返回 `response.completed`，按正常完成处理

OAuth 账号默认 `prefer_websocket`，客户端使用 HTTP/SSE 时仍可能选择上游 WebSocket；
可将账号上游传输方式设为 `http`，固定使用 HTTP/SSE。API Key 账号默认使用 HTTP/SSE，
也可配置 `prefer_websocket`。必须依赖 WS 的协议预热、非持久化新链和连接内续接不使用 HTTP-only 账号。
客户端配置的 `supports_websockets` 只控制第一段连接，不是服务端传输策略开关。
上游在响应终态前发送 Close 1000 仍属于失败，不能按“正常关闭”计为成功

Codex OAuth backend 的候选上游为 WS 时，无 `previous_response_id` 的普通新链若规范化
`response.create` 达到 15.5 MiB，发送前选择 HTTP/SSE；HTTP 和 WS 入站均适用。能关联到客户端线程时，
后续完整新链沿用 HTTP/SSE，直到帧长低于 13.5 MiB 再尝试 WS。预热和增量帧不改变此大小判断。
该阈值为已观察到的上游消息大小边界预留余量，不是网关输入长度上限或 OpenAI 公布的统一限制；
API Key 账号、小请求、显式 warmup、未知外部 previous ID 和持久化续接不因该大小策略改用 HTTP

OAuth 的大 connection-local WS 续接，以及 HTTP `store=false` 成功后标记为 `ReplayRequired`
的 WS 增量，均在发送前返回 `status: 400 / previous_response_not_found`。客户端应清除旧 ID，
携带完整历史及工具调用／输出重试。官方 Codex 支持重连 WS 后完整重发，也可能按自身重试预算
切到 HTTP；其他客户端需要自行实现此合同。代理不缓存 transcript，不把增量输入当作独立新链，
连接内续接失去原连接时仍需客户端恢复

因大小走 HTTP/SSE 且需要完整重放时，网关写出成功终态后，会以关闭码 1000 正常关闭空闲的下游 WS。
此规则适用于所有客户端。下一步请求应在新连接上发送完整输入，省略 `previous_response_id`。
已接收的后续请求按序处理，若增量先到，仍按上述 400 合同恢复。启用 WS 帧中间件时保留连接

已建立模型执行的 Responses、Images 和 Search HTTP 响应按以下规则返回关联 ID：
`x-gateway-request-id` 为模型执行 ID；`x-request-id` 保留有效上游值，只有上游
`x-oai-request-id` 时复用其值，没有上游 ID 时使用模型执行 ID。`x-oai-request-id` 不是必需字段，
也不要求客户端识别它。失败响应的关联 ID 不采用会话 opening ID，
错误正文读取失败时仍返回已知上游 ID；已采集的 turn state 等允许的会话头继续按原合同交付。
尚未建立执行的入口拒绝继续使用 middleware 的入口关联

WebSocket 在尚未交付上游业务事件时合成的错误，除下述容量恢复合同外，保留已确认的失败状态，
以及 Provider 提取的结构化 message/type/code；没有结构化错误时使用稳定安全文案，不把原始
HTML 或截断正文当作 message。
合成错误自身的 `headers` 携带允许下发的响应头：优先保留实际失败的上游 request ID，无上游 ID 时
提供网关关联 ID，并用 `x-gateway-request-id` 独立标识网关请求。除下述客户端错误兼容与原生续写额度恢复外，
已经取得的原始上游错误帧不重写

### 模型目录

`GET /v1/models` 默认返回 OpenAI 兼容列表 `{"object": "list", "data": [...]}`；请求携带非空
`client_version` query 参数（Codex 客户端）时改为返回 Codex 专用目录合同 `{"models": [...]}`

OpenAI Provider 按客户端传入的 `client_version` 请求上游目录，完整保留每个模型 JSON 对象，包括
`base_instructions`、`model_messages`、`service_tiers`、工具与能力字段，以及未知嵌套字段、显式 `null`
和字段缺失的区别。
API Key 上游返回完整 Codex `models` 目录时沿用该合同；仅返回普通 `data` 模型列表时使用通用画像，
未提供的推理能力保持未知，不补充推理档位。已知的专用生图、Live／Realtime 和语音合成模型以 `visibility: "hide"`
保留在 Codex 目录中，且不出现在默认选模列表。其他 ID 保持可见；此规则不影响普通模型列表或直接调用。
模型别名仅替换 `slug`，不替换上游展示名、提示词、能力或 `priority`；保持原生模型顺序，新增别名附在后面。
目录按当前路由快照的模型存在性及账号模型政策过滤，避免公布已知无法路由的模型；新模型需待后台目录对账后进入列表。

已知模型只路由到明确提供该模型的 Provider，账号不可用时不转发给其他平台的未知模型通道。
未知模型只在当前范围与路由约束下存在唯一 Provider 透传候选时发送；无法确定归属时返回不可用错误，
可通过 Client Key 分组限定目标平台。Provider 在选号时检查账号模型政策与目录资格，
具体边界见[账号模型限制](#账号模型限制)

目录账号只能来自本次 Client Key 冻结的账号范围。OpenAI 的 OAuth 目录按账号 ID 排序，使用首个成功读取的
合格账号，最多尝试三个账号；API Key 目录按账号模型权限过滤后聚合。同名模型优先采用 OAuth 原生对象，
其次采用 API Key 的完整 Codex 对象，再使用普通模型 ID；同类 API Key 目录按账号 ID 确定来源，不跨账号
或套餐混拼字段。因此单账号、无别名时可保持该账号的模型对象一致，多账号/多 Provider 聚合不代表
“与某个官方账号的整个目录完全一致”，也不会固定后续推理账号。
读取失败返回 `503 model_catalog_unavailable`，不以简化模板或空成功响应覆盖客户端缓存。
成功获取的 API Key 目录被账号模型政策全部过滤时返回空列表，区别于读取失败。
目录保留完整的原生 Codex 模型对象；OpenAI API Key 普通模型列表使用通用画像。

原生目录可能返回缓存结果，成功缓存有效期为 5 分钟。每次查询都检查账号资格和 Client Key 范围。
成功目录响应带 `Cache-Control: private, no-store`，不借用上游 ETag 标识经过选择/别名/聚合后的正文

`service_tiers` 的 `name` 用于生成 `/fast` 等命令，`id` 是请求使用的 `service_tier` 值，二者不能互换。
上游未声明或明确返回空数组时不补档位，也不根据模型名推断 Fast。
该合同对齐
[官方 Codex 0.154.0 的模型元数据](https://github.com/openai/codex/blob/6b9826e3aa83b1a5947db50f4332cb9c65f1b340/codex-rs/protocol/src/openai_models.rs)
与其动态服务档位命令；客户端需重新加载模型目录才能使用更新后的档位信息

Codex 专用目录中的 `context_window` 与 `max_context_window` 分别表示默认上下文窗口和客户端本地
覆盖的上限。OpenAI Provider 原样保留上游对应字段，缺失与 `null` 不互相转换；网关不通过部署配置
覆盖这些值。Codex 客户端配置 `model_context_window` 后，按该值与非空 `max_context_window` 的较小值
使用窗口；上限为空时保留客户端本地值

### 透传与错误恢复

#### OpenAI 透传

OpenAI 路径保留客户端 Responses wire 语义：除[正文兼容](#正文兼容)、[身份归一](#请求头与身份)、模型映射及已配置的请求策略外，
请求 body 的未知字段和字段顺序保持不变。HTTP SSE 与 WebSocket 的上游业务事件除下述客户端错误兼容及响应头隔离外按原始字节转发，
response ID 按 opaque 值处理而不假设 UUID 或固定长度；除客户端错误兼容与原生续写额度恢复外，
OpenAI 上游错误 envelope 和允许下发的 opaque header 值也不由 canonical 观测结果重写。
`reasoning.effort` 的数字和未知扩展值保留给上游判断，本地观测不限制参数形态。
`response.metadata` 的业务内容继续交付，仅其中的响应头沿用凭据、账号身份和逐跳头隔离规则。
WebSocket 文本不依赖 JSON 旁路解析成功；原始 SSE 心跳和无法提取事实的内容不构成协议失败。
Images 固定使用 OpenAI Provider，不重建正文或映射模型字段；Provider 从原始 JSON 旁路提取图片模型，
按[账号模型限制](#账号模型限制)选号。账号选择、鉴权头替换和端点路由
不改变请求正文，成功与非容量失败响应正文保持原始字节。

Images 排除已知为 Free 套餐的 OAuth 账号；套餐缺失或未知时不推断为 Free，API Key 账号不套用该套餐规则。
其余账号仍需满足模型限制、额度等调度条件，实际生图权限由上游决定

`/v1/alpha/search` 使用相同的 OpenAI Provider 原生端点边界：模型不映射，正文除会话 `id` 的账号归一及显式请求策略外原样转发，
Provider 旁路读取请求中的 `model`，按[账号模型限制](#账号模型限制)选号。
`x-codex-turn-metadata` 在移除客户端账号身份并按选中账号重写 installation ID 后转发；上游账号
Authorization、Cookie、account ID、originator 和 User-Agent 均由代理安全重建。
Images 与 Search 保留通过现有过滤规则的普通业务请求头及多值字节；Responses 原连接的 turn state
不随独立端点转发，turn metadata 仍由所属端点按当前账号处理。
模型映射是全局精确映射，未命中时模型名原样交给候选 Provider；分组只限定账号集合，不参与模型改名

严格亲和的后代线程因绑定账号不具备当前模型权限而等待超时，错误码仍为 `concurrency_queue_timeout`，消息说明模型限制。只有账号属于当前授权范围时才区分该原因，其余资格缺失使用一般不可用提示。

#### 容量拒绝与重试

没有 HTTP 状态码且明确携带 `invalid_request_error` 的流内拒绝归为请求错误；容量、Flex 和轮次限流的专用分类优先。
非流式返回请求错误，SSE/WS 保留消息和关联 ID；不因此换号、冷却账号或计入账号故障评分。

OpenAI HTTP 明确未发送的瞬时建连失败（含 TLS 握手 EOF）在单请求内共享最多 4 次 opening、30 秒预算；WS 瞬时建连失败也可为替代账号预留建连机会。
只有整个请求尚未发送、无续接依赖且 Provider 明确允许时才能建连换号；同账号恢复不消耗换号次数，跨账号选择仍受总尝试次数与换号上限约束。
先在原账号恢复；允许换号且至少剩余两次 opening 时，为后续选号保留时间和一次 opening。WS 重试或降级 HTTP 仍由既有传输策略决定。
确认连接账号代理的 TCP 入口失败时，本次请求按规范化主机与端口排除所有共用该入口的账号。TLS 证书、代理认证、SOCKS/CONNECT 握手拒绝不证明入口不可达。
排除不持久化、不输出代理凭据；候选全部使用故障入口时保留原错误结束，不回退该入口。其他建连失败选号不可用时仍可使用原账号剩余预算。
固定账号、禁止重试、已交付输出、取消与截止时间约束继续生效。Claude 保留既有传输禁止重试、OAuth 恢复和 veil 门禁规则

客户端推理请求提供有效逻辑线程、轮次及 `request_kind=turn` 时，网关按 Client Key 隔离短期失败状态。
“同账号重试后返回错误”策略终态失败后，5 分钟内同轮次再次命中该策略时直接返回错误，不重复执行其最多 5 次重试；
再次失败刷新有效期，成功结束后解除限制。其他重试策略不受影响，缺少有效身份、状态过期或状态存储不可用时沿用原策略。
这是一项轮次重试预算控制，不是客户端重试意图的精确检测；状态与交付顺序见[数据面请求生命周期](architecture.md#4-数据面请求生命周期)

`turn_rate_limit_exceeded` 保留原始错误和 `Retry-After`，由客户端按自身重试预算等待。
代理不因此写入账号冷却、换号或透明重试，其他会话仍可使用该账号

上游 WS `error` 转为 SSE `response.failed` 时，事件级的有效 `Retry-After` 同时放入
`response.error.headers`，供客户端读取；错误对象已有有效重试头时优先保留

Responses 的 HTTP/SSE 与 WebSocket 按顺序即时交付正常事件，不暂存前导等待后续输出。
未启用中间件链时，无输出的 `response.created`、`response.in_progress`
及 `codex.response.metadata`、`codex.rate_limits` 元数据交付后仍可按已有安全条件重试，
客户端可能收到下一次尝试的新 `response.created`。
其他事件也立即交付，但会关闭透明重试窗口，后续前导事件不再恢复重试资格。
中间件链不启用可替换前导例外；错误事件先按既有重试规则处理，无法重试时交付错误

等待沿用[请求期限](#请求期限)与传输超时配置。
WebSocket 独立 Ping 保活；Responses HTTP 流式请求完成准入后，执行仍在等待且连续 15 秒无输出时提交 `200`
和 SSE 注释心跳，此后的等待继续每 15 秒保活，包括请求中间件等待和上游尚未返回事件。
首次心跳前仍保留最终 HTTP 状态与响应头；心跳发出后发生的失败使用 `response.failed` 和 `[DONE]`，
后续取得的上游响应头无法再补发，`Retry-After` 转入流内错误的 `response.error.headers`。
心跳不代表业务输出，不关闭已有的安全重试窗口。
SSE 注释和 WebSocket Ping/Pong 不重置官方 Codex 客户端的事件空闲超时，
其 `stream_idle_timeout_ms` 默认是 300 秒；长请求需持续收到真实事件，
或同时调整客户端与上游传输的空闲等待预算，单独缩短心跳间隔不能延长业务等待

WebSocket 在交付有效输出前断线或超时，可按 `stream_max_retries` 做同账号、有退避的尽力重试
（默认最多 5 次），耗尽后仅在请求允许时改用 HTTP。已发送或发送结果不明不单独阻止这项恢复；
次数限制不保证上游只执行一次，诊断保留原始发送状态。精确连接续接、显式终止拒绝、已观测的
外部副作用和下游提交仍限制恢复；取消与请求截止时间随时终止等待

OpenAI 仅在允许安全重放且尚未交付不可重放输出时执行下表的同账号重试；次数指首次尝试之外的额外尝试：

| 上游拒绝 | 同账号重试上限 | 没有有效 `Retry-After` 时的退避 | 耗尽动作 |
| --- | --- | --- | --- |
| `server_is_overloaded`、`slow_down` 或明确模型容量不足 | 3 次 | 500ms 起步的指数退避，单次最多 8 秒 | 按调度规则换号 |
| 普通 `503` 且 `error.code=server_error` | 5 次 | 500ms 起步的指数退避，基准最多 2 秒，每次追加 `[50ms, 250ms)` 等待 | 返回最后一次上游错误，不因此换号 |

有效 `Retry-After` 优先，接受秒数、HTTP 日期和零延迟，不受本地退避上限截断，也不追加随机等待。
流内失败事件使用相同解析规则，WS 同账号传输重试与转 HTTP 回退也遵守服务器建议；等待受请求截止时间和取消约束，
重试受[请求总尝试次数](#账号亲和与换号)约束。

`maxRetryAfterSeconds` 限制网关接受的单次服务器建议等待，默认 240 秒，取值 0～4,294,967,295。
建议时间超过上限时跳过同账号等待，按原有耗尽动作安全换号或返回错误；无可用账号时返回最后一次上游错误，
容量恢复投影清除旧账号的 `Retry-After` 和 `retry-after-ms`，普通限流仍保留等待建议。等于上限仍可等待，0 表示不接受正数等待；未提供有效建议时继续使用本地退避。
该设置不限制请求总时长；续接仅可按下文的绑定迁移合同要求客户端重放，仍受已交付输出和换号次数限制。保存后对新请求生效。

已发送的容量失败计入[智能调度健康分](architecture.md#智能调度)，可用账号的会话亲和优先。
容量不足不触发 Provider 全局熔断，也不作为账号额度耗尽；明确的上游限流、额度及认证故障由各 Provider 的调度策略处理。
客户端错误兼容由 API 编码出口统一处理：最终交付的 `server_is_overloaded`、`slow_down` 错误码
投影为 `server_error`，HTTP 错误状态及 WS 包装错误的数字状态投影为 `503`，让客户端执行自己的
有界重试。Provider 已确认容量不足的初始失败，即使没有这两个错误码，也返回 `503`。
SSE/WS 的 `response.failed` 保留原消息、响应 ID 与其他业务字段；客户端无法消费的裸 `error`
继续按现有规则投影为 `response.failed`。容量恢复投影清除实际响应头及错误帧内嵌头中的 `Retry-After`、`retry-after-ms`；
普通限流与 Claude 原生等待提示不受影响。内部上游状态、错误码、原始事件及计量事实保持不变；已开始输出的请求由
客户端决定如何恢复，代理不因此重放已提交的请求。
`flex_unavailable` 是当前请求的终止拒绝，保留原始错误，不自动重试、切换传输或冷却账号

#### 额度耗尽与续写

明确额度耗尽触发账号隔离与安全换号，包括 WebSocket 握手返回的 429；不会因其长 `Retry-After` 而转入同账号传输恢复等待

Client Key 日周预算超限返回 `429`，`error.type` 为官方额度耗尽类型 `usage_limit_reached`，
`error.code` 分别保留 `key_daily_budget_exceeded` 与 `key_weekly_budget_exceeded`，`Retry-After` 指向预算重置边界。
下游按官方错误类型确认额度耗尽，由互联预算同步确认恢复，不写普通限流冷却；普通 429 限流仍独立处理

OpenAI 选号阶段确认本次可选账号全部额度耗尽时，HTTP 返回 `429`，WebSocket 错误帧返回
`status: 429`，两者的 `error.type` 与 `error.code` 均为 `usage_limit_reached`，提示客户端停止
本轮自动重试，等待额度恢复或补充可用账号。空账号池、认证失效和临时容量不足不按额度耗尽处理；
仍有其他可用 Provider 或可安全恢复的续写时，网关先按现有路由规则尝试恢复

带 `previous_response_id` 的 OpenAI 原生续写仍绑定原账号。若该账号明确拒绝请求且额度已耗尽，
并且请求可安全重放、尚无语义输出且未提交下游，网关隔离该账号，对客户端返回 HTTP `400`
（WebSocket 为 `status: 400`）及 `previous_response_not_found`，不附带额度窗口的 `Retry-After`。
支持该恢复协议的客户端应去掉 `previous_response_id`、携带完整历史重试，由正常调度选择可用账号；
官方 Codex 的 WebSocket 客户端支持这一流程。其他客户端需要自行处理，网关不会跨账号发送原增量输入。
原生续接的明确容量拒绝仅在安全重放证明成立、尚未交付语义输出、发送结果明确、建议等待超过 `maxRetryAfterSeconds` 且允许换号时进入绑定迁移。
网关在授权范围内选择替代账号，以原子准入更新真实会话绑定后返回同一 `400 / previous_response_not_found` 恢复指令。优先亲和在此路径迁移主绑定，严格亲和的后代仍只跟随根绑定。
迁移不发送旧账号的 `previous_response_id` 或增量输入，不新增生成请求或 Token 费用；内部保留原账号的容量分类、发送状态、上游请求 ID 和计量事实，客户端收到当前失败的关联 ID。
无真实会话身份、宽松亲和、无替代账号或预算耗尽时保留原容量失败；取消、超时和存储失败不报告迁移成功，选号租约按既有生命周期释放。
普通限流、发送结果不明以及已经交付语义输出的失败不触发此转换

### API Key 额度查询

`GET /v1/usage` 使用同一数据面入口认证，默认传入 `Authorization: Bearer <Client Key>`；
不接受会话 Cookie、管理 API Key 或查询参数。
只返回当前 Key 的日与周额度，不包含明文 Key、账号资料或其他 Key 的数据。查询本身不执行模型推理、扣费或占用推理并发/RPM，
不更新最近使用时间或开启预算窗口；额度耗尽后仍可查询。已配置的入口认证和 request 中间件仍按授权执行

成功响应直接返回以下 JSON，不使用管理接口信封，所有响应带 `Cache-Control: no-store`：

```json
{
  "unit": "USD",
  "daily": { "total": "1", "used": "0.640001", "remaining": "0.359999", "resetsAt": "2026-09-21T16:00:00Z" },
  "weekly": { "total": "5", "used": "2.35", "remaining": "2.65", "resetsAt": "2026-09-27T16:00:00Z" }
}
```

金额使用十进制字符串，`total` 为当前周期限额，`used` 为该周期已结算金额，`remaining` 为限额减已用且最低为零。
不限额时 `total`、`remaining` 均为 `null`，仍返回已用金额。`resetsAt` 为 RFC3339 窗口结束时间。
原生窗口尚未开启或已到期时，`resetsAt` 为 `null`，`used` 为 `"0"`；显式窗口到期后仍返回已结算金额和结束时间。
原生窗口按部署时区的自然日或首次使用起的七个本地日历日划分，显式窗口采用已保存的起止边界。
修改限额、管理员重置和费用结算均复用现有 Key 账本，不从请求日志重算余额

缺失、非法、已禁用或已删除的 Key 返回 OpenAI 风格 `401` 错误；未知查询参数返回 `400 invalid_usage_query`，
读取账本失败返回 `503 usage_unavailable`，不会用零余额掩盖故障

### CPR 互联只读协议

`GET {api_base}/cpr/link` 使用推理 Key 查询当前授权范围内的只读快照，不接受查询参数。
响应直接返回 JSON，所有状态均带 `Cache-Control: no-store`。第三方项目可独立实现此 HTTP 协议

#### 版本与可选能力

| 顶层必填字段 | 合同 |
| --- | --- |
| `object` | 固定为 `"cpr.peer"` |
| `schema_version` | 固定为 `3`，接入端只接受此版本 |
| `capabilities` | 当前密钥 获准公开的能力名称列表，最多 32 项；未知能力可以忽略 |
| `observed_at` | 本次快照组装时间，RFC3339 UTC 时间 |
| `expires_at` | 快照到期时间，必须晚于 `observed_at`，且不超过其后 30 分钟 |

`usage`、`key_limits`、`pool`、`quota`、`models` 按能力独立提供，未提供的数据块直接省略。
接入端只消费已声明能力的数据，缺失值保持未知，不解释为零余额、空池或额度恢复。
未知字段可以忽略；没有独立节点 ID，连接由本地账号及其凭据、出口版本识别

| 能力 | 对应数据 |
| --- | --- |
| `key_usage` | `usage`：当前认证 Key 的预算访问结论与可选金额窗口 |
| `key_limits` | `key_limits`：当前认证密钥公开的并发上限和 RPM |
| `pool_summary` | `pool`：账号数量、互斥状态计数及可选套餐分布 |
| `quota_windows` | `quota.accounts`：匿名账号及各自额度窗口 |
| `model_availability` | `models`：按当前 Key 公开的模型额度汇总，可独立于账号明细提供 |

提供请求限制和预算访问结论的响应示例：

```json
{
  "object": "cpr.peer",
  "schema_version": 3,
  "key_limits": { "max_concurrency": 5, "requests_per_minute": 60 },
  "capabilities": ["key_usage", "key_limits"],
  "observed_at": "2026-11-01T12:00:00Z",
  "expires_at": "2026-11-01T12:05:00Z",
  "usage": {
    "status": "available",
    "observed_at": "2026-11-01T12:00:00Z",
    "data": { "allowed": true }
  }
}
```

#### 密钥请求限制

`key_limits` 形如 `{"max_concurrency":5,"requests_per_minute":60}`。上游可省略整个对象或其中任一项。已提供的值为非负整数，零表示该项不限；缺失表示未知。
限制适用于整个密钥，有效期随顶层快照

#### Key 预算

`usage.status` 为 `available`、`unavailable` 或 `unknown`，未知状态按 `unknown` 处理。
`available` 表示数据可用，不表示预算尚有余额；没有可用数据时可省略 `data`。
`usage.observed_at` 是预算的原始观测时间，缺失或过期时不能据此改变账号资格

`usage.data.allowed` 是上游预算账本给出的访问结论，`false` 表示金额耗尽或当前不在允许消费的窗口内。
接入端以明确的 `allowed` 为准；缺失时，有效窗口中的 `limit_reached: true` 只能证明耗尽，不能据此推断恢复。
金额只供展示，不从 `total`、`used` 或 `remaining` 计算账号资格

需要公开金额时，在 `usage.data` 中提供 `unit` 和 `windows`：

```json
{
  "unit": "USD",
  "allowed": true,
  "windows": [
    {
      "key": "monthly",
      "label": "月额度",
      "window_seconds": 2592000,
      "limit_reached": false,
      "total": "100",
      "used": "20",
      "remaining": "80",
      "reset_at": "2026-11-30T12:00:00Z"
    }
  ]
}
```

- `windows` 只包含实际提供的周期，可以省略或为空；不要求提供日、周窗口
- 每个窗口的 `key` 必填且在该预算中唯一，使用稳定的机器标识；`label` 可选，省略时展示 `key`
- 提供金额窗口时 `unit` 必填，表示统一计费单位，如 `USD`；仅提供 `allowed` 时可以省略
- `used` 是必填的非负十进制字符串，`total` 与 `remaining` 使用相同格式；不限额时两者为 `null`
- `window_seconds` 是可选的正整数，表示本次周期的秒数；`reset_at` 是可选 RFC3339 时间。仅当两者均有效时，接入端可按窗口边界关联本机用量
- `limit_reached` 可省略，省略表示没有明确的窗口耗尽结论；重置时间已过意味着观测过期，不意味着预算已恢复

#### 账号池、额度与模型

`pool` 必须包含 `account_count` 与 `states`。`states` 包含 `ready`、`cooldown`、`exhausted`、`disabled`、`error`、`unknown` 六项非负整数，
各项互斥且总和等于 `account_count`。可选 `observed_at` 表示池观测时间；可选 `plans: [{ name, count }]` 表示套餐分布，
套餐总数不能超过账号数，不返回账号与套餐的逐项对应关系

`quota.accounts` 保留匿名账号及各自窗口。账号示例：

```json
{
  "id": "df306c3718c99d3615352e41d32751d3",
  "models": ["gpt-5.4"],
  "observed_at": "2026-11-01T11:58:00Z",
  "expires_at": "2026-11-01T12:28:00Z",
  "windows": [{ "key": "weekly", "window_seconds": 604800, "used_percent": 47, "reset_at": "2026-11-07T16:00:00Z", "limit_reached": false }]
}
```

账号的 `id`、`models`、`windows` 必填，匿名 `id` 在同一 Key 下稳定，不能当作接入端的本地账号 ID。
观测时间可省略，但缺失或过期时不能参与调度判断。返回结果不含 Key、邮箱、账号名称、代理或凭据正文

| 账号窗口字段 | 合同 |
| --- | --- |
| `key` | 必填，额度桶内稳定的窗口标识 |
| `group` | 可选的分组标识，省略时为 `general` |
| `limit_id` | 可选的额度桶标识；同账号的窗口按 `limit_id` 分桶，缺失时按 `group` 分桶 |
| `label`、`limit_name` | 可选的窗口与额度桶展示名；窗口名省略时展示 `key` |
| `role` | 可选展示角色：`primary`、`secondary`、`monthly`，未知值不指定角色 |
| `window_seconds`、`used_percent`、`reset_at` | 可选的周期秒数、已用百分比（0～100）与 RFC3339 重置时间 |
| `limit_reached` | 是否明确触顶，省略时不标记触顶 |

不同账号或周期的百分比不能相加，不能推算 Token 总额，也不能用本地请求用量填充上游额度

`models` 条目形如 `{ "model": "gpt-5.4", "coverage": "complete", "state": "available", "observed_at": "...", "expires_at": "..." }`。
`coverage` 为 `complete` 或 `partial`，`state` 为 `available`、`exhausted` 或 `unknown`。
只有该模型所有候选的资格与额度都明确、有效且全部耗尽，才能给出 `complete + exhausted`。
它是通用模型额度参考，不承诺 Images、Live 等操作的额外资格或即时容量；缺失、过期或部分覆盖保持中性，
请求仍需执行当前节点的正常授权与调度

#### 有效期与数量限制

顶层快照默认有效期 5 分钟，CPR 接入端仅将最近 5 分钟组装且未到期的快照用于访问与调度判断。
预算观测有效期为 5 分钟；账号与模型可提供各自的 `expires_at`，省略时按观测后 5 分钟处理，最长不得超过 30 分钟。
各组件保留原始观测时间，并受重置边界约束，不因查询或转发而更新；新的顶层时间不能延长旧观测

接口不分页，不截断账号集后声称完整。账号最多 3000 个，预算窗口和账号额度窗口分别最多 6000 个，
模型最多 2000 个，响应最大 4 MiB。窗口标识、展示名和额度桶标识不得为空或包含控制字符，最多 256 字节；
`group` 最多 128 字节，`unit` 最多 16 字节。可选字段没有有效值时省略

#### CPR 内置共享规则

CPR 默认路径为 `/v1/cpr/link`，使用同一数据面入口认证，不接受 Cookie 或管理 API Key。
只有当前 Key 显式开启 `peerSharing.enabled` 且绑定非空分组时可查询。
查询不执行推理、占用推理并发/RPM、更新最近使用时间或刷新上游；预算耗尽后仍可查询

| 共享配置 | 导出内容 |
| --- | --- |
| `enabled` | `key_limits` 与 `key_usage`，包含密钥请求限制、明确访问结论及本地日、周美元账本；金额与周期口径见 [API Key 额度查询](#api-key-额度查询) |
| `sharePool` | `pool_summary` |
| `sharePlans` | 已公开账号池中的 `pool.plans` |
| `shareQuota` | `quota_windows` 与 `model_availability` |

套餐分布沿用账号页口径：优先使用账号已有的明确套餐，缺失时从已保存的额度快照补全，无需开启 `shareQuota`。
仍未知的账号只计入账号总数，不推断为 Free

共享范围同时受当前 Key 的公开开关、分组及冻结权限限制，只包含 Provider 明确声明的直连 OAuth 账号。
互联 Key 的推理也使用该直连范围，不把 API Key 转接账号再次作为共享池，不递归查询其他节点。
这些开关和账号组织方式属于 CPR 内置实现；第三方通过自身授权规则生成协议数据

缺失、非法、禁用或删除的 Key 返回 `401`；未开启共享或没有显式分组返回 `403 peer_sharing_disabled`；
未知参数返回 `400 invalid_peer_query`；数量或响应大小超限、本地读取失败返回 `503 peer_snapshot_unavailable`。
每个进程对每个认证 Key 独立限制为每分钟 30 次，涉及账号池、额度或模型聚合的查询最多同时执行 8 个。
满额时返回 `429 peer_query_rate_limited` 和 `Retry-After`，聚合槽位满时等待秒数为 `1`；仅查询 Key 预算不占用聚合槽位。
上述限制均不消耗推理请求配额

### Codex Live 语音通话

语音通话复用数据面 Client Key 鉴权，并按该 Key 的账号分组选路。通话引导是一次性请求，走与其他
`/v1/*` 数据面请求相同的调度、准入与请求记录；通话建立后的 sideband 与 hangup 使用引导时选中的
账号（call id 与账号、创建 Key 绑定），音频流不经过网关。

| 路由 | 说明 |
| --- | --- |
| `POST /v1/live` | WebRTC SDP 引导；`POST /v1/realtime` 与 `POST /v1/realtime/calls` 为同一处理的别名，成功响应的 `Location` 会改写为 `/v1/realtime/calls/{call_id}` |
| `GET /v1/live/{call_id}` | 通话 sideband WebSocket，双向透传事件帧 |
| `GET /v1/realtime/calls/{call_id}` | 同上的 realtime 路径形态 |
| `GET /v1/realtime?call_id=…` | 同上的查询参数形态 |
| `POST /v1/realtime/calls/{call_id}/hangup` | 用钉住账号转发挂断，成功后解除绑定 |

引导请求接受三种入口形态并统一为上游要求的 `{"sdp": …, "session"?: …}` JSON：JSON 保留原有字段、
`application/sdp` / `text/plain` 包装为 `sdp` 字段、`multipart/form-data` 取 `sdp` 与 `session` 两个部分。
`session.model` 或顶层 `model` 中的 `gpt-realtime` 系模型名归一为 `gpt-live-1-codex`，其余模型原样透传；
模型字段缺失时按默认语音模型参与检查。归一后的模型参与账号模型权限检查：账号范围整体禁止该模型时
引导请求按路由错误拒绝，被禁账号不会服务语音通话

引导、sideband 与 hangup 的业务扩展头默认透传；认证、账号身份、下游环境与传输头遵守
[请求头与身份](#请求头与身份)边界。`X-Session-Id`、`Voice-Session-Id` 是 Live 业务会话标识，原样透传。
`Session-Id`、`Thread-Id` 等已识别追踪标识按选中账号的安装身份稳定映射，同账号各阶段保持一致，
换账号使用不同标识，客户端 trace 原样透传。后续 sideband / hangup 使用钉住账号的身份。
Live 不继承 Responses 的路由提示与续接状态；turn metadata 中的原账号绑定信息清理后保留其他字段。
真实 call ID 保留用于钉住账号访问通话。HTTP 业务头保留原始多值字节，sideband 握手沿用 WebSocket 可编码的头值

引导与 hangup 的成功响应透传上游状态码、普通业务响应头和 `Content-Type`，剥离认证、账号身份、Cookie
及传输字段；正文按字节透传。sideband 中继不做协议解释：Text/Binary 帧双向透传，
Ping 由网关本地应答，关闭帧保留原关闭码，异常中断投影为正常关闭。同一 call id 只允许一条 sideband，重复加入返回 `409`；
非创建该通话的 Client Key 访问 sideband 或 hangup 返回 `403`；call id 未知或已过期返回 `404`。
hangup 不受 sideband 占用限制，通话中可随时挂断；挂断与 sideband 都走钉住账号的出口代理。

通话使用引导时解析的 Client Key 画像，sideband、重连和 hangup 复用同一画像；画像配置更新只影响新通话。
后续连接与挂断读取钉住账号当前有效的凭据和代理配置

call 与账号的绑定保存在网关进程内，未连接时一小时过期；sideband 占用期间暂停过期，断开后重新计时，
客户端可重连加入同一通话。挂断成功或上游报告通话不存在时立即解除绑定。
进程重启后未完成通话失去 sideband 与 hangup 能力

限制：

- 引导只选择未明确标记为 Free 的 OAuth 账号，套餐缺失或未知时不推断为 Free；API Key 类账号不在语音调度范围
- 引导按当前账号范围、模型政策、额度与冷却状态选取账号；sideband 固定使用创建通话的账号，每次连接重新检查当前授权范围、模型政策与账号启用状态，失权时返回 `403`，不会切换账号
- 已连接的 sideband 不随权限变更自动断开；仍有效的创建方 Client Key 可调用 hangup 结束通话
- 标准实时 WebSocket（`GET /v1/realtime?model=…`）、realtime client secrets、legacy sessions、
  transcription/translation 会话与 SIP 控制返回 `501 realtime_capability_not_supported`。

### Anthropic 原生接口

Claude 域名对外使用标准路径，CPRS 内部使用 `/claude` 前缀，避免与 OpenAI 模型目录冲突：

| 公网路径 | CPRS 内部路径 |
| --- | --- |
| `POST /v1/messages` | `POST /claude/v1/messages` |
| `POST /v1/messages/count_tokens` | `POST /claude/v1/messages/count_tokens` |
| `GET /v1/models` | `GET /claude/v1/models` |
| `GET /v1/models/{id}` | `GET /claude/v1/models/{id}` |

客户端使用 CPRS Client Key，以 `Authorization: Bearer <key>` 或 `x-api-key: <key>` 鉴权。
重复凭据头、冲突凭据、逗号或空白混入的 Key 返回原生 401；无 Anthropic 账号访问权限返回 403。
认证后的内部 Key ID 用于同进程 veil 映射隔离，不向 Nginx 或其他服务回传归一化 Key。
veil 未就绪时，有效 Claude 推理、计数、目录、诊断与后台请求均停止，不回退直连

Claude 使用原生 Messages/count_tokens 终端，保持 JSON/SSE 的协议语义和一次性响应恢复。

Messages 和 count_tokens 保留原始 query 的编码、顺序与重复参数，固定转发到各自上游端点。
本次支持原生 HTTP JSON/SSE 与 Messages Thread；不提供透明 tunnel，也不将 Responses WebSocket 转成 Claude。

Thread 使用 `message-threads-2026-08-12` beta；`{"thread":{"type":"create"}}` 创建，
`{"thread":{"type":"continue","previous_message_id":"msg_..."}}` 发送增量续接。
续接固定原 Client Key、Provider 和账号，允许同账号 OAuth 刷新；缺失记录返回 404（`thread_not_found`），
跨 Key/权限拒绝返回 403，身份配置、协议或 veil 状态变化返回 409，检查点存储不可用返回 503。
稳定错误码在 `error.details.error_code` 中；409 后需携带完整历史创建新 Thread。
成功终态须先提交符合条件的检查点；存储失败发生在流开始后时返回原生 SSE `error`，不再发送成功终止。
`refusal` 或含 `fallback_message` iteration 的完整响应仍正常返回，但不登记为可续接检查点。
Redis 记录沿用现有 TTL/容量边界；到期或淘汰后客户端须以完整历史重新创建。

Messages 要求 JSON object、有效的 `model` 和正整数 `max_tokens`。
Messages 与 count_tokens 的请求体大小共用 `responsesMaxDecompressedBodyBytes`（默认 64 MiB），
支持 `identity`、`gzip`、`deflate`、`zstd`；解压后的正文和请求中间件改写后的正文均复核上限。
入口读取也受该字节预算限制；超限返回 Anthropic 格式的 `413 request_too_large`，
无效压缩正文、重复或不支持的编码返回 `400 invalid_request_error`，不向上游发送请求。
保留 `anthropic-version`、可合并的 `anthropic-beta` 及原生正文，流式使用原始 SSE。
`x-claude-code-session-id` 优先提供显式会话亲和；兼容 `metadata.user_id` 中 JSON 的 `session_id` 和旧格式的 UUID 会话后缀。
这些字段不是身份认证，不从 IP 或提示词推断会话；无 session 正常调度，管理端诊断不修改亲和。
不进行 Codex 到 Claude 的协议转换，未知模型不会自动替换；count_tokens 请求真实上游，拒绝时保留原始错误，
不合成精确计数

模型目录仅返回当前 Key 能访问的 Anthropic 模型，支持 `limit`（默认 20，范围 1–1000）、
`before_id` 或 `after_id`，两个游标不能同时使用。
目录读取本地账号快照，未采集或过期时明确不可用；后台和显式刷新负责访问上游，不合成旧模型列表。
上游 HTTP 错误经过严格占位符恢复后保留原状态及允许的响应头；流开始后发生错误不重放推理。
原生 `input_tokens`、cache read/write 分开保留；共享输入总量由三者加总，总 token 仅在输入与输出均已知时派生。
断流保留已观察到的部分用量，Provider metadata 的 `usageComplete` 标识是否观察到正常终态，缺价格不等于零费用。
费用是 API 等价估算，缓存 5 分钟／1 小时明细及当次计价依据位于 Provider metadata，
不等同于订阅实际扣费或剩余额度

账号通过通用管理接口选择 `anthropic`，支持浏览器 OAuth、官方 callback URL 或 `code#state` 回填，
以及包含 `claudeAiOauth` 的 Claude Code 凭据 JSON；不把订阅凭据下发给客户端。

浏览器授权总有效期 10 分钟，单次领取权 120 秒，状态依次为 `ready → exchanging → exchanged`。
profile 或账号事务失败后，同一 owner 可在原有效期内继续提交；已提交账号的重复回调由数据库 receipt 处理。
账号身份优先取成功 token 响应中的 account/organization UUID，缺失时查询 profile；上传 JSON 中的身份字段不作为证明。

凭据 JSON 要求非空 `accessToken` 和包含 `user:inference` 的 `scopes`，不支持仅 RT；`expiresAt` 可省略，提供时为 Unix 毫秒，
没有 RT 的账号不能续期。AT 已过期或 profile 明确返回 401 时使用 RT 受控刷新，403 与网络故障不触发轮换。
刷新检查点保留 30 分钟，成功刷新后先保存新 token 再提交账号，批量失败可在窗口内用同一原始文档重试；该窗口不随重试延长。
批量导入与导出每份 1–200 个账号，先解析整份材料再访问上游。

```json
{
  "claudeAiOauth": {
    "accessToken": "<access-token>",
    "refreshToken": "<refresh-token>",
    "expiresAt": 1893456000000,
    "scopes": ["user:inference", "user:profile"]
  }
}
```

部署及客户端设置见 [Claude OAuth 与 veil](../deploy/claude/README.md)

## 4. 浏览器认证

### 统一登录与会话

管理员和密钥登录共用 `/api/auth/*`。登录模式 `mode` 只用于选择凭据验证方式，不直接授予权限；
验证成功后，由后端写入身份和绑定 ID。浏览器通过 `cpr_session` HttpOnly Cookie 访问业务接口，
管理员另持有仅发送到 `/api/auth` 的 `cpr_refresh` HttpOnly Cookie。原始 Key 不进入 URL 或浏览器持久化存储。
管理员进入管理端；Key 登录后进入 `/key-usage`，只读取当前会话绑定 Key 的数据

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `POST` | `/api/auth/login` | `{ mode: "admin", username?, password }` 或 `{ mode: "key", apiKey }` | 验证凭据、创建会话；成功后撤销请求携带的旧会话 |
| `GET` | `/api/auth/status` | 无 | 只读校验 Cookie 对应身份，返回 `{ authenticated, session }` |
| `POST` | `/api/auth/refresh` | 无 | 使用管理员续期 Cookie 恢复访问凭据；返回同 status 的结构并同步 Cookie，Key 会话保持原期限 |
| `POST` | `/api/auth/logout` | 无 | 删除当前会话并清除 Cookie；存储失败返回 503，不假装退出成功 |
| `POST` | `/api/auth/password` | `{ currentPassword, newPassword }` | 仅管理员会话可用；验证当前密码后修改密码，撤销全部管理员会话并清除当前 Cookie |

登录返回 `data: { role: "admin" | "key", expiresAt }`；status 和 refresh 已登录时的 `session` 使用同一结构，
未登录时为 `{ authenticated: false, session: null }`。`role` 由服务端已验证身份推导，不接受客户端声明。
不返回凭据或绑定 ID

修改密码要求新密码至少 12 个字符、最多 1024 字节，不能包含控制字符、使用常见弱口令或与当前密码相同。
成功返回 `{ message }`，需要重新登录；当前密码错误或新密码不合法返回 400，并保留原会话。
并发修改中只有旧密码哈希仍匹配的请求可以提交，冲突返回 409；密码更新与安全审计在同一事务提交。
该入口共用登录尝试限流，超限返回 429。普通设置变更和管理员 API Key 变更不撤销密码登录会话，密钥身份会话也不受改密影响

会话由服务端保存，两种 Cookie 均为 `HttpOnly; SameSite=Lax`，`Max-Age` / `Expires` 对齐各自有效期，
`Secure` 沿用上述 Origin 规则。访问 Cookie 使用 `Path=/`，续期 Cookie 使用 `Path=/api/auth`，不能直接访问业务接口。
普通业务请求和状态轮询不会续期。管理员访问凭据到期后，仍可通过续期 Cookie 恢复登录；续期成功会轮换两种凭据，
并重新计算不活动期限，但不能超过登录时确定的最长有效期。访问凭据仍有效时，refresh 返回同一组凭据和期限。
轮换后 30 秒内重试上一代续期凭据返回同一组新凭据；超出窗口再次使用已识别的上一代凭据会撤销该会话。
续期已过期或会话已撤销时不会重新创建；未认证返回 `{ authenticated: false, session: null }`，不写 Cookie；依赖故障返回 503。
退出可以只携带续期 Cookie，并清除两种 Cookie。Key 登录清除续期 Cookie，密钥身份会话保持固定期限。
期限配置见[部署配置](../deploy/README.md#手动安装)

每次恢复密钥会话时重新确认 Key 存在且启用；停用或删除后会话失效，重新启用不会恢复已撤销会话。
依赖不可用时返回 503，不返回已认证或假装未登录。预算耗尽不妨碍登录

密钥会话访问管理接口返回 403，不清除仍然有效的会话。
浏览器会话不能替代 `/v1/*` 的 Bearer Key，数据面 Key 也不能替代浏览器会话

所有 `/api/auth/*` 响应带 `Cache-Control: no-store`；未知路径和错误 method 返回 JSON，不落入 SPA。
两种登录共享来源桶和全局桶，分别为每 60 秒 10 次 / 200 次；来源取连接 IP，不信任任意转发头。
拒绝时使用 `42901` 和 `Retry-After`

认证错误共用 `40101`（会话失效）、`40102`（凭据错误）和 `40301`（权限不足）。
`40102` 和 `40301` 不触发会话恢复，网络或依赖故障不清除身份；浏览器的统一恢复与请求重放职责见[登录与查询身份](architecture.md#登录与查询身份)

### Key 用量与客户端配置

以下接口仅接受 Key 身份的 `cpr_session`，不接受 Bearer Key 或管理 API Key。管理员会话返回 `40301`；
缺失、失效或已停用的 Key 会话返回 `40101`。所有响应带 `Cache-Control: no-store`，未知路径和错误方法返回 JSON

| 方法 | 路由 | 查询 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/key-usage/overview` | `period?`、`asOf?` 或 `startTime?`、`endTime?`，另含 `model?` | 用量汇总、趋势、当前额度和锚点所在日期的健康时间线 |
| `GET` | `/api/key-usage/records` | 同上，另含 `kind?`、`currentPage?`、`pageSize?` | 当前 Key 的请求记录 |
| `GET` | `/api/key-usage/config` | 无 | 当前 Key 的客户端配置凭据 |
| `GET` | `/api/key-usage/version` | 无 | “关于”弹窗使用的当前版本号和提交号 |

用量查询支持[页面时间合同](#页面时间合同)，默认 `period=7d`，一次最多 31 天。模型按字面量包含匹配，区分大小写，通配符按普通字符处理
不接受 Key ID、账号、Provider 等范围参数或其他未知字段。页码默认 1，每页默认 20，允许 1–100 条；
`kind` 为 `success`（默认）、`error` 或 `all`，沿用使用统计的请求结果筛选：错误包括失败和未完成请求，
全部还包括取消与进行中的请求；一次请求只占一条记录。分页响应为 `{ items, currentPage, pageSize, total }`

overview 返回 `asOf`、`asOfDisplay`、`startTime`、`endTime`、`key`、`summary`、`trend`、`healthTimeline`。
`key` 仅包含名称、掩码前缀、并发/RPM、日与周限额、已用 USD 及重置时间；零限额表示不限，
未启动窗口的重置时间为 null。额度使用现有结算账本，不受日志日期或模型筛选影响。
健康时间线沿用管理端的自然日分桶与可用性语义，以 `asOf` 为锚点，不受所选周期长度和模型筛选影响

汇总和趋势返回请求数、输入、输出、缓存读写、推理、总 Tokens 与 USD 成本；输入已包含缓存读写，
推理为输出的明细，不得把缓存或推理重复计入总消耗。趋势另含 `time` 与 `bucketSeconds`。
`costUsd` 为十进制字符串或 null；`costIncomplete` 表示部分请求计费不完整，不能把已知费用当作完整总费用。
空请求范围的成本为 `"0"`，缺失定价保持 null

日志只返回时间、公开请求模型、推理强度、接口、上下游传输方式、当前请求的 IP / User-Agent、
Token 明细、费用明细、用时/首字与状态。Token 和费用复用现有展示合同；延迟仅包含当前请求的首事件、
首推理、首文本、总耗时与上游性能指标，口径见[记录范围与统计口径](#记录范围与统计口径)，不含账号容量或调度诊断。
`status` 为 `success`、`error`、`incomplete`、`cancelled`、`running` 或 `unknown`，`statusCode` 只返回已记录的客户端状态码。
各结果均保留已采集的 Token/费用明细，缺失时为 null。不返回账号资料、Key ID、上游模型或请求标识、原始错误正文或诊断内容

version 返回 `{ version, gitSha }`，不接受查询参数，不包含部署模式、更新状态或内部诊断

config 返回 `{ name, plaintextKey }`，仅读取服务端会话绑定的当前 Key，不接受任何查询参数。
这是显式读取敏感凭据的入口，明文不进入用量响应；客户端应仅在需要配置凭据时调用，并避免持久化到浏览器

## 5. 账号

账号 API 使用统一路由，不存在 Provider Instance 或 Provider 专属账号路由。`provider` 使用 Provider ID，
固定为内置 `openai`、`anthropic`；操作可用性由平台实现与账号认证类型决定

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/accounts` | `page`、`pageSize`、`provider`、`groupId`、`search`、`status`、排序字段 | 分页查询账号与汇总 |
| `GET` | `/api/admin/accounts/concurrency` | `accountId`、`page=1`、`pageSize=20` | 查询账号当前并发占用；每页最多 100 条 |
| `GET` | `/api/admin/accounts/detail` | `accountId` | 查询账号详情、额度和本地用量 |
| `GET` | `/api/admin/accounts/export` | `accountIds`、`confirm=export_sensitive_accounts` | 显式导出最多 200 个账号的敏感 Provider 文档 |
| `POST` | `/api/admin/accounts/import` | `{ provider, data, settings?, outboundProxyId? }` | 导入或按上游身份更新账号，可同时应用调度、分组设置与默认代理 |
| `POST` | `/api/admin/accounts/import-tasks` | `{ submissionId, items: [{ provider, data, settings?, outboundProxyId? }] }` | 接受后台导入，返回 HTTP 202 和任务摘要 |
| `GET` | `/api/admin/accounts/import-tasks` | 无 | 当前管理员仍保留的任务，按创建时间倒序 |
| `GET` | `/api/admin/accounts/import-tasks/detail` | `taskId` | 任务摘要和逐条结果，不含原始凭据 |
| `POST` | `/api/admin/accounts/import-tasks/stop` | `{ taskId }` | 跳过未开始的条目，已开始的条目继续完成 |
| `POST` | `/api/admin/accounts/refresh` | `{ accountId }` | 手工刷新 OAuth credential（`idToken` / `accessToken` / `refreshToken`），不刷新额度 |
| `POST` | `/api/admin/accounts/recover` | `{ accountId }` | 停用账号只启用调度；已启用账号强制清除本地错误/额度/cooldown 事实，不访问上游 |
| `POST` | `/api/admin/accounts/update` | `{ accountId, enabled, concurrencyLimit, weight, groupIds, name?, notes?, modelAccess?, outboundProxyId?, outboundProxyUrl?, connection? }` | 一次更新账号设置；`connection` 支持 OpenAI OAuth 传输方式及 API Key 连接配置，见下文 |
| `POST` | `/api/admin/accounts/batch-update` | `{ accountIds, enabled?, concurrencyLimit?, weight?, groupIds?, modelAccess?, outboundProxyId?, outboundProxyUrl? }` | 一次事务更新所选账号；仅修改提供的字段，至少提供一项修改 |
| `POST` | `/api/admin/accounts/delete` | `{ provider, accountIds }` | 批量删除 1–200 个账号 |
| `GET` | `/api/admin/accounts/quota` | `accountId` | 读取当前额度，不强制访问上游；Provider 未提供额度能力时返回空额度投影 |
| `GET` | `/api/admin/accounts/quota-forecast` | `accountId` | 按需读取周/月容量预测、源窗口剩余估算与采样依据，不刷新上游额度 |
| `POST` | `/api/admin/accounts/quota/refresh` | `{ accountId }` | 按账号来源刷新原生或互联额度，同时同步额度所属状态 |
| `GET` | `/api/admin/accounts/personal-info` | `accountId` | 按需汇聚 Provider 上游个人资料、累计活动与订阅信息，不更新额度或 credential |
| `GET` | `/api/admin/accounts/profile-avatar` | `accountId`、可选 `version` | 返回支持头像能力的 Provider 字节流，需要管理员会话；`version` 仅区分浏览器缓存 |
| `GET` | `/api/admin/accounts/reset-credits` | `accountId` | 查询支持该能力的 Provider 重置卡，不读取本地库存 |
| `POST` | `/api/admin/accounts/reset-credits` | `{ accountId, creditId?, redeemRequestId }` | 使用 UUIDv4 幂等键消费一张 Provider 重置卡 |
| `GET` | `/api/admin/accounts/reset-credits/automation` | `accountId` | 读取自动使用开关与本地计划，不查询上游 |
| `POST` | `/api/admin/accounts/reset-credits/automation` | `{ accountId, enabled }` | 设置账号自动使用开关，默认关闭 |
| `GET` | `/api/admin/accounts/models` | `accountId` | 优先读取 Provider 模型目录缓存，缺失时有限实时拉取；返回 `{ models, observedAt, observedAtDisplay }` |
| `GET` | `/api/admin/accounts/models/catalog` | `accountId` | 读取指定账号的完整 Codex 原生模型目录，返回 `{ modelCount, observedAt, catalog }` |
| `POST` | `/api/admin/accounts/models/refresh` | `{ accountId }` | 强制拉取最新模型并覆盖缓存，响应结构与模型目录查询相同 |
| `GET` | `/api/admin/accounts/connection-test` | `accountId`、`modelId` | 通过 SSE 返回实时连接测试事件，不作为业务 Responses 用量记录 |
| `POST` | `/api/admin/accounts/oauth/start` | `{ provider, name, accountId?, outboundProxyId?, outboundProxyUrl? }` | 为支持登录的 Provider 创建 flow；`accountId` 表示重新授权 |
| `POST` | `/api/admin/accounts/oauth/complete` | `{ provider, flowId, callbackUrl, settings? }` | 消费 OAuth callback；首次授权可附带账号设置，重新授权保留原设置 |

### 查询与账号字段

账号列表支持以下稳定值：

- `provider`: `openai` 或 `anthropic`，省略或空值表示不过滤
- `groupId`: 分组 ID、`ungrouped`，或省略以不过滤
- `status`: `normal`、`quota_exhausted`、`rate_limited`、`disabled`、`error`
- `sortBy`: `email`、`status`、`planType`、`usage`、`lastUsedAt`、`expiresAt`
- `sortDirection`: `asc`、`desc`

账号列表的 `search` 仅按字面量包含匹配名称和邮箱，不区分大小写；`%`、`_` 和 `\` 按普通字符处理

账号模型用量的 `successRate` 按同一账号、模型和用量窗口内的[服务成功率](#记录范围与统计口径)计算，
范围为 0–100，无有效样本时为 `null`，`successRateDisplay` 显示 `—`

`requestCount`、Token、费用及 `lastUsedAt` 只统计完整交付的成功推理响应；
仅有失败记录的模型也返回统计行，没有成功用量时 `lastUsedAt` 为 `null`

账号视图的 `capacity` 返回查询时的普通并发容量：`usedSlots` 不含独立审批池和排队请求，
读取运行态失败时为 `null`。`totalSlots` 是按[并发与排队](#并发与排队)取值的本机上限，`null` 表示不限。上游独立执行其密钥限制

并发明细返回 `accountId`、`entries`、`total`、`page`、`pageSize` 和 `observedAt`，只统计普通并发池。每条包含 `leaseId`、`requestId`、`clientKeyId`、`clientKeyName`、`model`（入口请求模型）、`clientIp`、`startedAt`、`startedAtDisplay` 和 `occupiedMs`。占用时长从取得账号租约开始，不包含排队；缺少入口事实时相应字段为 `null`，租约仍计入总数

查询需要管理员会话，账号不存在返回 404；`page` 从 1 开始，`pageSize` 为 1–100，偏移不能超过 `u32` 范围。结果是查询时的实时快照，按租约有效期排列，续期和请求完成可能改变跨页位置；接口禁止缓存，不提供历史记录

模型目录导出保留上游原生模型对象和能力字段，不包含账号凭据；不支持 Codex 原生目录的账号不能导出。
响应中的 `catalog` 可直接保存为 JSON 文件，用于 Codex 的 `model_catalog_json` 配置。
账号设置保存不等待上游模型目录刷新完成

模型目录查询和刷新响应中的 `observedAt` 表示 Provider 最近一次成功获取目录的时间，`observedAtDisplay` 按部署时区格式化；Provider 未提供观察时间时分别返回 `null` 和 `—`

`models` 只表示上游目录返回了这些模型 ID，不表示 Responses、Chat Completions、Embeddings 等所有端点均已验证可用。
连接测试仅验证所选模型在 Provider 测试路径上的请求，其他端点需通过对应请求确认；目录观察时间不改变路由或账号能力判断

账号限流详情在 `quota` 中返回：`rateLimitReason` 为 `upstream_rate_limit`（上游临时限流）或 `null`，
`rateLimitedUntil` 表示冷却结束时间，到期后自动恢复调度

`quota.credits` 返回上游点数信息，缺失时为 `null`；包含 `hasCredits`、`unlimited` 和 `balance`。
余额是保留上游精度的十进制字符串，未知为 `null`，零为 `"0"`，与重置卡库存独立

支持点数的账号返回 `capabilities.quotaCredits=true`。通过 `POST /api/admin/accounts/batch-update`
提交 `{ "accountIds": ["acct_..."], "autoUseQuotaCredits": true }` 设置是否自动使用点数：

- 默认关闭，订阅额度耗尽即停止调度
- 开启后，上游确认 `hasCredits=true` 或 `unlimited=true` 且没有支出限制时，允许靠点数继续请求；余额数字本身不能证明可用
- 对已耗尽账号开启时复查额度；关闭时立即撤销仅靠点数的放行

点数不解除凭据错误、停用或冷却，也不触发对已被上游拒绝请求的重发

账号名称是本地显示信息，与账号 ID、上游邮箱和凭据独立。编辑接口的可选 `name` 省略或为 `null` 时保留原值；
提供字符串时去除首尾空白，空字符串清除名称并回退显示邮箱，原始输入最多 512 个 UTF-8 字节且不能包含控制字符。
改名保留账号配置和用量记录，凭据刷新与重新授权保留名称；列表搜索匹配当前名称和邮箱

账号列表和详情返回 `notes`（无备注时为 `null`）。编辑时省略或 `null` 保留原备注；字符串最多 500 个 Unicode
字符，允许换行和制表符，保存时去除首尾空白，空字符串清空备注。备注独立于上游身份，导入时未显式提供备注、
重新授权、凭据刷新及批量调度更新均保留已有备注

账号视图和 Dashboard 账号概览中的 `planType` 保留原始套餐值；`planTypeDisplay` 使用 Provider 提供的最终名称，
前端直接展示。OpenAI 的 `prolite`、`pro`、`promax` 分别显示为 `ProLite`、`Pro`、`ProMax`；
其余已知套餐使用官方名称，未知 OpenAI 套餐保留原值。
展示名称保留空格和括号，未提供专属名称规则的 Provider 使用通用大驼峰格式。
OpenAI 主动额度刷新和正常响应携带的明确套餐会同步到账号，支持升级与降级；
空值或 `unknown` 不覆盖已有套餐，同族泛化值（如 `team`）保留已知的具体套餐子类型。
账号套餐为空或 `unknown` 时，后端优先用已保存的上游额度响应
中的明确套餐值补全 `planType` 和 `planTypeDisplay`；两处均无套餐信息时才显示“未知套餐”

`outboundProxyId` 绑定已保存的代理，不要求出口测试成功；省略或 `null` 保留当前绑定，空字符串清除绑定。
`outboundProxyUrl` 兼容 HTTP、HTTPS、SOCKS5、SOCKS5H 代理 URL，可带用户名和密码；不能与 ID 同时设置。
编辑时省略或 `null` 表示保持原配置，空字符串表示清除代理并直连。列表和详情返回绑定的
`outboundProxyId`（未绑定时为 `null`，禁用代理仍保留 ID）和不含认证信息的 `outboundProxyEndpoint`
（未配置时为 `null`）；只有显式敏感导出包含完整 URL。
指定代理后，推理、OAuth 服务端交换/刷新及账号辅助请求使用同一出口；代理失败不会退回直连。
浏览器打开的第三方 OAuth 授权页仍使用浏览器自身网络。
账号出口与连接隔离见 [架构说明](architecture.md#账号出站代理)

### 上游互联快照

启用的 OpenAI API Key 账号由后台主动识别互联能力，使用该账号的 API 前缀、Bearer 密钥和出站代理
访问 `{base_url}/cpr/link`；地址可指向 CPR 或实现相同协议的其他项目。
账号保存后异步首查，无识别记录的存量账号由后台补查。明确不支持该协议时保存识别结果并停止周期探测，重启沿用结果，不影响模型请求。
地址、密钥或出站代理变化后重新识别；普通名称、备注编辑保留结果。已识别的互联上游继续周期同步额度

账号列表返回已保存观测的 Key 预算窗口与池汇总；详情和额度 GET 另含池内逐账号窗口。
查询这些接口不会访问上游，展开节点视图时通过账号详情读取窗口。
`account.upstream` 在账号停用或认证方式不支持互联适配时为 `null`；可探测的账号包含：

| 字段 | 含义 |
| --- | --- |
| `discovered` | 收到有效互联快照后为 `true`，明确关闭共享或不支持协议后为 `false`。其他失败保留当前识别结果 |
| `observedAt`、`observedAtDisplay`、`observedAtRelative` | 上游观测时间及服务端格式化结果，不代表本次页面查询时间 |
| `stale` | 快照是否已过期；过期值仅供查看 |
| `connectionRevision` | 当前连接身份的不可解释标识，用于隔离地址、密钥或出口变化前的展示结果 |
| `lastError` | 当前实例最近同步失败分类：`unauthorized`、`sharing_disabled`、`unsupported`、`rate_limited`、`invalid_response`、`unavailable`；已保存的不支持、共享关闭或互联失权记录重启后仍返回对应分类，同步成功时为 `null` |
| `maxConcurrency`、`requestsPerMinute` | 上游密钥的并发上限和 RPM，零表示不限，未观测到的项为 `null` |
| `pool` | 上游授权公开的账号数量、状态数量及套餐数量，未公开时为 `null` |

互联额度复用 `quota.windows`。上游 Key 账本按实际提供的周期展示，不补造缺失窗口；不限额也保留已用金额。
窗口返回 `amount: { usedDisplay, remainingDisplay, totalDisplay }`，字符串包含单位，美元使用 `$` 前缀；
不限额的总额与剩余额显示为 `∞`，`usedPercent` 为 `null`，金额不换算成 Token。
Key 消费窗口的 `localUsage` 按各自 `[resetAt - windowSeconds, resetAt)` 边界聚合本机请求，
与上游金额和百分比分开展示；未开启或边界缺失时不查询。
上游池内的原生窗口通过 `sourceAccountId` 保留授权 Key 内稳定的匿名账号归属，
不相加为统一余额，`localUsage` 保持缺失；Key 消费窗口与本机账号窗口不返回该字段。
互联窗口返回独立的 `observedAtDisplay` 和 `stale`

手动刷新复用 `POST /api/admin/accounts/quota/refresh`，请求 `{ accountId }`，与原生额度刷新一样等待查询结束并返回账号视图。
手动刷新允许重新探测已确认不支持的连接。不支持协议和明确关闭共享均返回账号视图，`lastError` 分别为 `unsupported` 和 `sharing_disabled`。
其他查询失败通过现有管理错误返回；失败后可回读账号取得更新后的身份与观测。
并发刷新合并，连续手动刷新至少间隔 15 秒，并遵守上游 `Retry-After`，不继承自动重试的长退避。
自动刷新由后台任务执行，刷新节流与临时失败提示属于当前实例的运行期状态，重启后重新建立。

- 收到有效快照时显示 CPR 互联。明确返回不支持协议（HTTP 404、405、501），或返回 `403 peer_sharing_disabled` 时，清除互联身份和共享数据，显示普通 API Key
- 普通 401/403 保留原识别结果并清除已撤销的共享内容。超时、限流、服务异常或无效响应保留原识别结果和已有观测。无观测时 `observedAt` 与 `pool` 为 `null`，额度窗口为空
- 展开可探测的 API Key 后，点击「刷新额度」可重新识别互联。卡片显示已有更新时间，手动刷新结果通过通知反馈。明确不支持协议的连接停止周期探测，共享关闭的连接按授权退避周期复查

账号地址、密钥或出口变更使旧观测与识别身份失效，重新识别当前连接。
账号停用后停止同步，重新启用后按当前连接的识别结果恢复后台任务

成功同步后，明确的预算访问结论决定账号是否进入 `quota_exhausted`；
有效的 `usage.data.allowed: true` 自动解除更早的耗尽，恢复参与调度。
Key 预算及其访问状态随互联默认公开，不要求开启 `shareQuota`。
快照和预算观测必须仍有效且晚于耗尽事实，重置时间到期、金额变化、缺失状态或查询失败均不代表恢复。
恢复只改变额度状态，凭据错误和独立的 `rate_limited` 冷却仍按各自规则处理

### 账号模型限制

账号列表和详情返回 `modelAccess: { mode, models }`，模型 ID 区分大小写并精确匹配：

| `mode` | `models` | 含义 |
| --- | --- | --- |
| `all` | `[]` | 不额外限制模型，默认值 |
| `allowlist` | 非空 ID 数组 | 仅允许指定模型 |
| `denylist` | 非空 ID 数组 | 排除指定模型 |

最多 256 项，每个 ID 最多 256 字节；重复 ID 去重，不接受空白、控制字符、`__` 前缀或 `*` 通配符。
允许保存当前上游目录尚未返回的 ID。限制按全局模型映射后的上游 ID 判断，不扩大账号本身的上游权限。
管理端刷新目录只更新目录，不修改模型限制。全选与取消全选只作用于上游目录中符合当前搜索条件的模型。
自定义模型取消勾选后在本次编辑期间保留，可再次勾选。保存时以最终选中项更新限制，关闭或取消编辑会丢弃草稿。
白名单的清理按钮列出当前目录未返回的已选 ID，默认不选中任何项，
由管理员确认后从当前表单移除，保存账号后生效。目录未就绪、刷新失败或返回空目录时不能清理，清理也不能将白名单置空。
图像、搜索和映射目标可能不在目录中，目录缺失本身不代表模型失效。
例如 Plus 设置 `allowlist` 并选中 luna 的实际 ID，Pro 设置 `denylist` 并选中同一 ID，即可严格分流；
Pro 使用 `all` 时也能参与 luna 调度。套餐名称不自动生成或修改规则

原生 Responses 默认排除有效目录未列出目标模型的账号。OpenAI OAuth 或 API Key 账号的白名单精确包含映射后的上游 ID 时，
允许通过该账号请求目录外模型，由上游确认是否可用。此设置只对配置它的账号生效，已有白名单同样适用。
账号禁用、Key 分组、认证类型、黑名单和模型冷却仍参与选号。
OAuth 按已知套餐匹配目录，API Key 按账号与凭据版本隔离目录，不借用其他账号的模型集合。
OpenAI 使用最近 30 分钟内同步的目录；未知套餐或无有效目录时交由上游验证，仍执行 `modelAccess`

原生 OpenAI Responses 上游返回 `error.code=model_not_found`，且状态为 400／403／404 或流内错误未提供状态时，
该账号与模型组合冷却 30 分钟；其他模型仍可使用该账号。仅消息提及模型不支持不触发冷却；
`unsupported_value` 属于请求参数错误，原样交回客户端，不触发换号或模型冷却。
冷却按凭据版本隔离，不修改 `modelAccess`，也不解除原生续写的固定账号约束。OpenAI 的受管上游适配器不应用原生目录限制和模型拒绝冷却

单账号更新或批量更新省略 `modelAccess` 时保留原值，显式提交 `{ "mode": "all", "models": [] }` 清除限制。
批量接口的调度、分组、模型与代理字段均可省略；省略的字段保持各账号原值。
提供 `groupIds` 时替换完整分组集合，提供 `concurrencyLimit: null` 时恢复继承运行参数

限制适用于 Responses HTTP、WebSocket 及其带压缩触发的请求，以及 Images 生图、编辑与独立 Search 选号，包括重试、亲和和换号；没有合规账号时沿用
无可用账号错误，不会回退到被禁止的账号。已开始请求使用冻结的政策，新请求使用已发布的新配置。
Images 和 Search 按原始请求正文的模型 ID 检查，不依赖模型目录或全局模型映射；可手动添加目录未列出的模型 ID。
模型缺失、重复或无法识别时，仅使用模型政策为 `all` 的账号，不推测上游默认模型。

### 独立代理管理 / Managed Proxies

所有端点要求管理员身份。所有响应只返回去掉认证信息的 `endpoint`，不会返回完整 URL

| 方法 | 路径 | 请求 | 结果 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/proxies` | `page`、`pageSize`（1-200）、`search`（名称） | `{ items, page }` |
| `GET` | `/api/admin/proxies/accounts` | `proxyId`、`page`、`pageSize`（1-200）、`search`（账号名称或邮箱） | `{ items, page }` |
| `POST` | `/api/admin/proxies/accounts/remove` | `{ proxyId, accountId }` | `{ configRevision }` |
| `POST` | `/api/admin/proxies/create` | `[{ name, proxyUrl, location?, autoLocation?, maxWebsocketConnections? }]` | 逐项结果数组，成功项的 `data` 为 `{ record, configRevision }` |
| `POST` | `/api/admin/proxies/update` | `{ id, revision, name, enabled?, proxyUrl?, location?, autoLocation?, maxWebsocketConnections? }` | `{ record, configRevision }` |
| `POST` | `/api/admin/proxies/probe` | `{ proxyUrl, detectLocation? }` | 测试未保存的地址或服务器直连出口，返回连通性及可选位置结果，不创建代理 |
| `POST` | `/api/admin/proxies/test` | `[{ id, revision, detectLocation? }]` | 逐项结果数组，成功项的 `data` 为最新代理记录 |
| `POST` | `/api/admin/proxies/delete` | `{ id, revision }` | `{ configRevision }` |

`probe` 的 `proxyUrl` 必填，空字符串表示服务器直连；非空值须为合法代理 URL，`null`、省略或非法地址均拒绝。
探测不创建代理或修改账号绑定

添加与测试统一接收 1–100 项数组，单条操作传一个元素，响应 `data` 按输入顺序返回等长数组。
每项为 `{ status: "succeeded", data }`、`{ status: "failed", httpStatus, code, message, data: null }`
或 `{ status: "skipped" }`。整个请求的 JSON、数组形状、数量或重复测试 ID 校验失败时返回 4xx，不执行任何项；
通过请求校验后返回 HTTP 200，各项独立提交，单项字段格式、名称或位置校验失败、重复 URL、版本冲突等只影响当前项。
遇到服务异常或结果不确定时停止派发后续项，已开始的测试等待结果，其余标记为 `skipped`。
调用方不能自动重试整批；请求中断时先刷新核对已保存项。测试每批最多并发两条，与单条探测共用四条并发上限；
调用方应按超时预算分组发送，请求中断不会回滚已提交项

`record` 包含 `id`、`name`、`enabled`、`endpoint`、`hasAuthentication`、`revision`、`accountCount`、`location`、
`autoLocation`、`detectedLocation`、`maxWebsocketConnections`、`lastTestAt`、
`lastTest: { success, latencyMs, exitIp, exitIpv4, exitIpv6, message, location }`、`createdAt`、`updatedAt`。
未测试时 `lastTestAt` / `lastTest` 为 `null`。连通性失败返回 HTTP 200 和 `lastTest.success=false`；
记录版本过期、重复 URL 或代理正在用于账号导入时对应 409，并发测试满载对应 429；
添加和测试在各项的 `httpStatus` 返回这些状态，其他单项接口使用 HTTP 状态码

代理列表只返回关联账号数量。关联账号按需查询，每项包含 `id`、`name`、`email`、`provider`、`enabled`、
`authenticationKind`、`planType`、`planTypeDisplay`、`upstream` 和 `groups: [{ id, name, color, enabled }]`，
不返回账号凭据。默认每页 20 条，按名称、ID 稳定排序；搜索不区分大小写，匹配名称或邮箱的字面子串。
不存在的代理返回 404，未绑定账号或没有匹配结果时返回空页。数量与当前页对应同一查询快照

`upstream` 复用账号列表的互联观测结构和发现状态语义，只读取本地观测，不触发远端刷新

移除关联账号只清除指定账号的代理绑定与连接地址，使其改为直连，保留凭据、调度参数与分组。
若账号已不再绑定请求中的代理，则返回 409
删除代理会自动解除全部关联账号的代理绑定与连接地址，账号保留其余设置并改为直连。

`maxWebsocketConnections` 为此代理的 OpenAI Responses 上游 WS 连接上限，范围 0–4294967295，0 表示不限制。
创建时省略默认为 0，更新时省略或 `null` 保留原值。关联账号共享预算，修改后新请求使用新值，已存在的连接仍计数。
容量回收、等待及多实例边界见[部署说明](../deploy/README.md#公网访问)

更新省略 `proxyUrl` 保留认证；连接配置改变时清除测试结果并更新所有绑定账号。
测试结果只在请求中的版本仍匹配时保存

代理默认启用，更新 `enabled: false` 后保留配置、测试结果和账号绑定，关联账号的新请求改为直连，
不应用该代理的位置覆盖；重新启用后恢复原出口及位置规则。省略 `enabled` 保留当前状态。
禁用代理仍可执行连接测试，但不能以 ID 选择用于账号绑定、新的账号授权或导入

`detectLocation: true` 在测试连接时解析出口位置；结果 `location.status` 为 `notRequested`、
`detected`、`failed` 或 `conflict`。`detected` 携带 `location`，`failed` 携带安全错误说明；
位置查询失败不等同于代理连接失败；检测结果不会自动开启持续跟随。
API 的 `autoLocation` 默认为 `false`；开启时使用已检测位置，测试连接会刷新检测结果。
`detectedLocation` 保存 `{ location, exitIpv4, exitIpv6, detectedAt }`，手动位置独立保留

`location` 为 `null` 或完整对象 `{ country, region, city, timezone }`。国家代码为两位大写 ASCII 字母；
地区、城市禁止控制字符，去除首尾空白后须为 1–128 个字符；时区必须是有效 IANA 名称，例如 `Asia/Tokyo`。
创建时省略或 `null` 表示继承全局；更新时省略表示保留，`null` 清除覆盖，完整对象替换覆盖。
只改位置不清空连通性测试结果，也不更改账号凭据版本

关联账号的 OpenAI/Codex Responses 请求（HTTP/SSE、WebSocket）优先使用代理位置，否则使用全局
运行设置中已开启的 `requestLocation`；两者均未开启时保留客户端原有位置和时区。全局覆盖按请求冻结，
新请求使用保存后的设置，无需重启；代理覆盖在每次执行时读取，
换号或换出口按该次选定账号解析。位置只影响客户端时间上下文的时区和 Web Search 的结构化位置，
不改变 epoch 时间戳、真实出口 IP、服务或管理端时区、数据驻留约束及 Anthropic 请求

时间上下文支持以下两种消息，文本块均为 `type: "input_text"`：

| 消息角色 | 完整 XML 根标签 | 内容分类 |
| --- | --- | --- |
| `user` | `environment_context` | `environments.environment_context` |
| `developer` | `codex_apps_client_time_context` | `additional_content.codex_apps_client_time_context` |

文本去除首尾空白后须由对应标签完整包围且为合法 XML，只替换根节点直属的 `timezone`。
`current_date` 保留客户端原值，不按请求时间刷新历史日期，也不根据仅有的日期推算目标时区日期。
有 `internal_chat_message_metadata_passthrough.content_item_kinds` 数组时，文本块的对应分类须匹配上表；
没有分类时，按消息角色和完整 XML 识别。
显式标为 `user.text` 或其他分类的内容、普通聊天中引用的示例、工具结果及无法解析的上下文保持原样。
客户端实际本机时区不受影响，工具读取本机时区后的输出仍可包含真实值；排查见
[时区与客户端环境信息](../deploy/README.md#时区与客户端环境信息)

测试经代理并发访问 IPv4 专用端点 `https://api.ipify.org?format=json` 与 IPv6 专用端点 `https://api6.ipify.org?format=json`，
分别验证并记录双栈出口（IPv4 与 IPv6 地址），在任一地址族可用时即判定连接成功。超时 15 秒，每进程最多同时测试 4 条。
解析位置时还会向 `https://ipwho.is/<出口 IP>` 查询地理位置，出口 IP 会发送给该第三方服务；
自定义位置和时区不改变这些探测请求
探测器复用 OpenAI 的证书信任配置：优先读取非空的 `CODEX_CA_CERTIFICATE`，
其次读取 `SSL_CERT_FILE`，并保留系统根证书；证书配置错误不会回退为不验证证书。
出口测试结果仅供诊断，不限制代理的选择和绑定；未测试或测试失败的代理仍可使用。
出口测试通过不表示 Provider 账号权限或额度可用；账号可用性使用账号连接测试。
导入请求可以携带顶层 `outboundProxyId`，在令牌交换前解析为默认出口；文件中显式的代理配置优先。
文件及 AT/RT 导入从凭据交换到落库期间保护所选代理；此时修改、删除或写入测试结果返回 409，
避免已轮换的凭据因代理状态变化而丢失。完成导入或请求取消后自动释放保护。
OAuth 等待回调期间不持有保护；提交仍拒绝已删除或连接配置改变的代理

### 账号连接测试 SSE

`GET /api/admin/accounts/connection-test` 固定探测请求指定的账号，不参与普通账号轮换。成功流沿用
`test_start`、`request`、`content`、`test_complete` 事件；失败事件为：

```json
{
  "type": "error",
  "source": "upstream",
  "gatewayErrorCode": "rate_limited",
  "sendState": "sent",
  "proxyHint": null,
  "error": "upstream unavailable",
  "providerErrorCode": "usage_exhausted",
  "providerErrorType": "invalid_request_error",
  "upstreamStatus": 429,
  "upstreamContentType": "application/json",
  "upstreamBody": "{\"error\":{...}}"
}
```

所有连接测试事件都包含服务端发出时的 `occurredAt`、完整 `occurredAtDisplay` 和短 `timeDisplay`。
浏览器本地取消或断网没有服务端发生时间，应保持事件顺序和状态，不伪造时间戳

- `source` 为 `gateway`、`provider` 或 `upstream`：分别表示尚未进入 Provider、Provider 本地且未发送、
  已发送/可能已发送或已经捕获到上游事实
- `gatewayErrorCode` 是 `GatewayErrorKind` 的稳定机器值，管理端据此生成中文摘要
- `proxyHint` 仅在 Provider 发送前失败且有代理诊断时返回：`invalid_configuration` 表示代理配置无效，
  `unreachable` 表示无法连接代理端点，`tunnel_failed` 表示代理隧道协商失败。
  `check_proxy` 表示本次使用代理且建连失败，不能据此判定故障在代理；无提示时为 `null`
- `sendState` 为 `not_sent`、`sent`、`ambiguous`，非 Provider 错误为 `null`
- `error`、`providerErrorCode`、`providerErrorType`、`upstreamStatus`、`upstreamContentType` 和
  `upstreamBody` 是实际捕获的原始诊断字段；缺失时为 `null`，不会由本地猜测或翻译

### 后台导入任务

管理端通过后台任务导入账号，关闭页面不会取消执行。`submissionId` 为客户端生成的 UUID；同一管理员在任务记录
保留期间使用相同标识和相同输入重新提交，会返回已有任务，内容改变则返回 409。修改输入须使用新标识

每个任务接受 1–200 个 `items`，请求体上限为 4 MiB。每个条目遵循下节的 Provider 文档与设置合同，独立处理
并返回结果。批量 AT / RT 按非空输入顺序拆成单账号条目，因此可逐条统计；
管理端上传的 OpenAI JSON 文件按 `accounts` 数组拆成单账号条目，每项保留该账号引用的代理，
成功与失败分别记录；Anthropic 及直接调用导入 API 的多账号文档仍由 Provider 按自身语义处理。
管理端按条目数和请求体大小分批创建任务，同一输入重试时复用各批的提交标识，已创建任务不会重复执行。
一个未拆分的文档可导入多个账号，条目成功数与入库账号数可能不同

摘要字段为 `taskId`、`createdAt`、`finishedAt`（未结束为 null）、`stopRequested`、`total`、`counts`。
`counts` 包含 `pending`、`running`、`succeeded`、`failed`、`unknown`、`skipped` 和 `importedAccounts`。
详情追加 `items: [{ index, provider, status, accountIds, message }]`，`index` 从 1 开始；`status` 对应上述前六类状态。
列表返回 `{ items: [摘要] }`。结果未知时保留 `unknown`，先核对账号目录再决定是否重新导入；服务端不自动重试凭据交换。
停止请求可重复调用，已结束的任务保持原结果；未知、已过期或其他管理员的任务 ID 返回 404

所有后台导入共享 3 个执行槽位；
最多接受 8 个未结束任务、保留 100 个任务，达到上限返回 429。成功、失败或跳过后立即释放对应输入，
终态结果保留 1 小时后自动清理。服务重启会丢失任务与未执行输入，已提交的账号不受影响；
任务记录仍在保留期内时，可通过列表接口查询当前管理员的任务及进度

### 账号能力、导入与 OAuth

敏感账号导出要求所选全部账号属于支持导出的内置平台。不支持时整个请求在读取任何凭据前拒绝，不返回部分导出结果

账号列表及额度详情中的 `capabilities` 则针对当前账号，包含布尔字段 `quota`、`quotaRefresh`、`profile`、
`subscription`、`avatar`、`resetCredits` 和 `consumeResetCredit`，分别表示额度读取、额度刷新、资料、订阅、
头像、重置卡查询及消费能力。服务端结合 Provider 实现与账号认证类型生成，客户端据此显示
操作；只读与写入能力分别判断，不按 Provider 名称推测。
能力是查询时的投影，不能代替实际操作的权限校验，也不保证上游可用

导入的 `data` 必须是 JSON object，Admin API 请求上限为 64 MiB；Provider 可以收紧限制，
内部 schema 由目标 Provider 独占解释：

- OpenAI 接受单账号 OAuth 或 API Key 文档、`accounts` 数组（最多 200 项）、CPR 账号 bundle 和含代理引用的 sub2api 导出
- OpenAI OAuth token 字段接受 `accessToken`、`refreshToken`、`idToken`，以及官方
  `auth.json` 中的 `access_token`、`refresh_token`、`id_token`，可以嵌套在 `tokens` 等账号 object 内；
  每项至少包含 AT 或 RT。仅含 `OPENAI_API_KEY` 的客户端代理配置不是 OAuth 账号导入材料；
  RT-only 会在导入时换取 AT，已知过期的 AT-only 会拒绝导入，其他 AT-only 不具备自动续期能力
- OpenAI 的账号条目接受 `outboundProxyUrl`；OpenAI 还会解析 sub2api 的 `proxy_key` 和顶层 `proxies`。
  代理在 token 刷新前绑定。缺失、重复、停用、带到期时间或配置回退策略的 sub2api 代理会拒绝导入
- Anthropic 接受 Claude Code OAuth 凭据 JSON；身份通过官方 profile 验证，重复身份更新已有账号
- OAuth 导入由目标 Provider 完成必要的 token exchange 或身份投影。API Key 导入校验凭据格式与地址，不调用 OAuth 或 ChatGPT 身份接口；可用性由模型目录和连接测试验证

API Key 账号使用以下独立凭据形态：

```json
{
  "provider": "openai",
  "data": {
    "authentication_kind": "api_key",
    "name": "团队上游",
    "base_url": "https://api.example.com/v1",
    "api_key": "<upstream-api-key>",
    "transport": "http"
  }
}
```

`base_url` 是 API 前缀，追加 `responses`、`models`、`images/generations`、`images/edits` 或 `alpha/search`，
不会自动补 `/v1`；支持根路径和自定义前缀。
地址支持 HTTP 和 HTTPS，包括内网 IP 与容器名，拒绝 URL 中的认证信息、查询串和 fragment。
HTTP 会明文传输 API Key 和请求内容，仅用于可信网络。
`transport` 可省略（默认 `http`）或设为 `prefer_websocket`。API Key 使用 Bearer 认证与普通 JSON，
不携带 OAuth Cookie 或 ChatGPT 身份。上游模型列表使用标准 `/models` 格式，按账号和凭据版本隔离；
目录用于模型发现，不作为能力白名单，未列出的模型仍交由上游判断。
API Key 每次导入创建独立账号；更新已有账号使用 `/api/admin/accounts/update` 的 `connection` 字段。
导出会显式包含密钥，沿用敏感导出的确认合同

sub2api 的 `platform=openai`、`type=apikey` 使用 `credentials.base_url` / `credentials.api_key`；
导入时按其端点规则将服务根、版本前缀或完整 `/responses` 地址转换为 API 前缀，缺省地址为官方 `/v1`。
非空模型映射、请求头覆盖、其他协议和启用的 `extra.openai_*` 设置尚未适配，返回输入错误，需先移除并在本项目重新配置

API Key 账号与 OAuth 共用现有 Responses、Images 和 standalone Search 请求与响应链路，
包括 Responses Lite 和原生压缩协议的透传，实际支持情况由上游决定；不提供独立 compact 路由。
模型目录、连接测试和本地用量统计可用；OAuth 刷新/重新授权、ChatGPT 额度、个人资料、订阅和重置卡不适用。
客户端的 `image_generation` 和 `X-OpenAI-Actor-Authorization` 声明不改变账号的实际能力。
未识别为 CPR 互联的 API Key 账号，其列表和详情 `usage` 汇总账号创建后仍保留的本地请求记录，
`usage.windowKind` 为 `general`，管理端显示累计用量。日志清理会影响累计范围，不代表上游余额

互联账号按上游 Key 已提供的最长消费窗口统计本机 Token、费用和模型用量。
日、周周期的 `usage.windowKind` 分别为 `daily`、`weekly`，其他周期为 `quota`。查询边界同账号额度窗口，为 `[resetAt - windowSeconds, resetAt)`；
未开启或缺少重置边界时显示无数据，不回退到历史累计，也不采用池内其他账号的周期。
OAuth 账号优先按实际周/月额度窗口统计。账号缺少可展示的上游额度窗口时，使用过去 24 小时的本地请求统计，
`usage.windowKind` 为 `daily`，请求图和用量统计共用该滚动范围。该统计可在上游额度快照缺失时展示

批量导入 AT / RT 使用 `accounts` JSON 数组，最多 200 项，不接受纯文本 token 列表。例如：

```json
{
  "provider": "openai",
  "data": {
    "accounts": [
      { "accessToken": "eyJ..." },
      { "accessToken": "eyJ...", "refreshToken": "rt_...", "idToken": "eyJ..." }
    ]
  }
}
```

RT-only 使用同一形状，只提交 `refreshToken`。不得把真实 token 写入日志、issue、fixture 或文档

账号导入与首次 OAuth complete 可附带 `settings: { enabled, concurrencyLimit, weight, groupIds, name?, notes?, modelAccess? }`。
提供 `settings` 时前四项均必填，`concurrencyLimit: null` 继承运行参数，否则为 1–4294967295 的整数；
`weight` 为 1–100，`groupIds` 为完整分组集合。设置应用于本次导入的全部账号，包括匹配到的已有账号，
与凭据在同一事务内提交；分组不存在时整次回滚。省略 `settings` 时新账号使用默认设置并保持未分组，
已有账号保留原有分组、权重与并发设置。可选 `notes` 与编辑备注使用相同的校验和清空语义，省略或 `null` 保留已有备注；
管理端新建表单留空时省略 `notes`。重新授权不接受 `settings`，凭据刷新保留账号设置

可选 `name` 与编辑名称使用相同的长度和控制字符约束，仅在单份导入数据解析出一个账号时覆盖名称。
省略、`null`、留空或多账号导入均保留导入数据或服务商提供的名称；管理端批量导入不提交 `name`

账号列表的每个 item 返回轻量 `groups: [{ id, name, enabled }]`

OpenAI 的 CPR 导出保持 OAuth 账号的既有 token 与过期时间字段。CPR 账号条目包含
`modelAccess`，重新导入时恢复；导入 `settings.modelAccess` 显式提供时覆盖条目中的政策。
导入条目未提供 `modelAccess` 且 `settings` 未指定该字段时，新账号默认 `all`，已有账号保留原值；凭据刷新、重新授权和目录刷新均保留政策。
sub2api 的 `credentials.model_mapping` 不转换为本项目的账号模型限制

编辑 Codex API Key 账号时，在 `POST /api/admin/accounts/update` 的账号设置中附带 `connection`：

```json
{
  "accountId": "acct_...",
  "enabled": true,
  "concurrencyLimit": null,
  "weight": 1,
  "groupIds": [],
  "connection": {
    "baseUrl": "https://api.example.com/v1",
    "transport": "http",
    "apiKey": "..."
  }
}
```

`connection.transport` 支持 `http`、`prefer_websocket`。省略 `connection` 时只更新账号设置；
省略 `connection.apiKey` 保留当前密钥，空字符串无效。
Codex OAuth 账号只接受 `connection: { transport }`，不接受 `baseUrl`、`apiKey` 或 OAuth token。
连接设置不能修改账号 ID、Provider 或认证类型，也不接受通用凭据文档。
凭据与设置在同一事务中保存，任一校验或持久化失败均不落库。
`GET /api/admin/accounts/detail` 对 API Key 账号额外返回 `credentialConfiguration: { base_url, transport }`；
OAuth 账号返回 `credentialConfiguration: { transport }`。响应不回显密钥或 token；不适用的账号省略该字段。
更新会推进凭据 revision 并失效目录与连接；旧版本会话不可静默续接到新上游

OAuth start 使用：

```json
{
  "provider": "openai",
  "name": "account name",
  "accountId": null
}
```

重新授权已有账号时，start 请求仍携带 `provider` 和展示用 `name`，只额外提供目标 `accountId`；
客户端不得提交 `credentialRevision`、旧 token 身份或其他并发控制字段。complete 请求也不重复提交
`accountId`，后端通过 `flowId` 中保存的目标绑定完成授权

`name` 可为空字符串，表示不设置本地名称；管理端新建授权默认留空，列表使用上游邮箱展示。
提供名称时去除首尾空白，长度与字符限制沿用账号名称规则

流程只允许发起它的管理员身份继续；尚未完成的流程过期后需重新发起。
账号提交成功后，`complete` 保留 24 小时的完成结果。同一管理员使用相同 `provider`、`flowId`
重试时返回原账号结果，不再执行授权或更新凭据；新提交的 `callbackUrl` 和 `settings` 不会再次应用

OpenAI 授权码交换与令牌刷新请求的连接超时为 30 秒，单次上游请求总超时为 60 秒

- 授权码交换未能建立连接时返回 `50301`，提示区分连接超时与其他连接失败
- 无法确认发送结果或未完整接收响应时返回 `50202`，应先核对账号状态，不立即重复提交授权码
- 代理出口探测成功不代表 OpenAI 授权端点可达

### OpenAI 身份、额度与状态

- OAuth 文件导入接受 camelCase 与 snake_case 的三个 token 字段，内部统一保存为
  `accessToken`、`refreshToken`、`idToken`，不接受含义模糊的 `token`。
  仅有 refresh token 时先换取 access token
- 普通 OAuth 的身份补全复用官方 `token_data.rs::parse_chatgpt_jwt_claims`：优先解析 `idToken`，缺失字段再由
  `accessToken` 补齐；`email` 优先 JWT 顶层值、其次 `https://api.openai.com/profile.email`，用户 ID
  优先 `chatgpt_user_id`、其次 `user_id`。该路径不调用 `whoami`，也不信任导入文档顶层的
  `userId/accountId`
- `at-` 开头的 Codex Personal Access Token（PAT）可直接粘贴到现有 **AT 导入** 入口，或使用
  `{"accessToken":"at-..."}` JSON；也接受官方 `auth.json` 的 `personal_access_token` 字段
  （兼容 `personalAccessToken`）。导入时向 OpenAI auth 的
  `/api/accounts/v1/user-auth-credential/whoami` 验证令牌，以响应中的用户 ID、账号 ID 和套餐建立身份，
  `email` 可缺失；不从导入文件的身份字段或附带的 ID token 回退补齐。验证失败不导入，接口区分提示
  PAT 格式无效、被上游拒绝、验证服务不可用和身份响应无效，不回显令牌或原始上游响应。
  PAT 不保存附带的 refresh token、ID token 或推测的过期时间，不参加 OAuth RT 刷新；
  失效后需取得新 PAT 再导入
- 首次 OAuth 保留回调 `state`、PKCE 与官方 token exchange，并持久化 `idToken`、`accessToken`、
  `refreshToken`。刷新响应中的三个 token 字段均按官方语义独立轮换：返回新值时替换，省略时分别保留
  现值。回调地址只承载 `code`/`state`，不以 host/path 形式作为拒绝条件
- OpenAI 重新授权允许登录另一个上游账号。保留本地账号 ID、名称和调度设置，按新授权更新令牌、
  上游用户与工作区 ID、邮箱和套餐。换号时清除旧账号的额度快照和耗尽状态，新额度由后台重新查询。
  无法识别新用户或新身份已绑定其他记录时，原账号保持不变
- OAuth 账号文件导入和 OAuth complete（包括重新授权）在 credential 提交后后台尝试一次额度观测，不等待
  观测完成才返回成功。观测失败只记录告警，不回滚已提交的账号；手工或后台 RT 刷新只更新 token，
  不隐式等同于手工额度刷新，也不更新既有账号资料或 OAuth principal；Anthropic 使用相同的提交后观察流程
- 同一 OAuth flow 不能并发完成；账号事务提交成功后保留可重试读取的完成结果。
  失败会释放流程占用，但上游 authorization code 通常只能交换一次；已完成 token exchange 而未提交账号时应重新创建 OAuth flow
- `GET /api/admin/accounts/quota` 只读取最后一次落库快照；`POST /api/admin/accounts/quota/refresh` 才访问上游。access token
  已过期时，额度刷新要求先走 credential 刷新或重新授权，不会拿过期 token 探测额度
- Codex 已耗尽账号首次可复核，在 `resetAt` 尚未到达或未知时按 30 分钟间隔兜底，也可发现
  官方提前重置。到达最早未恢复窗口的 `resetAt` 后，先等待 10 秒传播余量，再在下一轮后台检查
  复核；reset 后前 5 分钟最小查询间隔为 30 秒，之后仍耗尽则改为 5 分钟。正常账号有非零用量或触顶窗口时，仍在
  `resetAt + 2 分钟` 后复核，未更新则按 30 分钟间隔重试。后台每 30 秒检查条件，实际恢复时间还受
  排队、查询耗时和上游传播影响；查询失败也遵守间隔。无可用点数时，到期前仍按原窗口证据恢复；
  本次耗尽之后的已记录窗口 reset 全部到期后，新查询明确 `allowed=true` 可解除限制，即使用量仍为
  100%。未知 reset 或再次拒绝后残留的旧 reset 仍走原规则，不复用旧边界解封；时间到期本身不会
  解除限制，也不会将展示用量归零
- 账号 `enabled` 控制是否参与请求调度，不控制 OAuth 凭据续期。OpenAI 与 Anthropic 的停用账号仍按各自刷新
  策略维护 Token；刷新结果更新凭据状态，但不会启用调度。无 refresh token 或凭据已进入失效、无效、
  封禁状态的账号不参加后台自动续期
- `POST /api/admin/accounts/recover` 对停用账号只将 `enabled` 改为 `true`，保留已有额度快照、凭据、错误和 Redis
  cooldown；启用后仍按这些事实投影状态，不会把已有错误或耗尽改成正常。对已启用账号则执行强制恢复：
  清除 Redis cooldown 和已保存的额度/错误，恢复为可调度 credential。两条路径均不访问上游；强制恢复
  不验证上游账号是否已经恢复，下一次真实请求仍可重新写入失败事实
- 成功额度观测会 revision-fenced 写入 quota；明确 `Allowed` 投影为 `normal`，明确耗尽投影为
  `quota_exhausted`。额度观测不会清除凭据过期、无效或封禁事实；这些事实统一投影为 `error`，并由
  `errorReason` 区分。额度接口的 401/403 也不足以判定 refresh token 永久失效，credential 终态只由
  OAuth refresh 的明确永久错误写入
- 正常 Responses 请求会解析上游响应的 rate-limit headers，合并进同一 quota 快照并同步状态。Free、
  K12 等套餐共用该状态机；套餐只参与账号展示和按套餐隔离的模型目录 cache，不存在 K12 专属额度路径
- OAuth 账号详情的 Token 统计、模型排行和列表 Token 汇总优先使用账号级周额度窗口，无可统计的周窗口时
  使用月额度窗口；`usage.windowKind` 随选中的窗口返回 `weekly` 或 `monthly`。查询边界
  严格为 `[resetAt - windowSeconds, resetAt)`，不是自然周/月或最近 7/30 天；额度刷新若返回了更早的
  重置时间，会按新边界重新聚合。没有边界完整、可归属到账号的周/月窗口时显示无数据，`usage.windowKind` 为 `quota`
  （周/月额度窗口），不回退到 5 小时、日窗口或历史累计。
  金额原值保持完整精度，USD 展示值
  小于 1 美元时最多保留四位小数，其余最多保留两位，小数末尾不补零。
  额度百分比展示最多保留一位小数，小数末尾不补零；数值为零时返回 `0%`

### Claude 动态额度

Anthropic 账号的 `quota.providerData` 返回 `state`、`source`、`scope`、`queryFailedAt` 和独立的 `responseHeaders` 观测。`state` 区分 `unsampled`、`sampled`、`stale`、`query_failed`、`exhausted`；窗口的 `providerData` 分别保留来源、观测时间和作用范围，缺失百分比或重置时间保持未知

主动查询的动态窗口与账号额度访问事实保存在既有 Provider quota 存储中，提交受 credential revision 和观测时间约束。账号级窗口的明确耗尽参与调度，模型专属窗口不扩大为整个账号耗尽；额外付费用量已启用或状态未知时，不仅凭订阅窗口触顶断定整个账号耗尽。后台每分钟检查是否需要复核，查询间隔至少五分钟，已知 reset 未到时继续保留耗尽；时间到期本身不解除限制，查询失败与令牌刷新也不解除旧额度事实

响应头只保存有界的 `anthropic-ratelimit-*` 原始事实及其时间，不反推订阅余额；API 等价估算费用与订阅额度分别展示。OAuth 账号的周/月预测使用通用 `5h`、`7d` 窗口的响应头观测，见[周/月额度预测](#周月额度预测)

### 周/月额度预测

`GET /api/admin/accounts/quota-forecast?accountId=...` 使用管理员鉴权，每次查询返回独立的预测结果。
该接口不刷新上游额度；需要重新观测时，先调用 `POST /api/admin/accounts/quota/refresh`，再查询预测

响应 `data` 包含 `accountId`、`generatedAt` 和 `forecasts`（`weekly`、`monthly`）：

- `targetDays`、`extrapolated`：对应周期存在真实账号级窗口时采用实际时长；缺少对应窗口时，使用
  可统计的周/月窗口按 7/30 天折算，明确标记 `extrapolated: true`。不把短期限流或模型专属桶当作账号容量
- `estimatedTokens` / `estimatedUsd` 及对应 `*Display`：本周期已记录用量加预计剩余量，
  公式为 `(本周期已记录用量 + 样本用量 × (100 - usedPercent) / sampledPercent) × 目标窗口秒数 / 源窗口秒数`。
  真实周期由上游额度重置边界定义，不按自然周/月累计；只有折算结果才乘以目标与源窗口的时长比
- `source`：源窗口名称 `label`、已用比例 `usedPercent` / `usedPercentDisplay`、
  额度观测时间 `observedAt` / `observedAtDisplay`、用于过期检查的 `resetAt`，以及本周期累计
  已记录的 `tokensDisplay` / `usdDisplay`。`source: null` 表示没有可选的源窗口。
  `observedAt` 表示额度观测时间，与本次查询的 `generatedAt` 不同
- `unavailableReason`：不能估算时的说明，正常为 `null`。有效进度少于 5 个百分点不预测；
  `lowSample` 表示有效进度不足 10 个百分点，或增量采样少于 2 个完整段，仅是质量提示，不承诺精度。
  `incompleteCost` / `incompleteTokens` 仅提示费用 / Token 记录存在缺口，不阻断已有数据的预测。
  Token 使用已记录的正数用量，费用使用已知的有效 USD 金额；对应数据完全缺失时该项为 `null`，
  两项均不可估算时才返回无可用数据的说明。明确记录的零 USD 仍可估算为零，未知费用不能当成零。
  不按请求数量补齐未知消耗；漏记、失败或缺少计量可能使估算偏低，不代表完整账单或真实剩余额度

`sampledPercent` 是公式中的有效额度进度（百分点），不是时间进度或响应字段

样本仅使用当前额度窗口内、账号加入后开始且截至观测时间已完整交付的请求，缓存命中不重复计入输入

重置或套餐变化后重新积累有效进度，中途加入的账号可在新基线后采样，无需等待下一次重置，职责与断点处理见[额度预测架构](architecture.md#额度预测)

预测不参与调度或账单结算，站外消耗、日志清理、异步延迟、未成功交付的上游消耗和模型组合变化均可能造成误差

等价 USD 费用不是订阅价格或额度承诺，30 天折算不是自然月额度，记录覆盖率也不代表预测准确率

### Codex 账号辅助请求

OAuth 账号的额度、个人资料、订阅与重置卡请求先使用 `openai.api.base_url`。自定义上游明确返回 HTTP 404 时，
仅回退一次到官方账号接口，沿用当前账号凭据与出站代理；其他状态码、解析错误或传输失败不触发回退。
部署者需允许账号请求访问官方端点；回退不绕过账号出站代理，也不保证官方可达。
回退后的响应或错误作为最终结果，不以原来的 404 覆盖。重置卡消费回退复用原始请求体和幂等键，
传输失败仍按消费结果不明确处理。API Key 账号不会因此获得 OAuth 账号能力，也不会改变推理请求的目标地址

### 账号个人信息

`GET /api/admin/accounts/personal-info?accountId=...` 需要管理员会话，提供 Codex、Claude OAuth 账号的个人信息。后端并发读取资料统计与订阅，
一次返回；每次请求均重新查询，不刷新 credential，不读取本地 usage/billing 记录，也不缓存或估算统计结果。
OpenAI 查询沿用上述 404 路由回退；Claude 只读取官方订阅元数据和已保存的基础身份，不接入官方个人统计。

响应 `data` 包含：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `basicProfile` | object 或 null | 支持基础资料的账号返回已保存的名称、邮箱、用户 ID、账号或组织 ID 和套餐 |
| `profile` | object 或 null | 官方个人资料、累计统计与活动洞察；查询失败为 null |
| `profileError` | string 或 null | 资料查询失败时的安全错误提示；成功为 null |
| `subscription` | object 或 null | 当前绑定账号的官方订阅信息；无可用信息或查询失败为 null |

两部分的查询结果独立：资料失败时仍返回可用订阅，订阅失败时仍返回资料。账号不存在、查询参数不合法或
无管理员会话时，仍返回标准错误；请求期间账号身份或 credential revision 改变时拒绝整份结果

#### 官方资料与累计统计

`profile` 包含：

- `displayName`、`username`、`imageUrl`：官方账号资料
- `summary`：累计文本 Token、单日峰值 Token、最长任务时长、当前连续天数和最长连续天数
- `dailyUsage`：按日期返回的 Token 活动
- `activityInsights`：快速模式占比、上游原样返回的推理强度及占比、Skill 探索/使用数、聊天总数，
  以及插件与 Skill 调用排行

官方未返回的字段保持 `null`，不使用本地数据补齐；`hasStatsError: true` 表示账号资料可用，但官方统计
部分不可用。access token 已过期或官方返回 401 时，通过 `profileError` 提示先刷新 credential 或重新授权

#### 订阅信息

OpenAI 使用当前凭据、绑定的上游账号 ID 和账号出站代理访问 `/backend-api/subscriptions?account_id=...`，
不枚举其他账号，不返回上游订阅 ID 或原始响应。Claude 使用现有 OAuth 访问 `https://api.anthropic.com/api/oauth/profile`，
核对返回的用户和组织与当前账号绑定一致，读取真实响应中的 `subscription_created_at`、`subscription_status` 和 `billing_type`。

`subscription` 为 `null`（未获得可用订阅信息），或包含以下字段：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `createdAt` | RFC 3339 字符串或 null | 上游订阅开通时间，与本期开始时间不同 |
| `startsAt` | RFC 3339 字符串或 null | 上游本期开始时间 |
| `expiresAt` | RFC 3339 字符串或 null | 上游本期结束时间，不代表自动续费账号最终失效；未提供时为 null |
| `status` | string 或 null | 上游订阅状态，例如 Claude 实测的 `active` |
| `billingType` | string 或 null | 上游计费类型，例如 `stripe_subscription`；不代表计费周期 |
| `willRenew` | boolean 或 null | 自动续费状态；未知不推断为 false |
| `billingPeriod` | string 或 null | 上游计费周期标识 |
| `billingCurrency` | string 或 null | 上游计费币种 |
| `observedAt` | RFC 3339 字符串 | 本次查询时间 |

订阅不写入额度快照或数据库，不参与账号状态或调度；OpenAI 查询含 404 回退共用最多 5 秒预算，响应最多 64 KiB。
上游失败或未提供字段时保持未知，不从 Token 到期时间或额度重置时间推算账期，也不据此标记免费、过期或禁用；请求期间账号身份或 credential
revision 变化时丢弃结果

### 主动额度重置卡

`GET /api/admin/accounts/reset-credits?accountId=...` 每次都查询对应 Provider，不用本地计划代替库存。
当前由 OpenAI Provider 为 OAuth 账号提供，账号视图分别以 `resetCredits` 和
`consumeResetCredit` 表示查询和消费能力

查询响应：

```json
{
  "availableCount": 1,
  "credits": [{
    "id": "credit_...",
    "status": "available",
    "title": "...",
    "expiresAt": "2026-08-31T12:00:00Z",
    "resetType": "..."
  }]
}
```

消费请求的 `redeemRequestId` 必须是小写、带连字符的 canonical UUIDv4；`creditId` 可省略，由上游选择
可用卡。一次请求发出后若传输结果不明确，重试必须复用完全相同的 `redeemRequestId`、`creditId` 和
账号。服务在单副本进程内按账号串行消费，并在 credential 需要刷新时以同一命令重试一次；它不会对不明
结果自动创建新消费

若服务无法确认不可逆消费是否完成，返回 HTTP `502` / 业务码 `50202`；客户端应先刷新卡片与额度状态，
并在确需重试时复用原 `redeemRequestId`。明确的上游 HTTP 拒绝仍使用 `50201`，不会误标为结果未知

```json
{
  "accountId": "acct_...",
  "creditId": "credit_...",
  "redeemRequestId": "8fbf302d-11df-4bd5-82e4-08e4b3df7874"
}
```

消费响应只返回上游结果 `code` 和可选 `credit`。消费端确认成功后应重新 GET 卡片列表，并显式调用
`POST /api/admin/accounts/quota/refresh` 回读上游额度；未提供 `quotaRefresh` 能力时不发起该查询。
不得因为消费成功直接改写本地 `resetAt` 或解除冻结；Anthropic 不支持该能力

消费期间账号身份或凭据版本变化时返回结果未知，不将旧结果套用到新绑定账号

自动使用通过 `/api/admin/accounts/reset-credits/automation` 按账号开启，默认关闭；账号本身停用时暂停执行。
GET 和 POST 均返回 `enabled`、`leadMinutes`、`nextDiscoveryAtDisplay`、`nextConsumeAtDisplay`、`lastResult`，
时间展示沿用部署时区，无计划时为 `null`。运行设置 `resetCreditLeadMinutes` 默认 10，范围 1～1440 分钟

`resetCreditMaxWeeklyRemainingPercent` 是可选的全局剩余周额度上限，范围 0～100 的整数，默认 `null` 表示不限制。
设置后，每次新的自动消费前刷新账号周额度；剩余比例大于上限或无法确认周额度时，保留卡片并在到期前错峰重查，
等于上限时允许使用。仅采用账号级周窗口，不回退到短期、月度或模型专属额度；手动使用不受此上限影响

开启后在一分钟内错峰执行首次发现，此后按部署时区每天随机发现一次，普通发现失败也等待下一日。
已知且有明确到期时间的可用卡片在提前窗口内错峰执行，消费前重新查询；已在其他地方使用或已到期的卡片
会移出计划，查询失败不直接消费。上游返回 `already_redeemed` 后结束该次操作，不切换另一张卡重试。
无卡片 ID 或到期时间时不自动消费；每天发现一次可能错过两次发现之间发放并到期的卡片

`lastResult` 的 `pending` 表示消费结果待确认；重试沿用已持久化的幂等键，重启或关闭再开启也不换键。
`expired_unknown` 表示未确认操作的卡片已经到期，停止重试；`reset`、`already_redeemed` 表示对应终态，
`discovery_failed` 表示最近查询失败，`not_redeemed` 表示上游明确未完成消费。
`weekly_quota_above_threshold` 表示剩余周额度高于上限，`weekly_quota_unavailable` 表示暂时无法确认周额度；
`nothing_to_reset` 表示当前没有可重置窗口，这三种状态均保留卡片并稍后重查，其中 `nothing_to_reset` 的下一次消费使用新幂等键。
结果未知的未决操作继续使用原幂等键确认，不重新应用周额度上限；自动操作未决且对应卡片尚未到期时，手动消费返回 `409`

## 6. 账号分组

分组是 Provider-neutral 的账号集合；一个组可包含任意 Provider 账号，一个账号也可属于多个组。
分组详情和列表返回 `fastMode`，取值为 `default`（默认）、`enabled`（开启）、`disabled`（关闭）。
创建时省略默认为 `default`，更新时省略或 `null` 保留现值。
策略取自 Client Key 绑定的分组，包括已禁用分组；多分组冲突时关闭优先，其次开启，最后默认。
未绑定分组的 Key 跟随客户端，不按最终所选账号的分组判断

- 默认：保留客户端的 `service_tier`
- 开启：所选上游模型的目录声明 `priority` 时，将缺失、`null` 或显式 `default` 改为 `priority`；目录未声明则保留原值
- 关闭：将 `priority`（含 `fast` 别名）改为显式 `default`，继续处理请求

策略只作用于 OpenAI Responses 的顶层 `service_tier`，不改变 `flex`、`ultrafast` 等其他显式档位、嵌套字段或其他 Provider。
宿主策略在 request 中间件之后应用，形成 attempt 输入基线；attempt 中间件继续改写档位时，以改写后的实际发送值为准。
HTTP 和每个 WebSocket `response.create` 均使用请求开始时的分组策略，同一请求重试保持该策略；
HTTP 请求头及新建 WS 的握手提示按当时的最终出站档位构造；复用 WS 时不重发握手头，
每个 `response.create` 仍独立应用档位策略，请求档位统计与本地费用估算使用各帧的最终出站档位

分组详情和完整列表还返回 `allowImageGeneration`。创建时省略默认为 `true`，更新时省略或 `null` 保留现值。
Client Key 任一绑定分组（包括已禁用分组）为 `false` 时，禁止生图；未绑定分组的 Key 允许生图。
限制覆盖 Images 生成与编辑，以及 Responses 中的原生 `image_generation` 工具声明或显式工具选择，
HTTP 返回 `403` 和 `image_generation_disabled`，WebSocket 每个 `response.create` 独立校验并返回对应错误事件。
普通图片输入和 Codex 被动声明的 `image_gen` namespace 不触发拒绝；实际 Images 调用仍受限制。
请求采用开始时冻结的权限，重试期间保持不变；attempt 中间件添加生图工具也会在发送前被策略拒绝

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/account-groups` | `page`、`pageSize`、`search`、`enabled` | 分页查询分组；返回账号可用性、并发槽位（Redis 不可用时 `usedSlots=null`）及成功请求 USD 用量 |
| `GET` | `/api/admin/account-groups/options` | `page`、`pageSize`、`search`、`enabled` | 分页查询分组选择项；item 仅返回 `id`、`name`、`color`、`enabled` |
| `POST` | `/api/admin/account-groups/create` | `{ name, description, color, fastMode?, allowImageGeneration? }` | 创建空分组；`color` 严格为 `#RRGGBBAA`，返回时统一大写 |
| `POST` | `/api/admin/account-groups/update` | `{ id, name, description, color, fastMode?, allowImageGeneration? }` | 更新分组信息、Fast 模式和生图权限 |
| `POST` | `/api/admin/account-groups/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/account-groups/disable` | `{ id }` | 禁用；已绑定 Key 保持受限，不回退到全部账号 |
| `POST` | `/api/admin/account-groups/delete` | `{ id }` | 删除未被 Client Key 引用的组 |

写入返回 `{ id, record, configRevision }`。删除或已提交变更的展示数据暂不可用时，`record` 为 `null`；
客户端可重查列表获取展示数据

两种列表数据均为 `{ items, page, configRevision }`。完整列表 item 返回 `memberCount`、按 Provider 聚合的
`providerCounts` 和 `clientKeyCount`。查询分组成员使用账号列表的 `groupId` 筛选，
不提供独立的分组成员路由；账号的 Provider 不代表整个分组的 Provider。
`capacity.totalSlots` 为 `number | null`：`null` 表示可用成员中存在继承无限并发的账号，`0` 表示没有可用槽位。
分组容量只统计普通池，`capacity.usedSlots` 返回其实际在途数；Redis 不可用时为 `null`

分组费用按当前组成员实际完成请求的费用汇总，不按 Client Key 绑定的分组分摊。
账号属于多个组时，各组均包含该账号的请求费用；调整账号分组后，留存窗口内费用随当前成员关系重新汇总

## 7. Client Key

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/client-keys` | `cursor`、`limit`、`search`、`sortBy`、`sortDirection` | 游标分页查询 |
| `POST` | `/api/admin/client-keys/create` | 创建字段 | 创建带账号范围的 Client Key |
| `GET` | `/api/admin/client-keys/reveal` | `id` | 显式读取完整明文 Key |
| `POST` | `/api/admin/client-keys/update` | 更新字段 | 原子更新名称、分组范围和限额 |
| `POST` | `/api/admin/client-keys/reset-budget` | `{ id, period }` | 管理员清零日／周已用金额；`period` 为 `daily`、`weekly` 或 `all` |
| `POST` | `/api/admin/client-keys/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/client-keys/disable` | `{ id }` | 禁用 |
| `POST` | `/api/admin/client-keys/delete` | `{ id }` | 删除 |

创建字段为 `name`、可选 `label`、`groupIds`、`maxConcurrency`、`requestsPerMinute`、可选
`dailyLimitUsd`、`weeklyLimitUsd`、`customKey`、`providerRequestProfileOverrides` 和 `peerSharing`。更新请求携带 `id`，不接受 `customKey`。
`groupIds` 必须显式提交：空数组派生 `routingScope: "all"`，非空数组派生
`routingScope: "groups"`。响应同时返回分组引用 `groups`，以及从当前有效账号池派生、仅供展示的
`providerKinds`。创建和 reveal 响应会返回完整明文 Key，调用方
必须立即安全保存

`peerSharing` 为 `{ enabled, sharePool, shareQuota, sharePlans }`，各项均为布尔值且默认 `false`。
创建时省略保持关闭；更新时省略或 `null` 保留整个配置，提交对象时替换完整配置，对象内省略项为 `false`。
列表、创建及更新响应均返回完整对象。`sharePlans` 需要 `sharePool`；开启 `enabled` 必须提交非空 `groupIds`，
开启后推理和 [CPR 互联导出](#cpr-互联只读协议) 都限制到这些组内的直连 OAuth 账号。
账号池、原生额度窗口、套餐分布分别按对应开关公开；关闭共享不改变已保存的其他开关值

### Key 客户端身份

`providerRequestProfileOverrides` 按 Provider ID 保存身份选择，列表返回同名对象。创建时各项为完整配置对象，
未提供的 Provider 跟随通用设置；更新时省略某项保留原值，设为 `null` 清除该项覆盖。最多 256 项，每项最多 64 KiB。
独立配置整体覆盖通用设置，不逐字段继承；切换全局配置不会影响独立 Key。目录与选择规则见 [Provider 客户端身份](#provider-客户端身份)

OpenAI Key 的 `installationSeed` 接受带连字符的 UUID，保存时规范化为小写；通用设置不接受此字段。
在 `providerRequestProfileOverrides.openai` 中，继承全局 UA 时使用
`{"inherit":{"installationSeed":"3b11bc82-c666-417d-b9ff-2a34fd040385"}}`；独立预设或自定义 UA 将它与画像字段并列

- 省略、空字符串或 `null`：使用选中账号的安装身份
- 填写种子：按种子、Key 记录 ID 和账号记录 ID 派生安装 UUID；重启、UA 变化或凭据更新不改变结果，换 Key、换账号或修改种子会改变身份
- 种子不发送给上游，也不改写账号凭据；更改或清除种子使生效身份变化时，WebSocket 精确续写须重发完整上下文

兼容字段 `openaiClientProfileOverride` 对应上述映射中的 `openai`。Anthropic 不提供身份覆盖配置
创建时省略或 `null` 表示无覆盖；更新时省略保留、`null` 清除。与通用字段同时提交时必须一致，否则拒绝整个请求；
响应中的兼容字段从同一映射派生，不是第二份配置

### 名称与密钥值

密钥列表的 `search` 仅匹配名称和标签，不匹配密钥值或可见前缀；搜索不区分大小写，使用字面量包含匹配，`%`、`_` 和 `\` 按普通字符处理。
创建和更新时去除名称首尾空白，并按忽略大小写、首尾空格的名称查重，重复返回 `409`。
更新排除当前记录，并发写入同样执行查重；失败不会留下审计或配置版本变更。
历史重名数据不自动改名，已有凭据继续有效；再次保存时需使用未被其他密钥占用的名称

`customKey` 仅用于创建：省略、`null` 或空字符串时继续自动生成；非空时按原值保存，不追加前缀、
不截断、不修剪空白，也不要求固定长度。Key 须为 HTTP Bearer 可传输的非空可见 ASCII 字符，
支持标点，空格、控制字符和非 ASCII 文本会返回 `400`。重复 Key 返回 `409`，包括并发创建时。
应用不另设 Key 长度上限；创建请求和 `Authorization` 头仍受 Web 服务器及反向代理的通用大小限制。
更新接口不接受 `customKey`，防止修改策略时意外替换正在使用的凭据。
列表仅展示最多前 10 个字符，且至少隐藏一半字符；单字符 Key 的可见前缀为空。
完整值仍只通过创建和显式 reveal 返回，不进入普通 Debug 或审计

自定义 Key 创建示例：

```json
{
  "name": "迁入的客户端",
  "customKey": "legacy-platform-key/example+=",
  "groupIds": [],
  "maxConcurrency": 2,
  "requestsPerMinute": 0
}
```

自定义 Key 的分组权限、日／周限额和并发规则按本平台配置执行，不导入其他平台的历史用量

### 限额与调度

金额字段为非负十进制字符串，最多 10 位整数与 10 位小数，`"0"` 表示不限额。
创建时省略金额字段默认为零；更新时省略或 `null` 保留当前值，修改限额不会清空已用金额。
`maxConcurrency` 和 `requestsPerMinute` 是非负整数，零表示不限

列表返回 `dailyLimitUsd`、`weeklyLimitUsd`、`dailyUsedUsd`、`weeklyUsedUsd`（均为字符串）、
`dailyResetsAt`、`weeklyResetsAt`（RFC3339 或 `null`）。
记账和限额比较保留完整精度。
原生日窗口按部署时区的下一自然日边界重置；周窗口从首次准入当天日界起持续七个本地日历日，到期后在下一次使用时重新开启。
切换部署时区不修改已打开窗口的起止点或金额，到期后续接窗口不与旧窗口重叠。
已保存的显式窗口使用指定起止时间，
到期后仍返回当前消费与结束时间，不自动开启下一周期；未开始或已到期时拒绝准入，零金额上限也不例外。
手动重置清零所选周期的已用金额、退出显式模式并清除到期时间，保留限额上限和历史费用，返回 `{ id }`。
所选窗口的重置时间返回 `null`，下次使用时按新建 Key 的规则重新开启。重置前完成但延迟结算的费用不再计入所选周期，也不会开启窗口；
重置后完成的请求继续计费，包括重置时仍在进行的请求。操作保留管理员审计，不改变账号上游额度。
费用按请求完成时间归属窗口。并发按同一 Key 的执行中请求累计，包含 SSE 与每个 WebSocket
`response.create`；空闲连接不占名额，内部重试不重复占用。
修改 Key 策略对既有 WebSocket 连接的下一次请求同样生效，已开始的请求保持原有快照

运行设置可以分别启用 Key 与账号的有界排队。Key 并发满时按 Key 等待；OpenAI 优先亲和下会话请求优先主账号，
繁忙时先按调度策略选择可用账号，临时分流不改写会话绑定；没有可立即使用的账号时才按配置排队。严格亲和下已有绑定的后代线程只等待当前会话账号，
根线程迁移后跟随新账号。绑定缺失时，任何有可靠会话身份的请求都可按调度策略原子首绑，后续请求沿用该绑定

严格亲和下的后代线程始终启用账号排队，沿用 `accountQueueTimeoutSeconds` 和请求截止时间；
`maxWaitingPerAccount` 为 0 时使用每队列 1,000 人上限，否则沿用配置的上限。
可识别来源的 Search、Images、Live 创建请求采用同样规则，身份与轮次关联见[会话绑定](architecture.md#6-路由账号范围与-continuation)。
普通请求关闭账号排队且没有可用容量时返回 `503` / `account_capacity_unavailable`，
与无候选账号的 `no_available_provider` 区分；排队不绕过账号范围、模型权限、RPM 或金额限额

OpenAI 会话因之前的上游 `cyber_policy` 拒绝而排除全部可选账号时，选号失败返回 `400` / `cyber_policy`，
提示停止反复重试该会话，不代表账号被封或服务整体不可用；固定账号请求只判断其绑定范围。
该本地失败禁止自动重放；会话排除期限为一小时，严格亲和后代线程仍按上述规则等待。
同次请求换号耗尽时保留已收到的原始上游错误；后续请求没有上游失败可交付时才使用上述本地提示。
已开始交付的请求沿用流内错误合同，不能再改变 HTTP 状态

队列满返回 `429` / `concurrency_queue_full`，排队超时返回 `429` / `concurrency_queue_timeout`；
WebSocket 使用对应错误事件。等待期间不发送上游请求，取消后释放等待位置，排队重查不重复计入 RPM。
SSE 在 Key 准入完成并取得执行会话前不发送保活帧，此阶段保留 HTTP 错误状态；
取得会话后的账号等待受 SSE 保活覆盖，已开始交付的失败沿用流内错误合同

任一周期的已落库金额与当前进程已接管的待结算费用合计达到限额后拒绝新请求，已准入请求可完成并使金额超过阈值。
HTTP 返回 `429`，`error.code` 为 `key_daily_budget_exceeded` 或 `key_weekly_budget_exceeded`，
并附 `Retry-After`；WebSocket 每次 `response.create` 执行相同检查并返回协议错误事件。
只累计上游上报或按用量与模型价格计算出的 USD 费用；无法取得费用的尝试按零累计，
保留错误和用量诊断，不产生待核账记录或阻断。内部重试中已经取得的费用仍会累计。
预算检查超时、存储不可用或待结算队列无法接受新请求时返回 `503`、`key_budget_unavailable`

自动结算按网关请求 ID 幂等执行，响应交付不等待费用落库，额度查询中的已用金额在后台写入后可见。
删除 Key 会撤销访问，保留请求记录和原 Key 的费用账本。删除前已准入的请求仍按原 Key ID 结算，
同名新 Key 单独记账。请求记录按 `usageRetentionDays` 清理，费用账本不受该设置影响

## 8. 运行设置

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/settings` | 读取运行设置 |
| `POST` | `/api/admin/settings/update` | 原子保存运行设置；身份配置按显式提供的 Provider 项更新 |
| `POST` | `/api/admin/settings/location/detect` | 无请求体，检测网关服务器直连出口的位置，不保存设置 |
| `POST` | `/api/admin/settings/privacy/preview` | 使用管理员提供的样本预览隐私规则，不保存设置或样本 |
| `GET` | `/api/admin/settings/client-downloads/codex-desktop/windows` | 提取 Codex Desktop Windows 离线安装直链；`refresh=true` 强制刷新进程内短缓存 |
| `GET` | `/api/admin/settings/client-profiles/{provider}` | 读取支持身份配置的 `openai` 可选配置和 `globalConfiguration` |
| `POST` | `/api/admin/settings/client-profiles/{provider}/preview` | `{ configuration }`，完整配置对象或 `null`（解析当前通用设置）；使用已加载版本与最近检查时间，不触发上游检查或保存 |
| `GET` | `/api/admin/settings/admin-api-key` | 只返回管理 API Key 是否存在 |
| `POST` | `/api/admin/settings/admin-api-key/delete` | 删除管理 API Key |
| `POST` | `/api/admin/settings/admin-api-key/regenerate` | 重新生成并一次性返回完整管理 API Key |

读取设置返回 `configRevision`。更新请求必须携带读取时的同一版本；版本比较、设置替换和审计在同一事务内完成。
并发修改导致版本过期时返回 `409`，不保存当前请求；读取最新设置并确认差异后再提交，不能静默重试覆盖。
成功响应返回新版本，后续请求使用发布后的快照

位置检测返回 `status: detected` 与 `location`，双栈出口时区不一致时返回 `conflict`，
无法获取出口或位置时返回 `failed` 与 `message`。检测忽略进程代理环境变量，复用代理探测的超时与并发限制；
检测结果须通过设置更新接口保存后才会生效

设置更新字段按用途分组如下：

| 用途 | 字段 |
| --- | --- |
| 并发控制版本 | `configRevision` |
| [客户端身份](#provider-客户端身份) | `providerRequestProfiles`、`openaiClientProfile` |
| [请求位置](#请求位置) | `requestLocationEnabled`、`requestLocation` |
| [隐私策略](#隐私策略) | `codexPrivacyPolicy` |
| 模型映射 | `modelMappings`、`anthropicModelMappings` |
| 凭据刷新 | `refreshMarginSeconds`、`refreshConcurrency` |
| [并发与排队](#并发与排队) | `maxConcurrentPerAccount`、`maxWaitingPerKey`、`maxWaitingPerAccount`、`openaiGuardianReservedConcurrency`、`keyQueueTimeoutSeconds`、`accountQueueTimeoutSeconds`、`switchAccountOnQueueFailure` |
| [上游重试等待](#容量拒绝与重试) | `maxRetryAfterSeconds` |
| [亲和与换号](#账号亲和与换号) | `openaiAccountAffinity`、`openaiSessionAffinityTtlHours`、`anthropicAccountAffinity`、`anthropicSessionAffinityTtlHours`、`maxAccountRotations` |
| [调度策略](#智能调度) | `rotationStrategy`、`smartScheduling` |
| [请求解压](#请求解压上限) | `responsesMaxDecompressedBodyBytes` |
| [客户端门禁](#1-鉴权与公共约定) | `minCodexDesktopVersion`、`minCodexCliVersion`、`minClaudeCodeVersion` |
| [历史保留](#历史保留) | `usageRetentionDays`、`opsEventRetentionDays`、`auditRetentionDays` |
| [Prompt 审查](#prompt-审查) | 独立领域接口管理 `promptGuard` |
| [账号预热](#账号预热) | `accountWarmupEnabled`、`accountWarmupCron`、`accountWarmupModel` |
| [主动额度重置卡](#主动额度重置卡) | `resetCreditLeadMinutes`、`resetCreditMaxWeeklyRemainingPercent` |

`providerRequestProfiles.openai` 与 `providerRequestProfiles.anthropic` 分别配置两种供应商的客户端身份；模型映射分别使用 `modelMappings` 与 `anthropicModelMappings`，不跨供应商借用配置。
Claude 映射作用于 Messages、count_tokens 与原生模型目录：先映射，再校验目标模型、Key 和账号权限；拒绝循环与无法解析的过长映射链。
请求记录保留客户端模型，实际调用、用量与费用使用上游模型。增量 Thread 冻结实际模型，映射后目标改变或旧 Thread 缺少模型证据时须重建会话。

### Prompt 审查

`promptGuard` 核对 codex2api 提交 `811ad451e29d752cc01a13acaacc25074b515034`，使用 CPRS 的来源、类别与复审合同 `cprs-guard-v2`。内部职责见 [Prompt 审查架构](architecture.md#prompt-审查)。
覆盖 Codex Responses HTTP／WebSocket、Claude Messages 和 count_tokens；本地分析读取最终原文，Claude 外部复审投影先经 veil。
同一规则在重复归一化视图中不重复计分；只有当前用户文本参与决策，历史／系统／工具内容只形成审计证据。纯信号和仅观察类别不推高决策分。普通命中遵循本地告警／阻断策略，严格规则独立达到阈值后直接终止，不调用外部复审。
图片、二进制与签名历史按 `text_only` 明示范围；意外遗漏的可执行文本属于覆盖不足

| 字段 | 默认值与合同 |
| --- | --- |
| `enabled` | `true`，不提供按 Key 或模型绕过 |
| `mode` | `enforce` 或 `observe`；observe 保留 `wouldAction` 但放行，Claude veil 仍强制执行 |
| `localAction`、`threshold` | 默认 `block`、50；动作支持 `warn/block`，阈值 1–1000 |
| `strictTerminalEnabled`、`strictThreshold` | 默认 `true`、90；严格分独立累计，阈值不低于普通阈值 |
| `reviewEnabled`、`reviewFailClosed` | 默认 `false`、`true`；关闭复审仍执行本地策略，失败放行不能清除已有本地阻断 |
| `scope` | `local_candidates`（默认）或 `all`；会话锁和严格终止优先 |
| `reviewRequestMode` | `chat_completions`（默认）或 `moderations` |
| `reviewUrl` | 对应格式的完整 `/chat/completions` 或 `/moderations` URL；HTTPS 或 loopback HTTP，不跟随重定向 |
| `reviewModel` | 独立语义节点的模型 |
| `reviewApiKey` | 只写 Key 池，支持逗号、分号、空白分隔；省略保留，空字符串清除；只返回 `hasApiKey`、`apiKeyCount` |
| `reviewMaxConcurrent` | 默认 8，范围 1–64；按进程限制复审与学习调用，池内 Key 轮询，无隐式重试 |
| `moderationThresholds` | 13 个 Moderations 类别的独立阈值，单位 bps（1–10000）；按分类分数判定，不单独采用 `flagged` |
| `timeoutSeconds` | 10，范围 1–60 |
| `confidenceThresholdBps` | 7000，即 0.7；范围 1–10000 |
| `maxTextChars` | 32768，范围 1–262144；约束包含信封的复审投影，不限制为只扫描本地前 32K；必选材料不得截断 |
| `disabledRules` | 停用规则 ID，最多 1024 个；内置与已发布规则使用同一评分器 |
| `conversationLockEnabled`、`conversationLockTtlHours` | 默认 `true`、168 小时；Client Key＋协议＋会话／请求指纹隔离，支持人工解除 |
| `learningEnabled`、`learningModel` | 默认采集脱敏证据；Moderations 模式生成规则草稿须配置同服务商的对话模型；生成与发布分离 |
| `state` | 只读配置状态：`disabled`、`ready`、`candidate_review_unavailable`，不代表远程探测成功 |

节点必须独立于托管 OAuth 池。Codex 候选内容会发送到管理员配置的节点；密钥不随设置读取、审计或普通日志返回。
外部复审开启时必须有有效配置；复审失败按 `reviewFailClosed` 执行，fail-open 保留独立本地结论；observe 记录故障与拟处置。Chat Completions 复审包含来源标记，Moderations 只发送当前用户文本。
本地错误返回原生协议 JSON 与稳定 `prompt_guard_*` 错误码，不触发上游推理、凭据刷新、换号或账号降级。
有效策略阻断、无法归属内容和必选复审材料超限为 400，节点、规则执行或隐私依赖不可用为 503；取消与总期限沿用共享错误合同。

Claude 与审查配置从通用设置更新中迁出，使用以下管理员接口，响应沿用管理信封及 `no-store`：

| 方法与路径 | 合同 |
| --- | --- |
| `GET /api/admin/claude/settings` | 返回 `{configRevision, publicUrl}` |
| `POST /api/admin/claude/settings` | 仅保存 `{configRevision, publicUrl}`；URL 为空或无认证、查询参数及 fragment 的 HTTPS 根地址 |
| `GET /api/admin/claude/privacy` | 运行时、规则版本/包/类别、映射容量、库调用计数、受控错误和部署时区时间；`keyFingerprint` 为主密钥的域分离 SHA-256 指纹，用于与备份清单核对；`packs` 仅在内置规则包生效时返回，自定义规则会整体替换内置包；`rules` 为生效规则的只读视图（`source`、`allowlist`、`items`），正则规则返回 `pattern`，词典规则只返回 `wordCount`；不暴露词典原文、主密钥或映射内容 |
| `GET /api/admin/claude/models?keyId=…` | 指定 Key 的有效原生模型集合；不存在 404、禁用 403、依赖不可用 503 |
| `GET /api/admin/prompt-guard/settings` | `{configRevision, config, retentionDays}`，密钥只返回配置状态 |
| `POST /api/admin/prompt-guard/settings/update` | `{configRevision, config}`，revision 冲突 409；密钥省略保留、空字符串清除 |
| `GET /api/admin/prompt-guard/policy` | 当前规则版本与 `rulesRevision`、类别用途、阈值、表达式和启停状态 |
| `GET /api/admin/prompt-guard/records` | 根请求分页记录、同一查询快照的汇总及共享记录队列健康 |
| `POST /api/admin/prompt-guard/evaluate` | `{text, origin}`；本地评估已保存配置，来源为 `current_user/history/system/tool_output`，不保存文本或调用上游 |
| `GET /api/admin/prompt-guard/candidates` | `page/pageSize` 必填，可按 `protocol/status` 筛选；返回候选、脱敏证据及 revision |
| `POST /api/admin/prompt-guard/candidates/save` | `{id, revision, rule, rationale}`；id=0 创建人工草稿，仅 pending/disabled 可编辑 |
| `POST /api/admin/prompt-guard/candidates/action` | `{id, revision, action}`；支持 `generate/publish/dismiss/disable/reopen`，冲突返回 409 |
| `GET /api/admin/prompt-guard/profiles` | `page/pageSize` 必填，可按协议筛选；最近30天根请求事实及最多100个活动锁事件，受请求保留期约束 |
| `POST /api/admin/prompt-guard/locks/release` | `{id, revision}`；同一次拒绝的会话锁和指纹锁一起解除，操作同事务记审计 |

记录查询必填 `start`、`end`、`page`、`pageSize`（1–100），可选 `requestId`、`clientKeyId`、`protocol`、`model`、`mode`、`action`、`category`、`origin`、`policyVersion`、`reviewResult`、`minScore`、`maxScore`。
精确 `requestId` 查询不受页面时间窗限制，但已清理记录不会恢复；`action=missing` 查询缺失摘要。
来源过滤作用于最多 1024 条已保存命中，其余数量见 `omittedHits`；摘要不含原文与自由文本理由。
队列指标是整个网关进程启动以来的统计，不是 Prompt 专属计数；摘要写入可能失败，不保证与请求终态原子落盘。

明确的 `cyber_policy` 或 `content_policy_violation` 不自动换号，也不归因为账号故障。反馈按根请求去重，候选按协议和当前文本指纹去重；仅完整脱敏证据可调用学习模型，产物始终是草稿。发布／停用规则更新全局规则代次，下一次评估读取新代次。`upstreamPolicy.recorded=false` 表示持久化未确认，锁状态应以画像页为准。已发布规则不会随证据过期被撤销。

候选状态为 `pending → published → disabled`，待审核项可 `dismiss`，已忽略项可 `reopen`。仅 pending/disabled 可编辑，已发布规则须先停用再修改；人工新建规则同样须发布后才生效。
最多同时启用 256 条自定义规则，不能覆盖内置 ID 或发布匹配空串的规则。生成结果必须通过结构和正则校验并命中来源证据，表达式的泛化范围仍由管理员审核。
候选保存、状态变更与解锁均携带 revision，过期返回 409；状态、规则代次及管理审计在同一事务提交。证据仅管理员可见，反馈与学习证据沿用 `usageRetentionDays` 清理，管理审计沿用审计保留期。

`GET /api/key-usage/claude-models` 仅限 Key 浏览器会话，按服务端绑定身份返回目录，不接受客户端指定其他 Key ID，也不读取明文 Key。
账号诊断额外发送 `type=stages`，包含阶段结果、耗时及 Guard 的实际 `mode/action/wouldAction`；未执行阶段不表示成功，诊断不进入普通业务审查记录

### 历史保留

`usageRetentionDays` 控制请求明细及其用量统计的保留天数，默认 31，支持 1～31 的整数天数。
Prompt 上游反馈与学习证据沿用此保留期；清理样本不删除人工发布的规则。
`opsEventRetentionDays` 与 `auditRetentionDays` 分别控制独立运维事件和管理审计，默认 30、90，均须为正整数

清理任务每轮读取已保存设置，按完成时间分批删除过期请求及关联事件，不删除运行中的请求；有界单轮结束后等待一分钟再检查。
缩短保留期后，已有过期数据逐批清理，清理结果不可恢复；Client Key 结算账本不受影响

统计在后台异步更新，清理期间可短暂包含已删除请求的统计贡献，投影完成后更新为剩余数据。
查询范围无需随保留期缩短，超出保留期的已清理数据不再计入结果

### 账号预热

定时账号预热默认关闭。`accountWarmupCron` 使用部署时区的五段 Cron 表达式，
顺序为分、时、日、月、星期，总长度不超过 255 字节，默认空字符串，关闭时允许留空。
支持列表、范围和步长，例如 `0 8,13,19 * * *` 每天执行三次，`0 8 * * MON-FRI` 在工作日 08:00 执行。
星期可用 `SUN` 至 `SAT` 或数字 1 至 7（日到六），同时指定日与星期时两者均须满足。
同一分钟只领取一次；`accountWarmupModel` 默认 `null`，开启前必须填写有效表达式并显式选择模型。

任务面向可用的 Codex OAuth 账号，按账号级额度桶判断，跳过额度耗尽，以及已用比例大于 0 且距离重置仍超过 30 分钟的五小时窗口。
0% 或缺少活跃证据时允许一次轻量预热，不通过倒计时差值判断活跃，也不额外回查额度。
因此已激活但显示 0% 的账号也可能收到预热请求。
不存在的本地时刻跳过，重复时刻只执行较早一次；执行进度跨重启保留，时钟回拨不补跑已领取时刻之前的时段。
只有收到响应成功终态才记为预热成功；预热不计入客户端业务用量

### 请求位置

`requestLocationEnabled` 是必填布尔值，默认 `false`：关闭时不覆盖客户端原有位置和时区；开启时使用已保存的
`requestLocation`。关闭不会清空自定义值，代理自定义位置仍优先。
`requestLocation` 是必填的完整对象 `{ country, region, city, timezone }`，不接受 `null`；初始保存值为
`{ country: "US", region: "Ohio", city: "Piketon", timezone: "America/New_York" }`。
字段约束与[代理位置](#独立代理管理--managed-proxies)一致。全局自定义开启后，OpenAI Responses 使用全局位置，
关联代理配置了自定义位置时优先使用代理值。保存后通过现有配置发布机制对新请求生效，
已开始请求及其重试保持同一份全局值；普通文本、绝对时间戳和数据驻留要求不受影响

这两个位置字段同时作用于 Codex 与 Claude，Claude 没有独立的位置配置。账号（含关联代理）的位置覆盖优先于共享全局值。
Claude 仅覆盖原生 `web_search_*` 的 `user_location`，不插入 Codex 环境标签或改变出口 IP；管理员位置先参与审查，再于脱敏后恢复同一份结构化配置，不恢复用户提供的原文。

### 隐私策略

`codexPrivacyPolicy` 为必填对象，默认 `{ enabled: false, onError: "skip_rule", rules: [] }`。
只处理经网关发送的 OpenAI Responses HTTP／WebSocket 和独立 JSON 端点请求，不控制客户端直接发送的遥测。
规则按列表顺序执行，关闭策略或单条规则会保留配置。没有字段保护名单，认证、会话、工具等字段也可配置；改写后对上游行为的影响由配置者承担

每条规则包含唯一 `id`、`name`、`enabled` 和非空 `steps` 数组。步骤按顺序执行，后续步骤读取前序步骤的结果，
共享规则开关和执行结果。每个步骤包含 `scope`、`selector`、`action`、`pattern`、`replacement`、
`value`、`replaceAll`、`caseInsensitive`、`multiLine`。请求与返回均使用 `steps` 结构

作用范围如下：

| `scope` | 选择对象 |
| --- | --- |
| `turn_metadata` | 请求头、正文及 `client_metadata` 中的 turn metadata 对象，各副本独立处理 |
| `desktop_git_context` | 自动附加的 Desktop Git 上下文中解码后的 JSON 对象 |
| `environment_text` | 已识别的环境上下文或 Desktop 时间上下文文本，`selector` 为 `$` |
| `request_body` | 最终出站 JSON 正文 |
| `request_header` | 最终业务请求头，`selector` 直接填写头名称，重复头逐个处理 |

JSON 路径支持 `$`、`.成员`、`["带特殊字符的键"]`、`[索引]`、`.*` 和 `[*]`，不支持递归和过滤表达式。
缺少字段视为未命中；类型不符属于执行失败。`regex_replace` 替换字符串，`set_value` 保持原字段类型设置 JSON 值，
`rename_key` 替换所选对象的直接键名，重名时失败；`remove_field` 删除所选字段或数组项，不能删除作用范围根节点。
删除动作的 `pattern: null` 表示无条件删除，字符串模式表示只删除字符串值匹配的字段或数组项。
请求头规则生成的值必须是 ASCII 文本，以保持 HTTP 与 WebSocket opening 的一致性；metadata 范围会自动转义 Unicode

WebSocket 请求头在握手时发送。隐私规则改变握手头时，普通请求按对应连接身份选择连接，
处理结果稳定时可继续复用，不因每次执行删除动作而重连。
依赖原连接的精确续接保留原连接，规则变更不能撤回已有握手发送的值

例如，`turn_metadata` 范围的 `$.workspaces` 配合 `remove_field` 删除整个工作区映射；
`$.workspaces.*.associated_remote_urls.*` 配合 `remove_field` 和 `pattern: "private-org"` 只删除命中的远端；
`$.workspaces` 配合 `rename_key` 可改写工作区路径键名。环境文本替换必须保留 XML 包裹结构

管理端提供「统一工作区信息」预设，生成一条含多个步骤的规则，仅处理 turn metadata 的 `workspaces`：

- `/home/<用户名>`、`/Users/<用户名>` 及 Windows 用户目录中的用户名改为 `user`，保留其余目录结构和项目名
- Git 远端 URL 的账号或组织路径段改为 `alex`，保留协议、主机和仓库名，支持 HTTP(S)、SSH、Git 协议和 `git@host:owner/repo` 形式；多级命名空间及本地远端路径保持原值

提交、分支、工作区标识和会话关联信息保持原值。正文中的工作目录及其他执行路径也保持原值，
供客户端工具使用，仍可能包含真实用户名和项目名。安装身份由账号隔离逻辑处理，
UA、平台参数及自动 Git 上下文版本由 [OpenAI 上游客户端身份](#openai-上游客户端身份)管理。
保存后的规则按其 `steps` 执行，不随预设定义变化

一条规则内的所有步骤和承载副本共同提交或回滚。目标为 `null` 时仍遵循类型校验与失败策略，路径键重名时失败，不合并不同工作区

正则使用 Rust `regex` 语法，不支持前后顾或模式反向引用；替换内容支持 `${1}`、`${name}` 和字面量 `$$`。
`replaceAll` 控制首个或全部匹配，另支持忽略大小写和多行锚点。最多 32 条规则，所有规则合计最多 32 个处理步骤；路径最多 16 层、1024 字节，
模式与替换内容分别最多 4096 字节，固定值编码后最多 4096 字节。XML 上下文最多 10000 个节点，环境文本最多 64 层。
执行限制为正文 32 MiB、单目标文本及编码后的上下文 1 MiB、
累计处理文本 16 MiB、每次执行 10000 次访问与匹配步骤。
执行检查点响应请求取消，步数超限报告「规则执行步数超限」，不以固定墙钟时长判定规则失败

执行失败时，`skip_rule` 撤销本条规则所有步骤在所有副本上的修改并继续，意味着本条脱敏未完成；
`reject_request` 阻止发送并返回 `400` / `privacy_policy_rejected`，提示隐私规则处理失败，
不因该错误自动重试或换号。设置保存时校验所有规则，包含未启用的规则

预览请求为 `{ policy, body, headers, turnMetadata }`，其中 `headers` 为头名称到字符串数组的映射，
`turnMetadata` 为编码后的 JSON 字符串或 `null`。正文最多 256 KiB，请求头与独立 metadata 合计最多 256 KiB。
响应返回处理后的三个承载值，以及 `outcomes: [{ ruleId, matches, status, reason }]`；
状态为 `disabled`、`unmatched`、`applied` 或 `skipped`，错误不回显请求内容或表达式

### 并发与排队

`maxConcurrentPerAccount` 是默认账号并发上限，初始值为 3，取值 0～4294967295；`0` 表示不限制。
账号单独设置的正数 `concurrencyLimit` 优先。留空时，互联账号使用有效快照中密钥的正数并发上限，最大取 4294967295。
无有效的上游正数上限时使用本机全局默认值。上游 RPM 供展示，由上游执行。
无限并发仍统计在途请求，并遵守账号可用性与 Client Key 限制

`maxWaitingPerKey` 与 `maxWaitingPerAccount` 是全局统一的普通排队容量，取值 0～1,000，默认 10，`0` 表示关闭；
每个 Key、每个账号各自独立计数，没有单对象覆盖字段。执行并发为 5、最大排队数为 5 时，
该对象最多容纳 5 个执行请求与 5 个等待请求。Key 并发为 0（不限）时跳过 Key 排队。
OpenAI 严格亲和下后代线程的会话账号等待不随普通账号排队关闭，见 [Client Key 等待规则](#7-client-key)。

| 字段 | 默认值 | 行为 |
| --- | --- | --- |
| `keyQueueTimeoutSeconds` | `30` | Key 排队超时，1～120 秒 |
| `accountQueueTimeoutSeconds` | `30` | 账号排队超时，1～120 秒 |
| `switchAccountOnQueueFailure` | `false` | 开启后，账号队满或超时可再选一次允许且可立即准入的账号，不重新排队；关闭或无空位时返回原错误 |

两层从各自首次入队开始计时，账号重选、Provider 切换或内部重试不重置账号时限。
请求总期限优先，排队超时不中断已开始的上游生成。密钥排队失败、必须跟随根线程的后代、固定账号与原生续写不触发换号。
OpenAI 严格模式成功换号后迁移绑定，优先模式临时分流时保留主绑定。
设置更新须提交两层队列容量、两个超时值和换号开关，对新请求生效

`openaiGuardianReservedConcurrency`（默认 5，取值 0～4,294,967,295）设置 Codex Guardian 自动审批的每账号独立并发，保存后对新请求生效。
Guardian 以 `subagent_kind` 或 `client_metadata.x-openai-subagent` 值 `guardian` 识别。取值 R 大于 0 时，
普通请求保留账号原上限 L，Guardian 单独计数并最多运行 R 个，两类请求互不占用名额，也不借用对方空位。
例如 L 为 10、R 为 3 时，可以同时运行 10 个普通请求和 3 个审批请求；普通并发不限时，审批仍受 R 限制。
设为 0 时关闭独立额度，Guardian 使用普通并发。开启账号排队后，审批请求单独按 FIFO 等待，
不受普通队列位置或单账号排队上限约束，仍受自身队列总等待容量与等待时限约束。
两类请求仍遵守账号可用性和 Client Key 限额。设置更新请求须包含该字段

### 账号亲和与换号

`openaiAccountAffinity` 控制 Codex 账号亲和，默认 `strict`（严格）：

- `relaxed`（宽松）：会话内请求直接按 `rotationStrategy` 选号，不优先主账号
- `preferred`（优先）优先使用会话主账号，短时满载、限流或模型不匹配时临时分流，后备按同一会话的稳定顺序参与既有调度
  主账号被删除、停用或凭据过期、失效、封禁时，在后备账号成功准入时原子迁移绑定
  额度耗尽且重置时间未知或距离当前时刻超过账号排队时限时迁移，接近重置时保留主绑定，并给刚过重置时间的额度复核同长度宽限
  原生续接仍固定其状态归属账号
- `strict`（严格）：同一会话共用账号，后代请求等待当前账号并跟随根请求换号

原生续写的状态归属约束在三种模式下仍然生效

Chat Completions 和 Responses 通过原生 OpenAI Provider 选号时，缺少显式会话 ID 的请求会在同一 Client Key 下匹配账号亲和。
网关先匹配客户端传入的 `prompt_cache_key`，否则按上游模型、开头的指令和首条用户消息的文本匹配。
后续消息及工具列表不参与匹配。优先模式允许临时分流；严格模式沿用根请求的等待与迁移规则，启用账号排队时等待繁忙的绑定账号，关闭排队或账号失效时允许重新选号并迁移绑定。
宽松模式、没有缓存键又缺少对话开头文本的请求，仍按普通调度选号。内容亲和只用于选号，不建立会话身份或续接归属

`openaiSessionAffinityTtlHours` 是账号绑定及其会话关联的滑动保留时长，单位小时，默认 24，取值 1～720。
各类请求统一在发送前成功准入时按请求冻结值续期。
响应完成不回写或续期。调整时长不扫描已有 Redis 键，已有记录保留原到期时间，下一次成功准入时使用新值；
真正过期或丢失后按缺失绑定处理，存储读取错误不会被当作过期

`maxAccountRotations` 是单请求最大换号次数，默认 3，取值 0～31；0 表示不换号。
首次选号、同账号重试和选号时过滤不可用候选不计入换号次数，总路由尝试仍最多 32 次。
提高该值允许请求尝试更多账号，但不放宽安全重放或交付后的重试限制。
设置更新请求须包含账号亲和、亲和时长与最大换号次数，保存后对新请求生效，执行中请求及其重试沿用冻结值

### 请求解压上限

`responsesMaxDecompressedBodyBytes` 保留历史字段名，作为 Codex Responses/Chat 和 Claude Messages/count_tokens
共用的 HTTP 请求体上限，压缩正文按解压后大小检查，未压缩正文同样检查，默认 67108864（64 MiB）。
必须为正整数，且可表示为进程平台的 `isize`；管理端以整数 MiB 编辑。

保存并发布成功后，新请求使用新值；在途请求及其重试沿用冻结预算。调高上限会增加大请求的内存占用，不代表扩大模型 Token 上限、响应缓冲或整个进程的内存预算。

### 智能调度

`rotationStrategy` 可取 `smart`、`quota_reset_priority`、`round_robin`、`sticky`

`smartScheduling` 是必填的完整对象，仅在内置 `smart` 策略下生效，切换其他策略时仍保存其值：

```json
{
  "loadWeight": 1.0,
  "quotaWeight": 0.8,
  "healthWeight": 1.0,
  "latencyWeight": 0.5,
  "resetWeight": 0.0,
  "queueWeight": 0.0
}
```

六项系数分别调整负载、剩余额度、健康、首个有效输出延迟、额度重置和排队压力的评分偏好，范围 `0～10`、最多一位小数，
至少一项大于 `0`。`0` 仅关闭对应评分维度，不放宽账号资格、额度或并发限制；系数不表示流量百分比。
缺字段、`null`、未知字段及无效数值均返回 `422`，不写入配置。读取设置额外返回只读
`smartSchedulingDefaults`，内容为上述默认对象，用于恢复默认与自定义状态比较，不可提交到更新接口

`resetWeight` 越大越偏向即将重置额度的账号，复用已有有效重置时间，未知或已过期时不加分。
`queueWeight` 只在没有可立即使用的账号、需要选择等待队列时参与评分，越大越偏向等待人数少的账号。
设为 `0` 时沿用最短队列规则；启用后结合其余五项评分选择队列。队列人数限于当前进程，只有开启账号排队才会生效，
不影响已入队请求的位置、队内 FIFO、容量上限或等待超时。两项默认均为 `0`

没有可用亲和时，在最高可用权重层内按配置评分。原生续写账号绑定仍是硬约束。
配置随运行设置原子保存和发布，新请求使用新值，已开始请求及其重试沿用原快照，无需重启

### 模型定价

定价接口独立于运行设置的整体替换，仅允许管理员访问。改价影响本地估算费用与 Client Key 金额限额，
不改变上游报告金额、订阅账号实际扣额或历史费用。请求开始时冻结价格，内部重试沿用同一份配置

| 方法 | 路由 | 请求与返回 |
| --- | --- | --- |
| `GET` | `/api/admin/settings/pricing` | 返回 `{ defaults, synced, overrides, syncedAt }` |
| `POST` | `/api/admin/settings/pricing/update` | `{ provider, models, change }`，成功返回 `{ saved: true }` |
| `POST` | `/api/admin/settings/pricing/sync/preview` | 无 body；返回 `{ prices, skipped }`，不写入配置 |
| `POST` | `/api/admin/settings/pricing/sync` | `{ preview: { prices, skipped }, models: { openai: ["gpt-5.4"] } }`；成功返回 `{ saved: true }` |

价目使用 `Provider → 精确上游模型 ID → { multiplierBps, bands }` 的映射。优先级为人工覆盖、已同步价目、
内置价目；按档位合并，不从客户端模型别名或响应模型猜测价格。`syncedAt` 为 ISO 时间或 `null`。
每个档位包含四个非负十进制字符串：`input`、`output`、`cacheRead`、`cacheWrite`，单位 USD / 百万
Token，范围 0～1000000、最多四位小数。`"0"` 表示免费，缺少整个档位表示继承；不能只缺少部分单价

`bands` 可用键为 `standard`、`fast`、`ultrafast`、`flex`、`long_standard`、`long_fast`、`long_ultrafast`、`long_flex`、`image`。
OpenAI 长上下文为输入超过 272000 Token，Anthropic 已知 Sonnet 模型为输入超过 200000 Token；仍按 Provider 支持的
模型与服务档位判断是否能估算。OpenAI 内置 Ultrafast 价目覆盖 `gpt-6-astra` 与 `gpt-6.1-sol`。
Anthropic 不接受 `flex`、`long_flex`、`ultrafast`、`long_ultrafast`、`image`。图像端点的 `standard`
用于文本输入，`image` 用于图像 Token；不能用文本输出单价替代图像输出单价。用量无法完整拆分时不估算

`multiplierBps` 为 0～1000000 的整数，10000 表示 1 倍、0 表示本地估算为零，最大 100 倍。
它作用于本地费用及对应明细，独立于服务档位；缺少计价依据的请求即使倍率为零仍然是费用未知

Anthropic 请求费用仍是公开 API 等价估算，不代表订阅扣费或 Priority 容量合约账单。
实际服务档位与速度分别来自响应 `usage.service_tier`、`usage.speed`；Priority 不等于 Fast，
请求 `service_tier=auto` 也不代表实际档位。响应速度优先于请求速度；普通请求缺少速度时按标准价估算，
请求 Fast 却没有响应速度、缺少对应价格档位、Batch 或包含未计价服务端工具时保持费用未知。
内置 Fast 价覆盖 `claude-opus-5-5`、`claude-opus-5`、`claude-opus-4-8`，全上下文同价；
五分钟与一小时缓存写入分别按当前模式输入单价的 1.25 倍、2 倍估算，人工缓存写入单价覆盖此规则。

费用观测的 `pricing.serviceTier`、`pricing.speed` 保存实际响应枚举；`pricing.speedMultiplierBps`
保存应用自定义倍率前当前模式与标准模式总费用的比值（基点），不表示 Priority 的合同倍率。
`costBreakdown.standard` 是同一请求按标准模式计算的 USD 金额，与 `total` 应用相同的自定义倍率。
列表及详情只展示请求当时保存的费用与价格；历史记录缺少新增字段时显示未知，不按最新目录重算。

`provider` 为内置 `openai` 或 `anthropic`，对应价目由 `GET` 返回的 `defaults` 提供。
`models` 为 1～500 个模型 ID，每个 ID 为
1～128 字节且不含空白、控制字符。`change` 为以下形式之一：

- `{ "action": "replace", "pricing": { "multiplierBps": 12500, "bands": { ... } } }`：替换选中模型的人工
  配置，未提供档位重新继承来源；内置和同步均未登记的模型必须包含 `standard`
- `{ "action": "multiplier", "multiplierBps": 20000 }`：设置目标倍率，保留已有人工单价；重复提交不连续相乘
- `{ "action": "reset" }`：移除人工单价与倍率，恢复同步价或内置价；仅有人工价格的模型恢复为未配置
- `{ "action": "delete" }`：删除非内置模型的同步价目与人工配置；批量包含任何内置模型时整批返回 400，
  即使该内置模型已有人工覆盖也不能删除。删除不影响历史账单，之后可重新添加或选中同步导入

一批更新原子提交并写审计，不覆盖未选中的模型。未知字段、错误类型和非法价格字符串等 JSON 合同错误
返回 422；Provider、模型 ID、批量数量、倍率上限和不支持的档位等业务校验错误返回 400。
models.dev 同步只导入可表示为当前文本 Token 计价的 OpenAI/Anthropic 模型；不完整价格、其他输出模态及
不匹配的上下文梯度在 `skipped` 中返回 `provider/model`。确认会重新抓取价目；若与预览不同返回 400，
需重新预览。来源不可用返回 502，已有价目保持不变。`preview` 必须原样提交，`models` 按 Provider
指定 1～10000 个待同步模型；空选择或未登记的模型返回 400。同步只更新选中模型的来源层，未选中模型
及所有人工单价与倍率保持不变。选中的已同步模型若不再出现在来源价目中，则移除其来源层，恢复内置价格；
没有内置价目的模型变为未配置

### Provider 客户端身份

`providerRequestProfiles` 是按 Provider ID 索引的通用配置对象，最多 256 项、每项最多 64 KiB。
更新时省略某项保留原值，对象替换该项，`null` 删除显式选择并使用 Provider 默认值；不清除其他 Provider 的配置。
配置内容由对应 Provider 校验，不包含账号凭据。Key 可通过 `providerRequestProfileOverrides` 整体覆盖所属 Provider 的选择

`openaiClientProfile` 是该映射中 OpenAI 的兼容字段：省略保留，不能显式提交 `null`；
与通用字段同时提供时必须一致，否则整次更新拒绝。读取响应从映射派生该字段，不维护平行状态

身份选项和预览复用 `GET /api/admin/settings/client-profiles/{provider}` 与
`POST /api/admin/settings/client-profiles/{provider}/preview`。OpenAI 保持既有返回格式。
Anthropic 返回自身配置和有效身份，使用 `providerRequestProfiles.anthropic` 与 Key 的同名覆盖项。

### Claude 上游客户端身份

默认配置为 `mode=preset`、`client=cli`、`platform=macos`、`arch=arm64`、`versionMode=latest`。
客户端支持 `cli`、`vscode`，平台支持 `macos/linux/windows`，架构为 `arm64/x64`；
`versionMode` 支持 `latest/stable/fixed`，固定版本须提供 `version`。
`mode=custom` 要求完整 `userAgent`、对应 `version`/`client` 以及有效的 `sdkVersion`/`runtimeVersion`；
`mode=follow` 要求请求携带完整一致的 Claude UA、x-app 和 SDK/环境身份。
SDK 默认 `0.128.0`，Node 默认 `v26.12.1`，独立于自动同步的 CLI 版本。

UA、SDK 头、正文版本标记来自请求内同一快照。设备身份按内部 Key ID、账号 ID 和可选 Key
`installationSeed` 派生；更换访问令牌不改变设备身份，账号 UUID 来自所选 OAuth 账号。
会话使用显式 UUID 或新生成 UUID，不由提示词、IP 或路径推导。
Key 仅设置种子时使用 `{"inherit":{"installationSeed":"<UUID>"}}`，保持继承全局画像。
控制请求使用默认画像的身份头，不携带推理 metadata、billing 或 Key 会话；count_tokens 不注入生成 metadata/billing。

后台启动及每 24 小时在分布式租约内读取官方 latest/stable，先提交共享发布缓存再发布内存状态；
每进程每分钟读取共享缓存。失败保留最后成功版本，离线基线标注日期。
预览返回有效身份、发布来源、核验/检查时间、同步错误和独立模板修订；CLI 版本同步不代表模板重新核验。
已有 Thread 恢复保存的身份；自动发布更新不使它失效，显式配置、安装种子或模板变化会拒绝续接。

### OpenAI 上游客户端身份

`openaiClientProfile` 保存通用选择，首次默认 `MacOS · Desktop · 自动最新`。
兼容字段的更新语义见上节。初始化不读取 YAML 身份字段。
该配置作用于 Client Key 的 OpenAI 模型请求与原生模型目录，适用于 HTTP/SSE、WebSocket、Images 和 Search。
不改变 Anthropic、入站客户端版本门禁、账号认证或后台 Desktop 专属操作

自定义配置使用 `{ "mode": "custom", "userAgent": "完整 UA" }`。
已识别的 `Codex Desktop`、`codex-tui`、`codex_exec`、`codex_cli_rs` 前缀由后端解析 `originator` 和 Core `version`，
显式提供的配套字段必须与识别结果一致。未知前缀须另填 `originator` 和 `codexVersion`，这两个字段与 UA 一起发送。
UA 须为 1 至 4096 字节的单行可见 ASCII 文本，首尾不能含空白；`originator` 最多 128 字节，
`codexVersion` 最多 64 字节并须符合 SemVer。自定义配置不要求 Desktop 构建号，也不自动更新

没有 `mode` 字段的预设配置按以下合同解析，可选字段省略或 `null` 时使用所选预设参数：

| 字段 | 取值与语义 |
| --- | --- |
| `client` | 必填，`desktop` 或 `cli` |
| `platform` | 必填，`macos`、`linux` 或 `windows` |
| `versionMode` | 必填，`latest` 或 `fixed` |
| `versionLag` | 仅 `latest` 可选的滞后数量，1～10 的整数；`fixed` 必须省略或为 `null` |
| `cliEntry` | CLI 可选 `tui` 或 `exec`，省略或 `null` 保留 Core 默认身份；Desktop 不接受此字段 |
| `originator`、`osType`、`osVersion`、`arch`、`terminal` | 可选自定义参数，非空、最多 128 字节；只接受可见 ASCII，不能包含括号、分号、反斜杠及首尾空白 |
| `codexVersion` | `fixed` 必填的 Core SemVer；`latest` 必须省略或为 `null` |
| `desktopVersion`、`desktopBuild` | 仅 Desktop 的 `fixed` 模式必填，分别为数字点分版本和数字构建号；CLI 不接受这些字段 |

```json
{ "client": "cli", "platform": "linux", "versionMode": "latest", "cliEntry": "tui", "osType": "Alpine Linux", "osVersion": "3.24.1", "terminal": "xterm-256color" }
```

TUI 默认标识为 `codex-tui`，Exec 为 `codex_exec`，入口后缀使用同一次解析的 Core 版本。
`originator` 覆盖只更改产品名前缀和配套头，后缀仍表示所选入口。省略 `osType` 使用平台名称；
自定义运行环境在自动更新时保持不变。未指定 `cliEntry` 时使用 `codex_cli_rs` 默认值且不添加入口后缀

`terminal` 填写官方客户端的真实终端标记，如 `iTerm.app/3.5.0`、`vscode/1.99.0`、`Apple_Terminal`、
`Ghostty`、`WindowsTerminal`、`tmux-256color`、`xterm-256color`。省略时使用 `unknown`：
官方 Desktop 由 GUI 启动、无终端环境变量，其 UA 中的终端标记本就是 `unknown`，
因此 Desktop 预设保持默认即可；CLI 预设建议配置真实值，多 Key 可通过各自的画像覆盖差异化

`versionLag` 让 `latest` 模式跟随官方发布但滞后 N 个已观察版本采用，省略时采用最新版本。
档位基于网关观察到的发布序列解析：启动种子与历次 24 小时检查的核验版本按时间倒序构成历史，
取第 N 项；序列不足时回退最旧已核验版本，不会因此解析失败。历史仅保留在进程内，
重启后从启动种子与制品缓存重新积累

六套预设均支持自动更新：macOS Desktop 支持 arm64，Windows/Linux Desktop 及三套 CLI 支持 arm64、x86_64。
预设接口的 `automaticAvailable`、`reason` 表示当前组合的可用性；自定义架构可能使自动解析不可用。
每 24 小时后台检查官方稳定发布，失败保留同组合上次有效版本；固定值不受后台更新影响。
Desktop 的应用版本、Core 和构建号来自同一平台、架构的官方制品：macOS ZIP、Windows MSIX、Linux DEB。
Windows/Linux 通过 ETag 检查更新，未变化时复用已核验版本；CLI 依据官方 npm 稳定标签和对应平台依赖

预览返回 `configuration`、`source`（`global` / `override`）、`userAgent`、解析后的环境和版本字段，
以及 `versionSource`（`official` / `custom`）、`versionLag`、`verifiedAt`、`checkedAt`、`error`。
自定义预览中的 `recognized` 表示是否识别出配套请求头。
`verifiedAt` 只表示版本资料核验，不能代表自定义运行环境或 TLS 已核验；固定版本返回 `null`。
完整自定义配置不携带官方制品核验时间。
客户端画像配置控制应用层请求字段，不切换操作系统的 TLS 实现。默认 HTTP 使用 native TLS，
WebSocket 使用 rustls；配置自定义 CA 时 HTTP 也使用 rustls。TLS 指纹需按实际部署平台与传输路径核验。
未完成本次启动检查时 `checkedAt` 为 `null`。非法或当前不可用的选择返回 `400`，保存失败不提交其他修改

Responses 的自动 Desktop Git 上下文与请求头使用同一画像快照：已有 `appServerVersion` 同步为 Core 版本，
`appVersion` 同步为 Desktop 版本；CLI 或自定义 UA 未提供 Desktop 版本时使用 `null`。
只处理完整、可解析的自动上下文，保留 Git 状态、会话字段和普通对话，不补充缺少的版本字段。
该同步不依赖隐私策略开关；显式隐私规则在其后执行，仍可覆盖最终字段

配置在请求开始时冻结，Provider 首次解析的版本用于该请求的全部重试与换号。
WebSocket 精确续写沿用所属连接；安装身份配置与变更限制见 [Key 客户端身份](#key-客户端身份)

### Windows 客户端下载

Windows 离线包接口固定解析 Microsoft Store Product ID `9PLM9XGG6VKS` 的 Retail 包，不接受调用方提供
产品 ID、上游地址、ring 或文件名。后端只返回通过包名、架构、Microsoft CDN host/path、scheme 和失效
时间校验的 `x64` / `arm64` MSIX 直链，不代理安装包字节。Store 内容通道返回 HTTP/80 临时地址时保留
原始 scheme，不强制改写为该 host 不保证支持的 HTTPS。动态链接不足 10 分钟即失效时不会下发；某个架构
解析失败时只将该架构降级到 OpenAI 官方 HTTPS 稳定 MSIX，并通过 `warning` 说明。Store 解析最多等待
30 秒，官方回退包通过最多 3 秒的 HEAD 请求补充大小，大小查询失败不影响下载地址。响应形状为：

```json
{
  "resolvedAt": "2026-09-01T06:30:00Z",
  "cached": false,
  "warning": null,
  "packages": [
    {
      "architecture": "x64",
      "source": "microsoft_store",
      "version": "26.825.6671.0",
      "fileName": "OpenAI.Codex_26.825.6671.0_x64__2p2nqsd0c76g0.msix",
      "sizeBytes": 780402688,
      "downloadUrl": "http://dl.delivery.mp.microsoft.com/filestreamingservice/files/...",
      "expiresAt": "2026-09-01T07:30:00Z"
    }
  ]
}
```

`source` 为 `microsoft_store` 或 `official_openai`。Store 的四段 package version 只用于下载展示，不参与
Desktop 三段 SemVer 门禁，也不会自动回写最低版本设置。门禁规则见
[鉴权与公共约定](#1-鉴权与公共约定)，解析器职责见 [架构文档](architecture.md#11-生命周期安全与恢复)

## 9. 备份

全部备份端点位于 `/api/admin/settings/backups/*`，使用管理接口响应信封，字段为 camelCase，响应带 `Cache-Control: no-store`

归档为 `.sql.gz` 业务备份，包含账号、Key、设置、已用额度和已发布用量汇总，不包含请求明细与诊断/审计日志。
汇总范围和离线恢复限制见 [备份内容与限制](../deploy/README.md#备份内容与限制)；没有在线恢复端点

Claude 隐私存储（veil 映射卷）已初始化时，同一条记录在数据库导出完成后再生成映射库快照，以同名
`.veil.tar.gz` 对象一并上传，两个对象都通过远端校验后记录才进入 `completed`。映射卷从未初始化时只上传数据库归档；
已初始化但无法生成快照时，整条备份以 `backup.privacy_store_failed` 失败，不产出缺少映射的备份。
快照归档不含 veil 主密钥，恢复步骤见 [Claude 隐私存储恢复](../deploy/README.md#claude-隐私存储恢复)

统计重建或时间桶尚未发布完成时，任务以 `backup.analytics_not_ready` 失败；统计就绪后重新创建备份

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/settings/backups` | 无 | 读取存储配置（含明文 Secret）、验证状态与调度配置 |
| `POST` | `/api/admin/settings/backups/storage/update` | S3 配置 | 更新存储配置；`secretAccessKey` 为空字符串会校验失败 |
| `POST` | `/api/admin/settings/backups/storage/test` | 无 | 测试已保存的存储配置（Put/Head/Get/Delete 探针） |
| `POST` | `/api/admin/settings/backups/schedule/update` | 调度配置 | 更新 Cron 与保留策略 |
| `GET` | `/api/admin/settings/backups/records` | 查询参数 | 分页查询备份记录 |
| `POST` | `/api/admin/settings/backups/create` | `{ expiresInDays? }` | 创建手动备份，返回 `202 Accepted`；`expiresInDays` 为过期天数（0 或缺省表示不过期） |
| `POST` | `/api/admin/settings/backups/download-url` | `{ backupId, artifact? }` | 创建 5 分钟有效预签名下载地址（仅 completed）；`artifact` 为 `database`（缺省）或 `privacyStore`，记录不含隐私存储时请求后者返回 `404` |
| `POST` | `/api/admin/settings/backups/delete` | `{ backupId }` | 请求删除（进入 `deleting`，由 Worker 收敛硬删除） |

读取设置响应（Secret 以明文返回）：

```text
storageRevision, endpoint, region, bucket, accessKeyId, secretAccessKey, prefix,
forcePathStyle, verified, scheduleEnabled, cronExpression,
retentionDays, retentionCount, nextRunAt, nextRunAtDisplay,
lastVerifiedAt, lastVerifiedAtDisplay, updatedAt, updatedAtDisplay
```

更新存储请求字段：

```text
endpoint, region, bucket, accessKeyId, secretAccessKey, prefix, forcePathStyle
```

`secretAccessKey` 为空字符串会校验失败；由于 GET 会回传已保存的明文 Secret，保存时始终整体提交当前值。已有备份记录时，endpoint/region/bucket/forcePathStyle 不允许变化（存储身份锁定，`409`）；只允许轮换凭据与修改 prefix

保存相同配置保留验证状态、定时计划及配置版本。存储配置实际变化时，会同时使验证失效、暂停定时计划并清空下次运行时间；连接测试通过后需重新启用计划

更新调度请求字段：

```text
scheduleEnabled, cronExpression, retentionDays, retentionCount
```

`cronExpression` 为 5 段格式，按部署 `host.timezone` 解释；不存在的本地时刻跳过，重复时刻只执行较早一次。
调度时区没有独立请求字段。`retentionDays`/`retentionCount` 为 0 表示禁用对应清理。启用计划前必须已保存完整存储配置且通过连接测试

记录列表查询参数：

```text
page, pageSize, status, trigger
```

`status` 可取 `queued/dumping/uploading/completed/failed/deleting`；`trigger` 可取 `manual/scheduled`。记录响应字段：

```text
id, triggerKind, status, scheduledAt, objectKey, sizeBytes, sha256, privacyStore, attemptCount,
errorCode, errorMessage, startedAt, completedAt, expiresAt, createdAt, updatedAt
```

`privacyStore` 为 `{ objectKey, sizeBytes, sha256 }` 或 `null`，在进入 `uploading` 时与数据库归档的大小和摘要一起写入。
删除记录时先删除隐私存储对象，再删除数据库归档，任一对象删除失败都保留记录等待下一轮重试

记录中的时间点同时返回对应的 `*Display` 字段

`expiresAt` 在创建时确定：手动备份来自 `expiresInDays`，计划备份来自当时的
`retentionDays`；到期后由 Worker 进入删除流程

连接测试响应：

```text
{ ok, stage, code, message }
```

`stage` 为 `putObject/headObject/getObject/deleteObject`。探测成功后以 `storageRevision` CAS 写入 `lastVerifiedAt`；测试期间配置变化则丢弃结果

备份错误映射（`AdminErrorCode` 既有体系）：

| HTTP | 场景 |
| --- | --- |
| `400` | 配置、Cron 或状态参数无效 |
| `404` | 备份记录不存在 |
| `409` | 已有活跃任务、状态冲突或存储身份锁定 |
| `502` | S3 兼容服务返回无效或失败响应 |
| `503` | PostgreSQL、`pg_dump` 或对象存储暂不可用 |

审计动作：`backup.s3_config_updated`、`backup.s3_connection_tested`、`backup.schedule_updated`、`backup.created`、`backup.download_url_created`、`backup.delete_requested`。审计详情与记录表均不保存 Secret、数据库连接串或预签名 URL query

## 10. Dashboard、用量与错误

Dashboard 的当前账号摘要包含 `upstream`，结构与账号列表一致，用于统一互联身份展示；
`planType` / `planTypeDisplay` 仍保留原生订阅含义。历史请求记录保持请求发生时的身份与订阅快照

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/dashboard/summary` | Dashboard 汇总；支持 `kind`、`provider` 与统一时间范围参数 |
| `GET` | `/api/admin/dashboard/trend` | Dashboard 趋势；`kind=usage\|latency\|errors`，可按 `provider` 筛选 |
| `GET` | `/api/admin/usage/records` | 请求记录分页列表 |
| `GET` | `/api/admin/usage/records/detail` | 按 `id` 查询请求详情 |
| `GET` | `/api/admin/usage/records/summary` | 当前筛选条件的请求汇总 |
| `GET` | `/api/admin/usage/insights/overview` | 完整用量洞察 |
| `GET` | `/api/admin/usage/insights/health` | 请求健康 |
| `GET` | `/api/admin/usage/insights/performance` | 响应性能；`metric=latency\|firstToken\|throughput\|load` |
| `GET` | `/api/admin/usage/insights/cost` | 成本效率；`metric=cost\|savings\|cache` |
| `GET` | `/api/admin/usage/insights/diagnostics` | 按维度聚合诊断 |
| `GET` | `/api/admin/operations/errors` | 运维错误分页列表 |
| `GET` | `/api/admin/diagnostics/export` | 按 `requestId` 或 `errorId` 下载诊断 ZIP 附件 |

用量汇总的 `averageLatencyMsValue` 为可空毫秒数，前端按持续时长格式化；无延迟样本时为 `null`，
`averageLatencyMs` 保留为兼容展示字段

Dashboard summary / trend 的 `provider` 可取 `openai` 或 `anthropic`，省略时查询全部供应商；筛选贯通对比、趋势、健康、最近请求、账号与配额、容量及客户端身份。
请求数按根请求统计，不把内部重试另算用户请求；零 attempt 拒绝仅在已有真实候选供应商时记录归属，未知归属不根据模型名猜测。未知费用与无首 Token 样本保持缺失语义，不记成零。
统计页的时间和供应商作用于整页，Key、账号文本、模型与成功状态只筛选明细，不额外影响上方统计卡片。

### Dashboard 趋势指标

趋势摘要 `summary[]` 返回稳定的 `metric`、展示数值 `value` 和可空的 `ratio`，指标名称由前端本地化

| kind | metric |
| --- | --- |
| `usage` | `inputTokens`、`outputTokens`、`cachedInputTokens` |
| `latency` | `firstTokenP95Ms`、`latencyP95Ms`、`outputThroughputP50` |
| `errors` | `errors`、`successRate`、`requests` |

图表颜色、系列身份和选中状态使用 `metric`，不根据译文或数组位置推断指标

`points[]` 提供趋势图与首页卡片使用的请求数、Tokens、缓存比例、错误数、成功率、
`latencyValue`、首 Token P95、总延迟 P95 和输出吞吐 P50，以及时间标签与展示值
请求并发与内部调度等待通过 `/api/admin/usage/insights/performance?metric=load` 查询

### Dashboard 客户端身份

`wireProfiles[].attributes[]` 返回稳定的 `key` 和 `value`，字段名称由前端本地化

- `client_identifier`：上游客户端标识
- `client_mode`：客户端运行模式
- `token_auth`：认证协议
- `version_policy`：版本选择策略，值为 `fixed` 或 `latest`

页面按 `key` 取值；状态判断使用原始值，不依赖界面译文

### Dashboard 容量与账号用量

Dashboard 的 `capacityInfo` 统计普通容量，不含独立审批池；`maxConcurrentPerAccount` 为默认账号并发上限，`0` 表示不限制。
`capacityInfo.totalSlots` 为 `number | null`；可用账号池含无限并发账号时为 `null`，此时 `availableSlots` 也为 `null`。
`usedSlots` 仍表示实际在途数，Redis 不可用时为 `null`；没有可用账号时 `totalSlots` 为 `0`

Dashboard 的 `accountUsage[]` 返回稳定指标 `metric=requests|tokens`、可空原始数值 `metricValueRaw` 和
可空 RFC3339 `lastUsedAt`；前端负责指标名称和数字本地化，相对时间按[页面时间合同](#页面时间合同)翻译 `lastUsedRelative`。
`requests` 使用代表性额度窗口的本地请求数，`tokens` 使用今日 Token；未知数值为 `null`，零值保留 `0`。
`metricLabel`、`metricValue` 和 `lastUsed` 保留给兼容调用方，当前管理端不解析或显示这些字段

`accountUsage[].notes` 为账号当前备注，无备注时为 `null`

`usageWindow` 复用账号额度窗口合同，缺失额度事实时为 `null`；窗口标签、百分比、触顶状态、重置时间
和本地用量由 Provider/Admin 投影。套餐缺失不表示免费套餐，显示时舍入的百分比不用于判断触顶

滚动窗口使用相应时间范围的本地用量，独立于 Dashboard 的今日统计范围

### 请求字段与筛选

Dashboard 默认 `period=today`，用量、诊断和错误查询默认 `period=7d`，范围参数遵循[页面时间合同](#页面时间合同)。
Dashboard 返回原始 `asOf` 及 `asOfDisplay`，作为当前数据的查询锚点

请求明细列表接受 `currentPage`、`pageSize`、时间范围、`provider`、`clientApiKeyName`、`accountSearch`、`model` 和 `status`。旧参数 `accountName` 返回 `400`。
汇总和洞察接受时间范围与 `provider`；性能和成本另要求 `metric`，诊断另接受 `dimension`、`model` 和 `statusCode`。
各接口拒绝未知参数，明细筛选不改变顶部统计范围

请求列表的 `status` 可取 `success`、`error`、`all`，默认 `success`；未知值返回 `400`。
`success` 沿用成功用量记录范围，`error` 包含最终 `failed`、`incomplete` 的请求，
`all` 合并成功用量记录及全部非成功请求，包括执行中和已取消请求。每个请求只返回一行，
重试中间错误不改变请求最终分类；`logicalOutcome` 保留真实结果。状态筛选只影响明细列表，
不改变顶部统计，独立运维事件仍通过 `/api/admin/operations/errors` 查询

请求列表的 `errorCode` / `errorMessage` 提供最终请求的错误码与错误信息，未记录时为 `null`

请求列表与详情的 `errorKind` 保留网关分类，`websocketCloseCode` 只提取已保存关闭帧的数字码，
与原始上游 `errorCode` 分开。`continuationUnavailableReason` 和 `upstreamConnectionExitReason`
保留已记录的续接及连接退出原因，缺失字段为 `null`，不根据错误文案推断。
`recovered` 表示已关联到后续成功请求，原请求的 `logicalOutcome` 保持不变。
这些字段用于本地化摘要，原始错误信息及受控诊断仍按各自合同保留

`upstreamError` 仅在请求详情返回，从已保存的上游 JSON 投影 `code`、`errorType`、`message`、`param`，
或 WS 关闭帧的 `closeReason`、`lastEventType`；不重新分类错误，也不包含原始请求、任意响应头或本地原因链。
缺失、非 JSON 或未识别的结构返回 `null`，完整受控诊断通过诊断包获取。
详情的 `message` 是网关摘要，不等同于上游 `upstreamError.message`；
`retryAfterMs` 是已记录的重试等待毫秒数，未记录时为 `null`

诊断 `dimension` 可取 `model`、`account`、
`apiKey`、`provider`、`transport`、`failureClass`、`status`。诊断按请求量降序返回最多 100 项，
同请求量按维度标识稳定排列

`requestShare` 的分母为该维度筛选后、截取前的全部请求数；`failureClass` 只在带错误类型的请求内计算占比
`retryCount` 为额外执行尝试次数之和，`retryRate` 为发生过重试的请求数占该组请求数的比例，同一请求多次重试只计一次

使用记录列表和错误排查列表的 `accountPlanType` / `accountPlanTypeDisplay` 来自记录保存的账号订阅快照。
账号维度诊断使用该账号最近请求的订阅快照，并通过 `authenticationKind` 返回同一请求的认证类型快照；未保存的字段为 `null`，非账号维度的认证类型为 `null`

管理端请求列表及 Dashboard 最近请求中的 `accountNotes` 来自请求保存的账号备注快照，未保存时为 `null`

请求列表、Dashboard 最近请求及账号维度诊断的 `accountUpstreamPeer` 来自账号当前的互联摘要，
已发现且有观测时间时为 `true`，尚未识别成功时为 `false`；摘要缺失、账号已删除或读取失败时为 `null`。
该字段仅供 CPR 标识展示，不写入历史快照，非账号维度诊断为 `null`

`/api/admin/usage/records` 与 `/api/admin/operations/errors` 的 `clientApiKeyName` 来自请求保存的 Key 名称快照，
未保存时为 `null`，不影响记录返回，不包含密钥原文

上述快照字段不关联当前配置，改名、修改订阅或删除配置不改变已有快照。补写的历史 Key 名称与订阅取自补写时仍存在的配置，
不保证等于请求发生时的值；无法补齐的字段保持未知

运维错误记录的 `errorDetails` 为受控诊断文本，无详情时为 `null`。
错误来源快照以 JSON 文本保存：`causes.messages` 按外层到内层排列本地原因，`causes.truncated`
标记来源链截断；存在附属清理失败时，`causes.cleanup` 保存各自的原因快照。
`upstream` 保存上游正文，`redacted` 标记令牌等敏感上下文的移除；没有对应来源时为 `null`。
稳定错误分类、原始上游 code 和该详情分别保留，不以公共错误文案代替原始原因。
详情不出现在 Key 用量接口和普通错误信封中；[诊断导出](#诊断与恢复关联)在服务端重新脱敏后包含已存详情。
记录仍受异步观测写入与保留周期约束

管理端请求列表与详情保留 `requestedModel`（客户端请求）与 `upstreamModel`（网关发送）

Claude Messages 的推理配置按请求原文分别记录：`reasoningEffort` 为显式的 `output_config.effort`，
`thinkingMode` 为 `thinking.type`，`thinkingBudgetTokens` 为显式 token 预算；
不从模式、预算或模型名称推断强度，也不填入 Codex 的 `reasoningPreset`。
`reasoningObserved: true` 表示已采集该请求的推理配置，全部缺省时界面显示“未指定”；
旧记录或未采集的请求保持 `null`，界面显示“—”。这些可选字段同时用于管理端和 Key 自助用量接口。
count_tokens 只采集请求模型，不将计数请求记为推理配置。
思考 Token 仅使用上游明确给出的 `usage.output_tokens_details.thinking_tokens`，缺失或大于输出总数时保持未知；
该计数已包含在 `output_tokens` 中，不重复累加 Token 或费用。

请求明细的 `clientApiKeyName` 搜索历史 Key 名称，`accountSearch` 搜索请求发生时的账号名称或邮箱快照，同时适用于 Codex 和 Claude。
输入去除首尾空白后按字面量包含匹配，区分大小写；`%`、`_` 和 `\` 按普通字符处理。缺少邮箱的记录仍可按名称匹配，账号后来改名或删除不改变历史结果。
明细、分页总数及其内部聚合计数使用相同条件；时间、平台、状态及授权范围仍同时生效。前端修改筛选会清空旧结果并回到第一页，顶部统计卡片仍只按时间和平台查询。
Key 自助页的 Key 范围由服务端固定，不开放账号文本搜索。`model` 按同样的字面量包含规则匹配请求模型或上游模型

运维错误列表的 `search` 使用字面量包含匹配，`%`、`_` 和 `\` 按普通字符处理，不支持密钥值或可见前缀搜索。
请求详情通过 `/api/admin/usage/records/detail?id=...` 查询

### 卡片洞察查询

请求健康、响应性能、成本效率与热点诊断分别查询自己的路由；性能和成本按选中的 `metric` 查询，诊断按 `dimension` 查询。
各分项接口均支持时间范围和平台筛选。
`overview` 返回完整洞察，管理端卡片使用分项接口

健康、性能与成本接口返回 `analytics`、`granularity`、`metric` 和 `data`。
健康的 `metric` 固定为 `health`，`data` 含健康摘要和趋势；性能与成本的 `data` 含 `summary` 和 `points`，
每个点包含 `bucket`、`label`、`requestCount` 和所选指标的 `value`。缺失样本保留 `null`，不填充未查询指标

性能摘要与趋势分别返回总耗时分位数、首 Token 分位数、吞吐分位数或负载指标；只有 `metric=load` 读取请求并发。
成本的三个选项共用费用、节省、溢价、每次成功请求费用和覆盖率摘要，趋势分别返回实际与无缓存估算费用、缓存节省或缓存比例。
缺少 `metric`、选择其他卡片的指标或传入未知指标均返回 `400`

### 记录范围与统计口径

OpenAI 推理强度优先取 `input` 中最后一条 `type: "configuration_update"` 项的 `reasoning.effort`，
原生续接未再次更新时沿用已关联响应的配置，否则取请求顶层 `reasoning.effort`。
该字段是请求观测，不改写上游请求；网关未持有的续接历史无法用于推断档位

首页趋势、请求健康与热点诊断统一口径：错误为服务失败与未完成之和，成功率为 `成功 /（成功 + 错误）`
已恢复记录、客户端取消与调用方错误不计入，进行中不参与比例；调用方错误仅指失败终态的 4xx，不含取消请求的 499
无有效样本时，成功率与错误率返回 `null`，趋势线保留断点
请求健康保留各终态的独立计数，热点诊断的 `errorCount`、`errorRate` 使用相同口径，中间尝试失败不重复计入
`completionRate` 表示成功或失败终态占总请求的比例，`nonCompletionCount`、`nonCompletionRate` 表示取消与未完成的数量及其占总请求的比例
健康时间线以 15 分钟分桶，可用性不低于 95% 且有效样本不少于 10 个时显示稳定
没有成功且服务失败与未完成合计不少于 3 个时显示不可达，其他不足 10 个有效样本的时段显示低样本
有效样本为零时显示无有效样本，尚未到达的时间格显示未来

已进入模型执行会话、但尚未登记执行尝试的失败请求也进入错误及详情查询，
包含无可用账号、准备失败、启动/准备超时与取消。此时 attempt 数为零，未确认的 Provider、账号及
上游传输为空；可用模型执行 ID 在“全部平台”下查询，不从路由候选推断实际调用平台。
已登记的尝试即使尚未收到首个上游事件，也会保留真实 attempt。
执行会话前的拒绝记录为运维错误，不创建模型请求或用量。HTTP 鉴权、解码、未知路由、方法错误和
请求中间件提前失败使用 `api/http_ingress`，可按入口关联 ID 查询；路由、权限、取消与准入拒绝由 Core 记录。
摘要只含方法、路由模板、状态、原因码和已知参数路径，不保存请求正文、查询串或凭据。
这些记录均为 best-effort；异步投影仍可能延迟或丢弃

性能洞察的 `performance.load` 和 `performance.points[].load` 使用相同的负载结构，分别表示查询范围汇总与时间桶指标：

| 字段 | 含义 |
| --- | --- |
| `averageConcurrency` | 按有效时长加权的平均请求并发 |
| `peakConcurrency` | 最大重叠请求数 |
| `admissionDecisionP95Ms` | 准入判定耗时 P95，单位毫秒，无样本为 `null` |
| `accountSelectionWaitP95Ms` | 账号选择等待耗时 P95，单位毫秒，无样本为 `null` |

并发按逻辑请求的执行区间计算，不因重试 attempt 重复计数。
区间从 `started_at` 到 `completed_at`，执行中请求按查询范围末端仍在处理计算；与时间桶重叠的请求参与该桶，
平均值按重叠时长加权，峰值取桶内最大重叠请求数，无请求的时间桶为零。全范围平均按有效查询时长加权，峰值取各桶最大值

汇总与洞察中的请求数与 outcome 分布覆盖筛选范围内全部请求；token、缓存、延迟与成本聚合仅统计
已完整交付客户端的成功推理响应。OpenAI 的 `generate: false` 连接与上下文准备记录归类为
`requestKind: "prewarm"`，不进入用量列表、账号用量或额度预测的 Token / 费用覆盖统计，但仍可按
请求 ID 读取审计详情，响应中的额度观测仍可用于预测配对。此分类以实际 `generate` 字段为准，
不能仅凭客户端的同名 metadata 或输出 Token 为零排除普通推理；其他 Provider 不套用该规则

`latencyMs` 从模型执行会话开始计到终结，包含账号选择、重试及流交付等待，不包含此前的入口解析、路由和准入。
`firstTokenLatencyMs` 与它使用同一计时起点，首字边界由 Provider 协议定义。
OpenAI Responses 在 HTTP SSE 收到首个非空且非 `[DONE]` 的 `data` 载荷、
或 WebSocket 收到首个非空响应消息时记录首字，不要求实际输出内容；
Anthropic 采用首个非空文本、思考或工具参数增量。对应事件缺失时保留未知，不用其他事件补齐首字。
请求级首字统一通过 `firstTokenLatencyMs` 返回，由前端格式化。
`latencyDetails` 的首事件、首推理和首正文时间也使用请求级起点，首推理与首正文仍要求实际内容，
连接、响应头等传输阶段耗时独立计量，不能直接相加作为总耗时

`latencyDetails.upstreamResponseMs` 是上游返回的本次响应耗时，与网关请求计时独立。
OpenAI 仅使用同一条 `response.completed` 中的 `created_at` 与 `completed_at` 时间差，
将秒转换为毫秒保存，不拼接前导事件、会话累计计时或本地观测。
字段缺失、时间倒序或时间跨度为零时保持缺失；未采集官方计时的记录不会用网关耗时补齐。
返回整数秒时间戳时，短请求受秒级取整影响，不能用毫秒单位推断来源具有毫秒精度

`latencyDetails` 还可包含以下上游性能指标，单位为毫秒，保留上游返回的小数。
OpenAI 从当前响应的 `responsesapi.websocket_timing.timing_metrics` 读取，
WebSocket 握手不主动添加专项计时请求头，客户端显式提供的开关保留原值。
各字段独立可选；响应开始前、终态后的计时和明确属于其他响应的事件不写入当前请求，重试时清空

`critical_path` 仅采集 `scope=response`、`coverage=complete` 且边界为
`actionable_output_item_done` 的响应级指标：引擎耗时取 `engine_wall_ms`，API 开销为
`responses_pre_inference_ms` 与 `responses_other_ms` 之和，缺失分项不按零补齐。
外层 `logical_turn` 累计时间不进入请求指标，也不用于补齐引擎 TTFT 或 Token 间隔

| 字段 | 上游口径 |
| --- | --- |
| `upstreamApiOverheadMs` | API 排除引擎与客户端工具时间后的耗时 |
| `upstreamEngineMs` | 响应级引擎耗时，取完整 critical path 的墙钟时间或直接返回的 Engine Service 时间 |

以上指标按上游内部口径展示，不相加重建总耗时，不替换请求级 `firstTokenLatencyMs`。
它们仅补充性能诊断，不改变列表与聚合统计的速率分母

列表与性能统计的速率为 `outputTokens × 1000 / upstreamResponseMs`，只在两者为正时计算。
输出 Token 保留上游用量口径，OpenAI 的输出已包含推理 Token，不重复相加或扣除。
该值表示上游响应创建到完成期间的平均输出速率，按官方时间戳精度计算，
不表示模型内部的纯解码速度，也不用于替代网关观测的首个输出等待与请求总耗时

### 诊断与恢复关联

详情接口按 `id` 可读取成功、失败或未完成请求，普通详情与列表不传输时间线。
管理员通过 `GET /api/admin/diagnostics/export` 下载 ZIP 附件，必须且只能指定
`requestId` 或 `errorId`；`errorId` 指运维错误事件 ID，无模型请求的后台错误也能导出。
响应类型为 `application/zip`，设置 `Content-Disposition: attachment` 与 `Cache-Control: private, no-store`。
包内 `diagnostics.json` 保存完整结构化证据，`summary.txt` 汇总请求结果、模型、时延、用量与缺口，
`README.txt` 说明证据边界；三个文件均来自服务端脱敏后的文档

诊断包 `schemaVersion: 5` 用于故障分析，不是备份或导入格式。后端按目标 ID 收集请求、尝试、
完整已存时间线、错误原因链、原始上游错误及恢复关联；前端只负责下载。
`requests[].evidence` 保存详情、`trace`、`relatedRequests`、事发配置版本和尝试原因；
`requests[].errors` 保存同一请求的错误记录，`error` 保存按事件 ID 选中的错误，该事件不在请求内重复导出。
`environment.scope = export_time` 明确版本、构建与部署信息是导出时状态，不能代替事发环境

`request.routed.data.accountPolicy` 保存执行时冻结的账号策略与预算。
OpenAI 账号队满或超时后的重选记录为 `account.queue.reselect`；绑定迁移成功后，
`account.binding.migrated` 保存拒绝原因 `reason`、原账号 `fromAccountId` 和新账号 `toAccountId`。
首次认领、临时分流和失败重选不记录为绑定迁移

`attempt.failed.data.recovery` 保存 Provider 的重放安全性和恢复意图；`retry.decided` 保存当时
实际参与决策的门槛、剩余预算和发送状态，`retry.policy` 保存宿主策略的停止事实。
已有的 `provider.precommit.released` 数据保留释放原因、预取字节数与等待时间，不等同于下游已提交

Trace v3 为路由、账号选择、失败、重试、响应边界及终态事实预留独立的 64 KiB 预算，
普通事件另有 64 KiB 预算，总事件数最多 128。包含元数据的完整 JSON 快照最多 128 KiB，
生成与持久化共用同一上限；完整编码超限时先淘汰普通事件，再淘汰关键证据。
`stageCounts` 记录至多 64 类已采集阶段的次数，
事件淘汰不抹除阶段计数；`droppedEvidenceEvents` 与 `stageCountOverflow` 分别报告关键证据淘汰和阶段统计超限

OpenAI Responses 的 `provider.request.input` 与 `provider.request.outbound` 分别记录 Provider 收到的
已解析正文和最终准备发送的正文指纹，包括指令、工具、缓存键、模型设置与有序输入。
输入保留前 16 项的独立指纹、每 16 项一块的前 32 块指纹及全量输入指纹，超出分段范围明确计数。
出站关联头仅保存指纹，`provider.request.account_scope` 保存账号归属依据及状态处理前后的指纹。
这些观测不重组转发正文，准备发送不等同于确认发送，指纹也不证明上游缓存仍存在

`provider.response.usage` 从已解析的上游终态单独提取已知数值字段，区分缺失、无效和明确的零，
同时保存共享 parser 的解析结果；`request.usage` 保存最终提交持久化前的网关归集值。
`provider.response.parse_gap` 标记 JSON 或 canonical 投影失败，原始 wire 仍按原有规则透传。
上述关键事实随请求终态保存；未采集的事实无法由导出补回

采集阶段仍按有界 trace 合同处理未知 JSON 键和值，未知事件名称保留摘要；导出不会补造被省略的原文。
下载端按 Provider 凭据、管理认证与传输头合同移除令牌、密钥、认证头、Cookie 和 URL 认证部分，
移除账号联系信息及客户端 IP，保留关联 ID、错误 code/message、套餐、状态和重试事实。
OAuth 字段依据 CPR 导出结构及官方 Codex `TokenData`，不按 `token / secret / credential` 等泛称删除业务字段。
原始错误中可解析的嵌套 JSON 展开后脱敏；任意业务内容不属于凭据识别范围

收集共享 20 秒预算，最多包含 8 个关联请求和 128 条错误记录，优先保留主请求的最终错误。
导出另有文本、节点与深度预算，正文截断仍保留格式版本、目标和缺口；超限会写入 `exportTruncated`、`redaction.truncatedValues` 和缺口记录。
`availability.gaps` 明示未记录、淘汰、截断、读取失败与旧记录缺少的决策事实，`omitted` 说明主动省略范围。
异步观测尚未落盘、已过期记录、未采集的请求正文与上游内部状态无法由导出补回，`null` 不表示没有发生错误

错误记录中的“已自动恢复”表示系统关联到了后续成功请求，不会把原来的失败记录改为成功。
`upstreamSendState = ambiguous` 表示无法确认该次上游执行结果，不代表后续恢复请求失败；
恢复关联也不等于逐字节验证过两次请求正文

### 费用与服务档位

OpenAI 与 Anthropic 的本地费用估算按实际发送给上游的请求模型（`upstreamModel`）查价，结合响应中的实际
用量计算；客户端请求 A、路由后发送 B 时按 B 计价，响应返回 C 不改变计价模型。实际发送模型缺少
定价时不估算，也不借用响应模型的价格。Provider 明确上报的已计费金额仍优先于本地估算

OpenAI Responses 用量记录的 `serviceTier` 与本地费用估算统一采用 Provider 最终发给上游的请求
`service_tier`，不使用响应档位覆盖或回退。例如发送 `priority`、响应回显 `default` 时，仍显示
`Fast` 并按 Priority 价格估算。未发送档位时，`serviceTier` 保持缺失，展示与估算按标准档处理。
计费展示把 `priority`/`fast` 映射为 `Fast`，`flex` 映射为 `Flex`，缺失或 `default`/`standard`
映射为 `Standard`；其他非空值以首字母大写展示。各模型、档位及长上下文区间使用明确登记的价格；缺少对应
价格时不估算（包括 `auto` 和未知档位），不以固定倍数兜底。展示的倍率由所选档位与标准档位费用之比
计算，托管工具调用费不随 Token 档位倍增。
Provider metadata 分别保留 `requestedServiceTier` 与 `upstreamServiceTier` 供诊断；发送给客户端的
原始 `response.service_tier` 不变。用量中的 Fast 仅表示发送档位，不能证明上游实际加速，本地费用
估算也不能代替官方账单。档位与费用在请求记录生成时确定；查询不会回填历史档位或重算已存储费用

本地计价使用[模型定价](#模型定价)的生效规则。缺少内置、同步及人工价格时不生成估价。本地
费用明细、有效单价、服务档位与自定义倍率随终态记录持久化，后续改价和清除覆盖不重算历史明细。
记录没有费用快照时，按内置规则核对总额后补充拆分，核对失败只显示原总额。
`billing.longContextBillingApplied` 表示已应用长上下文价格区间，与服务档位、自定义倍率独立；
费用快照未记录该事实或只有总额时返回 `false`，不按当前价格倒推历史标识。图像明细通过可选的
`billing.image` 返回 `inputAmountDisplay`、`cacheReadAmountDisplay`、`inputPriceDisplay` 和
`cacheReadPriceDisplay`；存在该字段时，普通输入与缓存字段仅表示文本输入，输出字段表示图像输出

Anthropic 请求的 `metadata.billingKind` 为 `api_equivalent_estimate`，`rawUsage` 保存原始计数、缓存 TTL 细分及已知服务档位、速度枚举，
`pricing` 保存当次计价依据，`costBreakdown` 保存同精度费用项。输入总量包含普通输入及缓存读写；独立 total_tokens 未报告时保持未知。
5 分钟／1 小时混合写入不投影为单一官方单价，附加工具费或未知速度价目无法确认时不生成完整总费用；订阅剩余额度另从真实窗口观测

## 11. 版本、更新与重启

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/system/version` | 无 | 当前构建、部署模式及进程内的运行通道摘要 |
| `GET` | `/api/admin/system/update/detail` | `refresh?`、`summaryOnly?`、`channel?` | 按临时通道读取或刷新 Release 详情 |
| `GET` | `/api/admin/system/update/events` | 无 | SSE 更新事件流 |
| `POST` | `/api/admin/system/update` | `{ targetVersion, channel? }` | 受理后台在线更新，返回 `202` |
| `GET` | `/api/admin/system/update/status` | 无 | 查询当前更新或回滚状态 |
| `POST` | `/api/admin/system/rollback` | 无 | 回滚到保留的上一版本 |
| `POST` | `/api/admin/system/restart` | 无 | 受理重启 |

版本响应的 `timezone` 为部署配置 `host.timezone` 的 IANA 时区名，供管理端日历筛选使用。

在线更新遵循[版本命名与升级规则](../deploy/README.md#版本命名与升级规则)。版本接口的
`updateChannel` 由当前版本推导，取值为 `stable`、`alpha`、`beta`、`rc`、`exp`，无法识别时为 `unknown`。
详情接口的可选 `channel` 仅作用于本次查询，不保存实例偏好；省略时按当前运行版本推导。
响应的 `policy` 包含本次 `channel` 与 `availableChannels`，普通实例可选 `stable`、`rc`、`beta`、`alpha`，
实验实例仅允许 `exp`。不可用通道返回 `40901`，未知枚举值返回参数错误。
执行时应同时发送页面确认的 `channel` 与 `targetVersion`，服务端冻结该通道并重新复核远端目标；
省略通道时按运行版本推导。版本接口只读本地构建和已有运行通道摘要，缓存未就绪时 `updateCached=false`，不会发起网络请求。
检查与执行使用同一规则，禁止的通道转换、跨实验线、跨大版本、降级或同版本重装均以 `40901` 拒绝。
`hasUpdate=true` 仅表示当前构建支持在线更新，且存在允许的更高版本；`latestVersion`、`releaseUrl` 和
`notes` 对应这个候选。没有可升级候选时，`hasUpdate=false`、`latestVersion` 为当前版本，
`releaseUrl` 和 `notes` 保留当前版本的已发布 Release 信息；找不到匹配当前版本的 Release 时为空。
当前构建不支持在线更新时不查询 Release，`hasUpdate=false`、`latestVersion` 为当前版本，
`releaseUrl` 和 `notes` 为空，不支持原因通过 `updateSupported=false`、`unsupportedReason` 返回。
`summaryOnly=true` 优先于 `refresh`，按发布索引检查版本，仅对更高候选核对公开 Release 和当前平台附件；`notes`、`releaseUrl` 为空。详情或摘要检查失败时返回 `warning` 和 `hasUpdate=false`，此时 `latestVersion` 不代表已确认的通道版本。GitHub 限流时，`retryAt` 为最早可重试的 RFC 3339 时间，`retryAtDisplay` 按部署时区展示；其他情况两者均为 `null`。
普通查询缓存 20 分钟，手动刷新最短间隔为 60 秒；临时错误缓存 60 秒，限流按来源退避。
后台安装重新核对通道和目标版本，目标变化须再次检查并确认；下载与校验流程见[在线更新](../deploy/README.md#管理端在线更新)

更新 POST 在本地校验目标版本并持久化任务后返回 `202`，数据包含 `operationId`、`targetVersion`、
`deploymentMode` 和 `message`，只表示已受理。Release 查询、远端目标复核、下载、校验及文件替换在后台
执行，结果通过 `/update/status` 的 `operation` 查询：`status` 为 `idle`、`running`、`succeeded` 或 `failed`，
终态包含 `finishedAt`，失败原因在 `error` 中。SSE 的 `operationId` 用于关联进度；终态事件发出前状态已落盘。
连接中断不取消已受理任务；响应丢失时先查询状态，不自动重复提交。打开更新页面时也会恢复最近一次任务。
下载并解包后校验发行身份；文件替换失败或取消时成组恢复二进制与 Web 资源。

重启受理后，旧服务拒绝更新、回滚和重复重启，返回 `40901`；客户端等待新进程就绪后重新连接

状态响应的 `currentVersion` 表示已安装文件的版本，运行中的版本仍以 `/version` 为准。
`needRestart=true` 表示已验证的安装文件尚未在当前进程生效，此时应调用重启接口，不能重复发起更新或切换通道。
最近一次 `operation` 仅表示操作历史，成功记录不等于待重启，也不改变远端 `hasUpdate`。
手动部署后按实际文件校准 `currentVersion`；无法核实的回滚备份不返回 `previousVersion`。
运行期间的外部文件变动或不完整安装返回错误，不伪装成安装成功。
Host 关闭或任务取消会记录失败终态；状态查询会收敛无执行锁的遗留 `running`。
异常退出留下的锁仍遵循 30 分钟过期规则，未过期前不会抢占其他进程的操作。
实例升级和仓库发版见 [部署文档](../deploy/README.md#镜像升级与源码构建)
