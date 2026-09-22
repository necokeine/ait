# Paseo WebSocket 接口移植：第五阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`cf9ed23`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Workspace 初始化与脚本分组的 5 个规范 WebSocket 方法。累计已接通 38 个规范方法，
剩余 153 个 catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `workspace.setup.status.request` | `workspace_setup_status_request` | 返回 cached snapshot、durable blocked snapshot 或 null |
| `workspace.setup.run.request` | 同名 | 只为 untrusted Workspace 清除 block 并启动 setup，重复批准返回 false |
| `workspace.script.list.request` | 同名 | 解析有效配置、刷新 process exit、稳定排序并保留 runtime orphan |
| `workspace.script.start.request` | 同名、`start_workspace_script_request` | trust gate、实际 shell 子进程、重复运行拒绝与逻辑 terminal identity |
| `workspace.script.stop.request` | 同名 | 终止 Unix process group/直接子进程、等待回收并返回 stopped payload |

两个下划线名称没有注册为 wire alias。两个 Paseo script start 名称合并为一个 capability；请求继续使用
新 server 的统一 envelope，Paseo method payload 位于 `params`/`result`。

## 实现边界

`server-protocol::workspace_automation` 复制 Paseo setup status/detail/command、blocked source、script
payload 和 inline error 形状。`server-application::workspace_automation::WorkspaceAutomation` 只协调
Paseo-shaped Workspace registry 与 runtime port：archived/missing Workspace 不能执行；runtime snapshot
优先于 registry 派生状态；change-request provenance 必须在 setup 执行前持久化清除。

`server-ports::workspace_automation::WorkspaceAutomationRuntime` 是阻塞文件/process 边界；
`server-workspace::LocalWorkspaceAutomation` 实现：

- 只读取 Workspace cwd 的普通 `paseo.json`，拒绝 symlink、非 object、无效 JSON 和超过 1 MiB 的文件；
- 兼容 setup string/array，过滤空命令；scripts 过滤无 command/空 command/非 object 条目，未知 type
  按普通 script，service 只采纳合法非零 u16 port；
- setup 顺序执行、首错停止，注入 5 个 Paseo 环境变量；单命令 30 分钟/8 MiB capture 上限，公开
  log 保留 64 KiB 首尾；running/completed/failed、exit code、duration 与 per-command log 留在内存；
- 为每个 Workspace 分配进程内稳定 port；真实启动 plain/service shell 子进程，按
  `(workspaceId,scriptName)` 保存 lifecycle/exit/terminal identity；list 会刷新退出状态并保留配置已删除
  但 runtime 仍存在的 orphan；
- Unix 子进程创建独立 process group，stop 先 TERM、1 秒后 KILL；adapter 最后一个 owner drop 时清理
  仍在运行的 process；Windows 使用 direct child kill；
- worktree create 在 Git 与 Workspace registry 提交后触发 background setup。setup 配置错误不回滚已
  创建的合法 worktree，失败 snapshot 可由 status 读取。

## 与 Paseo 的对齐和差异

1. setup config normalization、命令顺序/首错停止、Paseo 环境、blocked approval、status snapshot、
   worktree create 后后台启动与 Paseo 对齐。Rust runtime 额外设置 30 分钟与 8 MiB capture hard limit；
   Paseo 的 terminal stream 有自己的背压/截断路径，因此极端长命令的终止点不同。
2. script config filtering、未知 type 回退 plain script、service optional port、稳定 name sort、duplicate
   start、exit refresh、stop 结果和 inline error 与 Paseo 对齐。
3. Paseo 通过 `TerminalManager` 创建 PTY，保存 terminal history、支持 input/subscribe，并把 terminal
   lifecycle 投影到 script。当前直接启动 shell，stdout/stderr 置空，`terminalId` 只是逻辑 process ID；
   Terminal 接口接入后需要用同一 manager 替换此 adapter，不能把当前 ID 当 PTY 使用。
4. Paseo service proxy 生成 branch/project-aware hostname、local/public/proxy URL、端口占用诊断与 health
   monitor。当前 hostname 使用 script key，port 使用 configured port 或 Workspace port，URL/health 为
   null/省略；没有 HTTP proxy 或 TCP health probe。
5. Paseo 发送 `workspace_setup_progress` 与 `script_status_update`，并支持 session event subscription。
   当前只支持 RPC 轮询；在 `session.events.set_subscription.request` 接入前不发送无订阅、无序号事件。
6. Paseo setup 完成后可启动 `worktree.terminals`，archive 时停止 Workspace scripts/terminals、执行
   `worktree.teardown` 并回收 Agent。当前没有这些生命周期，第四阶段 archive 的 `removedAgents` 仍为空。
7. setup/script/runtime port 状态只在进程内保存；server 重启后 running state 与已完成 setup snapshot
   不恢复。Paseo 同样以 live terminal/runtime store 为权威，但另有 worktree metadata 保存 runtime port；
   当前重启可能分配不同的 `PASEO_WORKTREE_PORT`。

## 测试执行

从 Paseo `messages.workspaces.test.ts`、`worktree.posix.test.ts`、
`workspace-scripts-service.test.ts`、`script-status-projection.test.ts` 和 setup session tests 的对应场景
移植：canonical collision、camelCase/default wire shape、blocked source、approval idempotency、active/trusted
gate、配置过滤与排序、invalid/symlink config、plain/service type、真实 one-shot/service process、duplicate
start、exit refresh、stop、setup 顺序、环境、首错停止、worktree-create auto start 以及真实 WebSocket
blocked → approve → completed → script lifecycle → 重启前持久化链路，并验证 background setup 不持有
旧 runtime owner。

本阶段新增 17 个测试：protocol 4、application 3、workspace adapter 6、API 3、真实 binary WebSocket 1。
阶段性验证：

```text
cargo test -p server-workspace workspace_automation
  6 passed, 0 failed
cargo test -p server-api --lib
  21 passed, 0 failed（包含新增 3 个 projection 测试）
cargo test -p server-bin --test process binary_runs_canonical_workspace_setup_and_script_methods
  1 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings
  passed
```

coverage 运行中的 12 个新 server test target 共 187 个测试通过，0 失败、0 ignored。生产 Rust
源码为 8,080/9,164 行，行覆盖率 88.17%；本阶段 4 个带可执行行的 Workspace automation 模块为
751/869 行，行覆盖率 86.42%。完整 crate 级数据、命令和基线记录在
[phase 5 coverage artifact](paseo-websocket-surface-phase-5-coverage.json)。

全 workspace 严格 Clippy 通过。`cargo test --workspace --no-fail-fast -j1` 的 72 个普通 target 中，
675 passed、2 failed、5 ignored：本次引入的 API → ports 边界失败已通过 re-export projection type 到
application 并移除依赖修复，dependency guard 与 server-api doctest 随后单独通过。剩余失败是旧
`ait-daemon` 的 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`：本机约 33 秒才 ready，
超过既有 15 秒 desktop readiness window；隔离重跑仍复现。该测试不经过新 server crate，本阶段没有
放宽产品时限或修改旧 daemon 来隐藏环境失败。
