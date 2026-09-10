# ADR-001：普通 API Provider 的 Run 权限快照

- 状态：Accepted
- 日期：2026-09-10
- 依赖：ADR-001 v4、ADR-011、NEC-208 ADR-001

## 背景

NEC-208 已经把桌面的三级 sandbox 设置解析为不可变的 `RunPermissionProfile`，但应用层仅在
Codex Provider 上执行解析。OpenAI、DeepSeek 等普通 LLM API Provider 无条件得到 serde
默认的 `read_only`，既忽略成员的明确选择，也绕过 daemon 的 `max_sandbox` 管理员上限。

普通 Provider 当前通过 `AgentProviderGateway::complete` 执行单次纯文本请求。网关会清除
function catalog，不执行 shell、文件或其他宿主工具。因此权限等级首先是 Run 准入时固定的能力
上限；在宿主工具桥接完成前，它不能被解释成模型已经获得对应的操作能力。

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
3. sandbox 等级是宿主可授予能力的上限，不是 API 模型自身的权限声明。当前纯文本 Rig 网关在
   三档下都不暴露工具，因此没有本地文件副作用；`workspace_write` 和 `full_access` 只会让 Run
   保存对应上限，不会凭空给远程模型提供主机访问。
4. `permissions.approval` 仍是 Codex 原生 harness 的策略。普通 API Provider 没有原生审批协议，
   其 Run 保留保守的 `OnRequest` 值，不把 Codex 的 `untrusted_only` 分类语义伪装成通用能力。
5. 普通 Provider 的准入会验证权限上限，但当前不获取 Codex workspace lease，因为纯文本网关
   不写工作区。未来接入 ToolUse/ToolResult 执行桥时，宿主必须读取 Run 快照，在工具执行前按
   该 sandbox 上限限制路径和进程，并为任何工作区写入复用 Project 写租约；不得由 adapter
   自行扩大权限。

## 后果

- OpenAI、DeepSeek 与 Codex 的新 Run 都能准确展示成员选择的三级 sandbox，且共同受 daemon
  上限限制。
- 历史 Run 和执行中的 Run 保持原快照；迁移时缺少权限字段仍由 serde 以 `read_only` 恢复。
- 当前普通 Provider 仍是纯文本能力。权限设置不夸大为尚未实现的工具执行或操作系统 sandbox。

## 验证

- application 集成测试分别覆盖 OpenAI、DeepSeek 的 `read_only`、`strict`、`workspace_write`
  和 `full_access` 快照，以及设置变化不回写已有 Run；包括发送、显式 Fork、Derive 复用及实际分叉。
- Fork/Derive 的同步执行及异步提交测试验证管理员上限、未知/缺失值被拒绝后，Session、Message、Run
  保持原状且不调用远程网关。
- 确定性并发测试在准入后、CAS 提交前修改 Settings，验证冲突重试读取新权限并以零副作用拒绝请求。
- workspace 格式、lint 与全量测试继续验证现有 Codex 权限、审批和隔离行为不回归。
