# ADR-018：Project 独立恢复与运行期独占接管

- 状态：Accepted；format-3 实现已接入生产路径，验证范围和首期限制见[实现报告](../reports/adr-018-portable-project-runtime-ownership.md)。
- 日期：2026-09-20
- 基线：ADR-001 v4、NEC-235 双层存储、NEC-252 typed transaction、ADR-016、ADR-017、NEC-304 Cron。
- 修订：NEC-235 的永久 coordinator 归属、全局 revision 和跨文件提交协议；NEC-252 的单一 revision 校验；NEC-304 的 Cron 存储位置；ADR-016 的全局 binding 权威和跨 catalog 配置引用。
- 保留：Message 不可变、Session 指针语义、Run 完成屏障、所有 Codex 请求经过 ait-worker、原生 writer 校验和独立 Git 收尾。

## 1. 背景与目标

当前 `<Project>/.ait/project.sqlite3` 保存 `project_id + coordinator_id`；后者标识创建它的
全局 `ait.sqlite3`。打开项目必须同时匹配两个 ID。Desktop 添加目录时又会生成新的 Project ID，
因此重新加入已有目录、切换开发/正式 catalog 或丢失旧全局库，都可能遇到身份不匹配。

该校验保护了实际存在的依赖：项目的 prepared batch 由旧全局库的 pending commit 决定是否
提交，记录路由、事件编号、Project 元信息与执行配置也依赖旧 catalog。只清空 coordinator
字段会绕过这些依赖，不能完成可靠接管。

目标使用方式：

1. Ait A 打开项目后独占管理；A 关闭项目或退出后，Ait B 可在原目录打开同一项目。
2. 新格式项目可以在旧全局库不可用时读取已保存历史、恢复本地提交、重建本机索引。
3. 项目身份和历史不随打开它的 Ait 改变；缺失的执行配置明确显示为待绑定。
4. 崩溃后的锁释放与业务恢复分开处理，未确认的模型输入、工具或 Git 操作不会被自动重做。

首期范围是同一操作系统用户、支持可靠文件锁的本地文件系统。不同 catalog 可以分别打开
不同项目；同一项目只允许一个后端持有管理权。多个界面可以连接该后端。
跨机器共享目录、网络文件系统、多用户分布式锁、同项目多后端并行写入不在首期范围。

## 2. 决策摘要

- `project_id` 是稳定身份；移除“创建者 catalog 永久拥有项目”的授权语义。
- 新增运行实例身份、项目文件锁和持久化接管代次。owner 字段仅用于诊断，OS 锁负责互斥。
- Project 元信息、项目业务记录、局部版本、事件和恢复依据在项目库内形成完整持久化边界。
- 项目变更在一个项目 SQLite 事务中提交；全局 catalog 不再决定项目事务是否生效。
- 全局库保留共享配置和本机注册信息；项目路由与事件订阅是可重建投影，允许延迟更新。
- 新后端先取得锁并恢复，再开放写入。身份匹配、运行锁、worker fence、原生 writer 各自校验。
- 旧格式必须通过可审计的转换进入新协议；本文不授权删除已有会话或递归删除 `.ait`。

## 3. 身份、锁和接管代次

| 概念 | 生命周期与作用 |
| --- | --- |
| `project_id` | 项目创建时分配，重新打开和原地接管保持不变 |
| `catalog_id` | 标识一份全局配置数据库，用于配置来源和订阅游标命名空间，不授予项目写权 |
| `runtime_instance_id` | 每次 daemon 启动生成新的随机 ID，同一 catalog 的两个进程也不同 |
| `owner_epoch` | 在项目库中持久化，每次取得管理权后事务性递增，正常运行期间不重置 |
| Project ownership guard | 持有 OS 锁及实例/代次，构成存储读写、恢复和执行的有效上下文 |
| `owner_info` | 实例、PID、主机和取得时间等诊断信息；可以陈旧，不能用它证明存活或授权 |

不把现有 `coordinator_id` 简单改成 nullable 运行锁；升级后将 catalog 来源和运行 owner
分为不同字段，避免后续实现再次依赖旧含义。

### 3.1 获取与保持

1. 只读规范化目录、识别 Git root、探测项目格式和身份；已有项目不能分配另一个 Project ID。
2. 非阻塞取得 `<Project>/.ait/project.lock` 的 OS 独占锁，再取得当前用户公共运行目录中按
   Project ID 命名的锁。后一个目录独立于 `--database` 和开发/正式配置，防止两个复制目录
   在本机同时以同一 Project ID 活动。获取顺序固定，失败立即释放已取得的锁。
3. 持锁后重新校验路径、数据库文件与身份，防止探测到获取之间目录或符号链接变化。
4. 在项目事务中递增 `owner_epoch`、写入本次实例信息，然后执行恢复和依赖解析。
5. 同一个 daemon 复用该项目的 guard；导航离开页面不释放它，显式关闭项目或 daemon 退出
   才结束管理周期。配置列表和侧栏缓存不应自动占用所有已注册项目。

管理状态为 `closed → acquiring → recovering → open → draining → closed`。获取失败保持
closed/busy；恢复结果不明确时停在持锁的 recovery_blocked，允许读取已提交历史。缺少执行
配置是 open 的能力限制，不伪装成锁失败。执行、同步和恢复使用的 cwd/Run 租约仍各自存在，
Project guard 不改变不同 Session 之间既有的执行并发规则。

锁文件不能通过删除/重建来“解锁”，也不传给 worker 子进程继承；文件存在不表示有人持锁。
同一路径别名必须落到同一目录锁。公共 Project ID 锁只保证上述本机、同用户范围；它不是
远程身份服务。复制项目不能据此获得跨机器并发修改同一份外部资源的能力。

同名 ID 的不同物理副本不得自动合并。UI 要求明确选择本次打开的副本并更新本机注册位置；
仍保持同一稳定 ID。创建独立副本/新身份属于单独的显式导入功能，不在打开流程里修改历史 ID。

### 3.2 写入失效与运行释放

所有项目写入、worker ACK、审批答复、后台标题、历史同步、Git 收尾和恢复任务都必须校验
`project_id + runtime_instance_id + owner_epoch`；worker 原有 `worker_instance_id + lease_epoch`
继续校验。项目代次不能替代 Run 代次，两者共同阻止旧执行者发布结果。

每次重新获得所有权都产生新上下文；旧 UI 请求、缓存 read plan 和延迟回调不能仅因为 revision
数值碰巧相等而继续使用。worker 协议和公开请求版本要能表达并拒绝过期上下文，升级时拒绝
不支持该校验的旧 worker。

正常关闭顺序为：停止准入 → 完成已接纳的短事务 → 取消/排空执行与写入任务 → 收回子进程和
外部 writer → 保存可恢复状态 → 清理本实例 owner 信息 → 释放锁。不能在后台仍可写入时先
释放 guard。显式关闭活动项目先完成取消/收尾；不提供忽略持锁者的“强制接管”。

进程崩溃时 OS 释放其锁；owner 信息可能仍存在。新实例成功取得锁后依据日志恢复，不能
根据旧 PID、时间戳或 owner 字段为空与否判断安全。数据库 fence 只能阻止旧结果提交，不能
撤回遗留进程的文件写入或网络请求：执行监督器必须防止孤儿 worker 继续产生副作用，或能
确认其已结束。无法确认的运行保持 `recovery_blocked`，项目可显示已提交历史，但禁止启动
可能冲突的新执行。Codex writer 仍由原生协议确认，不能用 Project 锁代替。

## 4. 持久化职责

| 数据 | 新权威位置 | 全局库中的用途 |
| --- | --- | --- |
| Project ID、名称、根消息、Git 基线、默认配置引用 | 项目库 | 注册路径与显示缓存 |
| Message、Session、Run、执行 receipt、审批/交互、输入 intent、Git journal | 项目库 | 可重建定位索引 |
| Session 私有 Agent、执行配置快照 | 项目库 | 不需要全局原子创建/删除 |
| Cron 定义与 occurrence 去重记录 | 项目库 | 跨项目列表和调度候选缓存 |
| `project_revision`、owner epoch、项目事件/outbox、提交幂等记录 | 项目库 | 已投影的水位与订阅映射 |
| 命名 Agent preset、Provider、全局默认/Small Agent、Settings | 全局库 | 保持共享配置权威 |
| 凭据秘密值 | 凭据后端 | 仅保存本机引用，不复制进项目 |
| catalog revision、catalog 自身事件、项目注册路径 | 全局库 | 本机配置与目录操作 |

另有独立于 catalog 的本机公共锁目录，以及第 7 节的原生 Thread 归属登记；它们协调本机
独占和外部原生资源，不保存项目历史或决定项目内容是否提交。公共锁文件持久存在，锁本身
随持有进程释放；原生归属登记则按稳定 Project ID 保留，不能随 daemon 退出抹掉。

Project 当前打开路径属于宿主事实；项目库可以保存历史路径供解释，但写入前以当前规范化
目录重新授权。Project 默认 Agent 的历史选择与本机可用绑定分开保存。

Session worktree、NativeCwd、Git common directory、附件和外置资源不因数据库可接管就自动
变为可搬迁。只在路径与资源验证成功后允许继续执行；移动后失效的 managed worktree 需要
显式修复，不能在打开项目时创建同名空目录冒充旧工作区。

### 4.1 Agent / Provider 配置

项目保存引用的来源 catalog、稳定身份、配置 revision 和允许持久化的非秘密快照。
Session 私有 Agent 在项目内拥有完整生命周期；命名 Agent preset 仍在全局编辑，项目保存
已采纳的版本和本地绑定。打开历史无需成功读取全局 Agent 表。

同一来源 catalog 可继续按版本解析 preset；另一个 catalog 中同名或碰巧同 ID 的记录不算
自动匹配。跨 catalog 打开默认产生待绑定项，由用户选择本地 Agent/Provider。绑定必须检查
provider 类型、模型/能力和权限，保存绑定结果及版本；不得为了能运行而静默替换为全局默认
Agent。历史 Message 和已开始 Run 的配置快照不改写。

新 Run 在准入时读取有效配置并冻结；缺凭据时允许查看历史、阻止调用。凭据引用和 endpoint
等宿主配置必须重新验证，项目里的旧路径或引用不能直接成为新宿主的授权。
恢复执行仍需满足当前宿主的管理员权限上限；旧 Run 快照不授予超出当前限制的能力。

### 4.2 Cron

Cron 的定义、目标 Message、Agent 引用和 occurrence receipt 进入项目事务边界，保持
NEC-304 的一次 occurrence 原子创建 Session + Run、相同 occurrence 幂等的语义。
全局展示通过项目投影聚合，不再以全局 Cron 修改与项目 Run 修改组成跨文件原子提交。

跨 catalog 接管后保留定义，但执行启用需本机明确确认且依赖可用；不得因打开外来项目自动
运行任务或补发错过的 occurrence。共享调度器只有持有当前项目 guard 才能触发任务，缓存
中的 enabled 不构成执行授权。当前实现尚无持续时钟循环，本 ADR 不把该能力计入交付。

## 5. 版本与事务协议

### 5.1 分域版本

全局保留 `catalog_revision`，项目新增单调的 `project_revision`。正常业务的 Message、Session、
Run、配置绑定、Cron 和相关事件更新在同一项目事务中提升项目版本；progress 可以有独立序号，
不能拿它假装业务版本更新。Session 自身 version 和 Agent revision 等领域版本继续保留。

typed read plan 记录实际依赖的项目版本、owner 上下文，以及所用 catalog 的身份与配置版本。
纯项目操作只校验项目；读取了全局配置的准入需同时验证配置未变化，失败重新读取和计算。
数值来自不同 catalog 时不能互相比较，也不能通过重设全局版本“适配”旧项目。

需要冻结最新全局配置时，短暂持有对应 catalog 的配置锁，在锁内读取并验证配置，随后完成
项目本地准入事务。此过程不写全局业务状态，不做网络、模型或 Git 长操作。持有项目运行
guard → catalog 短锁 → 项目短事务的顺序固定；全局写命令不得反向等待项目运行锁。
CAS 重试只重新读取和计算，不重复模型调用、建 worktree 或执行工具。

### 5.2 项目本地提交

项目命令的线性化点是项目 SQLite 事务提交。一个事务同时写入：

1. Message append、Session/Run/Cron 等业务变更；
2. 必需的 input intent、worker receipt、执行状态或 Git plan；
3. `project_revision` 和本地事件/outbox；
4. 操作幂等 receipt（稳定 operation ID、请求指纹和可重读结果）。

提交前崩溃则 SQLite 回滚；提交后响应丢失通过相同 operation ID 查询原结果，换请求内容
必须冲突。幂等 receipt 不随着普通 UI 事件窗口一起被丢弃。项目的已提交/未提交状态不再依赖
旧 catalog 的 `pending_commit`。

全局命令只在全局事务修改命名配置、Settings 或本机注册。禁止继续提供任意混合全局和多个
Project 的原子 `apply`：项目私有 Agent、Cron 已归入项目；注册先初始化/验证项目再幂等登记
catalog，登记失败返回“项目已保存、登记待重试”，不能让客户端重新执行业务。删除全局配置
不级联破坏离线项目，已有快照保留，后续执行显式暴露缺失依赖。

确有跨范围流程时必须逐项定义 intent、幂等步骤和失败结果，不能引入新的通用双库提交决定。
本 ADR 不承诺跨项目命令的原子性。原有要求单次原子提交的 Session/Run/receipt 不得拆散。

### 5.3 执行副作用

模型调用、工具执行和 Git 更新仍按现有持久 intent/receipt 协议工作；它们不会因数据库使用
单库事务而获得 exactly-once 保证。旧 Run 接管时先增加执行代次、核对结果，再决定恢复方式。

Codex 的 send-unknown 输入只读取原生历史对账，不自动重发；未知工具结果不自动重复执行。
已发布原生结果的 Git finalization 按保存的 plan/commit ID 恢复，不能重新运行模型或生成
另一份提交。旧审批授权失效，待处理交互根据原执行身份恢复。恢复失败局限于对应项目/Run，
不得让该项目的日志阻塞其他项目的 catalog 查询与恢复。

## 6. 事件、索引与读取

项目事件使用项目本地连续序号。事件键包含 `project_id + project_stream_id + event_seq`，
不复用全局 SSE cursor。普通重新打开保留 stream ID；显式恢复旧备份或创建独立历史副本时
需要新的 stream generation 和订阅重置，不能把回退后的序号当成原流继续发送。

全局 feed 可继续聚合事件，但只是投影：在一个全局事务内按项目事件键去重，更新路由/显示
缓存、投影水位并分配本机 feed 序号。项目成功提交后，即便 catalog 投影失败，成功结果仍成立。
后台重放 outbox 完成更新，客户端不应因此重复发送 Run 输入。

公开订阅游标带 `catalog_id + feed_generation + sequence`。切换 catalog、重建 feed 或
检测到无法覆盖的事件缺口时返回明确 reset，客户端重读当前视图；不静默沿用旧数字游标。
投影重建先读取项目的一致快照及事件水位，再接续其后的事件。事件保留窗口不足时再次取
快照；不得为修复缓存修改 Message 历史。

按 ID 的全局路由是提示索引。权威读取需要当前可用的 Project 上下文；索引缺失/落后时先
完成该项目必要的重建或返回 rebuilding，不能直接把真实存在的 Session 判成不存在。
API 尽量显式携带 project_id；保留的纯 ID 接口通过注册索引解析且核验归属，不能每次请求
扫描全部项目。离线/被其他后端持有的项目只显示带状态的 catalog 缓存，首期不绕过锁直接
读取活动项目库。缓存不能用于执行准入。

## 7. Codex 原生绑定与项目接管

所有发现、读取、同步、writer 取得和执行继续经过 ait-worker；本 ADR 不新增第二条 Codex
调用路径。Project 锁不阻止 Codex App 或其他原生客户端，writer busy 仍按 ADR-016/017 处理。

原生 Thread 的权威来源与 Ait Provider catalog ID 分开：接管时保留 thread ID、历史投影和
原生 cwd，重新验证本机 Provider 是否访问同一原生存储和同一 Thread。不能仅凭 Thread ID
字符串或 Agent 名称相同就建立关系。协议不能证明相同来源时保持未解析，禁止写入；历史
投影仍可查看。项目库不包含 Codex rollout，不承诺跨机器自动恢复原生 Thread。
当前 Session 中的 provider_id 是 catalog 引用，不能直接充当存储身份；适配器需要显式暴露
可验证的本机原生来源上下文，不能臆造 app-server 字段。此处允许重建同一来源的宿主连接映射，
不放开把已绑定 Thread 切换成另一原生来源或其他 Provider 的限制。

唯一绑定不能只依赖某个 catalog 的缓存。项目内持久化绑定 receipt；同机跨 catalog 的发现/
绑定操作需使用按已验证原生来源和 thread ID 键控的公共 reservation，关联 Project ID。
该 reservation 只仲裁唯一归属，没有项目内容的事务提交决定；与项目 receipt 通过稳定
operation ID 对账。失败停在可重试/待核对状态，不把 reservation 成功当作导入成功。
归属记录跨 daemon 退出保留，同一 Project 换 catalog 只更新宿主定位信息。公共记录丢失时
先从可验证的项目 receipt 重建；无法证明旧归属的 Thread 标记 binding_unknown，禁止直接
登记给另一个项目。离线旧项目没有出现不能作为“无归属”的证据。重建不依赖旧 catalog，
但原生可写继续仍取决于这些独立的外部资源条件。

无法证明来源唯一性或 writer 可取得时，不提供“强制接管 Thread”。本 ADR 不放开原生 fork、
steer、Codex Cron 或便携 archive 的既有能力限制。

## 8. 打开、关闭与 UI 行为

Desktop、CLI 和 HTTP 共享同一个 application 打开项目用例：

1. 无 Ait 项目库的目录按现有规则创建项目；存在项目库则识别并复用 Project ID。
2. 当前实例已打开同一物理目录时直接复用，不创建新项目、根 Message、Session 或 Run。
3. 尝试获取管理锁；忙时显示“项目正在由另一个 Ait 后端使用”，可重试或连接已知后端。
4. 检查格式、取得新 owner epoch，恢复项目事务与执行状态，验证工作区和本机依赖。
5. 按项目库重建注册/路由/事件投影，然后展示项目。配置缺失、writer busy 或某个 Run 待核对
   独立显示，不把整个项目历史隐藏为身份错误。

注册列表与运行管理权分离。侧栏折叠、切换页面不关闭项目；提供显式关闭操作，保留磁盘内容
和可选的最近项目记录。启动恢复只对需要恢复的已知项目非阻塞尝试加锁，被占用则标记 busy，
不能阻塞所有项目启动。新接管的外来项目不会自动触发 Cron、标题生成或排队的模型输入。

错误需要区分 busy、未知格式、配置缺失、工作区失效、恢复受阻、旧格式需要原 catalog；
UI 展示可执行的处理方向，不直接透出 Electron `Error invoking remote method` 包装串。
打开与关闭用例必须可幂等重试，业务回执与后续 UI 刷新成功与否分开。

## 9. 旧格式转换与恢复边界

本文讨论的独立接管适用于完成转换的新格式。旧项目缺少的 Project 元信息、Agent 配置、Cron
和全局提交决定，不能通过猜测补回。ADR-017 曾获授权的一次性删除旧会话不延伸到本次转换。
首个转换器以当前全局/项目 `user_version=2` 且已完成既有切换为输入；更早格式走其明确的
升级流程，未知格式拒绝写入。本次转换不能重新触发 ADR-017 已完成的一次性历史清理。

### 9.1 原 catalog 可用

1. 停止旧后端与 worker，先用原 catalog 完成旧协议已经决定的提交；不能绕过 identity 校验。
2. 转换工具独占旧 catalog 和目标项目，创建一致的备份；先在全局事务中写入带稳定 operation
   ID 的转换清单并提升全局格式版本，确保旧程序在任何项目转换开始前就拒绝访问。
3. 按项目复制必要元信息、非秘密配置快照、私有 Agent、Cron 和去重依据；重建本地版本、事件
   namespace 和索引。保持 Project/Message/Session/Run ID、消息内容及树结构不变。
4. 每个项目在本地事务内完成转换并写入完成 receipt。旧的无提交决定的 prepared batch 只能
   在原 catalog 已核对无决定后清理，不能凭新后端自己的空 pending 表判断。
5. 全局转换清单登记项目已转换；中途崩溃通过项目 receipt 继续，不能再次覆盖已经转换的内容。
   首期采用整个 catalog 离线转换：只要清单尚有 preparing 项，新 daemon 就拒绝启动并提示
   重跑转换命令。无需运行时分派新旧写协议；未完成旧事务必须在格式屏障前整体恢复。
   项目成功转换后，正常打开按本地事实重建索引和 feed。
6. 项目本地转换事务同时提升项目格式版本；转换清单区分转换进行中、已转换与已投影
   的语义边界；首期持久化 preparing/converted，索引由独立水位管理，以本地完成 receipt
   为转换结果依据。验证完成前不删除旧 catalog 源记录与备份；
   清理是单独的显式动作。旧版本程序对两种新格式均拒绝访问。

转换不是先改 coordinator 再碰运气恢复；转换清单也不能成为新格式正常运行的永久依赖。
原 catalog 在转换后不再对该项目执行旧 pending commit 或恢复任务。

### 9.2 原 catalog 不可用

普通打开返回明确的 legacy recovery required，保留现有文件。可提供独立、只读的历史检查/
导出途径，但不能声称已经安全接管或完整恢复执行配置。若存在 prepared batch，无法仅凭
项目库确定旧全局提交决定，不允许自动执行或丢弃。

今后可以设计显式的数据抢救流程，但它必须区分可验证历史、缺失配置和未知提交结果；本 ADR
不提供“清空 coordinator”或“重建新库覆盖旧库”的快捷路径。

### 9.3 新格式备份与目录副本

新格式项目在线备份包含全部项目事务状态，可在旧 catalog 丢失后重新注册；凭据、机器资源
与 Codex 原生历史仍需独立满足。恢复备份前先停止相关执行并获取锁，明确建立新的运行上下文
和事件 generation；不能用数据库回退重放已经发生的外部副作用。发现原副本仍被持有时拒绝
启用恢复副本。普通打开不自动执行备份恢复、历史合并或新身份复制。

## 10. 代码边界与实施顺序

- `domain` 仅增加必要的纯数据身份/版本与不变量，不依赖 Tokio、SQLite、路径锁、UI 或 provider。
- `ports` 表达 Project ownership guard、分域读取/提交、操作 receipt 和投影重建能力；SQL 和
  OS 锁保留在 adapters。废弃把所有记录提交到一个全局 revision 的通用生产写入口。
- `storage-sqlite` 实现项目 schema、本地事务/事件、catalog 投影和可重试格式转换。
- `application` 负责打开/关闭、配置绑定、准入、恢复、Cron 和后台操作的所有权约束；typed
  context 继续限制最小读取与变更范围。全局配置与项目业务的组合必须有明确的版本验证。
- daemon/worker/IPC 接入 owner fence、关闭顺序及协议版本；Desktop/CLI/HTTP 只投影这些语义。

实施分四步，每一步都保持 Cargo workspace 可构建，但只有完整链路通过验收后才开放跨
catalog 接管：

1. **项目自足存储**：元信息、私有配置、Cron、本地 revision/event/receipt 和新事务 port；
   完成旧格式转换工具与故障注入，保留现有 owner 拒绝规则直到后续完成。
2. **项目运行管理**：公共/目录锁、owner epoch、所有写入口 fencing、退出与异常恢复。
3. **全局解耦**：重建注册/路由/feed、跨 catalog 配置绑定、原生绑定 reservation 对账；移除
   新格式路径上对旧 pending commit、global revision 和永久 coordinator 的依赖。
4. **统一打开体验**：复用已有项目、busy/缺配置/待恢复状态、显式关闭、各入口与真实双后端验收。

不能把“去掉校验”作为可独立发布的第一步。迁移和新事务协议的实现需要单独代码评审，不能
将此文档状态或当前已有测试通过视为新行为已经得到验证。

## 11. 验收与故障矩阵

| 场景 | 必须满足的结果 |
| --- | --- |
| A 正常关闭，B 使用另一 catalog 打开 | 复用 Project/历史身份，无新根消息；索引可重建，缺配置可解释 |
| 两个 daemon 同时打开同一目录或路径别名 | 仅一个获得所有权，另一个 busy，无数据库变更竞争 |
| 两个副本具有相同 Project ID | 同机不能同时成为活动项目；选择副本不合并历史 |
| daemon 被终止、owner 字段残留 | OS 锁可重新取得；新 epoch 拒绝旧请求，先恢复再允许执行 |
| worker/工具子进程仍存活 | 能回收或证明结束；否则保持执行受阻，不仅依靠数据库 fence |
| 项目事务前/中/后、响应前分别崩溃 | 回滚或原结果可查；Message/Session/Run/receipt 不部分发布 |
| 项目提交成功，全局投影失败 | 业务仍成功，重放不重复；新 catalog 可从项目快照重建 |
| 切换 catalog、事件缺口或恢复旧备份 | 游标 reset 后重读；无事件键冲突、漏刷新或历史改写 |
| 配置更新与 Run 准入竞争 | 使用可验证版本冻结配置，冲突重试不产生重复外部调用 |
| Agent/Provider 不存在、同名异源或缺凭据 | 历史可读，执行要求明确绑定，不静默换 Provider |
| 原生 Thread 不可访问、writer busy、输入结果未知 | 保留绑定与历史，对账或受阻；不开新 Thread 冒充原会话 |
| Git plan 已持久化或 ref 更新后 ACK 丢失 | 按原 plan/commit ID 对账，不重新执行模型 |
| Cron occurrence 与接管并发 | 持有者才能准入；单项目事务去重，外来项目不自动启动调度 |
| 每个旧格式转换边界中断 | 使用原决定和转换 receipt 继续，不删历史，不覆盖已转换项目 |
| 旧 catalog 丢失且 prepared batch 未决 | 报明确恢复限制，不猜测提交决定 |
| 一个项目恢复受阻 | 不阻塞其他项目及全局配置访问 |

需要真实双进程/双 catalog、SIGKILL 或等价终止、真实 SQLite/Git 及 worker drain 测试；纯 mock
不能作为 OS 锁和崩溃恢复正确性的证据。macOS/Windows 至少完成发布平台验证，Linux 验证后
再声明支持。故障测试覆盖所有提交、ACK、投影、owner 切换和转换清单边界。

## 12. 取舍与未采用方案

- **仅退出时清空 coordinator**：无法处理异常退出、未决提交、全局索引和配置依赖，不采用。
- **只增加项目文件锁，沿用全局事务**：可以排除同时写入，但旧 catalog 丢失后仍无法判断提交，
  不满足项目独立恢复目标。
- **永久共享一个 daemon/catalog**：仍是多个客户端协作的有效方式，但不能满足退出后由另一份
  Ait 配置独立接管，因此不作为唯一使用方式。
- **所有配置都放入项目**：会把用户级共享 preset、连接和凭据管理带进项目；保留全局共享配置，
  通过项目快照和明确绑定解决依赖。
- **同项目多写者**：需要更复杂的冲突、执行和外部资源协调，首期保留单后端管理。

代价是 schema/port/事件协议变化、显式配置绑定，以及全局视图的最终一致性。收益是项目
历史与恢复不再永久依附创建它的 catalog，并能把单项目故障限制在该项目内。

## 13. 实现与验证

实现、运行方式、故障测试、覆盖率和未验证平台记录在
[ADR-018 实现与验证报告](../reports/adr-018-portable-project-runtime-ownership.md)。

首期 Codex 原生来源仍按不可证明处理：跨 catalog 历史可读，原生写入与 Agent 重新绑定受阻。
公共 reservation 在缺少来源证明时保守地只按 Thread ID 仲裁，可能拒绝不同来源的同 ID，
不会因此授予原生写权限。该保守行为不能表述为已经支持跨机器或跨原生来源续跑。

普通打开不提供旧备份恢复或新身份复制；发现已知 stream 的 revision 回退时要求专门恢复，
不自动重放输入。数据库操作 receipt 覆盖相同读版本与批次的重试；HTTP 业务输入继续采用
各用例已有的稳定 ID/intent/receipt，不新增“任意 HTTP 请求 exactly-once”的承诺。

发布平台的系统锁、路径替换和进程树行为必须独立验证。未验证的平台不能因 Rust 可编译就
声明已完成本 ADR 的崩溃接管验收。
