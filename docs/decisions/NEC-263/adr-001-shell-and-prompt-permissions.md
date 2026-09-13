# ADR-001：Prompt 权限入口与 API Shell 权限执行

- 状态：Proposed，待 NEC-263 验收
- 日期：2026-09-13
- 基线：ADR-001 v4、NEC-234、NEC-247、NEC-192

## 决策

Desktop Prompt 工具栏提供 Readonly、Workspace Write、Full Access，保存既有全局
`permissions.sandbox` 默认值。保存沿用 Settings revision/CAS，携带完整设置，仅改该字段；
保存过程中禁止提交。失败恢复已确认的设置，成功后对新 Run 生效。活动 Run 显示其固定快照，
Send/Fork/Derive/Cron 的快照和管理员上限沿用现有 application 规则，不引入第二套权限状态。
这是一项新 Run 默认设置，适用于所有 Session；不是临时单次升级批准。

普通 API Provider 的 `bash` 从 echo/printf/sleep 白名单改为真实 Bash。该适配仍位于
`ait-tools` 的宿主执行器内，经 `SandboxToolFactory` 的管理员上限核验，不改变 domain 或 ports。
仅广告平台可执行的工具：

| 平台/档位 | 执行方式 |
| --- | --- |
| macOS Readonly | Seatbelt 默认拒绝；允许读文件、启动受继承策略约束的子进程；禁止写文件和联网 |
| macOS Workspace Write | 同上，仅增加 Session workdir 写权限，保护 `.git`、`.ait` |
| Linux 受限档位 | 需要系统 `bwrap`，独立 PID/network/IPC namespace，宿主只读挂载；Workspace Write 增加工作区可写挂载并将根下已有 `.git`、`.ait` 挂回只读 |
| Unix Full Access | 直接运行 Bash，不施加 OS 文件/网络沙箱，仍受 Run/管理员权限上限和资源限制 |
| 缺少所需后端、Windows | 不广告 Bash，不降级成未隔离命令；read/grep/glob 仍可用于仓库浏览和统计 |

受限 Shell 不通过字符串解析判断读写性；系统策略作用于整个命令与子进程。默认 cwd 为
Session workdir，显式 workdir 规范化后检查范围。环境仅保留 PATH、工作区 HOME 和语言设置，
不继承 Provider 凭证或 Bash 启动配置。Full Access 允许工作区外 cwd。

Shell 默认 10 秒、最长 120 秒；stdout/stderr 分别有界，超限继续排空并返回截断标记，
非零退出保留 stderr 和 exit_status 供 Agent 诊断。每个调用拥有独立进程组；取消、超时、
正常返回均清理组内剩余子进程，完成回收后释放 tracked worker。没有后台任务 API；主动脱离
进程组的进程不在进程组回收保证内，Full Access 本身是受信任执行档位。
Shell 不再声明 parallel_safe；文件修改和其他工具不会被同批并行执行。

`sandbox_permissions` 在请求不超过固定 Run 档位时直接允许；更高请求继续通过现有拒绝审批
路径持久化 ToolResult，不修改 Run。文件 read/write/edit 继续使用能力句柄、禁符号链接和
隐藏路径的保守边界；Full Access 不使这些结构化文件工具自动变成任意路径 API。

## 仓库浏览与结果展示

- `read` 支持目录列表和大文本文件的 offset/limit 窗口，目录/行 offset 为 1-based。
- `glob` 支持 path 和 basename/相对路径 glob；`grep` 支持文件/目录 path、include glob、
  content/count/files_with_matches；搜索结果 offset 为 0-based。
- count 统计匹配的文本行，返回每文件 count 和整个查询范围的 total_count；total_count
  不受结果分页影响。包括无尾换行的末行，语义不等同于 `wc -l` 的换行符统计。
- 扫描流式读取，最多 100,000 个目录项、64 层递归、单文件 16 MiB、单行 64 KiB，逐步检查取消。
  隐藏路径、target、node_modules 被排除；不跟随符号链接。二进制、无法读取或超过扫描上限
  的文件计入 skipped_files；count_complete=false 明确表示总数不完整。
- 输出分页/裁剪使用 truncated、next_offset，遍历上限使用 scan_truncated，不再丢弃全部
  搜索结果并仅返回通用 64 KiB 错误。较长匹配行带 text_truncated。
- Desktop 保留原生 Message/ToolResult 不变，只投影展示；API ToolResult 默认折叠成
  ToolUse result，显示状态及可展开的 output/error，不显示存储 envelope，也不标成 You。
  原生 assistant sub_messages 保持文本/ToolUse 顺序。运行状态使用实际 Agent 名称。

## 验证与平台限制

真实 macOS 测试覆盖三级写入边界、绝对路径、symlink 越界、受保护路径、网络、显式权限请求、
非零退出/stderr、截断、取消及组内子进程回收。仓库夹具覆盖超过 64 KiB 的文件、每目录计数、
分页、glob 和不完整扫描。Desktop 测试覆盖 DS-Flash 状态、API 工具消息和 HTML 转义。
Linux 挂载/namespace 行为需要具备 bubblewrap 和用户 namespace 的主机；启动失败不会降级。
Windows 当前没有安全 Bash 后端。本轮不调用付费模型，也不将离线测试等同于跨平台真机验收。

实现依据：[Seatbelt 基础策略](https://github.com/openai/codex/blob/main/codex-rs/sandboxing/src/seatbelt_base_policy.sbpl)、
[bubblewrap](https://github.com/containers/bubblewrap)，查阅日期 2026-09-13。
