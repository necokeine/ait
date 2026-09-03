## ADR-003：Agent Adapter crate 与首个 Codex 集成

- 状态：Proposed，协议原型已验证
- 依赖：ADR-001 v4、ADR-002
- 官方协议基线：Codex app-server，`codex-cli 0.151.0` 生成 schema
- 来源：NEC-151

## 决策

1. 新建独立 `agent-adapters` crate，存放 Codex、Claude Code、DeepSeek harness 等“完整 Agent harness”适配器；普通模型 HTTP Provider 继续留在较低层 provider contract。
2. Codex 采用官方 app-server stdio JSONL 协议。官方 SDK 当前面向 TypeScript/Python；Rust 主程序直接实现 JSONL 客户端，避免引入 Node/Python sidecar。
3. 每次 Adapter invocation 启动一个受管 app-server 子进程，完成 `initialize → thread/start|resume → turn/start`，把通知归一化为 AgentEvent；以后可在 daemon 层复用长连接。
4. Codex Thread ID 作为 Adapter checkpoint 返回给上层，Run 恢复时显式传入；它不替代本项目 Message/Session/Run 的领域主键。
5. Codex command/file/MCP item 作为富事件暴露，但只有 assistant message delta 和最终 item 由上层转换并持久化成领域 Message。Adapter 不直接推进 Session。
6. 审批通过 `ApprovalHandler` 回调；默认全部拒绝。权限 profile 必须用显式 Raw 响应，不能把空泛的“允许”升级成不受控文件或网络权限。
7. Codex 使用本机已有登录态；Adapter 不接收或导出 API key/ChatGPT token。stderr 只排空，不自动写日志。
8. app-server 增加未知通知时保留为 RawNotification；当前实现不因未知事件中断有效 Run。

## 完成语义

`turn/completed` 仅表示 Codex turn 完成。上层 Run supervisor 仍须持久化全部事件、处理队列与工具结果，并经过 ADR-001 终止屏障后才能把本项目 Run 标为 completed。

## 后续

- workspace 建立后把 `agent-adapters` 纳入 Cargo members，并由 application/runtime crate 注入 ApprovalHandler。
- 增加 Codex 长连接池、thread checkpoint repository 与 schema 兼容矩阵。
- 真实 Codex smoke test 设为显式 opt-in，默认 CI 使用零网络、零费用的内存 JSONL 测试。
