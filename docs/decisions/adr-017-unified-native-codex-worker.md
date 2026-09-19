# ADR-017：Codex 统一原生 Thread 与 Worker 执行

- 状态：Accepted，2026-09-19
- 上位约束：ADR-001 v4、ADR-016、ADR-013
- 取代：NEC-174 的历史 prompt 重组、NEC-209 的每 Run worktree、NEC-212 的合并/回滚与 checkpoint 重放；修订 NEC-169 Worker 协议和 NEC-235 会话存储切换。

## 决定

生产环境全部 Codex 请求经过 `daemon → WorkerSupervisor → ait-worker → codex app-server`。范围包括创建/恢复 Thread、Turn、完整历史读取、Thread 列举、模型发现以及标题生成。辅助操作有独立 scope，不伪造 Run。daemon 不创建 app-server，不保存其 stdio writer。

Ait 新建会话与导入会话都绑定持久原生 Thread。新会话在固定 `<Project>/.ait/<session-id>` Session worktree 中运行；导入会话使用原生 cwd。每 Run 临时 worktree、变更回集、失败回滚、路径改写及对应 prompt 包装全部删除。Codex 直接修改 cwd；失败和取消也保留已经产生的文件修改。

`thread/start` 使用 `ephemeral:false`，只在新 Thread 设置 cwd 和显式 Project developer instructions。恢复使用 `thread/resume`，不覆盖 cwd/developer instructions。`turn/start` 只发送本次用户输入，使用冻结的 model、effort、sandbox、approval policy 和 `approvalsReviewer:user`。不发送 `Conversation:` 历史拼接，不要求模型提交 Git，不重建 Codex 原生工具定义。

## 原生 API 与发送边界

协议按本机 Codex 0.153.4 验证：`thread/read` 使用 `includeTurns:false` 读取元信息，再通过 `thread/turns/list`、`itemsView:full` 遍历完整历史。`turn/completed` 中的轻量 items 不能当作权威历史。

新建 Thread 在第一条输入前可能尚未 materialize，不能立即分页读取或通过新进程恢复。准入使用 `thread/start` 返回的空历史；Worker 持有进程并等待 Ait 完成 CAS。Run 和待发送 input intent 持久化后才发 `Start`。发送前先写入 send-unknown，原生 `clientUserMessageId` 只用于归因，不能视为幂等键。

恢复不重发未知输入。已排队但未发送的输入终结为明确拒绝；首次明确拒绝时解除未落盘的空 Thread 绑定，下次请求可新建 Thread。发送结果未知时保留原 Thread 绑定，后续读取/同步以 clientId 对账。用户 Message 仅从已确认的原生历史物化。进程关闭、审批与进度 drain、完整历史发布完成后才释放执行所有权。原生 writer 准备与重读不持有 daemon 关闭屏障；仅持久化准入短暂持锁，并在锁内重新检查 draining，避免关闭后接纳输入。

Worker wire 协议升级为 2.0，使用 `scope_id` 和必需的 `native-codex-v1` capability；旧 Worker 被明确拒绝。Worker 结果采用有序分块和总量上限，writer ownership proof 单独传递；本地展示中的字段不能伪造已获得 writer。Worker 继续执行 wall-clock、step、token、输出限制；未取得可靠报价时拒绝启用成本上限的模型调用。

## Ait 自动 Git 提交

`codex.auto_commit` 是可选客户端功能，默认关闭。设置在 Run 准入时冻结，导入与新建 Thread 使用同一规则。仅成功的原生 Turn、权威历史发布以及执行收尾后进入独立 Git finalization。Run 的 `git_commit` 展示 pending/prepared/committed/skipped/failed；不修改 Message。

起始目录有未提交修改、HEAD/分支/index 改变时跳过提交；这些情况不阻止模型执行。没有变化时跳过。失败/取消/资源超限的模型执行不提交。

提交范围是该 cwd 在收尾时的文件差异。Ait lease 只协调 Ait 自身执行；删除每 Run 隔离后，无法区分执行期间用户或其他进程新增的未暂存文件改动，这些改动也可能进入提交。共享原生 cwd 时应由用户据此决定是否开启自动提交。

Git 使用独立 index 生成 tree 和 commit object，先持久化精确 plan，再通过 Git ref transaction 锁住 HEAD 与分支、在锁内复核符号引用并发布。工作文件不重写。index receipt 与 index.lock 使用同一文件的硬链接证明所有权，允许进程中断后恢复自己的锁，不能删除外部 Git 锁。ACK 丢失后复用已记录的 commit ID；不再次运行 Codex。

Git 失败保持模型 Run completed，并单独显示失败原因。CLI `ait run retry-commit --run-id …`、HTTP `POST /v1/run/retry-commit` 与 Desktop 按钮只重试 Git。准备好的 plan 不替换成另一个 commit。

## 旧会话切换

不迁移旧 Ait 会话。首次打开旧 catalog，在全局锁内先完成已有存储提交，再登记一次性 Project 清理清单，删除 Session/Run/历史索引、旧事件和 Session 私有 Agent。依赖被删除消息的 Cron 同步删除；Project 根消息、项目配置、命名 Agent、Provider 和独立根 Cron 保留。

每个 Project 在验证数据库身份和 coordinator 后，事务性清理旧 Session、Run、非 Project 根消息、progress、旧执行 journal。离线 Project 延迟到下次打开。完成标记与删除同事务提交，新会话不再被清理。格式升级阻止旧版本误读。

该操作仅删除 Ait 数据库内的会话记录，不递归删除 `.ait`，不删除 Session 工作文件、项目源码或 Codex rollout。正常运行仍保持 Message 不可变；这里是用户明确授权的一次性格式切换。

## 当前能力边界

### 项目菜单中的历史发现与同步（2026-09-19）

Desktop 在 Project 的操作菜单提供 `Pull from Codex…`：用户先查看匹配的原生 Thread，
再勾选导入或同步。普通项目导航仍只读取 Ait 已持久化的会话，不隐式拉取原生历史。

`ListCodexThreads` 与 `GET /v1/codex/thread/list` 接受可选 `project_id`；CLI 对应
`ait codex list --provider-id … --project-id …`。不指定时保留完整发现行为。指定时由
application 使用导入的同一归属规则筛选：已有绑定优先；未绑定 Thread 的 canonical cwd
必须唯一匹配注册 Project 的根目录或其后代，或精确匹配已有 Session workdir。
嵌套项目等多重归属、无匹配和无法规范化的目录不出现在该项目列表中。

发现列表按 `created_at` 正序分页以减少更新引起的条目位移，最终展示仍按 `updated_at` 排序。
遵循 ADR-016 的非快照语义，同页、跨页及归档扫描间的重复 Thread ID 合并为一项，摘要取最后
一次观察值，不将 `updatedAt` 当作版本号；重复 cursor 和无效身份仍报协议错误。

新导入使用所选 Provider 的启用全局 Agent，默认优先当前 Project 的 Codex Agent；
已导入会话保留绑定 Agent。执行同步时 application 再次校验归属、Agent 与 writer 状态。
同步通过现有 worker 历史端口完成，不发送 `turn/start`，不创建 Run 或触发自动 Git 提交。
成功回执独立于后续视图刷新；批次允许部分成功并逐项重试失败项。关闭弹窗停止尚未发出的
批次项，已经发出的同步仍可能完成，并只刷新原目标 Project。

### 执行能力

支持新任务、已有原生 Thread 继续执行、历史导入/同步、审批、取消、崩溃对账、模型发现、标题与可选自动提交。

原生 Thread fork/steer 尚未实现。任意历史节点的派生、繁忙 Session 的自动分支、Codex Cron 以及已绑定原生 Thread 的便携 Ait archive 导入/导出返回明确能力错误。允许从 Project 根创建独立新任务；允许在可复用当前 Session 时继续。绑定原生 Thread 后不能切换 Provider。不存在旧执行路径回退。
