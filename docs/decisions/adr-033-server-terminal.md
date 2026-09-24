# ADR-033：独立 server-terminal 与完整 Terminal 方法分组

- 状态：Accepted。
- 日期：2026-09-24。
- 授权：实现 `server-terminal`，继续实现 Paseo Terminal 相关接口。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 范围：独立 server；不修改旧 daemon、Message、Session 或 Run 的领域语义。

## 能力和依赖

新增纵向能力包 `server-terminal`：`protocol` 拥有 Paseo payload 与二进制帧；`service` 校验
Workspace/Project placement、管理进程和 resize 所有权；`ports` 隔离目录解析与 PTY；`local`
用 `portable-pty` 启动真实进程，`screen` 用 `vt100` 维护有界屏幕和输出；`rpc` 负责参数与结果。

该包只直接依赖 `server-metadata` 的 registry 端口，不依赖 Provider、旧 Ait crate、Tokio、SQL 或
HTTP。`server-api`、`server-protocol` 和 `server-bin` 可以依赖该包。这更新 ADR-031 的依赖表；
依赖守卫覆盖 optional/dev/build/target-specific 边。新依赖的 unsafe 实现在外部库内部；workspace
继续 `unsafe_code = "forbid"`，不加入本地 unsafe。

Terminal 不是 Agent 原生 session，也不是 Message/ToolResult。终端的进程、屏幕和输出只在当前
server 实例中存在；不保存或恢复 shell 进程，不把终端输出写入不可变 Message 历史。

## 全部 Terminal 入站方法

| 规范名称 | 行为 |
| --- | --- |
| `terminal.list.request` | 全部、按 Workspace ID、或按最深 Workspace 根目录筛选运行终端 |
| `terminal.list.subscribe.request` | 连接拥有的列表快照与后续 `terminal.list.changed` |
| `terminal.list.unsubscribe.request` | 按原 cwd/workspaceId 释放列表订阅 |
| `terminal.create.request` | 创建 PTY，支持 executable、字面 args、name 和初始 size |
| `terminal.rename.request` | 设置 1–200 UTF-16 单元的 trimmed title，覆盖后续 OSC title |
| `terminal.subscribe.request` | 分配连接 slot，原子取得屏幕和输出 cursor，再发布快照/增量 |
| `terminal.unsubscribe.request` | 释放该连接的 terminal 输出订阅 |
| `terminal.input` | event：文本输入、resize claim/update、应用启用的鼠标模式 |
| `terminal.kill.request` | 幂等关闭并回收进程，现有 observer 先排出尾部输出再收到 exit |
| `terminal.capture.request` | 渲染后的 history + grid，包含闭区间与负数索引 |

十个方法全部进入已实现能力；catalog 总名称集合不变。名称继续采用 ADR-026 的规范 dotted name，
不开放旧名称 alias。请求关联 ID 位于统一信封，不复制 Paseo payload 内的 requestId。
`agent.items.close.request` 的 `terminalIds` 由 API 调用 Terminal 服务，Provider 只处理 Agent。
关闭结果保留逐项 `terminalId/success`；并未把终端依赖引入 Provider。

创建要求 active Workspace 和 active Project。未提供 workspaceId 时按规范化 cwd 选择最深根；
显式 workspaceId 也必须包含 cwd，拒绝通过 symlink 越过该 Workspace。registry 继续保留用户路径
写法，Terminal 比较时规范化。`agentId` 对应 Paseo 已退役的 Agent-backed Terminal，用明确失败回应。

## 连接和流

二进制头为 `[opcode, slot]`，保留 Paseo 0x01 output、0x02 input、0x03 JSON resize、
0x04 JSON snapshot、0x05 ANSI restore。与既有 0x10–0x12 文件帧分流。写入必须协商
`terminal.input`，binary slot 只能访问当前物理连接自己的订阅；服务端方向 opcode 不可作为输入。

不指定 restore 时发送 JSON cells/grid/scrollback/cursor；live 发送 input-mode preamble；
visible-snapshot 默认最多 200 条历史、请求上限 500；full-snapshot 返回全部保留历史。
输出 revision 与快照在同一屏幕锁内取样，恢复之后只交付更新的 revision，防止重放/遗漏竞争。
每个 Terminal 有 256 KiB 最近输出环；observer 落后时发送恢复快照，live 退到 visible restore。
终端 size 改变后先发送 0x03，再发送新的快照，不拼接不同尺寸的旧增量。

列表和输出订阅计入已有每连接 16 个订阅上限，支持通用 `subscription.release.request`；相同目标
重订阅替换旧订阅。响应先入队，再发送 bootstrap。单个连接读循环每 40 ms 拉取增量，响应、释放、
事件和二进制发送保持串行顺序；释放响应后不会再发送该订阅的新消息。断线只释放 observer，
终端进程继续运行，允许新连接恢复屏幕。退出发布 `terminal.stream.exit`。

resize 的 claim 将尺寸控制交给物理连接，update 仅对当前 owner 有效，未传 intent 等同 claim。
输入不要求拥有 resize 权限。发送继续使用原有 4 MiB/消息数队列预算与 socket 写超时；慢连接
不会使 PTY reader 停止读取，也不会无限积累输出。

## 生命周期和预算

独立的 Terminal 阻塞任务预算避免 Git/Forge 长请求占用 PTY 输入通道。每个实例最多保留 32 个
Terminal entry；满额时淘汰已退出/关闭的屏幕。尺寸上限 100 rows、200 cols、10,000 visible cells；
快照与 capture 按最多 12,000 总 cells 输出。parser 最多保留创建时配置的 1,000 history rows；
resize 不重建 parser，以保留 UTF-8/ANSI 分片和 alternate screen，最宽时内部历史最多约 200,000 cells。输入每次
最多 64 KiB，最多 16 个排队 chunk；超限返回稳定资源错误。

本机 reader/writer 使用独立线程，启动中任何失败都由 process owner 回收已经启动的子进程。
Unix 关闭 child process group 和当前 foreground group，并等待 child；关闭成功前 drain reader。
首次观察到 shell 自然退出时也清理其后台进程组；已经回收的进程记录不再向旧 PID 发送信号。
宿主 drain 后先等待已接纳操作，再 shutdown 全部 Terminal，最后释放实例 lease。
Workspace/Project archive/remove 由独立的 250 ms reconciliation 清理，即使没有连接也运行。
服务重启清空所有终端；认证环境 `AIT_SERVER_*` 不传给 child，屏幕与输出不写持久化文件。

## 明确差异

这是十个 Terminal 方法及其 binary transport 的完整入口实现，不表示移植 Paseo 的整个 xterm、
provider hook 和自动化生态：

- `vt100` 保留常见 ANSI、颜色、宽字符、鼠标、alternate screen 和输入模式；不是 xterm 的完全
  仿真。某些样式（例如 strikethrough）、扩展键盘协议、resize reflow 与光标样式尚未完全对齐。
- `activity` 返回 null；未安装 shell/Agent activity hooks、activity HTTP token route 或通知 hook。
- Workspace setup/script 仍由 ADR-031 的 metadata executor 运行；其中逻辑 `terminalId`、setup
  transcript、proxy/health、自动 profile 启动与 teardown 不在本 ADR 中宣称变为 PTY。
- 已自然退出终端的 capture 可读取保留屏幕直到淘汰；显式 kill 后 capture 为空，旧输出 observer
  可以排出其尾部。无法恢复已退出/重启后的进程。
- Unix 保证 child 和已知进程组回收；自行脱离 session 的守护进程不在此范围。Windows 使用
  `portable-pty` 的 ConPTY 行为，未进行 Windows 实机验证。

测试、覆盖率和命令见[实施报告](../reports/server-terminal.md)。
