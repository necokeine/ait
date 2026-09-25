# Push Token 管理

## 实现范围

基于固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`，实现 `push.register` 客户端事件与 `push.unregister.request` 请求。规范接口总数仍为 202，有处理器的接口从 150 增至 152，占位从 52 降至 50；加上自定义接口，生产处理器为 159/209。

- metadata 的 PushTokens 协调租约，TokenStore 端口隔离存储，FileTokenStore 适配私有 JSON 文件；未使用旧 Ait 组件，也未新增 metadata 到其他能力包的依赖。
- 注册没有成功响应；撤销返回关联请求的空结果对象，request ID 由公共信封携带。空白 Token 是幂等 no-op，登记会替换当前物理连接的续租目标，但不删除此前持久租约。
- 租约为 48 小时；剩余时间大于 24 小时不写盘，等于或少于 24 小时续租。UTC 服务端时钟决定到期时间，不使用客户端心跳时间延长租约。
- 只有成功持久化才更新服务内存和连接登记；撤销失败不会让后续心跳丢失续租目标。
- 心跳只续租当前物理连接的登记。其他连接撤销同一个 Token 后，原登记连接仍可通过心跳重新登记，保持 Paseo 的来源局部语义。断线不会撤销租约；重启恢复租约，但新连接仍须自行登记后才能续租。
- `push-tokens.json` 保持 Paseo `subscriptions: [{token, expiresAt}]` 形状，日期使用 UTC RFC3339 毫秒。支持旧 `tokens` 数组迁移，忽略无效记录，失败的迁移不发布新状态。
- 文件通过同目录临时文件、文件同步和原子替换写入；Unix 文件权限 0600，父目录 0700，读取时修复已有文件权限。文件与路径错误不包含 Token；服务 Debug 只显示数量。
- 所有运行期磁盘操作走公共 Runtime 的阻塞任务预算；心跳续租不阻塞 Tokio 执行线程。

## 上游对齐和差异

参考 `packages/server/src/server/push/token-store.ts`、`token-store.test.ts`、`push/index.ts` 及 `session.ts` 的注册、撤销和心跳处理。

已覆盖上游 TokenStore 的私有写入、读取时权限修复、撤销持久化失败不改变内存；另覆盖半租约边界、过期剔除、迁移、失败续租、重启和跨连接行为。

限制和有意差异：

- 本轮只实现 Token 管理，不包含 Expo HTTP 投递、无效设备回执处理或 Agent 完成事件触发通知。这不是完整推送通知服务。
- 活跃 Token 最多 4096 个，每个最多 4096 字节，文件最多 32 MiB。续租时清除过期项以回收容量；Paseo 原实现没有这些容量限制。
- 损坏文件、不可读取文件或迁移写入失败会导致独立 server 启动明确失败，不像 Paseo 记录警告后使用空集合；不会静默丢弃既有登记。非 RFC3339 到期时间不兼容 JavaScript Date.parse 的宽松格式。
- Unix 权限修复失败会报告失败，而非忽略；符号链接和非普通文件被拒绝。Windows 未实测 Unix 权限语义。
- 日期采用服务端墙上时钟；与 Paseo 一样，系统时钟调整会影响租约。不会把租约文件、真实 Token 或运行期文件提交到仓库。

## Test coverage

- 修订：`97f1372` 加当前未提交工作树，包含本轮开始前的用户修改；平台 macOS aarch64，默认 features。
- 命令：`cargo llvm-cov --offline --workspace --html -- --test-threads=4`，执行整个 workspace 的单元与集成测试并生成覆盖率。结果：**985 通过、0 失败、5 跳过**。没有额外排除文件或手工跳过测试，使用 llvm-cov 默认文件过滤；该命令不运行 doctests，其他操作系统未验证。
- workspace 行覆盖率 **82.82%（46,260 / 55,858）**；本轮新 Push 连接、租约与文件存储模块合计 **96.92%（220 / 227）**。
- 上一轮只成功测量了选包覆盖率，范围不同，没有同口径 workspace 基线可用于计算变化。
- 可审阅制品：[逐文件覆盖率 JSON](server-push-tokens-coverage.json)。HTML 在 `target/llvm-cov/html/index.html`，仅为本地制品，未发布共享站点。

| 相关包 / 模块 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-bin | 474 / 495 | 95.76% |
| server-api | 877 / 904 | 97.01% |
| server-metadata | 5,773 / 6,549 | 88.15% |
| server-protocol | 57 / 57 | 100.00% |
| Push connection | 63 / 65 | 96.92% |
| Push service | 110 / 111 | 99.10% |
| Push storage | 47 / 51 | 92.16% |

测试与覆盖率分开说明：

- 新增 4 个租约单元测试、2 个文件存储单元测试和 1 个真实生产 WebSocket 进程测试。catalog 回归确认生产 159 个处理器、50 个占位，并逐一检查占位响应。
- 最终 `cargo clippy --offline --target-dir target/server-audit-lint --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 均通过；固定 Paseo 的 205 项 fixture 核对通过。
- 初次普通 server 回归有一个既有 checkout 用例在 refresh 响应断言失败（`Null` 与 `true` 不匹配）；最终上述完整 workspace 运行中该用例通过，本轮未修改该 checkout 用例或实现。
- 5 个显式 ignored 用例需要真实 Codex/DeepSeek 凭据、付费模型调用或外部 `AIT_TEST_WORKER_EXECUTABLE`。本轮没有启用这些外部依赖测试。
- 未覆盖分支主要是容量上限的 WS 错误映射、超大/过量持久化文档及少数底层文件 I/O 失败；失败写入不改变内存和连接登记已有回归。后续接入真正投递时还需要 Expo 回执、失效 Token 撤销和 Agent 事件发送链路测试。
