# 后端测试

测试维护当前 CPRS 的业务合同。旧用例可以复用，但预期必须对应二开行为；删除功能时同步删除该功能的专属用例，不能通过降低生产约束、跳过有效用例或放宽业务断言让 CI 变绿。

## 覆盖范围

| 范围 | 主要验证内容 |
| --- | --- |
| Core | 鉴权、授权范围、路由、请求提交屏障、失败分类、预算、取消、租约释放和计费事实 |
| Codex | HTTP／SSE／WS、原生续接、完整历史恢复、Strict／Preferred 亲和、故障代理避让、目录缓存、OAuth、额度、账号身份隔离 |
| Claude | Key 及版本准入、四个原生入口的权限和 veil 门禁、压缩正文上限、Thread 校验、模型反向分页、明确撤销／封禁、5 分钟／1 小时缓存计费、未定价模型 |
| veil | 首次初始化、重启复用、checkpoint 的 Key 绑定、旧卷不完整时拒绝初始化、所有控制面出站门禁 |
| 管理与存储 | 名称／邮箱历史快照、双供应商查询、旧参数拒绝、Key 自助隔离、事务与 CAS、备份恢复、保留期、Redis 冷却／原子准入／ACL |
| 架构与宿主 | 依赖方向、模块边界、配置与后台任务生命周期 |

账号搜索必须验证改名、删除后历史记录不变，空邮箱仍可按名称匹配，特殊字符按字面值查询。恢复绑定只做选号，不能向新账号发送旧增量，也不能增加生成次数或费用。

需要访问私有协议细节的单元测试使用独立 `tests.rs` 与私有 `#[cfg(test)] mod tests;` 入口，避免为测试扩大生产 API；架构检查仍禁止生产逻辑按测试配置切换实现或使用隐藏路径加载。

不维护已退役的冻结保护／探测恢复任务、xAI 专属规则、固定 15.5 MiB 请求大小预降级及内容拒绝后的账号／会话封锁预期。通用多供应商测试使用 Codex、Claude；内容拒绝测试验证不影响下一次独立请求。

## 本地执行

先按照 [开发环境](development.md#环境与依赖) 准备工具链和 veil 源码。所有模拟上游都使用本机临时监听端口，不需要真实账号。数据库测试应使用独立 PostgreSQL 18、Redis 8 实例和临时凭据，禁止连接业务实例。

从仓库根目录执行：

```bash
sh scripts/prepare-veil.sh
export VEIL_SKIP_UI_BUILD=1
export RUST_MIN_STACK=16777216
export CPR_TEST_DATABASE_URL='postgres://cprs_test:<password>@127.0.0.1:<port>/cprs_test'
export CPR_TEST_REDIS_URL='redis://:<password>@127.0.0.1:<port>'
export CI=true
cd backend
cargo +1.99.1 test --workspace --all-features --locked --no-fail-fast -- --test-threads=8
```

示例地址和密码必须替换。URL 中的特殊字符须百分号编码，不能把真实凭据写入版本库或报告。备份测试还要求可执行的 `pg_dump`、`psql` 与服务端大版本一致。CI 用服务容器中的客户端，避免 runner 自带工具的版本差异。

`CI=true` 使缺少任一测试服务地址直接失败。本地不设置 CI 且未提供地址时，Store 用例会提前返回，因此这种运行不能报告为完整验证。只运行纯逻辑测试时应明确选择包／用例，并记录未运行的存储范围。

改动定位阶段可按包和名称筛选；最终完整验证包含所有 workspace 测试目标，不限于 `--test main`。Windows 的 GNU Rust 工具链可用于编译与静态检查，发布前以 Linux 的全量运行结果为准。

## CI 与发布门禁

`.github/workflows/_quality.yml` 固定 nextest 0.9.143，一次构建收集全部库内及显式测试入口，使用 8 个测试并发并拒绝空测试集合。测试服务使用随机密码，随后关闭 Redis default 用户，通过具名 ACL 用户重跑 Redis Store 用例。JUnit 与 Cargo 编译耗时报告作为 Actions artifact 保存。

推送 main／PR 的后端质量任务包含这些测试；main 镜像仅在同一 SHA 的 CI 成功且仍为当前 main 时发布。手动 Release 复用的 `quality / backend` 成功结果也包含测试，不可用旧提交的绿灯代替当前提交。

## 结果解释

编译成功、模拟协议测试通过、真实上游联调是三个不同层次。报告应写明工具链、实际通过／失败数量、服务是否启用、是否有跳过，以及尚未验证的真实账号和部署场景。发现具体缺陷时保留触发用例，修复后先复跑相关范围，再完成 CI 对应全量检查。
