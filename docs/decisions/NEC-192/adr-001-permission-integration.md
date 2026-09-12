# ADR-001：三级权限的集成约束与审批边界补齐

- 状态：Accepted
- 日期：2026-09-12
- 关联：NEC-192
- 依赖：ADR-001 v4、NEC-208、NEC-234、NEC-209、NEC-212、NEC-241

## 决策

父任务集成核验发现，原权限档案审批的边界检查未覆盖通用 command/file 审批；审批路径的
Project 展示转换也没有与隔离执行目录闭环。本修订补齐这些限制，不改变 Message 历史、
Run 生命周期、Agent revision 或领域依赖。

| 入口 / Provider | Read only | Workspace write | Full access |
| --- | --- | --- | --- |
| Desktop/Rust settings、HTTP、Run JSON | `read_only` | `workspace_write` | `full_access` |
| daemon `--max-sandbox` | `read-only` | `workspace-write` | `full-access` |
| Codex native sandbox | `read-only` | `workspace-write` | `danger-full-access` |
| 普通 API Provider | 纯文本，无宿主工具 | 纯文本，无宿主工具 | 纯文本，无宿主工具 |

新建/重置 settings 默认 `read_only + on_request`；`strict` 仅为历史 `read_only` 别名。
`full-access` 是 daemon 默认允许的最高上限，不会替成员选择 `full_access`。Send/Fork/Derive/Cron
均在新 Run 准入时固定权限；旧 Run 不读取后续 settings。普通 API Provider 的 approval
仍按 NEC-234 固定为 `on_request`；Codex 的未知值、缺失值和 `always` 均拒绝准入。

1. 每次 `turn/start`（含 resume）都显式发送 sandbox policy、cwd 和 approval policy。
   `workspaceWrite.writableRoots` 仅包含该 Run 的隔离 cwd，`excludeTmpdirEnvVar` 与
   `excludeSlashTmp` 都设为 true，避免 native 配置中的额外根或默认临时目录扩大写入范围。
   受限档位 `networkAccess=false`；明确的原生网络审批仍独立处理。
2. 通用 command/legacy command 审批无法通过当前 `accept` 响应证明 shell 仍被限制在
   Run sandbox 内，因此受限 Run 拒绝批准；仅明确选择 `full_access` 且管理员允许的 Run
   可以授权。command preview/cwd 不是安全证明，不尝试解析 shell 文本推测只读性。
   明确的 managed-network host/protocol 审批保持独立语义。
3. file/legacy patch 审批在 `read_only` 下拒绝；在 `workspace_write` 下同时检查 grant root
   和每个变更路径。`..` 一律拒绝，避免 symlink/parent 的词法化简掩盖实际逃逸；用
   `symlink_metadata` 寻找最近存在的祖先，再 canonicalize，失效链接或越界链接 fail closed。
4. Codex bridge 在转换展示路径前，以及审批等待返回后，检查实际隔离目录中的授权目标。
   指向 primary worktree 的绝对路径不能冒充隔离工作区路径。返回 permissions 时把经过宿主
   校验的 Project 路径转换回原隔离目录；不会向 Codex 发送 UI 展示路径作为执行授权。
   显式 permissions grants 继续遵循 NEC-208 的 Project 限制。
5. 恢复 queued Run 时，在 provider 调用前重新核对当前管理员上限。超限走现有 failed
   终结路径；恢复 settling checkpoint 时超限走现有 interrupted/recovery-required 路径，
   保留恢复材料。两者均保持原权限快照、释放 Session，且不调用 provider 或发布 Git 结果。
6. 未知策略与无效 permission JSON 的错误不含原输入。HTTP 设置保存和审批决定的 JSON
   拒绝保留原 4xx 状态，返回 `INVALID_CONFIGURATION` 通用信封，避免 serde 回显秘密值/键。

## 核验对应关系

- `crates/api-http/tests/permissions.rs`：真实 router → settings → durable Run →
  `CodexWorkspaceAgent` → native sandbox 参数；OpenAI/DeepSeek 纯文本 gateway；三档/strict、
  发送/Cron、reset 后快照不变、未知设置/管理员上限拒绝、零新增 Message/Run 与零 provider 调用。
- `crates/application/tests/session_agent_config.rs`：发送、Fork、Derive 的快照及 CAS 重读；
  三档 command/file/legacy 审批矩阵、路径逃逸、symlink、拒绝不携带 scope/grant、秘密错误脱敏。
- `crates/application/tests/run_execution.rs`：queued/settling 恢复时上限重新核验。
- `crates/agent-adapters/tests/codex_workspace.rs`、`codex_protocol.rs`：Run 快照到 wire 映射、
  实际隔离路径校验和 permissions 往返、原 request ID、拒绝/取消、只读实际写入丢弃。
- `crates/api-http/tests/http.rs`：非法权限 action/scope 和 settings JSON 不回显输入。
- 原 Desktop approval/IPC 测试与 CLI workflows 继续覆盖 schema 投影、显式 scope、权限设置和默认值。

## 限制与协议依据

普通 API Provider 尚无宿主工具桥；三级值是能力上限，不是新工具能力。Codex 的实际进程/文件
隔离仍由所安装的 native harness 执行；本轮离线协议和 Git 集成测试不替代真实模型、各 OS 的
sandbox 验收。应用层拒绝无法证明安全的授权；未来支持受限 command escalation 时，需要
携带可验证的精确权限集合并重新核验协议，不得单凭命令文本放行。

官方协议：[Codex App Server approvals](https://learn.chatgpt.com/docs/app-server#approvals)，
读取日期 2026-09-12。通用 command/file 决定与精确 permissions grant 是不同协议；路径、scope
与实际授权集合必须分别核对。

`turn/start.sandboxPolicy` 的字段还通过本机 `codex-cli 0.153.4` 的
`codex app-server generate-json-schema` 输出核对（无模型调用）：支持显式 writableRoots、
excludeTmpdirEnvVar、excludeSlashTmp 与三档 policy type。离线协议测试逐字段验证新建与 resume。
