# ADR-016：以 Codex app-server 历史为权威的双向 Session、Message 与 sub-message 互操作

- 状态：Proposed
- 日期：2026-09-19
- 协议基线：Codex app-server v2，`codex-cli 0.153.4` 的稳定/实验 schema 与隔离协议实测
- 协议复核：2026-09-19；构建指纹、已验证行为和验证范围见“协议依据”
- 修订：NEC-150 ADR-001 v4、NEC-151 ADR-003、NEC-162 ADR-004、NEC-174 ADR-001、
  NEC-204 ADR-001、NEC-205 ADR-001、NEC-208 ADR-001、NEC-209 ADR-001、NEC-226 ADR-001、
  NEC-235 ADR-001、ADR-009、ADR-013；以下均为 Codex Session 专用修订

## 背景

本 ADR 提出前，Ait 把 Codex app-server 当作一次 Workspace Agent 调用边界：每次 invocation 启动
app-server，执行 `initialize -> thread/start|resume -> turn/start`，再把部分流事件归一化为
Ait progress 和最终 assistant Message。Codex Thread ID 只作为恢复 checkpoint 暴露，Ait 不会列出、
读取或导入 Codex 已经持久化的历史，也不能从 Ait 无缝继续其他 Codex 客户端创建的 Thread。
当前实现进度见“实现复核”；下文的设计目标不等同于已交付功能清单。

新的产品目标是让 Ait 与 Codex CLI、IDE、App 和其他 app-server 客户端互操作：

- 从 app-server 发现和导入已有 Thread；
- 用 Ait Session 展示并跟踪同一个原生 Thread；
- 用现有 user/assistant Message 与 sub-message 表达完整历史；
- 在 Ait 中向已导入 Thread 继续输入、接收流式事件、处理审批并获得回复；
- 其他客户端后续写入同一 Thread 后，Ait 能再次同步并幂等收敛。

app-server 的持久化核心结构是：

```text
Thread (thread.id，可继续或 fork 的分支)
  -> Turn (一次请求及其工作)
       -> ordered ThreadItem[]

thread.forkedFromId -> 来源 Thread（若可用）
thread.sessionId   -> 原生 session metadata，不充当跨 fork 的永久共享根
```

`turn/start.input` 和 `turn/steer.input` 是请求字段；在持久化 Turn 中，用户输入表现为
`ThreadItem.type = userMessage`，其 `content` 是 `text`、`image`、`localImage`、`audio`、
`localAudio`、`skill`、`mention` 等 user input 的有序列表。Turn 自身不保存独立 `input` 字段。

本文的“完整历史”指 app-server 可返回的持久化 ThreadItem 投影，不等同于原始模型 prompt、全部
流事件或原始工具输出的无损备份。`itemsView = full` 也只承诺该持久化投影中的 item 已完整加载。

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
6. Ait 通过 `thread/resume` 取得可写上下文，重新核对完整 head、实际配置与本进程活动状态后，才可
   调用 `turn/start`；对同一 Ait Run 持有的 active Turn，可以使用 `turn/steer` 和 `turn/interrupt`。
   `thread.status` 不是跨进程 writer 所有权证明。
7. 输入接纳和 Message 发布分开：收到原生 `userMessage` 只确认 pending input，并更新 durable
   staging；Turn 终态且完整历史校验通过后，才按最终顺序原子发布整个 Turn 的 Message segments。
8. 导入外部历史不创建虚假的 Ait Run。只有 Ait 实际接纳和监督的输入、审批、取消和结算才产生
   Run；首次导入的外部 Turn 没有 `run_id`，复用已有 Message 则保留其真实的原始 Run 来源。
9. Message 创建后仍不可变。上游历史发生回滚、修正或截断时，Ait 创建新的投影分支并以 CAS 移动
   Session 指针，不改写或删除已导入 Message。
10. 名称、归档、fork 等 Thread mutation 必须写穿 app-server，再由同步刷新本地投影；不得先把 Ait
    cache 当成原生历史权威。

## 身份与聚合映射

### Ait 历史来源关系

本次实测中，普通持久化 `thread/fork` 的子 Thread 获得新的 `sessionId`，同时通过 `forkedFromId`
指向来源 Thread；不能依赖 fork 保留相同 `sessionId`。原生 `sessionId` 只按返回值保存，不直接
成为 Ait Session ID 或 Message 树根身份。

Ait 为每个 `(project_id, provider_id, lineage_id)` 创建或复用一个不可见的 synthetic system
root Message。`lineage_id` 是 Ait 分配的稳定身份，来源关系另行记录：

```text
CodexTreeRoot {
  project_id,
  provider_id,
  lineage_id,
  schema_version,
  created_at
}

CodexThreadLineage {
  project_id, provider_id, thread_id, lineage_id,
  forked_from_thread_id?,
  relationship_state: independent | unresolved | verified
}
```

该根只建立 Ait Message 树不变量和 fork 共享边界，不声称复原 Codex 当时的 system/developer
instructions，也不进入普通对话展示或全文索引。

仅在同一 Project 内，通过 `forkedFromId` 和完整历史中确证的公共前缀建立共享关系。缺少来源
Thread 时先保存 `unresolved` 关系并使用独立根，不按文本或 `sessionId` 猜测合并。来源之后可读时，
可通过历史 reconcile 将 Session 移到已验证的共享链，旧根和旧 Message 保留。跨 Project 只记录
来源关系，不共享 Message 或 synthetic root；来源关系必须无环。

### Session 对应 Thread

每个已物化的 Codex Thread 对应一个 Ait Session：

```text
SessionSource =
  | Managed
  | CodexThread {
      provider_id,
      thread_id,
      codex_session_id,
      lineage_id,
      forked_from_thread_id?,
      source,
      history_mode,
      native_cwd,
      native_project_id?,
      native_status,
      writer_state,
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
  host policy。每次 Run 的配置以成功 resume 的实际返回值及明确的 turn 覆盖值为准，不能把
  list/read 中可空的 model、reasoning effort metadata 当成实际执行配置。
- `thread.name`、`preview`、`model`、`reasoningEffort`、`gitInfo`、`section`、`sectionEnteredAt`、
  `source`、创建/更新时间等作为原生 metadata 投影。该基线没有 `thread.isPinned`；`source` 可以
  是字符串或 `custom` / `subAgent` 结构，不能按 `sourceKinds` 的过滤枚举解码。
- `thread.path` 是不稳定协议字段，只可用于诊断，不参与身份、幂等或文件定位。
- provider runtime status、writer 所有权和 Ait `active_run_id` 分开保存。`notLoaded` 表示当前
  app-server 未加载该 Thread，不能推断其他进程是否空闲；外部活动不伪造 Ait Run。

### Turn 对应 Message 段

Ait 在确认完整终态历史后，按最终 ThreadItem 顺序执行以下确定性投影：

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
`userMessage` 之前或之后只有原生操作时，可以只形成 assistant Message。只有已确认终态且
`itemsView = full` 的 Turn 确实没有任何 item 时，才形成一个只含 Turn status/error StructuredData
的 assistant Message。`turn/start` 响应和 `turn/completed` 通知中的 `items = []` 可能伴随
`itemsView = notLoaded`，不能据此生成空 Turn Message。

运行中的 user item、已完成的非 user item 和 delta 全部进入 durable staging，不提前追加到
Message 树。最终发布使用 `publish_provider_turn`：在单 Project transaction 中按完整顺序追加
整段链、保存 projection、按预期 Session version 将指针移到段尾，并提交 outbox。链的首节点必须
连接预期 base，每个后继连接前一个节点；这是 Codex 整 Turn 发布对逐条推进规则的专用修订。

每个投影 Message 都保存：

```text
CodexMessageProvenance {
  provider_id,
  observed_in_thread_id,
  turn_id,
  turn_status,
  segment_index,
  source_item_ids,
  turn_content_hash,
  projection_version
}
```

`observed_in_thread_id` 只记录首次物化时读取该内容的 Thread，不宣称内容最初在哪个 Thread 生成。
当前 Thread 包含哪些 Message 由 `CodexTurnProjection` 侧表记录；多个 Thread 可以引用相同
Message。复用时不改写首次观察来源、`run_id` 或 `run_seq`。

这不是新的 Message kind，而是 provider provenance metadata。普通 role 和 Message 不变量保持有效：

- `userMessage` item 投影为 `role=user, message_kind=standard`；
- 非 `userMessage` item 段投影为 `role=assistant, message_kind=standard`；
- app-server 的 `functionCallOutput`、command、file change、MCP 等是 Codex 原生历史 item，作为
  assistant sub-message 保存，不转换为 Ait ToolResult 或 ToolExecution；
- 只有 Ait 自己的工具循环继续使用 `ToolUse`、ToolResult user Message 和 ToolExecution。

导入的 user Message 使用 `origin=provider` 并记录原生 provenance，而不是伪造 `origin=human` 的
Project Git snapshot。Ait 通过 Codex Session 提交的输入在确认关联并最终发布时可以记录
`origin=human` 和 `submitted_via=ait`，但 NativeCwd 模式不要求工作树干净，也不强制生成
`git_commit`；原生 provenance 是该消息的审计事实。这是对 ADR-001 v4 human user Message Git
前置条件的 Codex Session 专用修订。

ManagedWorktree 的每项人类输入仍在发送前验证 clean Git 并捕获 `git_commit` 到 pending record，
最终发布使用该输入的快照；steer 也不例外。工作树已被当前 Turn 改写而无法满足条件时，拒绝该项
输入，不伪造提交、不因 steer 静默放宽 ManagedWorktree 的 Git 前置条件。

`origin=provider` 和 `SubMessage::ProviderItem` 是本 ADR 明确增加的纯数据变体；需同步迁移领域
校验、持久化 codec、导出与读取 DTO，但不增加 Message role/kind 或引入 provider SDK 依赖。

### ThreadItem 对应 sub-message

`userMessage.content` 映射到 user Message 的有序 sub-message：

- `text` -> `Text`，并保留原生 `text_elements` metadata；不得机械改为 camelCase 字段；
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
  payload,
  payload_schema_version
}
```

- `ordinal` 是 item 在 Turn 最终快照中的位置；展示和哈希均使用该稳定顺序。
- `payload` 保存经过大小限制和敏感字段策略处理的原始 JSON。超限 command output、tool result、图像
  等进入内容寻址附件，payload 只保存摘要与引用。
- Adapter 可以为已知 item 生成独立的 `display` 缓存，但它不是不可变 sub-message 内容；重新
  生成 display 不改变 Message 内容身份。
- 未知 `item_type` 以原始 payload 导入并显示通用卡片，不能导致整个 Thread 失败。
- reasoning 原文、命令输出和工具结果默认不进入 FTS；可检索文本仅来自允许索引的 userMessage、
  agentMessage、plan、reasoning summary 和显式标题字段。
- `item/completed` 确认当前 item 的最终流状态，但仍写入 staging。只有完整终态 Turn 的发布
  事务才创建不可变 sub-message；断线后以持久化历史重读对账，delta 不成为历史权威。

## fork 与不可变历史

同一 Project 内已验证属于同一 lineage 的 Thread 可共享 synthetic root。导入器按 Turn 原生 ID、
规范化内容哈希、投影版本和父序列寻找最长公共前缀：

```text
synthetic root
  -> turn A/user -> turn A/assistant -> turn B/user -> turn B/assistant
                                                    \-> turn C/user -> ...
```

一个 Turn 的全部 Message segments 作为不可分割投影单元复用。只有 Turn ID、内容哈希和此前父链均
一致时才复用；不得仅因文本相同而合并。如果 app-server 没有保留 fork 前 Turn 的相同 ID，导入器
可以保留两条内容相同但身份不同的链，正确性优先于推测式去重。

内容哈希覆盖 Turn 身份、终态 status/error、有序 item ID/type 和规范化后的持久化 payload。
规范化规则及敏感字段/截断策略随 projection version 固定；附件使用内容摘要，不使用机器路径。
哈希排除当前 Thread membership、首次观察来源、Ait Run 来源、display 缓存和同步时间；这些本地
事实不应阻止原生公共前缀共享。共享前缀上的既有 Run 来源保留，fork 不创建替代 Run。

已发布历史因上游改写或父链变化而重建的节点属于 provider 重投影，不作为旧 Run 的第二次输出
分配 `run_id/run_seq`；需要追溯时在侧表保留原 Message/Run 引用。原 Run 已发布的节点保持不变。

当既有 Turn 内容、item 顺序或历史后缀发生变化时，导入器从最长未变化 Turn 前缀创建新的完整
Message 后缀，并用 Session version CAS 移动 `current_message_id`。旧后缀保持不可变、可审计，但
不再属于该 Session 的当前视图。这是同步专用 `reconcile_provider_history` 转换，不得复用常规 Run
“只推进到直接子节点”的操作。

活动 Ait Run 期间，周期扫描不能越过该 Run 的发布/恢复门禁移动 Session；外部快照只更新待对账
状态。完整性或终态尚未确认的尾部保留在 staging，旧的已发布 Message 路径继续可读。

确认 Thread 删除时，Ait 将 Session 标记为 `provider_missing` 并默认归档；不得级联删除 Message。
列表缺失只产生复核候选，判定规则见“初次与周期扫描”。

### 原生 fork 的操作边界

`thread/fork.lastTurnId` 包含指定的整个终态 Turn，不能定位其内部 item 或 Message segment。
Codex Session 的原生 fork 因此仅允许完整终态 Turn 的段尾；不得把用户选择的中间节点静默扩展到
整个 Turn。新 fork 对应新原生 Thread 和新 Ait Session，不能为同一 Thread 再建一个可写 Session。

首期对 Turn 中间节点只提供历史查看，禁用从该处继续、编辑和重生成；从终态段尾“打开新 Session”
执行原生 fork。跨 Provider 切换及从中间节点重建上下文属于显式转换，首期不支持，不能把不含原生
instructions/完整模型上下文的展示投影静默作为等价上下文发送。普通 Managed Session 的任意 Message
派生规则不变；这些限制修订 NEC-162、NEC-226 在 Codex Session 上的适用范围。

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
4. 对新 Thread、摘要哈希变化或 `updatedAt` 变化的 Thread 读取完整历史。已知活动、未完成、
   `send_unknown`、上次读取不完整和写入前的 Thread 必须强制复核；其余已绑定 Thread 也按持久化的
   `next_full_verify_at` 轮转重读，不因摘要不变而永久跳过。重读间隔有有限上限，失败不得标为已验证，
   重试需保留公平调度，防止持续新增 Thread 使旧条目饥饿。
5. 规范化并校验完整快照，只发布已确认终态的 Turn；运行中或终态存疑的尾部留在 staging。
   取得全局 binding reservation 后，以单 Project transaction 执行 Message append、Session
   reconcile 和事件 outbox 提交；活动 Ait Run 由同一发布/恢复门禁协调。
6. 非归档与归档两组扫描都完整成功后，对缺失的已绑定 Thread 按 ID 直接读取。只有来源范围与
   provider 存储身份未变、直接读取明确 not found，且连续完整扫描复核仍缺失，或已收到可信的
   删除通知并复核后，才标记 `provider_missing`。权限、schema、读取失败和过滤范围变化均不证明删除。

`updatedAt` 只是变更提示，不是可靠复制序号；导入必须允许时间戳相同、回退和重复页，并以原生 ID
与内容哈希保证幂等。分页 cursor 不跨扫描持久化为唯一恢复点。列表分页不是一致性快照；归档移动、
排序变化或并发写入可以使一次成功遍历漏掉条目，因此“列表扫描完整”不单独证明 Thread 缺失。

app-server 长连接为 Ait 当前持有的 Thread/Turn 提供实时 `thread/*`、`turn/*`、`item/*` 和审批
事件；周期扫描仍是其他客户端写入、断线和进程重启后的最终对账路径。实时事件不能取代持久化历史
重读。

### 完整读取、分页与能力降级

本次 `0.153.4` 的稳定 schema 包含 `thread/turns/list` 和 `thread/items/list`；实测初始化
`experimentalApi: false` 时均可调用。默认创建的 Thread 为 `historyMode = paginated`，完整
`thread/read(includeTurns: true)` 和 `thread/resume` 也成功。`historyMode` 是原生存储契约，
不是“只能读取摘要”的标志；其他构建按已验证能力处理，不据版本字符串推断所有行为。

首期支持以下读取路径：

1. `thread/read` 显式传 `includeTurns: true`；所有待发布 Turn 必须完整加载。只读摘要请求与
   `thread/list` 返回的空 `turns` 不能用于截断本地历史。
2. 分页读取 `thread/turns/list` 时显式指定 `itemsView: full` 和 `sortDirection: asc`，直到
   `nextCursor` 耗尽。服务端默认是 summary 和倒序，不能依赖默认值或按秒级时间戳重新排序。
3. 需要分开加载 item 时，使用 `thread/items/list` 并显式指定正序，按返回的 `turnId` 归属，
   以 API 返回的顺序组装；必要时逐 Turn 过滤。不能从 item 页推断缺失的 Turn status/error。
4. `Turn.itemsView` 区分 `full | summary | notLoaded`；按目标 schema 的默认规则解码缺省值。
   `summary`、`notLoaded`、未耗尽的分页或读取失败都不得发布成完整 Turn。通知中的空 item 列表
   不触发删除、截断或空 Turn 投影。

跨页读取不被视为原子的历史快照。发现页间 ID/内容冲突、head 改变或尾部仍在变化时，重新读取受
影响范围；没有历史 revision token 时以重复读取相同有序 ID/内容哈希作收敛检查。该检查不证明
没有外部 writer；冷读的不确定尾部只能用于显示和后续复核，写入前必须取得可写上下文并重新确认。

若已验证的完整读取路径均不可用或预算耗尽，保存摘要、读取进度和 `history_completeness =
summary_only | partial`，保留此前完整投影；UI 明确显示未完整同步，并禁止从 Ait 继续输入。
能力缺失不等于 Thread 损坏或删除。实验能力只能在明确协商、具有相应 handler 和兼容测试后启用。

### app-server 版本与 schema

Adapter 以目标 Codex binary 生成的 JSON Schema 为字段基线，另以隔离协议 fixture 验证运行语义；
记录 CLI/protocol 版本、binary/schema fingerprint、能力开关和验证范围。稳定 schema 与显式
`--experimental` 生成的 schema 分别保存，不将实验字段混入默认请求。Rust 解码层对已知字段使用
类型化结构，对 ThreadItem 保留 raw fallback：

- 新增可选字段和未知 item 类型向前兼容；
- 缺失当前必需身份、非法父链或重复不一致原生 ID 时 fail closed；
- schema 升级不得原地改写已经导入的 Message；新的规范化结果通过 projection version 和新历史
  分支表达。

## 可写互操作与 Run 语义

### writer 所有权与进程拓扑

本次双进程实测中，同一 Thread 在执行进程内为 `active/inProgress`，在另一个 app-server 的
完整 read 中却为 `notLoaded/interrupted`；后者 resume 返回 `already has an active writer`。
即使执行进程已变为 idle，只要仍持有 writer，其他进程也不能 resume。因此：

- 保存 `writer_state = unknown | owned_by_ait | busy_elsewhere`，并关联宿主 operation、进程代次
  与 Thread；连接丢失立即失效 `owned_by_ait`。不得把 runtime idle 或冷读 interrupted 当成所有权。
- 首期扫描连接只调用读取 RPC，不通过 resume 扫描或探测终态，以免长期占用所有 Thread。
  执行连接使用 Ait 独占监督、仅服务当前 Thread/Run 的 stdio 子进程；Run 内保持连接，完成
  历史落盘、审批与工作区结算后退出并等待该子进程被回收，从而释放 writer。
- `thread/unsubscribe` 不保证立即卸载或释放 writer，不能作为首期接管协议。未来共享 app-server
  时必须另行定义所有连接的写入协调、事件路由和卸载确认；当前假设不覆盖未受控的共享连接。
- 只将已验证的 writer 冲突映射为 `CODEX_THREAD_WRITER_BUSY`；其他 `-32600` 仍按原参数/协议
  错误处理。外部 writer 必须先释放后才能接管，等待 Turn 终态本身不足以放行。
- 冷读中缺乏已验证终结证据的 interrupted 尾部保留为 `terminal_unconfirmed` staging；不得以
  `Thread.status = notLoaded` 或多次相同冷读证明终态。取得 writer 并重读后，才可确认恢复结果。
  无 writer 的只读导入，只能采用目标构建 fixture 已证明来自实际终结记录的字段；空的
  `completedAt` 不能补成完成时间。未通过这种验证的尾部保持待确认，不能让摘要猜测替代终态证据。

### 取得可写上下文后开始 Turn

1. 为 Session、原生 Thread 和工作目录取得 Ait 准入许可，保存 admission operation，校验 Project
   绑定并固定 Agent revision、用户权限选择和管理员上限。准入阶段尚不创建 Message 或宣称已发送。
2. 启动独占执行进程并调用 `thread/resume`。NativeCwd 默认不覆盖 cwd，ManagedWorktree 显式发送
   固定 worktree；传入经过校验的权限/审批路由设置，不静默沿用外部客户端的审批权限。
3. 成功 resume 后重新读取完整 head，确认本进程无活动 Turn，核对返回的 `model`、`modelProvider`、
   `reasoningEffort`、`cwd`、`sandbox`、`approvalPolicy`、`approvalsReviewer` 及能力兼容性。
   在同一 Project transaction 中 reconcile head、创建 pending input 和 queued Run、占用
   `active_run_id`；Run 的 `base_message_id` 固定为此时已确认的历史段尾，实际 provider 配置也在
   此处冻结。准入失败释放本次许可和执行进程，不产生虚假输入历史。
4. 在调用前持久化 `send_unknown` 和请求关联信息，然后调用 `turn/start`，将 pending input ID
   作为 `clientUserMessageId`，发送新输入及有效的 `sandboxPolicy`、`approvalPolicy`、
   `approvalsReviewer` 和已冻结的模型设置。`turn/start` 在 active Thread 上可能转为 steer，
   并非“仅空闲时启动”的原子条件；独占调用门禁必须覆盖第 3 步到发出请求。
5. 响应确认原生 Turn 接纳；`userMessage.clientId` 确认具体输入的关联。所有 item 先写入 staging，
   不提前移动 Session。发布前再从持久化历史核对 ID、内容和顺序。
6. Turn 已确认终态且 `itemsView = full` 后，通过 `publish_provider_turn` 原子发布全部 segments、
   推进 Session 和更新 pending/projection/outbox。Ait 产出的 segments 按父链分配连续 `run_seq`，
   第一条 user Message 也属于该 Run 的产出；这是先创建 user Message 再启动 Run 的专用修订。
7. Ait Run 只有在最终历史已落盘、审批已结算、ManagedWorktree 的 Git settlement 已完成、队列
   已 drain 且终止屏障通过后才能 completed。新入队与终止屏障仍以 `queue_version` 协调。

外部 Thread 可能依赖原客户端的 dynamic tools、用户提问或 MCP elicitation handler。完整导入
历史不证明 Ait 具备这些能力；已知不支持的依赖在发送前拒绝继续。协议不能预先完整披露的请求，
执行时必须明确拒绝并记录能力错误，不能伪造工具成功或审批。`approvalsReviewer` 必须显式配置到
受 Ait 管理的审批路径；原生设置与管理员上限冲突时 fail closed。

### 输入接纳与结果不明恢复

`clientUserMessageId` 是关联值，历史 item 中对应 `userMessage.clientId`，与原生 `item.id`
不同。它不提供服务端幂等保证：本次实测重用相同 ID 可以创建新的 Turn 和 user item。

pending input 至少区分 `queued | send_unknown | turn_accepted | accepted | published |
rejected | cancelled`，分别记录原生 Turn ID、user item ID 和发送前 head。RPC 成功但尚未读到
对应 user item 时只进入 `turn_accepted`；确认 item 关联后进入 `accepted`，仍不创建 Message。
输入记录、Run 队列和发布事务共用稳定 operation ID，不能把一次网络重试变成另一个逻辑输入。

断线、超时和进程退出不证明输入未接纳。恢复时先取得受控可写上下文、重读完整历史，再按本 Thread
的 clientId 和预期 Turn 关联：唯一匹配则绑定已有 item；多匹配报 `CODEX_INPUT_CORRELATION_FAILED`；
零匹配或旧历史缺少 clientId 时继续保留结果不明，不能按文本猜测，也不能自动重发。只有未发送，
或已得到明确的未接纳证明时才允许发送/重试。结果持续不明时返回可审阅的待处理状态，不能无限占用
执行进程或把 Run 标成 completed；后续用户选择终止对账或另发输入必须是新的显式操作。

### active Turn 上 steer

仅当 active Turn 属于当前 Ait Run 时，新输入才在短事务中进入该 Run 队列；单一 dispatcher 持有
发送许可，按序调用带 `expectedTurnId` 和独立 `clientUserMessageId` 的 `turn/steer`。接纳后出现的
user item 仍留在同一 Turn 的 staging，最终以 `user -> assistant -> user -> assistant` 顺序发布。
steer 不新建 Run、不产生新的 `turn/started`，也不接受模型、cwd 或权限等 turn 级覆盖。

若 Turn 已结束，只有明确未接纳的队列项可在同一 Run 完成该 Turn 发布后转为下一次 `turn/start`；
结果不明的 steer 先对账，不能自动改道 start。下一 Turn 继续使用原 Run 的固定配置、权限和
连续 `run_seq`。第一阶段不加入外部客户端启动的 active Turn，不接管其审批或取消权限。

本节显式修订 ADR-009：Ait 持有的 Codex Run 允许人类输入入队，入口不再跨整次执行持有排斥所有
输入的许可；实际 start/steer 仍串行，其他 Session 配置变更仍被活动 Run 阻止。其他 Provider
保留 ADR-009 的 `SESSION_BUSY` 行为。外部 active 返回 `CODEX_THREAD_ACTIVE_ELSEWHERE`，writer
被其他进程占用则返回 `CODEX_THREAD_WRITER_BUSY`，两者均不触发模型调用。

### 取消、审批与故障恢复

- Ait 只对自己持有的 Turn 调用 `turn/interrupt`；外部 Turn 不因本地 Session 关闭而取消。
- app-server 原生审批继续通过 Ait durable approval port，以进程/连接代次、JSON-RPC request ID
  和 thread/turn/item ID 关联当前 Run。重启后旧 request ID 与 grant 不得重放到新连接；新请求
  重新校验 Run 快照和管理员上限。Run 内延长连接存续期不把 session grant 扩展到后续 Run。
- `item/completed` 更新 staging；最终完整持久化历史决定不可变投影，delta 只用于临时 progress。
- `turn/completed` 只表示 Codex Turn 结束，不直接授予 Ait Run completed。
- app-server 进程断开后，Ait 重新取得 writer、读取 Thread 并按原生 ID 对账；已持久化的 Turn 不
  重放。正常断线不把 pending input 自动判为 rejected，也不把冷读 interrupted 直接判为 Run 终态。
- 取消先持久化取消意图，停止发送队列项并 interrupt 自有 Turn；已被原生历史确认的输入和输出仍
  通过受该意图 fencing 的最终对账发布，不能因取消丢掉已发生的原生事实。迟到 delta 不触发普通
  追加，未发送输入标记 cancelled；最终结算 Run 并释放 Session/进程。此规则修订 Codex 路径上
  “取消后直接丢弃所有迟到输出”的旧行为。

### Thread metadata 写穿

对名称、归档、恢复归档和 fork 的 Ait 操作先调用 `thread/name/set`、`thread/archive`、
`thread/unarchive`、`thread/fork`，成功后读取返回值或重新同步；fork 遵守完整 Turn 边界限制。
本地事务失败时保留待对账状态；不得通过回滚 app-server 操作伪造跨进程原子性。永久删除属于破坏性
操作，不在本 ADR 的首期实现范围。

## 持久化与事务边界

全局 catalog 保存轻量 Codex Thread 索引、未绑定条目、provider 扫描状态和权威 binding
reservation；Project 数据库保存已物化的 Session、Message、provider item、lineage、
Thread/Turn projection、pending input、staging、binding receipt 和附件引用。

```text
CodexThreadIndex(
  provider_id, thread_id, codex_session_id, summary,
  sync_state, history_completeness, next_full_verify_at
)
CodexThreadBinding(
  provider_id, thread_id, project_id, ait_session_id,
  operation_id, generation, status: reserved | materialized
)
CodexBindingReceipt(project_id, provider_id, thread_id, ait_session_id, operation_id, generation)
CodexTurnProjection(
  project_id, provider_id, thread_id, turn_id, content_hash,
  parent_message_id, ordered_message_ids, projection_version
)
CodexHistorySnapshot(project_id, provider_id, thread_id, snapshot_id, ordered_projection_ids)
CodexPendingInput(
  provider_id, thread_id, client_user_message_id,
  operation_id, run_id, expected_turn_id?, observed_turn_id?, observed_item_id?,
  pre_send_head, payload_ref, git_commit?, status
)
CodexTurnStaging(
  project_id, provider_id, thread_id, turn_id,
  run_id?, process_generation?, items_view, terminal_confirmation,
  ordered_items_ref, observed_status, checkpoint_version
)
```

`CodexTurnProjection` 保存每个 Thread 的成员关系；唯一键包含 Project/Provider/Thread/Turn、
内容哈希、父 Message 与 projection version。相同 Turn 内容出现在不同父链时不能覆盖旧关系。
`CodexHistorySnapshot` 保存有序 projection 引用并按内容确定稳定 snapshot 身份；相同快照重复
读取时复用，旧快照不被覆盖。Message 上的首次观察来源不承担成员关系。

全局 catalog 与 Project 数据库不能假装成一个 ACID 事务。按以下顺序执行可恢复绑定：

1. 在全局 catalog 对 `(provider_id, thread_id)` 做唯一 reservation，预先分配稳定 Session ID、
   operation ID 和 generation。相同逻辑操作复用 reservation；绑定到其他 Project 的请求返回
   `CODEX_THREAD_BINDING_CONFLICT`。未取得 reservation 不得在 Project 创建 Session。
2. 在目标 Project 事务中校验 reservation 的身份和 generation，幂等创建 Session、Message
   projection 与 `CodexBindingReceipt`；本地 UNIQUE 作为第二道校验，不替代全局唯一性。
3. catalog 根据相同 receipt 将 reservation 标记为 materialized。若此步失败，保留 reserved
   状态，恢复时读取同一 Project 的 receipt 补齐，不再分配 Session 或切换 Project。

reservation 不能仅因超时而重新分配到另一 Project；撤销前须 fence 并终止旧 materializer，确认
目标 Project 未提交 receipt，数据库不可达时保留占位。Project 已提交而 catalog 状态丢失时，
恢复流程先扫描并核对 Project receipt；冲突阻止新绑定，不能任取一份覆盖。轻量 Thread 索引可由
app-server 重建，但 Project 绑定和 operation receipt 是 Ait 的事实，备份与恢复必须保留。

原始 provider payload 不进入普通日志、错误 details 或事件广播。导出和备份沿用 Project 数据保护
策略，凭证、ChatGPT token 和本地 app-server 认证信息永不持久化。

## 公开读取模型

Desktop、HTTP 和 CLI 继续读取统一的 Session/Message 路径，不直接读取 app-server DTO：

- Session 视图增加 `source`、`workspace_mode`、provider status、writer state、历史完整性、
  `sync_state` 和原生 metadata，并暴露原生 fork/历史派生的能力限制。
- Message 继续暴露现有 role/kind；Codex provenance 放在明确、版本化的 provider metadata 中。
- 同一 `turn_id` 的相邻 Message 可以在 UI 中组合为一个 Turn 区块，但存储仍是普通 Message 链。
- provider item sub-message 由统一 renderer 按类型显示；未知类型使用安全的通用摘要卡片。
- pending input 和 staging 明确区分待发送、结果不明、已确认、待最终发布；完整 Turn 发布后，
  以同一事务提交的关联映射替换临时展示，不能重复显示 user item 或丢失 steer 的中间段。

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
- `CODEX_THREAD_BINDING_CONFLICT`
- `CODEX_HISTORY_RECONCILE_CONFLICT`
- `CODEX_HISTORY_CURSOR_REPEATED`
- `CODEX_THREAD_NOT_SYNCED`
- `CODEX_THREAD_ACTIVE_ELSEWHERE`
- `CODEX_THREAD_WRITER_BUSY`
- `CODEX_THREAD_CAPABILITY_UNSUPPORTED`
- `CODEX_FORK_BOUNDARY_UNSUPPORTED`
- `CODEX_INPUT_NOT_ACCEPTED`
- `CODEX_INPUT_OUTCOME_UNKNOWN`
- `CODEX_INPUT_CORRELATION_FAILED`

只读请求的网络、进程暂时故障和可恢复 app-server overload 可以重试；writer busy 可以重新检查
准入，但不重发结果不明的输入。start/steer 的传输错误先进入对账，`CODEX_INPUT_OUTCOME_UNKNOWN`
不能作为自动重发信号。身份冲突、非法结构和 schema 不兼容不可自动重试。错误不得包含完整
item payload、命令输出或凭证材料。

## 不变量

1. 一个已物化 Codex Thread 恰好对应一个 Ait Session；全局 reservation 保证
   `(provider_id, thread_id)` 唯一，Project 事务通过 receipt 幂等恢复。
2. 一个已确认终态且完整加载的 Codex Turn 对应一个有序、非空 Message segment 列表；整个列表
   原子发布，每个 Message 保留相同 Turn ID、内容哈希和唯一 segment index。
3. `userMessage` item 只能投影为 user Message；所有其他 Codex item 只能进入 assistant Message。
4. 一个 ThreadItem 在一个 Turn projection 中恰好出现一次，顺序不变；未知类型不丢失。
5. Codex 原生 item 不创建 Ait ToolUse、ToolResult 或 ToolExecution；Ait 工具循环语义保持独立。
6. 外部历史导入不创建 Ait Run；复用 Message 保留真实的原始 Run 来源。Ait Run 只记录 Ait 实际
   接纳和监督的执行。
7. 同步只追加 Message 或以 CAS 移动 Session 投影，不修改或删除旧 Message。
8. incomplete scan 不执行 missing reconciliation；完整列表仍须按 ID 复核缺失。
   unbound Thread 不跨 Project 落库，Message 和共享根不跨 Project 复用。
9. pending input 接纳后仍在 staging，完整 Turn 发布前不成为 Message；关联 ID 不是服务端幂等键，
   已接纳或结果不明的输入不得自动重发。
10. 同步、实时事件、重试和崩溃恢复对相同 provider snapshot 幂等收敛。
11. NativeCwd 与 ManagedWorktree 的 Git/commit 语义不得混用或静默切换。
12. Codex metadata 不替代 Ait Agent revision、权限快照、Run ownership 或终止屏障。
13. `sessionId` 不充当永久 lineage，runtime status 不证明 writer 所有权；写入必须经过独占
    resume、head/config 复核和串行准入。
14. `itemsView = summary | notLoaded` 不产生完整 Message projection；paginated 不直接触发降级。
15. Codex 原生 fork 只支持完整终态 Turn 边界，不能静默扩大用户选择的历史范围。

## 实现分期

1. **协议与 fixture**：固定 stable/experimental schema 和隔离 binary 行为 fixture，实现
   list/read/turns/items/resume/turn DTO、raw ThreadItem fallback、版本记录和请求关联。
2. **发现与读取**：实现非归档/归档扫描、完整分页、周期强制复核、unbound catalog、Project 匹配、
   全局 binding reservation 与 receipt，以及 Session/Message 历史投影和 durable staging。
3. **继续 Thread**：实现 NativeCwd、独占执行进程、writer 取得/释放、实际配置冻结、pending input、
   `turn/start`、整 Turn 发布、progress、审批、取消和结果不明对账。
4. **同 Run steer**：实现 ADR-009 的 Codex 专用准入、`turn/steer`、有序 staging、Turn 边界移交
   和队列 drain；其他 Provider 仍拒绝活动输入。
5. **fork 与持续对账**：实现已验证 lineage、共享前缀与来源侧表、整 Turn fork 限制、上游回滚/改写
   分支化、归档/name 写穿和启动恢复。
6. **Managed adoption**：显式把原生 Thread fork/采用到 Ait worktree，保持两种 workspace mode 可见且
   不混淆。

### 实现复核（2026-09-19）

本轮修正实现审查中发现的八项问题，继续使用上述 0.153.4 协议基线：

- native writer port 分为 `resume -> connection.start/read -> close`。独占进程取得 writer 后，
  核验返回的 Thread、model、modelProvider、cwd、sandbox 类型、approvalPolicy 和
  approvalsReviewer，再完整读取历史。resume 不覆盖 NativeCwd 或原生 developerInstructions；
  `turn/start` 显式设置 effort、完整 sandboxPolicy 与 `approvalsReviewer: user`。
  model/effort 和实际 modelProvider 随 Run/输入记录冻结；配置或 cwd lease 不一致则在发送前拒绝。
- SendMessage 准入仅写 queued Run 和 durable input，不写乐观的 user Message；Run.base 是
  writer 下重新确认的历史 head。发送前提交 send_unknown，使用 Run ID 作为 clientUserMessageId。
  当前单输入实现保存 queued/send_unknown/published/rejected 四态；完整终态历史才确认发布。
- 完成、失败和中断均走同一套完整 Turn 投影；按 `userMessage.clientId` 对账，按父链赋原 Run ID
  与连续 run_seq，后续 sync 复用相同 Message ID。零匹配保留结果不明，多匹配拒绝猜测归属。
  对账恢复的 Run 与 history 同事务提交，并发出 run.updated，驱动已连接桌面刷新状态。
  重启恢复只取得 writer 并读取对账，不自动重发；旧实现缺少 durable correlation 的 Run 禁止重放。
- close 会终止并回收专属子进程；进度 drain 和审批结算后才提交终态并释放 Session。
  writer busy 识别实际错误 `thread <id> already has an active writer`，与发送结果不明分开。
- 同步在外部读取前取得本地 revision，CAS 冲突后重新读取原生历史。冷读 interrupted/null
  completedAt 不阻止取得 writer；writer 确认 idle 后可发布，但不伪造 completedAt。后续冷读仅在
  Turn 内容哈希与已发布记录相同时复用确认结果。
- 桌面逐项显示 ProviderItem 的回复、推理摘要、计划和操作；未知类型保留单 item fallback。

这仍是 NativeCwd 单输入路径。同 Run steer、周期扫描、原生 fork/ManagedWorktree adoption、
独立 staging/projection 索引及完整 binding reservation/receipt 协议尚未作为本轮交付完成。
首条输入的实时回显仍应作为独立 pending 展示处理，不能通过提前创建 Message 实现。
ADR 的整体状态保持 Proposed，不把上述后续分期标为已完成。

## 验证

以下是完整 ADR 的验收要求；本轮实际执行结果见
[实现修正与验证报告](../reports/adr-016-implementation-fixes.md)。

- adapter fixture 覆盖非归档/归档分页、显式 source kinds、结构化 source、重复 cursor、未知字段、
  未知 item、含 `localAudio` 的 user input、`text_elements`、toolOutput；区分 legacy/paginated
  与 full/summary/notLoaded，验证无实验能力时的分页请求和完整 read/resume。
- projection 测试覆盖 `user -> assistant`、仅 assistant、多个 steer 形成
  `user -> assistant -> user -> assistant`；确认发布前无 Message、发布后 parent/hash/run_seq 正确，
  覆盖真正空 full Turn、空 notLoaded 终态通知、失败/中断 Turn 和原子发布失败重试。
- application 测试覆盖重复扫描不重复 Session/Message、同秒且摘要不变的内容修改、强制重读调度、
  分页期间归档移动、缺失按 ID 复核、读取失败不误删、unbound Thread 和 Project 歧义。
- binding 测试覆盖不同 Project 并发 reservation、各提交点崩溃、catalog 回写失败、过期执行者
  fencing、Project receipt 恢复和数据库不可达时不释放绑定。
- fork 测试覆盖 fork 后 sessionId 改变、已验证 lineage、缺失父 Thread 后的对账、跨 Project
  根隔离、共享 segments 的多 Thread membership 和原始 Run 来源、拒绝 Turn 中间节点 fork、
  上游截断/改写新分支以及 Session CAS 冲突保留历史。
- writable 测试覆盖 clientUserMessageId 到 clientId/item.id 的关联、同 ID 重发确实产生新输入、
  发送前后断线、零/一/多匹配恢复、结果不明不重发、steer 与 Turn 终态竞态、取消后的最终对账。
- 双进程 binary fixture 覆盖 active/inProgress 与冷读 notLoaded/interrupted 的差异、idle
  仍持有 writer、释放后 resume、实际配置校验；单进程覆盖 active 上 turn/start 非空闲条件行为。
- 审批测试覆盖 connection generation、旧 request ID/grant 不重放、不支持原客户端 handler 的
  明确拒绝，以及 Ait 原生权限上限；ADR-009 测试确认 Codex steer 例外不影响其他 Provider。
- Workspace 测试覆盖 NativeCwd 不创建/提交 worktree、ManagedWorktree 保持现有 Git settlement、lease
  冲突和权限上限，以及 ManagedWorktree 的 steer 仍须捕获合法 clean Git 输入快照。
- 安全测试覆盖 payload 大小限制、附件引用、FTS 排除 reasoning/command/tool 原文、日志与错误脱敏。
- 进入 Rust 实现后按工程规范执行 format、workspace build、clippy、tests 和覆盖率报告。

## 协议依据

官方入口为 [Codex App Server](https://developers.openai.com/codex/app-server)。2026-09-19 读取的
在线文档与本机 binary 在分页门控、paginated 支持、fork 的 sessionId 和 isPinned 上存在差异；
本文记录指定构建的 schema 与实测，不把在线文档或版本号单独作为运行保证。升级须重跑兼容验证。

### 2026-09-19 构建记录

| 项目 | 记录 |
| --- | --- |
| CLI | `codex-cli 0.153.4` |
| binary SHA-256 | `b973d440acac501fd2594a43e7ca9ce41e0a65b9dfb28d0d7a7837c99e1261e3` |
| stable schema SHA-256 | `03cd0961387d55845ca2ac1cb7127a9a9724d31ec53897b5a2993b4541168a7b` |
| experimental schema SHA-256 | `a4e7ee85a1237179f0f8cec1e69dc7085e05e038416f7d8f2a2ccbd780ef2a79` |

分别执行以下命令生成 schema；bundle fingerprint 将相对路径按字典序排序，对每个 JSON 文件
依次散列 `relative_path + NUL + file_bytes + NUL`，路径统一使用 `/`，字节为 UTF-8：

```sh
codex --version
codex app-server generate-json-schema --out <stable-directory>
codex app-server generate-json-schema --experimental --out <experimental-directory>
```

### 精确字段与行为

| API/字段 | schema 或本次实测结论 | Ait 约束 |
| --- | --- | --- |
| `thread/turns/list` | 位于 stable ClientRequest；默认 summary、倒序；关闭实验能力仍调用成功 | 显式 full、正序，读取全部页 |
| `thread/items/list` | 位于 stable ClientRequest；默认正序；条目带 turnId | 保留 Turn 关联，不从 item 页猜测 Turn 终态 |
| `historyMode` | 默认新 Thread 为 paginated，完整 read/resume 成功 | 不按 paginated 标签降级 |
| `thread/fork` | 省略或指定 lastTurnId 均返回新 sessionId；forkedFromId 指向源 Thread，继承 Turn/item ID 保留 | 用已验证来源关系建立 lineage |
| `Thread.status` | 两进程分别返回 active 与 notLoaded；冷读 Turn 可显示 interrupted | 不充当跨进程 writer 或终态证明 |
| `thread/resume` | 另一进程持有 loaded/idle Thread 时仍报 active writer | 区分 writer busy 与 Turn active |
| `turn/start` | active 上调用可返回同一 Turn ID | 不具有 expected-idle 前置条件 |
| `turn/steer` | 必需 threadId、input、expectedTurnId；可选 clientUserMessageId | 不覆盖 turn 级模型/cwd/权限 |
| `clientUserMessageId` | 回填到 userMessage.clientId；重复使用可产生新 Turn/item | 只关联，不依赖服务端去重 |
| `turn/completed` | 实测 items 为空且 itemsView=notLoaded | 重新读取历史，不生成虚假空 Turn |
| `Thread` metadata | 无 isPinned；有 section、sectionEnteredAt；source 可为结构 | 按 schema 解码，不混用在线字段 |
| `thread/resume` response | 单独返回实际 model/modelProvider/effort/cwd/sandbox/审批配置 | Run 配置冻结前核验有效值 |
| `userMessage.content` | 包括 localAudio；text 元素字段为 text_elements | 保留原生字段及未知输入 fallback |

表中 effort 指响应字段 `reasoningEffort`；`turn/start` 的覆盖字段才是 `effort`。运行时不得把
thread metadata、resume 返回值和 turn 请求参数的不同字段名混用。

### 实测范围与限制

隔离探针使用临时 Codex 数据目录、合成输入和指向 `http://127.0.0.1:1/v1` 的离线 provider，
未访问用户历史或使用真实模型凭据。验证了 stable/experimental schema、多进程 read/resume、
start/interrupt、完整历史读取、分页和持久化 fork。两个稳定分页请求均在
`capabilities.experimentalApi = false` 下成功。

本次没有验证成功的真实模型响应、全部 native 工具/审批、所有 legacy 历史格式或 Ait 尚未实现的
投影/事务协议。以上结果是协议基线，不是功能验收；进入实现时须将这些行为固化为无凭据的
compatibility fixture，并完成“验证”一节的项目测试。

## 结果

Ait 保持 Session 作为用户可见分支、Message 作为不可变 user/assistant 历史节点、sub-message 作为
节点内部的有序内容，同时让三者与 Codex Thread、Turn 中的角色边界和 ThreadItem 对齐。Ait 不需要
引入复合 ProviderTurn Message；同一 Turn 通过 provenance 关联一段普通 Message 链。用户可以在 Ait
中读取并继续原生 Codex Thread，其他 Codex 客户端产生的后续历史也能通过同一权威来源重新同步。
