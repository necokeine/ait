# ADR-016：以 Codex app-server 历史为权威的双向 Session、Message 与 sub-message 互操作

- 状态：Proposed
- 日期：2026-09-19
- 协议基线：Codex app-server v2，`codex-cli 0.153.4` 生成 schema
- 修订：NEC-150 ADR-001 v4、NEC-151 ADR-003、NEC-174 ADR-001、NEC-204 ADR-001、NEC-205 ADR-001、ADR-013

## 背景

Ait 当前把 Codex app-server 当作一次 Workspace Agent 调用边界：每次 invocation 启动
app-server，执行 `initialize -> thread/start|resume -> turn/start`，再把部分流事件归一化为
Ait progress 和最终 assistant Message。Codex Thread ID 只作为恢复 checkpoint 暴露，Ait 不会列出、
读取或导入 Codex 已经持久化的历史，也不能从 Ait 无缝继续其他 Codex 客户端创建的 Thread。

新的产品目标是让 Ait 与 Codex CLI、IDE、App 和其他 app-server 客户端互操作：

- 从 app-server 发现和导入已有 Thread；
- 用 Ait Session 展示并跟踪同一个原生 Thread；
- 用现有 user/assistant Message 与 sub-message 表达完整历史；
- 在 Ait 中向已导入 Thread 继续输入、接收流式事件、处理审批并获得回复；
- 其他客户端后续写入同一 Thread 后，Ait 能再次同步并幂等收敛。

app-server 的持久化核心结构是：

```text
Codex session tree (thread.sessionId)
  -> Thread (thread.id，可继续或 fork 的分支)
       -> Turn (一次请求及其工作)
            -> ordered ThreadItem[]
```

`turn/start.input` 和 `turn/steer.input` 是请求字段；在持久化 Turn 中，用户输入表现为
`ThreadItem.type = userMessage`，其 `content` 是 `text`、`image`、`localImage`、`audio`、`skill`、
`mention` 等 user input 的有序列表。Turn 自身不保存独立 `input` 字段。

## 决策摘要

1. 对 Codex Provider，app-server 是原生 Thread、Turn 和 ThreadItem 历史的权威来源；Ait 保存有
   来源标识的不可变领域投影、同步状态、Project 绑定和 Ait 自己的 Run 控制事实。
2. 一个 Codex `Thread` 投影为一个 Ait `Session`。Ait 使用自己的 Session ID，并以
   `(provider_id, thread_id)` 唯一绑定原生身份。
3. 一个 Codex `Turn` 不强制压入单个 Message，而是投影为一段有序 Ait Message 链：
   `userMessage` item 成为 user Message；连续的非 `userMessage` items 成为 assistant Message。
4. 同一 Turn 可以因 `turn/steer` 包含多个 `userMessage`，因此合法投影可以是
   `user -> assistant -> user -> assistant`。所有 Message 都保存相同 `turn_id` 和不同 segment index。
5. 不增加新的 Message 类型或 role。Message 继续只使用现有 `standard | tool_result` kind 和
   `user | system | assistant` role；Codex 原生操作作为 sub-message，不冒充 Ait ToolUse/ToolResult。
6. Ait 可以恢复并继续已导入 Thread。对 idle Thread 使用 `thread/resume` 后调用 `turn/start`；对由
   同一 Ait Run 持有的 active Turn，可以使用 `turn/steer` 和 `turn/interrupt`。
7. Ait 发出的输入在 app-server 接纳前只存在于 durable pending input 和 UI staging；收到原生
   `userMessage` item 或完整历史快照后才成为不可变 user Message。
8. 导入外部历史不创建虚假的 Ait Run。只有 Ait 实际接纳和监督的输入、审批、取消和结算才产生
   Run；外部 Turn 的投影 Message 没有 `run_id`。
9. Message 创建后仍不可变。上游历史发生回滚、修正或截断时，Ait 创建新的投影分支并以 CAS 移动
   Session 指针，不改写或删除已导入 Message。
10. 名称、归档、fork 等 Thread mutation 必须写穿 app-server，再由同步刷新本地投影；不得先把 Ait
    cache 当成原生历史权威。

## 身份与聚合映射

### Codex 会话树

`thread.sessionId` 是 fork 后共享的会话树根身份，不能从 `thread.id` 推导，也不直接成为 Ait
Session ID。Ait 为每个 `(provider_id, codex_session_id)` 创建或复用一个不可见的 synthetic
system root Message：

```text
CodexTreeRoot {
  provider_id,
  codex_session_id,
  schema_version,
  created_at
}
```

该根只建立 Ait Message 树不变量和 fork 共享边界，不声称复原 Codex 当时的 system/developer
instructions，也不进入普通对话展示或全文索引。

### Session 对应 Thread

每个已物化的 Codex Thread 对应一个 Ait Session：

```text
SessionSource =
  | Managed
  | CodexThread {
      provider_id,
      thread_id,
      codex_session_id,
      forked_from_thread_id?,
      source,
      history_mode,
      native_cwd,
      native_project_id?,
      native_status,
      workspace_mode,
      sync_state
    }

CodexWorkspaceMode =
  | NativeCwd { cwd }
  | ManagedWorktree { workdir }
```

约束如下：

- `(provider_id, thread_id)` 在整个 Ait catalog 中唯一，重复扫描必须命中同一 Ait Session。
- Ait Session ID、Codex `thread.id` 和 Codex `thread.sessionId` 是三个不同身份，不得相互推导。
- Codex Session 仍绑定一个 Ait Codex Agent；该 Agent 必须引用同一 `provider_id`。Agent 提供 Ait
  host policy，Thread 的 model、reasoning effort 和原生设置作为每次 Run 的实际 provider 配置快照。
- `thread.name`、`preview`、`model`、`reasoningEffort`、`gitInfo`、`isPinned`、`source`、创建/更新时间
  等作为原生 metadata 投影；它们不冒充 Ait Agent revision 或 Project Git 权威事实。
- `thread.path` 是不稳定协议字段，只可用于诊断，不参与身份、幂等或文件定位。
- provider runtime status 与 Ait `active_run_id` 分开保存。其他客户端启动的 Turn 可以让 Thread
  active，但不会伪造一个由 Ait 拥有的 Run。

### Turn 对应 Message 段

Ait 按最终 ThreadItem 顺序执行以下确定性投影：

```text
segments = []
assistant_items = []

for item in turn.items:
  if item.type == userMessage:
    flush assistant_items as one assistant Message
    append item as one user Message
  else:
    append item to assistant_items

flush assistant_items as one assistant Message
```

连续的多个 `userMessage` item 各自形成一个 user Message，不插入空 assistant Message。Turn 在
`userMessage` 之前或之后只有原生操作时，可以只形成 assistant Message。终态 Turn 若没有任何 item，
则形成一个只含 Turn status/error StructuredData 的 assistant Message，使原生 Turn 身份仍可追踪。

每个投影 Message 都保存：

```text
CodexMessageProvenance {
  provider_id,
  thread_id,
  turn_id,
  turn_status,
  segment_index,
  source_item_ids,
  turn_content_hash,
  projection_version
}
```

这不是新的 Message kind，而是 provider provenance metadata。普通 role 和 Message 不变量保持有效：

- `userMessage` item 投影为 `role=user, message_kind=standard`；
- 非 `userMessage` item 段投影为 `role=assistant, message_kind=standard`；
- app-server 的 `functionCallOutput`、command、file change、MCP 等是 Codex 原生历史 item，作为
  assistant sub-message 保存，不转换为 Ait ToolResult 或 ToolExecution；
- 只有 Ait 自己的工具循环继续使用 `ToolUse`、ToolResult user Message 和 ToolExecution。

导入的 user Message 使用 `origin=provider` 并记录原生 provenance，而不是伪造 `origin=human` 的
Project Git snapshot。Ait 通过 Codex Session 提交的输入在 app-server 确认后可以记录
`origin=human` 和 `submitted_via=ait`，但 NativeCwd 模式不要求工作树干净，也不强制生成
`git_commit`；原生 provenance 是该消息的审计事实。这是对 ADR-001 v4 human user Message Git
前置条件的 Codex Session 专用修订。

### ThreadItem 对应 sub-message

`userMessage.content` 映射到 user Message 的有序 sub-message：

- `text` -> `Text`，并保留 text elements metadata；
- 已由 Ait 附件存储接纳的图像或音频 -> `FileRef`；
- 外部 URL、本地路径、skill、mention 及未来未知 input -> 有界 `StructuredData`，不得在导入时
  擅自复制、读取或信任路径指向的内容。

非 `userMessage` ThreadItem 映射为 provider item sub-message：

```text
SubMessage::ProviderItem {
  provider_kind: codex,
  external_item_id,
  item_type,
  ordinal,
  display?,
  payload,
  payload_schema_version
}
```

- `ordinal` 是 item 在 Turn 最终快照中的位置；展示和哈希均使用该稳定顺序。
- `payload` 保存经过大小限制和敏感字段策略处理的原始 JSON。超限 command output、tool result、图像
  等进入内容寻址附件，payload 只保存摘要与引用。
- Adapter 可以为已知 item 生成 `display` 投影，但它不是历史权威；重新生成 display 不改变 Message
  内容身份。
- 未知 `item_type` 以原始 payload 导入并显示通用卡片，不能导致整个 Thread 失败。
- reasoning 原文、命令输出和工具结果默认不进入 FTS；可检索文本仅来自允许索引的 userMessage、
  agentMessage、plan、reasoning summary 和显式标题字段。
- live delta 只进入 progress checkpoint；`item/completed` 或最终 `thread/read` 快照才进入不可变
  sub-message。

## fork 与不可变历史

同一 `codex_session_id` 下的 Thread 共享 synthetic root。导入器按 Turn 原生 ID、规范化内容哈希、
投影版本和父序列寻找最长公共前缀：

```text
synthetic root
  -> turn A/user -> turn A/assistant -> turn B/user -> turn B/assistant
                                                    \-> turn C/user -> ...
```

一个 Turn 的全部 Message segments 作为不可分割投影单元复用。只有 Turn ID、内容哈希和此前父链均
一致时才复用；不得仅因文本相同而合并。如果 app-server 没有保留 fork 前 Turn 的相同 ID，导入器
可以保留两条内容相同但身份不同的链，正确性优先于推测式去重。

当既有 Turn 内容、item 顺序或历史后缀发生变化时，导入器从最长未变化 Turn 前缀创建新的完整
Message 后缀，并用 Session version CAS 移动 `current_message_id`。旧后缀保持不可变、可审计，但
不再属于该 Session 的当前视图。这是同步专用 `reconcile_provider_history` 转换，不得复用常规 Run
“只推进到直接子节点”的操作。

整个 Thread 在 app-server 中删除或连续完整扫描后确认缺失时，Ait 将 Session 标记为
`provider_missing` 并默认归档；不得级联删除 Message。一次失败、超时或不完整分页不能推断删除。

## Project 与 Workspace 绑定

Codex 历史扫描属于 Provider catalog；Ait 只有在 Thread 能明确绑定 Project 后，才在该 Project 的
conversation 存储中物化 Session 和 Message：

1. 首选已有显式 `(provider_id, thread_id) -> project_id` 绑定。
2. 否则将规范化 `thread.cwd` 与已注册 Project 根或 Ait 已知 worktree 做无歧义归属匹配。
3. `thread.projectId` 作为 Codex metadata 保留，不能直接当作 Ait Project ID。
4. 无匹配、路径不存在或路径有歧义的 Thread 保留在全局 Codex import catalog，状态为
   `unbound`；不得猜测 Project，也不得创建临时 Git Project。

从其他客户端导入的 Thread 默认使用 `NativeCwd`，以原生 `thread.cwd` 继续交互；Ait 不替它创建
linked worktree，不要求 clean Git 状态，也不自动执行 `git add` 或 commit。Ait 仍以 Project/native
cwd lease 阻止本机内已知冲突，并对 turn 使用 Ait 管理员权限上限和用户选择的 Run 权限快照。

Ait 自己新建的 Codex Thread 可以继续使用 `ManagedWorktree` 和现有 Git settlement。把
`NativeCwd` Session 转为 `ManagedWorktree` 会改变执行目录和 Git 语义，必须通过显式 adoption/fork
操作，不能在普通 resume 中静默发生。

这修订 ADR-013、NEC-174 与 NEC-209：固定 linked worktree 和宿主 Git commit 只约束
`ManagedWorktree` Session，不约束为保持跨客户端互操作而导入的 `NativeCwd` Codex Session。

## 历史同步

### 初次与周期扫描

1. 建立 app-server 连接并完成 `initialize` / `initialized`。
2. 分别以 `archived: false` 和 `archived: true` 调用 `thread/list`，显式传入需要导入的
   `sourceKinds`，不能依赖服务端默认来源集合。
3. cursor 只在本次扫描中作为不透明分页 token 使用；检测重复 cursor 并以稳定错误结束扫描。
4. 对新 Thread、摘要哈希变化或 `updatedAt` 变化的 Thread 调用
   `thread/read(includeTurns: true)`。
5. 规范化并校验整个 Thread 快照，然后以单 Project transaction 执行 Message append、Session
   reconcile 和事件 outbox 提交。
6. 只有 active 与 archived 两组扫描均完整成功时，才执行 missing reconciliation。

`updatedAt` 只是变更提示，不是可靠复制序号；导入必须允许时间戳相同、回退和重复页，并以原生 ID
与内容哈希保证幂等。分页 cursor 不跨扫描持久化为唯一恢复点。

app-server 长连接为 Ait 当前持有的 Thread/Turn 提供实时 `thread/*`、`turn/*`、`item/*` 和审批
事件；周期扫描仍是其他客户端写入、断线和进程重启后的最终对账路径。实时事件不能取代持久化历史
重读。

### 分页历史与能力降级

`thread/turns/list` 和 `thread/items/list` 当前属于实验性 API。第一阶段不要求启用
`experimentalApi`。对于 `historyMode = paginated`、服务端拒绝完整读取或只返回摘要的 Thread：

- 保存 Thread 摘要和 `history_completeness = summary_only`；
- 不创建缺少完整 item 的 Message；
- UI 明确显示历史未完整导入；
- 在完整历史不可读取时禁止从 Ait 继续该 Thread，避免基于不完整 head 写入；
- 不把能力缺失当作 Thread 损坏或删除。

未来启用分页 API 时必须先扩展 capability negotiation 和兼容测试，不能静默切换。

### app-server 版本与 schema

Adapter 以当前 Codex binary 生成的 JSON Schema 为精确协议基线，记录 CLI/protocol 版本和 schema
fingerprint。Rust 解码层对已知字段使用类型化结构，对 ThreadItem 保留 raw fallback：

- 新增可选字段和未知 item 类型向前兼容；
- 缺失当前必需身份、非法父链或重复不一致原生 ID 时 fail closed；
- schema 升级不得原地改写已经导入的 Message；新的规范化结果通过 projection version 和新历史
  分支表达。

## 可写互操作与 Run 语义

### idle Thread 上开始 Turn

1. Ait 强制刷新 Thread，确认完整 head、provider idle、Session version 和 Project 绑定。
2. 在同一 Ait transaction 中创建 pending Codex input、queued Run、固定 Agent/权限快照并占用
   `active_run_id`；此时不创建 Message。
3. 调用 `thread/resume` 加载并订阅原 Thread。NativeCwd 模式默认不发送 cwd 覆盖；
   ManagedWorktree 才显式发送固定 worktree。
4. 调用 `turn/start`，以 Ait pending input ID 作为 `clientUserMessageId`，发送新的 user input 和有效
   sandbox/approval policy。
5. 收到 `userMessage` item 后，将其与 pending input 关联，持久化 user Message 并推进 Session。
6. 非 user item 以 progress 展示；Turn 终态后按最终 item 顺序持久化其余 Message segments。
7. Ait Run 只有在最终历史已落盘、审批已结算、ManagedWorktree 的 Git settlement 已完成、队列已
   drain 且终止屏障通过后才能 completed。

如果 `turn/start` 未被 app-server 接纳，pending input 进入 failed/retryable 状态，不产生历史
Message。如果 app-server 已接纳但 Ait 在本地提交前断线，恢复必须通过 `clientUserMessageId`、
`turn_id` 和历史同步绑定同一输入，不能再次调用模型。

### active Turn 上 steer

当 active Turn 由当前 Ait Run 持有时，新输入进入该 Run 队列，并通过带 `expectedTurnId` 的
`turn/steer` 发送；app-server 接纳后出现的新 `userMessage` item 会在同一 Turn 中形成新的 user
Message segment。`turn/steer` 不创建新 Run，也不产生新的 `turn/started`。

第一阶段不接管其他客户端已经启动的 active Turn。此时 Ait 展示 provider activity，并以
`CODEX_THREAD_ACTIVE_ELSEWHERE` 拒绝新的 start/steer，直到 Turn 终态并同步。未来若支持加入外部
active Turn，必须另行定义 Run ownership、审批归属和取消权限。

### 取消、审批与故障恢复

- Ait 只对自己持有的 Turn 调用 `turn/interrupt`；外部 Turn 不因本地 Session 关闭而取消。
- app-server 原生审批继续通过 Ait durable approval port，严格以 thread/turn/item ID 关联当前 Run。
- `item/completed` 和最终 `thread/read` 是持久化 item 权威；delta 只用于临时 progress。
- `turn/completed` 只表示 Codex Turn 结束，不直接授予 Ait Run completed。
- app-server 进程断开后，Ait 重新连接、读取 Thread 并按原生 ID 对账；已持久化的 Turn 不重放。

### Thread metadata 写穿

对名称、归档、恢复归档和 fork 的 Ait 操作先调用相应 app-server RPC，成功后读取返回值或重新同步。
本地事务失败时保留待对账状态；不得通过回滚 app-server 操作伪造跨进程原子性。永久删除属于破坏性
操作，不在本 ADR 的首期实现范围。

## 持久化与事务边界

全局 catalog 保存轻量 Codex Thread 索引、未绑定条目和 provider 扫描状态；Project 数据库保存已
物化的 Session、Message、provider item、Thread/Turn projection、pending input 和附件引用。

```text
CodexThreadIndex(provider_id, thread_id, codex_session_id, summary, sync_state)
CodexThreadBinding(provider_id, thread_id, project_id, ait_session_id)
CodexTurnProjection(
  provider_id, thread_id, turn_id, content_hash,
  ordered_message_ids, projection_version
)
CodexPendingInput(
  provider_id, thread_id, client_user_message_id,
  run_id, expected_turn_id?, status
)
```

全局 catalog 与 Project 数据库不能假装成一个 ACID 事务。Project 侧 binding 唯一约束和内容哈希是
物化事实；全局索引是可由 app-server 重建的 catalog 投影。若 Project commit 成功而 catalog
回写失败，下一次扫描必须从 Project binding 恢复关联，不能重复创建 Session。

原始 provider payload 不进入普通日志、错误 details 或事件广播。导出和备份沿用 Project 数据保护
策略，凭证、ChatGPT token 和本地 app-server 认证信息永不持久化。

## 公开读取模型

Desktop、HTTP 和 CLI 继续读取统一的 Session/Message 路径，不直接读取 app-server DTO：

- Session 视图增加 `source`、`workspace_mode`、provider status、`sync_state` 和原生 metadata。
- Message 继续暴露现有 role/kind；Codex provenance 放在明确、版本化的 provider metadata 中。
- 同一 `turn_id` 的相邻 Message 可以在 UI 中组合为一个 Turn 区块，但存储仍是普通 Message 链。
- provider item sub-message 由统一 renderer 按类型显示；未知类型使用安全的通用摘要卡片。
- pending input 和 live progress 是可恢复的临时投影，原生 Message 落盘后必须被替换而不是重复显示。

## 稳定错误

新增错误码：

- `CODEX_HISTORY_LIST_FAILED`
- `CODEX_HISTORY_READ_FAILED`
- `CODEX_HISTORY_SCHEMA_UNSUPPORTED`
- `CODEX_HISTORY_INCOMPLETE`
- `CODEX_THREAD_ID_CONFLICT`
- `CODEX_TURN_ID_CONFLICT`
- `CODEX_THREAD_PROJECT_UNBOUND`
- `CODEX_THREAD_PROJECT_AMBIGUOUS`
- `CODEX_HISTORY_RECONCILE_CONFLICT`
- `CODEX_HISTORY_CURSOR_REPEATED`
- `CODEX_THREAD_NOT_SYNCED`
- `CODEX_THREAD_ACTIVE_ELSEWHERE`
- `CODEX_INPUT_NOT_ACCEPTED`
- `CODEX_INPUT_CORRELATION_FAILED`

网络、进程暂时故障和可恢复 app-server overload 可以重试；身份冲突、非法结构和 schema 不兼容
不可自动重试。错误不得包含完整 item payload、命令输出或凭证材料。

## 不变量

1. 一个已物化 Codex Thread 恰好对应一个 Ait Session；身份由 `(provider_id, thread_id)` 唯一。
2. 一个终态 Codex Turn 对应一个有序、非空 Message segment 列表；每个 Message 保留相同 Turn ID、
   内容哈希和唯一 segment index。
3. `userMessage` item 只能投影为 user Message；所有其他 Codex item 只能进入 assistant Message。
4. 一个 ThreadItem 在一个 Turn projection 中恰好出现一次，顺序不变；未知类型不丢失。
5. Codex 原生 item 不创建 Ait ToolUse、ToolResult 或 ToolExecution；Ait 工具循环语义保持独立。
6. 外部历史导入不创建 Ait Run；Ait Run 只记录 Ait 实际接纳和监督的执行。
7. 同步只追加 Message 或以 CAS 移动 Session 投影，不修改或删除旧 Message。
8. incomplete scan 不执行 missing reconciliation；unbound Thread 不跨 Project 落库。
9. pending input 在 app-server 接纳前不成为 Message；重试不得重复发送已经接受的输入。
10. 同步、实时事件、重试和崩溃恢复对相同 provider snapshot 幂等收敛。
11. NativeCwd 与 ManagedWorktree 的 Git/commit 语义不得混用或静默切换。
12. Codex metadata 不替代 Ait Agent revision、权限快照、Run ownership 或终止屏障。

## 实现分期

1. **协议与 fixture**：固定 schema compatibility fixture，实现 list/read/resume/turn DTO、raw
   ThreadItem fallback、版本记录和请求关联。
2. **发现与读取**：实现 active/archived 分页扫描、unbound catalog、Project 匹配和 Session/Message
   历史投影。
3. **继续 idle Thread**：实现 NativeCwd、pending input、`thread/resume`、`turn/start`、消息分段、
   progress、审批、取消和断线对账。
4. **同 Run steer**：实现 active Turn ownership、`turn/steer`、追加 user Message segment 和队列 drain。
5. **fork 与持续对账**：实现共享前缀、上游回滚/改写分支化、归档/name 写穿和启动恢复。
6. **Managed adoption**：显式把原生 Thread fork/采用到 Ait worktree，保持两种 workspace mode 可见且
   不混淆。

## 验证

- adapter fixture 覆盖 active/archived 分页、显式 source kinds、重复 cursor、未知字段、未知 item、
  user input 类型、toolOutput、legacy/full 与 paginated/summary-only 历史。
- projection 测试覆盖 `user -> assistant`、仅 assistant、多个 steer 形成
  `user -> assistant -> user -> assistant`、空 item Turn、失败/中断 Turn 和 item 顺序。
- application 测试覆盖重复扫描不重复 Session/Message、同秒 `updatedAt`、失败扫描不误删、unbound
  Thread、Project 歧义和跨数据库恢复。
- fork 测试覆盖整 Turn segments 共享、缺失相同 Turn ID 时不猜测式合并、上游截断/改写创建新分支
  和 Session CAS 冲突保留两条可恢复历史。
- writable 测试覆盖 accepted 前无 Message、`clientUserMessageId` 关联、接纳后断线、重复恢复不重发、
  外部 active 拒绝、同 Run steer、interrupt 和原生审批。
- Workspace 测试覆盖 NativeCwd 不创建/提交 worktree、ManagedWorktree 保持现有 Git settlement、lease
  冲突和权限上限。
- 安全测试覆盖 payload 大小限制、附件引用、FTS 排除 reasoning/command/tool 原文、日志与错误脱敏。
- 进入 Rust 实现后按工程规范执行 format、workspace build、clippy、tests 和覆盖率报告。

## 协议依据

- Codex App Server：<https://developers.openai.com/codex/app-server>
- 精确 schema 由目标 Codex binary 执行
  `codex app-server generate-json-schema --out <directory>` 生成；实现和 fixture 必须记录对应的
  CLI 版本与 schema fingerprint。

## 结果

Ait 保持 Session 作为用户可见分支、Message 作为不可变 user/assistant 历史节点、sub-message 作为
节点内部的有序内容，同时让三者与 Codex Thread、Turn 中的角色边界和 ThreadItem 对齐。Ait 不需要
引入复合 ProviderTurn Message；同一 Turn 通过 provenance 关联一段普通 Message 链。用户可以在 Ait
中读取并继续原生 Codex Thread，其他 Codex 客户端产生的后续历史也能通过同一权威来源重新同步。
