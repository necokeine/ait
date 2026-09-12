# ADR-001：普通 API Provider 的 Run 权限快照

- 状态：Accepted
- 日期：2026-09-10
- 依赖：ADR-001 v4、ADR-011、NEC-208 ADR-001

## 背景

NEC-208 已经把桌面的三级 sandbox 设置解析为不可变的 `RunPermissionProfile`，但应用层仅在
Codex Provider 上执行解析。OpenAI、DeepSeek 等普通 LLM API Provider 无条件得到 serde
默认的 `read_only`，既忽略成员的明确选择，也绕过 daemon 的 `max_sandbox` 管理员上限。

NEC-247 已将普通 Provider 接入既有 RunCoordinator 和宿主工具执行器。
本文定义的权限快照与管理员上限继续适用；实际能力、持久化和恢复见
`../NEC-247/adr-001-api-provider-host-tool-loop.md`。

## 决策

1. application 为所有 Provider 统一解析 `permissions.sandbox`：`read_only` 和兼容别名
   `strict` 映射 `ReadOnly`，`workspace_write` 映射 `WorkspaceWrite`，`full_access` 映射
   `FullAccess`。解析结果在创建 Run 时写入 `RunPermissionProfile`；后续设置变化不修改历史或
   正在运行的 Run。
2. daemon 的 `PermissionPolicyLimits.max_sandbox` 对普通 API Provider 同样生效。缺失值、未知值
   或超过管理员上限的值，必须在追加 user Message、创建 Run 或发起远程 Provider 请求前以
   `INVALID_CONFIGURATION` fail closed。`SendMessage`、`ForkSession` 与 `DeriveSession` 的
   预准入读取及事务重读都必须包含同一 revision 的真实 Settings；派生的复用和分叉遵守同一规则，
   被拒绝时也不得新增 Session 或修改源 Session。
3. sandbox 等级是宿主能力上限。API 工具表与真实执行器求交，`read_only` 不广告写操作，
   `workspace_write` 与 `full_access` 也必须经过执行器自身的路径和命令约束。
4. `permissions.approval` 仍是 Codex 原生 harness 的策略。普通 API Provider 没有原生审批协议，
   其 Run 保留保守的 `OnRequest` 值，不把 Codex 的 `untrusted_only` 分类语义伪装成通用能力。
5. NEC-247 的 API 工具 Run 获取 Project 写租约；执行器读取固定 Run 快照，限制路径和进程，
   不得自行扩大权限。当前审批升级保守拒绝，并保存明确的 ToolResult。

## 后果

- OpenAI、DeepSeek 与 Codex 的新 Run 都能准确展示成员选择的三级 sandbox，且共同受 daemon
  上限限制。
- 历史 Run 和执行中的 Run 保持原快照；迁移时缺少权限字段仍由 serde 以 `read_only` 恢复。
- 普通 Provider 只开放真实执行器能力；权限设置不代表任意进程或完整工具目录已获授权。

## 验证

- application 集成测试分别覆盖 OpenAI、DeepSeek 的 `read_only`、`strict`、`workspace_write`
  和 `full_access` 快照，以及设置变化不回写已有 Run；包括发送、显式 Fork、Derive 复用及实际分叉。
- Fork/Derive 的同步执行及异步提交测试验证管理员上限、未知/缺失值被拒绝后，Session、Message、Run
  保持原状且不调用远程网关。
- 确定性并发测试在准入后、CAS 提交前修改 Settings，验证冲突重试读取新权限并以零副作用拒绝请求。
- workspace 格式、lint 与全量测试继续验证现有 Codex 权限、审批和隔离行为不回归。
