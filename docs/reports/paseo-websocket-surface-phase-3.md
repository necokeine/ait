# Paseo WebSocket 接口移植：第三阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`91f4b53`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Workspace 标签分组的 5 个方法，以及后续 connection-owned 订阅共用的释放方法。
累计已接通 30 个规范方法，剩余 161 个 catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `workspace.label.list.request` | 同名 | snapshot/压缩 changes、generation/sequence cursor、可选 live 订阅 |
| `workspace.label.assignment.set.request` | 同名 | active Workspace assignment；首次 true assignment 创建 definition |
| `workspace.label.update.request` | 同名 | 名称/颜色单次原子编辑，重命名同步改写 assignments |
| `workspace.label.delete.inspect.request` | 同名 | 只读统计 active 与 archived assignments |
| `workspace.label.delete.request` | 同名 | 幂等删除 definition，并清除 active/archived assignments |
| `subscription.release.request` | 同名 | 按服务端 ID 幂等释放本连接拥有的单个订阅 |

DTO 复制 Paseo 的十色 palette、camelCase request/result、sync cursor、removal 和 live upsert/remove
字段。现代连接的订阅 ID 由 host 分配；同一物理连接可拥有多个独立订阅。bootstrap 先注册监听，
缓存并按 label key 压缩竞态更新，发送 list response 后只放行 sequence 高于响应 head 的事件。

## 领域、事务与恢复

`server-domain::workspace_labels` 保存 definition、颜色、名称规范化和不区分大小写的 identity；
`server-application::workspace_labels` 串行协调 catalog 与 Workspace，维护进程 generation、单调
sequence、256 条 journal 和监听生命周期。只有 durable commit 之后才发布 live change；监听回调
panic 或 outbound 失败不能回滚已经提交的写入。

`server-ports::workspace_labels::WorkspaceLabelStore` 定义 compound snapshot/commit 边界。
`server-storage::workspace_labels::FileWorkspaceLabelStore` 复刻 Paseo 的三个固定文件：

```text
<data-dir>/projects/workspace-labels.json
<data-dir>/projects/workspaces.json
<data-dir>/projects/workspace-labels.transaction.json
```

写入先保存 prepared journal 和 catalog after-image，再原子写 Workspace，最后把 journal 改成
committed。prepared 中断在重启时把 catalog 与 Workspace 都恢复到 before-image；committed marker
表示两份数据已 durable，只读当前 catalog 并清理 marker，不重放可能已经过时的 after-image。
失败后若 journal 不可读或结果处在 commit point 之后，store 与 Workspace registry 都冻结写入，
返回 `workspace_label_storage_uncertain`，重启恢复前不会冒险重试。

文件 adapter 额外使用 4 MiB 上限、symlink/非普通文件拒绝、同目录临时文件、fsync 与原子替换。
catalog CAS、Workspace registry 锁和 application 操作锁共同防止并发编辑丢失。

## 与 Paseo 的对齐和差异

1. 名称规范化、case-insensitive identity、existing definition 优先、assignment/no-op 规则、原子
   name+color 编辑、冲突拒绝、case-only rename、幂等删除和 active/archived 统计与固定 Paseo
   快照一致。
2. cursor 有效性、generation、sequence、256 条 journal、rename/delete 压缩和 durability 后发布
   与 Paseo `WorkspaceLabelSequence` 一致。generation 只在内存中，重启后旧 cursor 回退完整快照。
3. Paseo 使用 method-specific 顶层 `workspace.label.update` frame；本 server 延续 ADR-026 的统一
   `{type:"event",method,params}` envelope。payload 字段保持一致，并额外显式携带 connection-owned
   `subscriptionId`。
4. Paseo 同时支持旧客户端兼容模式；新 server 没有 legacy 会话模式，始终采用现代 owned
   subscription 规则，因此 `subscribe.subscriptionId` 被拒绝，响应只返回服务端分配 ID。
5. `subscription.release.request` 已覆盖 server status 和 Workspace 标签。catalog 中的 terminal、
   diff、timeline 等订阅尚未实现；它们接通时会登记到同一个连接所有权表。
6. Rust `split_whitespace` 与 JavaScript `/\\s+/gu` 对普通及绝大多数 Unicode 空白一致；极少数
   Unicode whitespace code point 的集合可能不同。公开契约当前只承诺 trim、连续空白折叠和
   case-insensitive identity。
7. Paseo 测试通过可注入 file writer 穷举 prepared/catalog/workspace/commit-marker 的每个 lost-ack
   点。当前 Rust adapter 没有生产 file-I/O 注入接口；本阶段直接覆盖 durable prepared 回滚、
   committed 清理不重放、不可判定结果冻结、正常 reopen 和 registry 失败语义。未逐个复制其余
   人工 lost-ack 注入用例，这是剩余的测试对齐差距。

## 测试执行

从 Paseo protocol、workspace-label service、catalog transaction 和 owned-subscriptions 套件移植
了 payload 必填字段、颜色/sequence 拒绝、名称规范化、目标 definition 优先、no-op 静默、有效/
过期 cursor、名称循环与连续编辑删除压缩、name+color 原子性、冲突不落盘、case-only rename、
active/archived delete、prepared/committed recovery、uncertain freeze、多个独立订阅及单个释放。

本阶段新增 25 个测试：domain 1、protocol 6、application 9、storage 5、API 2、真实 binary
WebSocket 2。阶段性验证：

```text
cargo test -p server-protocol -p server-application -p server-storage -p server-api -j 1
  94 passed, 0 failed
cargo test -p server-bin --test process -j 1
  10 passed, 0 failed
cargo clippy -p server-domain ... -p server-bin --all-targets -- -D warnings
  passed
```

新 server 的 12 个普通测试目标共 **143 passed、0 failed、0 ignored**，其中本阶段新增 25 个
测试全部通过。格式、diff whitespace 和默认 feature 的全 workspace Clippy 均通过。

`cargo test --workspace --no-fail-fast -j 1` 的 72 个普通测试目标结果为 **634 passed、
0 failed、5 ignored**。第二阶段曾超时的旧 daemon readiness 用例与偶发返回 `Busy` 的
`ait_workspace_local` 用例本次都通过。普通测试结束后，第一个旧 crate
`ait_agent_adapters` 的 rustdoc 再次持续 80 秒处于无 CPU、无输出状态，因而手动终止 doc-test
尾段；新 server 各 crate 的 doc-test 已在 focused run 中通过。

## Test coverage

测量命令：

```sh
cargo llvm-cov \
  -p server-bin -p server-api -p server-application -p server-domain \
  -p server-ports -p server-protocol -p server-storage -p server-workspace \
  --json --summary-only --output-path /tmp/paseo-ws-phase3-coverage-raw.json \
  --no-fail-fast -j 1
```

覆盖率运行的 12 个测试目标为 **143 passed、0 failed、0 ignored**。范围只包含全部新 server
package 的默认 features 和生产 Rust 源；test/build 源按 cargo-llvm-cov 默认规则过滤。

| 范围 | Covered / total lines | 行覆盖率 | 相对第二阶段 |
| --- | ---: | ---: | ---: |
| 全部新 server package | 6,222 / 6,963 | 89.36% | +0.15 个百分点 |
| server-bin | 325 / 337 | 96.44% | +0.05 个百分点 |
| server-api | 1,691 / 1,954 | 86.54% | +0.47 个百分点 |
| server-application | 1,305 / 1,432 | 91.13% | -0.50 个百分点 |
| server-domain | 219 / 219 | 100.00% | +0.00 个百分点 |
| server-ports | 16 / 16 | 100.00% | +0.00 个百分点 |
| server-protocol | 477 / 563 | 84.72% | -0.61 个百分点 |
| server-storage | 1,656 / 1,787 | 92.67% | +0.11 个百分点 |
| server-workspace | 533 / 655 | 81.37% | +0.00 个百分点 |

本阶段有可执行 LLVM 行的四个 Workspace 标签主模块合计覆盖 **887 / 984 行（90.14%）**：
API 241 / 275、application 407 / 452、domain 6 / 6、storage 233 / 251。protocol DTO 与 port
trait 没有独立可执行行，仍由 6 个 schema 测试及真实 WebSocket payload 往返覆盖。application
未覆盖行主要是存储错误映射、极限 journal 淘汰和 listener panic 分支；storage 未覆盖行主要是
少数平台文件错误与恢复失败分支。

完整本地 HTML 位于 `target/llvm-cov/html/index.html`；可评审摘要保存在
[覆盖率 artifact](paseo-websocket-surface-phase-3-coverage.json)，包含命令、测试数、crate 汇总、
本阶段模块行计数和第二阶段基线。
