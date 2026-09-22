# Paseo WebSocket 接口移植：第一阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`5e0eb7f`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 接口目录与命名

`server-protocol::methods` 已登记用户清单中的全部 **191** 个入站名称，并为每项记录规范名称、
功能分组和 request/event/response 方向。legacy 下划线和 slash 名称仅用于审计，不会被路由器
接受。catalog 的允许合并只有三组：

- `agent.create.request` / `create_agent_request` → `agent.create.request`
- `project.icon.get.request` / `project_icon_request` → `project.icon.get.request`
- `workspace.script.start.request` / `start_workspace_script_request` →
  `workspace.script.start.request`

本阶段接通 15 个方法，其余 176 个目前为 catalog-only，不在 server info 的 capabilities 中。
这表示命名已固定、实现仍待后续分组完成，不能把 catalog 条目视为可用接口。

## 已接通方法

| 方法 | 实现状态 | Paseo 对齐范围 |
| --- | --- | --- |
| `project.add.request` | 已接通 | 检查本地目录/Git、派生 ProjectKey、复用活动 Project，保留所选目录作为根 |
| `project.create_directory.request` | 已接通 | 创建单层子目录、注册失败时回滚、返回稳定业务错误 |
| `project.list.request` | 已接通 | 返回 active Project；sync 暂不支持 |
| `project.rename.request` | 已接通 | trim 名称，空白清除 override |
| `project.remove.request` | 已接通 | 归档 active child Workspace 后删除 Project，缺失时幂等成功 |
| `project.config.read.request` | 已接通 | 读取/验证 `paseo.json`，缺失返回 null config/revision |
| `project.config.write.request` | 已接通 | schema normalize、revision CAS、pretty JSON 原子替换 |
| `project.icon.set.request` | 已接通 | automatic/upload、图片类型/尺寸/大小验证、Project revision 更新 |
| `project.icon.get.request` | 已接通 | custom 优先，否则按 Paseo 候选名发现 automatic icon |
| `workspace.open.request` | 已接通 | 复用 active、恢复 archived 或创建 Workspace |
| `workspace.create.request` | 部分接通 | directory source 可用；worktree/Agent/订阅/idempotency 尚未接入 |
| `workspace.list.request` | 部分接通 | filter/sort/page/emptyProjects 可用；sync 和订阅尚未接入 |
| `workspace.archive.request` | 已接通 | 保存 archivedAt，缺失/重复归档按业务结果返回 |
| `workspace.title.set.request` | 已接通 | trim title，空白清除 |
| `workspace.pin.set.request` | 已接通 | 保存或清除 pin timestamp |

生产 binary 在独立数据目录组装 Paseo Project/Workspace JSON registry、目录/Git inspector、
Project config store 和 icon store。`server-api` 只调用 application coordinator，不直接引用
ports/storage。旧新服务的 `project.open/list/get/close` 租约能力和 Agent preset 能力仍作为
过渡接口并存；新目录方法不复用旧 Ait 组件或旧数据。

## 明确的 Paseo 差异

下列差异需要在后续实现中消除或继续作为有意边界记录：

1. 顶层 WebSocket 使用新 server 的 hello/capability 与统一 request/response envelope，没有复制
   Paseo 的顶层 discriminated message union。
2. `project.list` 和 `workspace.list` 尚无 live sync/subscription；Workspace cursor 是本地十进制
   offset。descriptor 暂无 Agent 状态、activity、diff、script、Git/forge runtime 聚合。
3. `workspace.create` 只完成 directory source；worktree 创建、setup、首 Agent、creation 订阅、
   idempotency 和 `firstAgentContext` 会明确返回 `unsupported_capability`。
4. 恢复 archived Workspace 时保留既有 auto-archive 字段，没有查询 forge 以判断已合并 PR；
   自动名称、生命周期广播和后台 forge snapshot 尚未实现。
5. Git inspector 当前只读取 `origin`；ProjectKey 覆盖普通 URL、scp、默认端口、GitHub 大小写、
   subdirectory 和 host fallback，但没有承诺覆盖 Paseo parser 的全部异常 URL。发现的 linked
   worktree 暂不判定为 Paseo-owned。
6. Project remove 不负责尚不存在的 Agent、terminal、script、worktree 服务清理；只尽力删除
   custom icon。`project.github.clone.request` 也尚未实现。
7. Project config 暂无 `hasUncommittedWorktreeSetupChanges`；本地 adapter 另加 4 MiB 读取上限，
   Paseo 原实现没有这个显式上限。
8. 上传 icon 的 base64 解码比 Node `Buffer.from` 严格；ICO 只验证容器尺寸，不提取内嵌 PNG；
   raster 文件只检查 magic/header/尺寸而没有完整解码，自动发现的目录遍历顺序和错误字符串也
   不保证逐字等于 Node 实现。

所有 legacy wire 名称均返回 `method_not_found`，包括 `read_project_config_request`、
`write_project_config_request` 与 `project_icon_request`。这是 ADR-026 的协议选择，不是遗漏别名。

## 测试执行

测试覆盖 protocol schema/命名 catalog、application 协调、Git/目录/config/icon adapter、registry
交互和真实 `server` 进程上的 WebSocket 往返。新 server 的 12 个普通测试目标共
**104 passed、0 failed、0 ignored**；其中 application 10、protocol 16、workspace adapter 14、
真实 server 进程 6、依赖边界 2，其他既有 server 测试 56。

`cargo test --workspace --no-fail-fast` 的 72 个普通测试目标结果为 **594 passed、1 failed、
5 ignored**；24 个 doc-test 目标完成，其中 1 项 doc-test 通过。唯一失败是未修改的旧 daemon
用例 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`：它两次均在 16 秒假
Provider 返回后才 ready，超过固定的 15 秒断言窗口。单独复跑结果为 **0 passed、1 failed、
4 filtered**。本批新增/修改的 server 测试全部通过。

为隔离上述 daemon 失败，另一次全 workspace 诊断重跑跳过了该用例。普通测试阶段遇到一个既有
`ait-storage-sqlite` 用例的瞬时 `PROJECT_BUSY`（`project.lock would block`）；该用例在首次
全量运行和覆盖率运行中均已通过，随后精确单测复跑结果为 **1 passed、0 failed、35 filtered**。
诊断重跑进入 doc-test 后，`ait-agent-adapters` 的 `rustdoc` 在无 CPU、无输出状态下挂起超过
5 分钟，因而手动终止。这里不把这次未完成的诊断重跑计作绿色验证结果。

`cargo fmt --all -- --check`、`git diff --check` 和默认 feature 的
`cargo clippy --workspace --all-targets -- -D warnings` 已通过。额外执行
`cargo clippy --workspace --all-targets --all-features -- -D warnings` 时，被既有
`bins/worker/tests/process_providers.rs` 阻断：`ProviderKind::Mock` match arm 缺失；错误不位于本批
修改文件。

## Test coverage

测量命令：

```sh
cargo llvm-cov --workspace --html -- \
  --skip command_approval_secrets_never_reach_durable_or_reconnected_views \
  --skip daemon_is_ready_and_rejects_unsent_native_recovery_without_replay
```

覆盖率测试为 **593 passed、0 failed、5 ignored、2 filtered**，72 个普通测试目标。第一个排除项
沿用既有覆盖率报告中的 instrumentation 超时；第二个是上节已复现的旧 daemon readiness 失败。
默认 features、35198 行 workspace 生产 Rust 源参与测量；test/build 源按 cargo-llvm-cov 默认
规则过滤，doc-test 不插桩。

| 范围 | Covered / total lines | 行覆盖率 | 相对 ADR-025 |
| --- | ---: | ---: | ---: |
| Workspace | 27,926 / 35,198 | 79.34% | +0.10 个百分点 |
| 全部新 server package | 4,527 / 5,067 | 89.34% | -6.00 个百分点 |
| server-bin | 299 / 311 | 96.14% | +0.22 个百分点 |
| server-api | 1,191 / 1,406 | 84.71% | -12.41 个百分点 |
| server-application | 839 / 915 | 91.69% | -7.75 个百分点 |
| server-domain | 213 / 213 | 100.00% | +0.00 个百分点 |
| server-ports | 16 / 16 | 100.00% | +0.00 个百分点 |
| server-protocol | 368 / 396 | 92.93% | -3.52 个百分点 |
| server-storage | 1,068 / 1,155 | 92.47% | +0.00 个百分点 |
| server-workspace | 533 / 655 | 81.37% | -13.46 个百分点 |

完整本地 HTML 位于 `target/llvm-cov/html/index.html`；可评审摘要保存在
[覆盖率 artifact](paseo-websocket-surface-phase-1-coverage.json)，包含命令、版本、排除项、
crate 汇总和变更文件行计数。可比基线是 ADR-025 的
[artifact](paseo-registry-coverage.json)：26,237 / 33,111（79.24%），新 server package
2,841 / 2,980（95.34%）。

新增生产面使新 server 百分比下降，主要未覆盖处是 API 的逐类业务错误映射、图标所有格式与
I/O 失败分支、路径权限/非 UTF-8 分支、registration rollback failure，以及 ProjectKey 的异常
URL。workspace 总体仍略低于 80% 目标；后续接口批次应优先通过真实错误注入和 Paseo fixture
覆盖这些分支，不能用无意义调用抬高数字。
