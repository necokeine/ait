# ADR-047：移除 Plugin 接口并独立实现 Schedule 与 Browser

- 状态：Accepted
- 日期：2026-09-25
- 延续：ADR-037、ADR-044、ADR-045；遵循 ADR-001 v4
- 用户范围：删除 Plugin；补齐 Schedule 和 Browser，独立于旧 daemon/Ait 能力

## 决策

1. 从生产协议目录和前端 Rust transport 映射移除 Plugin 的 15 个入站方法。固定上游 205 项 fixture 保留；测试排除清单增至 34 项（Hub、Chat、Loop、Plugin）。当前 171 个上游名称合并三个兼容别名后为 168 个规范方法；7 个独立扩展使生产服务为 175 项。
2. 新建 `server-schedule`，拥有九个 dotted RPC、Paseo 形状的 Schedule/Run/Cadence/Target、五字段 cron/IANA 时区、原子 JSON Store 和有界调度 actor。对内仅依赖 `server-model`，通过 Runner/Progress ports 请求宿主执行并确认运行身份落盘。每个 schedule 只允许一个正在运行的 occurrence，进程最多同时执行 16 个。手动运行不阻塞同连接的后续消息；断线不取消已持久接受的任务。
3. `server-bin` 实现 Runner，组合独立 `server-provider`、`server-metadata`、`server-filesystem`：已有 Agent 恢复后发消息；新 Agent 每次创建独立目录 Workspace 或 Git worktree。启动 turn 前保存 Workspace/Agent ID，默认结束后归档；重启清理未结算的新 Agent 工作区并将 running 记录标为 failed，不补跑历史时刻。
4. 新建 `server-browser`，拥有宿主注册、自动化执行回调、22 种命令的校验、tab→host 归属及待回传请求。仅依赖 `server-model`。lease 绑定物理 WS，释放或断线必须清理宿主与 pending；回调只能完成同连接宿主的请求。多宿主 list_tabs 全部成功后才学习归属，new_tab 选最新宿主，其余按 tab 归属选择；超时/取消/发送失败删除 pending。
5. `server-api` 继续只负责认证、协商与组分发；安装两个 crate 自己声明的能力。Browser 发出的 Event 通过前端 adapter 转成 Paseo 扁平 request；客户端回调从 payload 提取真正 requestId。新增 `Api::browser()` 供宿主调用 broker，但不新增客户端执行自动化的 RPC。
6. Schedule 在服务 draining 时停止入场，先取消/结算调度工作再关闭 Provider。调度数据与旧 daemon 的领域 Run/Message、旧 scheduler crate 完全独立。Schedule Run 只是定时执行记录，不改变 ADR-001 的完成屏障或不可变消息约束。

## 边界与限制

本次移除的是独立 server 的 Plugin 接口；导入的上游源码保留用于参照。生产 catalog 无占位不代表全量 Paseo 行为一致。Browser 尚未向 Codex 注入上游 MCP browser tools；Schedule 当前通过已有 Codex 执行能力运行，不自动扩大审批权限，其他 Provider 和完整 timeline 输出聚合未实现。具体差异、资源上限及测试证据见[实施与 Test coverage 报告](../reports/server-schedule-browser.md)。
