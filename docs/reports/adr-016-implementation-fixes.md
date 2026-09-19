# ADR-016 实现审查修正

日期：2026-09-19。基线：`d080d257f6085fbb70a7ef460eb8fd6bf035d6fb`；修正为本工作区变更。

| 审查问题 | 修正与回归验证 |
| --- | --- |
| 审批路由可能继承 auto_review | resume/start 显式传 user；校验返回值，auto_review 在发送前失败。独立 stdio fixture 验证。 |
| 拒绝输入仍写入 Message | writer 准入后仅持久化 pending，发送前写 send_unknown；拒绝不会把本次输入写成 Message 或推进到该输入。 |
| 完成与 sync 产生重复消息、丢失 Run | 共用确定性完整 Turn 投影，按 clientId 关联 Run、分配 run_seq；成功、失败、断线和再次 sync 验证。 |
| 延迟同步覆盖新 head | 外部读取前取 revision；CAS 冲突后重读。双请求受控交错测试验证。 |
| 冷 interrupted 尾部阻止接管 | 允许先取得 writer，再确认 idle/full；保留 null completedAt；后续同内容冷读不回退。 |
| resume 后未校验 head/配置 | 核验有效配置及 NativeCwd lease，writer 内重读后再冻结 Run.base；外部新增历史测试验证。 |
| 桌面输出整个 native_message | 已知 ProviderItem 显示为消息或操作，未知类型按 item fallback；覆盖混合顺序与 envelope 隔离。 |
| writer busy 误判为发送结果不明 | 匹配实际 active writer 错误；区分明确 RPC 拒绝与发送后的断连/最终读取失败。 |

原生执行使用独立连接生命周期。关闭并回收进程、排空 progress 后，才持久化终态并释放 Session。
重启只读取和对账已有输入；clientUserMessageId 不作为服务器幂等键，不自动重发。
同步及下一次输入准入若恢复先前 Run，会在同一事务中发送 run.updated，保证订阅端得到最终状态。
取消测试确认 interrupted 的最终完整 items、null completedAt 及进程退出。

边界与未完成分期见 [ADR-016 的实现复核](../decisions/adr-016-codex-history-import.md#实现复核2026-09-19)。
本轮没有执行真实模型调用、付费请求或修改用户原生 Codex 历史；协议 fixture 基于本机
`codex-cli 0.153.4` schema 和前次隔离实测，不宣称验证其他版本。

## Test coverage

测量范围为 macOS arm64、默认 features、整个 Cargo workspace，使用 `cargo llvm-cov --workspace --html`。
普通测试包含 1 项 doctest；本次覆盖率不计 doctest。通过测试数量不代表覆盖率。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对同范围基线 |
| --- | ---: | ---: | ---: |
| workspace | 23,567 / 29,910 | 78.79% | +0.43 个百分点 |
| application | 9,763 / 11,675 | 83.62% | -0.06 个百分点 |
| agent-adapters | 4,242 / 5,823 | 72.85% | +1.90 个百分点 |
| ports | 130 / 233 | 55.79% | +0.00 个百分点 |

- `cargo fmt --all --check`、`cargo build --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 最终 `cargo test --workspace`：484 通过、0 失败、5 忽略；覆盖率运行：483 通过、0 失败、5 忽略。
- 原生 application 回归 9 项、独立 stdio/错误映射回归 5 项通过；相对审查前新增 12 项 Rust 测试。
- 桌面 `node --import tsx --test test/*.test.ts`：135 通过；`npm run typecheck` 通过。
- 首次并行编译/测试期间，未修改的 `ait-tools` 定时取消用例出现一次失败；单独复跑和随后顺序执行的完整覆盖率、普通测试均通过。未为通过测试修改该用例。

可随代码审查的产物：[覆盖率摘要 JSON](adr-016-coverage-summary.json)，包含比较基线、crate 聚合与相关文件行数。
完整本地 HTML：`target/llvm-cov/html/index.html`；它未上传为共享 CI 产物，仓库摘要用于共享复核。
当前 workspace 仍未达到 Rust 规范中建议的 80% 目标。主要未覆盖项包括 native 准入的部分配置/CAS 失败分支、
损坏持久化状态和进程 I/O 异常组合；后续应以针对性故障注入覆盖，而非把测试数当作覆盖率。

重要限制：真实 Codex 模型运行、不同 Codex 构建以及跨进程崩溃的所有时间窗口未完整覆盖。
现有付费模型与显式外部 worker fixture 保持忽略；后续升级 app-server 时须重跑隔离兼容验证。
