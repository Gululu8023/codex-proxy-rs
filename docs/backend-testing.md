# 后端测试

后端测试全部使用本机模拟的上游服务，不需要真实账号。涉及存储的测试需要一套**专用**的 PostgreSQL 和 Redis，不要连接正在使用的业务实例。

## 运行测试

先按[开发指南](development.md#环境与依赖)准备工具链和 veil 源码，再准备一套测试用的 PostgreSQL 18 和 Redis 8（使用临时密码）。然后在仓库根目录执行：

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

- 地址和密码换成自己的，URL 里的特殊字符要做百分号编码。不要把真实凭据写进仓库或测试报告
- 备份相关的测试还需要 `pg_dump` 和 `psql`，版本要与 PostgreSQL 服务端一致
- **`CI=true` 很重要**：设置后，缺少测试数据库地址会直接报错。不设置时，存储相关的测试会悄悄跳过，这样的结果不能算完整通过
- 测试会在数据库里创建独立的 schema，结束后自行清理；备份恢复测试会另建临时数据库

排查问题时可以按包名或用例名筛选，只跑相关部分。但最终验证要跑全部 workspace 测试，不能只跑 `--test main`。Windows 上的 GNU 工具链可以用来编译和检查，发布前以 Linux 上的完整结果为准。

## 测试覆盖什么

| 范围 | 主要内容 |
| --- | --- |
| Core | 鉴权、权限范围、路由、发送边界、失败分类、预算、取消、租约释放、计费 |
| Codex | HTTP / SSE / WebSocket、续接与会话亲和、故障代理避让、模型目录缓存、OAuth、额度、账号身份隔离 |
| Claude | 密钥和版本准入、各原生入口的权限与 veil 检查、请求体上限、模型分页、账号撤销与封禁、5 分钟 / 1 小时缓存计费、未定价模型 |
| veil | 首次初始化、重启复用、映射按密钥隔离、旧卷不完整时拒绝启动、控制面请求也必须经过 veil |
| 管理与存储 | 历史记录中的名称和邮箱快照、双供应商查询、密钥自助查询的隔离、事务与并发写入、备份恢复、保留期、Redis 冷却与 ACL |
| 架构 | 模块依赖方向、模块边界、配置与后台任务生命周期 |

## 编写测试的约定

- **测试描述的是当前的业务规则**。删掉某个功能时，一并删掉它专属的测试；不要为了让 CI 通过而放宽断言、跳过用例或降低生产代码的约束
- 需要访问模块私有细节的单元测试，放在独立的 `tests.rs` 中，通过私有的 `#[cfg(test)] mod tests;` 引入，不要为了测试把生产代码改成 `pub`
- 生产代码不能根据是否在测试中切换实现
- 多供应商相关的测试统一使用 Codex 和 Claude 两个供应商

## CI 中的测试

CI 的后端质量任务（`.github/workflows/_quality.yml`）使用 nextest 运行同样的测试：

- 测试服务使用随机密码，并且会关闭 Redis 的 `default` 用户，用具名 ACL 用户再跑一遍 Redis 相关测试
- 测试集合为空会直接判定失败
- JUnit 报告和编译耗时报告保存为 Actions artifact

`main` 的镜像只在同一提交的 CI 通过后才会发布；Release 工作流复用的也是当前提交的检查结果，不能拿旧提交的成功记录代替。

## 怎么报告测试结果

编译通过、模拟测试通过、真实上游联调成功是三个不同的层次，不能互相替代。报告时写清楚：工具链版本、通过和失败的数量、测试数据库是否启用、有没有跳过的用例，以及哪些真实账号或部署场景还没验证。
