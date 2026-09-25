# Plugin 删除与 Schedule / Browser 实施报告

日期：2026-09-25。工作树基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba`，保留此前用户修改，未提交。参考 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`；使用本地固定 checkout，不宣称跟随其最新版本。

## 接口结果

| 范围 | 结果 |
| --- | --- |
| Plugin | 15 项从 Rust catalog、枚举、前端映射、可协商能力中删除；调用返回 unknown method |
| Schedule | 9 项均有实际实现，无 `to be implemented` |
| Browser | 注册与回调 2 项均已实现，连接 broker 与前端适配 |
| 当前 Paseo 范围 | 205 原始名称 − 34 显式排除 − 3 别名合并 = 168 个规范方法 |
| 生产 server | 168 个 Paseo 方法 + 7 个扩展 = 175/175 安装，无 catalog 占位 |

空 host 的测试仍验证未安装服务时的占位错误：170 个可登记方法、9 个内建实现、161 个未安装。这不是生产 server 的状态，也不是功能等价百分比。

## Schedule 行为

- 新 `server-schedule` 独立拥有协议、执行端口、原子文件存储与 actor；没有依赖旧 Ait scheduler/domain/daemon。
- `schedule.create/list/inspect/logs/update/pause/resume/delete/run_once.request` 全部有业务路径。create/list/pause/resume 返回 summary，inspect/update/run_once 返回完整记录，logs 返回 occurrences；`self` 归一为 agent。
- 支持 interval、五字段 cron、列表/区间/步长、IANA 时区及 DST；日期和星期同时约束；cron 默认下一分钟槽，interval 默认尽快首次执行。更新 cron 保留未显式覆盖的时区，支持 nullable name/maxRuns/expiresAt 和模型配置清除。
- 自动 tick 跳过运行中的任务，处理过期/次数限制；失败也计入次数。manual 可以运行暂停任务，保持原 cadence 和暂停状态；已完成或正在运行时拒绝。
- 同一任务最多一次运行；不同任务最多 16 个并发；run_once 等待不占用 WS 读循环。持久化失败不发布未提交状态；结算失败保留结果待重试，不重跑 Provider side effects。
- 新 Agent 每次分配 Workspace，可选择 local/worktree，执行前记录 workspaceId/agentId；沿用 schedule labels，默认结束后归档。已有 Agent 自动恢复并发送 `<paseo-system>` 格式通知。输出取 Provider 最终文本；权限等待返回失败并尝试取消该 turn。
- 数据写入独立目录 `schedules/schedules.json`，版本 1、临时文件同步后原子替换，拒绝路径 symlink/traversal。重启把未结算 occurrence 标为 failed、跳过错过的槽，并按 archiveOnFinish 清理其已记录工作区。

## Browser 行为

- `browser.host.register.request` 校验 hostKind/supportedCommands，分配物理连接所有的 subscriptionId；通用 release 和断线自动释放。
- Broker 实现 22 个上游命令的参数与结果校验、默认值、目标身份和命令关联。覆盖 list/new/snapshot/click/fill/wait/type/keypress/navigate/back/forward/reload/screenshot/upload/select/hover/drag/logs/evaluate/scroll/resize/close。
- list_tabs 发给所有宿主，按登记顺序汇总，全部成功才学习 tab 归属；new_tab 选最新宿主；其他命令按归属选择，单宿主可兜底未知 tab，多宿主则要求先 list。已断线宿主的 tab 不会转给其他宿主。
- 回调只接受发出该请求的物理连接所持 lease，错误连接及未知/迟到 requestId 忽略；匹配请求的畸形结果以明确错误结算。超时、调用者取消、发送失败、释放和断线均清除 pending。
- 前端适配把 server Event 还原成 Paseo 扁平执行 request，并从回传 payload 提取 requestId，避免错误套 payload 或丢失关联 ID。

## 与上游仍有差异

1. **Browser Agent 工具入口未接入**：提供可调用的 `Broker::execute` / `Api::browser()` 并验证 WS 往返；生产 Codex 没有自动注入 Paseo 的 MCP browser tools。注册了宿主不意味着聊天中的 Agent 已可直接使用浏览器。本次不新增客户端执行 RPC，不内置浏览器进程。
2. **Schedule Provider 范围**：沿用独立 server 当前 Codex adapter。没有补齐上游全部 Provider、background/unattended/notify 配置编排，也不会自动提高审批权限；需要审批的运行明确失败。输出是最终文本，未合并上游 curateAgentActivity 的完整工具摘要；已有 Provider 的外部取消状态精度也保持原有限制。
3. **生命周期联动**：目标 Agent 消失/归档、cwd 消失会在执行时使 schedule completed，未接上游 Agent archive/delete 的即时 schedule sweep。重启会清理已 checkpoint 的新工作区，但工作区创建与 checkpoint 不是跨存储事务，极短的崩溃窗口仍可能产生孤立工作区。未实现上游 schedule 系统通知/跨 session 管理工具注入。
4. **有界行为**：1024 schedules、每条最多 4096 occurrences、状态文件 16 MiB、每次输出/错误最多 64 KiB；16 个并发执行（上游 tick 顺序等待）。Browser 最多 128 hosts/128 pending/4096 tabs，物理连接共享 16 个订阅槽，执行 timeout 必须大于 0 且不超过 120 秒。新 lease 不能自动继承旧宿主身份，重连后需 list_tabs 重新建立归属。
5. **协议/存储**：沿用 Rust request/response/event envelope，经 adapter 对接 Paseo；Schedule 错误使用稳定 `schedule_request_failed`，不回传任意内部错误。默认 cron 时区 UTC；不迁移 Paseo 原生存储。JSON 数据受现有 1 MiB WS 帧上限约束，超大 logs 仍需后续分页设计。持久化失败或退出超时留下的 running 由下次启动恢复，不能保证突然断电时磁盘控制器已落盘。
6. **验证范围**：使用真实 server + 离线 Codex app-server fixture + 内存浏览器宿主；未使用真实付费 Provider、真实桌面 Chromium、其他操作系统或完整 Paseo TS 测试套件。

## 上游测试对应

| 上游测试职责 | 本次 Rust / adapter 对应 |
| --- | --- |
| schedule cron / timezone / DST | `server-schedule/src/cadence/tests.rs` |
| schedule store、CRUD、暂停/手动、上限、重启、运行中更新 | `engine/tests.rs`、`storage/tests.rs`、`service/tests.rs` |
| schedule session response shapes | `bins/server/tests/process/schedule.rs` 的九个 RPC、输出与重启断言 |
| new-agent workspace/worktree、archive、permission | 同上真实进程测试 |
| browser broker host/affinity/fanout/timeout/disconnect | `server-browser/src/broker/tests.rs` |
| browser automation schemas | `server-browser/src/protocol/tests.rs` |
| WebSocket ownership、release、subscription budget | `server-api/src/tests/browser.rs` |
| browser request / execute.response adapter | `apps/app/src/runtime/rust-server/transport.test.ts` |

没有把“覆盖了对应职责”写成逐个搬完全部上游测试；未覆盖事项与上述行为差异一并保留。

## Test coverage

本轮已实测 **workspace 行覆盖率 83.32%（49,114 / 58,949）**。修订为上述 HEAD 加当前未提交工作树，macOS aarch64、默认 features。命令：

```sh
cargo llvm-cov --offline --workspace --html -- --test-threads=4
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-schedule-browser-coverage.json
```

可审阅的逐文件制品：[server-schedule-browser-coverage.json](server-schedule-browser-coverage.json)。HTML 在本地 `target/llvm-cov/html/index.html`，未发布至共享站点。没有额外文件排除，使用 llvm-cov 默认报告过滤；没有启用 nightly doctest coverage。其他平台未验证。

| 修改包 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-schedule | 771 / 820 | 94.02% |
| server-browser | 638 / 726 | 87.88% |
| server-api | 909 / 954 | 95.28% |
| server-model | 293 / 309 | 94.82% |
| server-protocol | 57 / 57 | 100.00% |
| server-bin | 761 / 811 | 93.83% |

同口径历史完整测量 [Skills 制品](server-skills-coverage.json) 为 82.98%（46,917 / 56,540），本轮增加 **0.34 个百分点**。两次测量之间也包含用户的 Provider 流式输出等改动，不能把 workspace 变化完全归因于 Schedule/Browser。两个新 crate 没有自身历史基线。

测试执行结果与覆盖率分开记录：

- 上述 workspace 覆盖率运行：**1049 通过、0 失败、5 ignored**。五项为仓库预先标注的真实 Codex/DeepSeek 付费或凭据测试、外部 worker 回放，完整名称与原因保存在制品中；本轮未额外跳过测试。
- `cargo test --offline --target-dir target/server-audit -p server-bin -p server-api -p server-schedule -p server-browser -p server-protocol`：**123 通过、0 失败**。包括新增 Schedule 19、Browser 9、真实 Browser WS 2、生产 Schedule 3 项；随后 workspace 覆盖率运行覆盖最终代码。
- `node /tmp/ait-paseo-client-test-deps/node_modules/vitest/vitest.mjs run --config /tmp/ait-remove-groups-vitest.mjs`：前端 adapter **13 通过**。该临时配置隔离运行 Rust transport 测试，不等于完整桌面应用构建成功。
- `cargo clippy --offline --workspace --all-targets --target-dir target/server-audit-lint -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 通过。
- `check-paseo-protocol.py` 固定 checkout 对照完整 205 项通过；`check-paseo-client-methods.py` 验证 171 个前端映射与 168 个规范方法一致。

重要未覆盖行为：Schedule 结算写失败后的 actor 重试、磁盘容量/线程创建等故障注入及部分 shutdown 中途取消分支；Browser 全部命令虽有参数样例，结果 schema 的 snapshot/图片/拖拽等变体尚未逐项动态覆盖，128 pending/4096 tab 上限和互斥锁故障也未穷举。Host Runner 为 89.76%（228 / 254），尚缺目录消失、部分 Provider 创建/归档失败路径的进程测试。后续应补这些故障注入与真实桌面/Provider 联调，不能将高行覆盖率解释为上游功能等价。
