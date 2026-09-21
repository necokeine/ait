# 独立 server 实施计划

- 日期：2026-09-21。
- 状态：`new` 分支已实施 M0；M1–M3 保留为后续工作。
- 架构决定：[ADR-022](../decisions/adr-022-independent-server.md)。
- 代码基线：`77ab2f5ffab094cd8ee0f7ef2c6a5690584b6575`。
- 所需内部组件全部新建；新旧服务使用独立数据和首期独立项目。

## 1. 交付目标

第一个里程碑交付“可以独立启动、握手并干净关闭的 server”；第二个里程碑证明完整的本机
持久化任务链；第三个里程碑才宣称支持一个真实 Provider。真实执行的审批、取消和未知结果
处理属于接入条件，不能以“后续补可靠性”为由提前发布。

所有阶段的 crate 都按实际消费者增量建立，不先造九个空壳。一个功能的 API、use case、
持久化与验收一起完成，再扩展下一个功能。

## 2. M0：可启动的独立服务端

本批实现 M0.1 与 M0.2，创建三个有实际消费者的 crate。
[使用说明](../operations/independent-server.md)记录具体配置与 wire 契约。
[M0 验证报告](../reports/independent-server-m0.md)记录本批代码与测试范围。

### M0.1 入口与独立性

- 创建 `bins/server`，package `server-bin`，产物名 `server`。
- 实现显式 `--data-dir`、`--listen`、日志配置、配置校验和操作系统信号处理。
- 建立新配置目录、单实例锁和服务身份持久化；默认端口建议 7316。
- 加入依赖边界检查，禁止任意旧 workspace crate 的直接/传递依赖，包括测试依赖。
- 使用临时目录完成集成测试，不触碰用户的 Ait 数据或原生 provider session。

验收：`server --help` 可用；无旧 daemon/worker 也能启动；同一 data-dir 重复启动被拒；
不同 data-dir/端口可以同时启动；端口占用失败不产生项目接管；退出释放自己持有的锁。

### M0.2 协议与服务生命周期

- 创建 `server-protocol` 和 `server-api`；HTTP 提供 health/readiness/info 与 WS upgrade。
- 实现凭据、Host/Origin 校验、hello/server_info、版本和必需能力协商。
- 建立 ClientConnection、request ID、统一错误、帧上限、有界发送队列和订阅生命周期骨架。
- capability 只列出已实现能力，业务 capability 暂不声明；不返回虚假的成功结果。
- 主进程持有并等待 listener/connection/background task handles，支持有期限的关闭。

验收：合法/非法握手、超大 frame、未知方法、无凭据、同 client ID 的多个物理连接隔离、
draining readiness、持续连接下退出、进程重启 instance ID 变化而 server ID 保留。
M0 不需要 Project/Agent/Run，也不需要先建立 domain/application 空 crate。

本批订阅只实现每物理连接自有的 `server.status.subscribe/unsubscribe`，提供临时
ready/draining 状态。业务订阅、持久 seq/cursor、snapshot/replay 在 M1 实现。
已关闭连接的请求相关性不保留；持久幂等 receipt 在引入业务写操作时加入。

## 3. M1：新领域与离线任务闭环

依次创建 `server-domain`、`server-ports`、`server-application`、`server-storage`、
`server-workspace`。应用内的执行 seam 使用测试专用 fake；fake 不进入生产 provider catalog。

| 纵向切片 | 交付内容 | 关键验收 |
| --- | --- | --- |
| 项目打开 | Git root 验证、新项目身份/根 Message、新格式数据库、目录注册、owner | 同路径别名、已有新项目重开、旧格式/旧项目拒绝、初始化 receipt 恢复 |
| Agent 配置 | 独立配置入口、revision、秘密引用、显式默认选择 | 无 Electron 初始化依赖，配置错误在执行前返回 |
| 会话与工作区 | 指针、version、固定 Session worktree、创建 journal | 创建失败报告保留状态，重试不重复创建，历史不复制 |
| 输入接纳 | operation/key/fingerprint、Run、固定配置、队列 | 相同重试同一 receipt，参数冲突拒绝，并发提交只绑定一个 Run |
| 结果发布 | fake 完整 Message、Session CAS、Run 终止屏障与 outbox | 事务回滚不留下半条事实，queue version 竞争不会漏输入 |
| 历史与订阅 | 固定 head 分页、scope cursor、快照/追赶/实时切换 | 快照与事件无缺口，重复投递可去重，断线后可重建 |

验收流程：打开独立临时 Git 项目 → 配置 fake 执行 seam → 创建 Session → 接纳输入 →
生成结果 → 关闭连接 → 用新连接读取相同 Run 和 Message → 重启服务再读。

此阶段就验证 ToolUse/ToolResult 形状、Message 不可变、Session CAS 和 Run 队列屏障。
不需要真实模型才能证明这些领域契约。领域单测使用纯数据，adapter 契约测试使用受控临时资源。

## 4. M2：一个真实 Provider 与受监督执行

新增 `server-execution`、`server-providers`，启用 `server __worker` 内部入口。
首个 Provider 建议 Codex；这是选型建议，不表示可以引用旧 Codex adapter。

### M2.1 独立协议验证

以实际目标版本的 provider 协议和隔离环境生成新 fixture，记录版本与验证范围。验证创建、
恢复、输入接纳、完整历史分页、运行结束、取消、审批、writer 排他、父进程断开后的子进程行为。
先明确原生内容到 Message/ToolUse/ToolResult 的映射，再实现转换；无法支持的行为明确返回能力错误。

不把旧 ADR 中记录的 provider 版本视为当前事实，不把 provider 请求 ID 当作已验证的幂等键。

### M2.2 私有 IPC 和执行监督

- 新 framing、version/capability handshake、bootstrap、commit ACK 与有界输出。
- 新 worker instance/lease 与 Project owner 校验，迟到结果不能越过 fence。
- worker 不打开业务数据库；provider 进程只在 worker 模式中创建。
- EOF、取消、超限、panic、父进程崩溃时停止进程树；无法确认停止则阻止冲突执行。
- 在真实能力可证明之前，不宣称某平台已具备可靠进程回收或 OS sandbox。

### M2.3 用户可用闭环

接通模型发现、创建/继续会话、输入队列、流式预览、权威历史、一次性审批和取消。
先持久化输入 intent/接纳回执，发送结果不明时对账；关闭客户端不取消任务。
Run completed 必须发生在确认结果发布、队列 drain 和执行资源收尾之后。

验收：一个经过配置的客户端完成真实任务，期间可以断开、重连、查询 operation、处理审批、
取消并再次继续同一 Session。真实 provider 测试与离线测试结果分别报告，涉及真实调用时使用
明确配置的隔离项目及测试账号环境，不把用户现有会话用作 fixture。

## 5. M3：恢复与并发故障矩阵

本阶段扩大故障覆盖；M2 已经要求具备安全失败和基本恢复，而非到此才开始考虑恢复。

| 注入点/场景 | 必须保持的结果 |
| --- | --- |
| 接纳提交后、RPC 回执前断开 | 同 key 重试得到同一 operation，不产生第二次任务 |
| input intent 后、provider 确认前 worker 退出 | 标记 unknown 并对账，不盲目重新发送 |
| Message 已提交、worker ACK 丢失 | receipt 去重，Message 不重复，Session 指针不重复推进 |
| 队列刚判空时新输入进入 | 完成 CAS 失败并消费新输入；或明确进入下一 Run |
| 审批等待时客户端/worker 重启 | 请求与决定持久化，过期/重复决定不会扩大授权 |
| 慢客户端、超大输出、多连接同 client ID | 有界内存，隔离连接，其他客户端和 Run 继续运行 |
| 服务正常关闭或 kill 后重启 | 可读已提交历史；先确认执行者失效，再恢复冲突操作 |
| 旧 owner/worker 延迟写回 | 明确拒绝，不覆盖新状态 |
| 新旧 server/daemon 同时启动 | 独立 clone 与独立数据下互不影响；不宣称支持共享项目 |

记录 macOS、Linux、Windows 的真实验证范围。未经验证的平台标明限制；命令退出成功不等于
provider 的子进程树或文件写入已经停止。

## 6. 后续能力的依赖顺序

1. 最小新客户端/SDK：消费已有协议，独立连接新 server；不把旧 Desktop 接入作为前置条件。
2. 项目文件和 Git 查询：通过新的 Workspace port，避免 API handler 直接访问文件系统。
3. 终端 PTY：独立二进制流、尺寸控制、会话归属和背压；按需要新增 crate。
4. MCP：薄 adapter 调 application，沿用权限/receipt；无需另造业务状态机。
5. Cron：持久化 occurrence、幂等触发和 Run 编排；在没有客户端时照常运行。
6. 第二个 Provider：由实际差异验证 port，再决定是否拆出 provider-specific crate。
7. 远程接入/Relay、旧数据迁移/跨后端接管分别设计，不与本机首版捆绑。

## 7. 工程与验证门槛

实现阶段每次交付执行：

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --html
```

此外运行新依赖边界检查。测试代码和 helper 放独立 child files；Rust 库无 unsafe，domain
保持纯粹。coverage 报告分别记录 workspace 和新 crate 的行覆盖率及 covered/total、对应
revision/features、排除项、平台范围、基线差异和可评审 artifact。

## 8. 前一轮设计交付的 Test coverage（历史记录）

- **not applicable — no Rust behavior changed**：仅新增 ADR/实施计划并更新文档索引。
- 本次不生成新的覆盖率百分比或引用旧百分比充当当前测量。
- 验证代码基线：`77ab2f5ffab094cd8ee0f7ef2c6a5690584b6575` 加本次文档变更；
  Darwin 25.6.0 arm64，rustc 1.98.1，workspace 默认 features，无额外 feature 或测试过滤。
- `cargo fmt --all --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo test --workspace`：通过；60 个普通测试目标共 491 项通过、0 失败、5 项 ignored；
  另有 1 项 doc-test 通过。此结果验证现有 workspace，不表示新 server 已经实现或获得覆盖。
- 原有 ignored 项：`codex_native_tools_create_and_verify_python_hello_world`、
  `deepseek_live_default_catalog`、`wf11_real_deepseek_python_hello_world`、
  `wf10_create_project_with_real_codex_and_commit` 需要真实模型环境；
  `permission_change::replay_permission_change_with_external_worker` 需要显式指定独立 worker。
- 三份文档的本地链接、代码围栏及空白检查通过。本轮没有验证 Linux/Windows 或真实 provider。
