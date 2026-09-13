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
| macOS Readonly | Seatbelt 默认拒绝；仅允许读 Session 和明确列出的系统运行目录；子进程继承策略；禁止写文件和联网 |
| macOS Workspace Write | 同上，仅增加 Session workdir 写权限，保护 `.git`、`.ait` |
| Linux 受限档位 | x86_64/aarch64 需要系统 `bwrap`，独立 PID/network/IPC namespace 与 seccomp socket/io_uring 限制，从空根文件系统仅挂载系统运行目录和 Session；Workspace Write 使用工作区可写挂载并将根下已有 `.git`、`.ait` 挂回只读 |
| Unix Full Access | 直接运行 Bash，不施加 OS 文件/网络沙箱，仍受 Run/管理员权限上限和资源限制 |
| 所需后端不存在、实际启动探测失败、Windows | 不广告 Bash，不降级成未隔离命令；read/grep/glob 仍可用于仓库浏览和统计 |

受限档位沿用 ADR-001 v4 与 NEC-192 的 Project 读取边界，当前执行范围为 Session
workdir，不授予 Project 主检出、其他 Session、其他 Project、用户目录或宿主临时目录的读取权。
macOS 只允许 Session、`/bin`、`/sbin`、`/usr/bin`、`/usr/sbin`、`/usr/lib`、
`/System/Library` 的读取，以及根 vnode（非递归）和必要的 null/zero/random/fd 设备。
Linux 从空根开始，只读挂载 bin/sbin/lib/lib64（含 `/usr` 下对应目录）与
`/etc/ld.so.cache`，再挂载 Session、私有 proc 和最小 dev；不挂载整个 `/`、`/etc`、
`/home`、`/root`、`/run` 或宿主 `/tmp`。系统目录保持只读，受限 PATH 固定为系统
bin/sbin，不继承用户 PATH；`/usr/local`、Homebrew 和用户安装目录不在读取授权内。
符号链接不扩大读取范围；Linux 不为 `.git`/`.ait` 符号链接创建额外宿主挂载。
只有显式 Full Access 可读工作区外的私有文件。

创建每个 Run 工具集时，受限后端使用与真实执行相同的策略、挂载、namespace 和 seccomp
运行 `/bin/bash --noprofile --norc -c 'exit 0'`。每个候选最多等待 3 秒，失败或超时
回收子进程并不广告 Bash；结果仅缓存在该工具实例内，新 Run 重新探测。二进制存在但不可
执行、user namespace/AppArmor 拒绝等均视为不可用，运行中策略变化导致失败也不降级。

Linux 受限进程的 seccomp 策略禁止创建 socket、socketpair 和 io_uring，并拒绝兼容 ABI，
避免通过宿主路径 Unix socket 绕过 network namespace。策略经匿名文件传给 bubblewrap，
不继承宿主 socket 或 ring 句柄。

受限 Shell 不通过字符串解析判断读写性；系统策略作用于整个命令与子进程。默认 cwd 为
Session workdir，显式 workdir 规范化后检查范围。环境仅设置系统 PATH（Full Access 沿用宿主 PATH）、工作区 HOME 和 C locale，
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
  denied 与 failed/declined 使用危险色，cancelled 使用中性色，均保留可读状态文字。

## 验证与平台限制

真实沙箱测试覆盖三级读写边界、用户目录/其他 Session/临时目录读取、绝对路径、symlink
越界、受保护路径、网络、显式权限请求、
非零退出/stderr、截断、取消及组内子进程回收。仓库夹具覆盖超过 64 KiB 的文件、每目录计数、
分页、glob 和不完整扫描。Desktop 测试覆盖 DS-Flash 状态、API 工具消息和 HTML 转义。
OpenAI/DeepSeek 离线 HTTP fixture 对三档权限核验：外部测试 marker 在两档受限
Shell 的 stdout、持久化 ToolResult 和下一轮 Provider 请求中均不存在，Full Access 对照组可读。
后端探测回归覆盖存在但不可执行、非零启动失败和超时，均不广告 Bash。Linux CI 设置
`AIT_REQUIRE_SHELL_SANDBOX=1`，真实后端不可启动将导致验收失败，不能静默跳过隔离测试。
本轮本机为 macOS；Linux 挂载/namespace 真机验证由 CI 执行。
Windows 当前没有安全 Bash 后端。本轮不调用付费模型，也不将离线测试等同于跨平台真机验收。

实现依据：[Seatbelt 基础策略](https://github.com/openai/codex/blob/main/codex-rs/sandboxing/src/seatbelt_base_policy.sbpl)、
[bubblewrap](https://github.com/containers/bubblewrap)、[Linux seccomp](https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html)，查阅日期 2026-09-13。
