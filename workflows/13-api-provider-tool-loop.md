# WF-13：远程 Provider 创建并验证项目文件

公共 `Session → RunCoordinator → API Provider → HostTool` 路径的默认离线验收：

```bash
cargo test -p ait-application --test api_tool_loop
cargo test -p ait-tools --test host
cargo test -p ait-runtime --test run_coordinator
cargo test -p ait-worker --test process_providers subprocess_api_providers_keep_tool_result_order_and_sqlite_receipts -- --exact
cargo test -p ait-worker --test process_providers permission_change::changed_permission_reaches_worker_and_repository_inspection -- --exact
```

HTTP fixture 使用实际 Rig OpenAI Responses、DeepSeek/MiniMax Chat Completions 与 Gemini GenerateContent adapter。
首轮提出 `write(hello.py)`，第二轮提出 `read` 与 `grep`，第三轮收到原 call id 的结果后给出最终答复。
宿主持久化 ToolExecution intent 后执行；测试程序不代写文件。读取与搜索可以并发，结果仍按提案顺序追加。

独立验证文件内容、Python 执行结果、Git `?? hello.py`、Run/Message 身份、parent/run_seq、
Agent revision、单一 attempt、usage、唯一 ToolResult，以及重新打开 SQLite 后的查询结果。
故障回归另外验证真实文件读取重叠、发布前取消/deadline 等待 worker 清理、取消后崩溃快照
恢复及 store error/task panic 的原子终态结算。取消请求可先显示 cancelling；只有工作线程
退出后才确认 cancelled 并释放 Session，慢磁盘上的已开始系统调用可能延迟这一确认。
API 文件变化保留在 Session worktree 供成员审阅，没有自动 Git 提交；确认后自行提交，再发送下一条需要干净 Git 基线的输入。

新建和重置设置默认 `workspace_write`，允许工作区内创建/编辑。显式选择 `read_only` 时，读取/搜索和只读 Shell 按原档位执行；write/edit 及显式更高权限请求须获得单次人工批准。管理员上限仍是硬边界。详见 [API 工具审批](../docs/operations/api-tool-approvals.md)。
结构化文件工具的范围外路径、隐藏目录/文件、符号链接，以及未知工具、无效参数或审批升级均在副作用前拒绝。
`full_access` 仍受管理员上限约束；结构化文件能力仍限制在 Session worktree 内。

NEC-263 已将 `bash` 的 echo/printf/sleep 白名单替换为真实 Shell。macOS 使用 Seatbelt、
Linux 使用 bubblewrap 限制 Readonly/Workspace Write 的文件和网络访问；启动探测失败时不广告 Bash。
Readonly 可读取 Session，Workspace Write 增加 Session 内写入，保护 `.git`、`.ait`；
Full Access 使用无 OS 沙箱的 Bash。命令受超时、输出和进程组清理约束，没有后台任务 API。
请求 `sandbox_permissions=workspace-write` 在 Run 已是 Workspace Write 时直接允许，超过固定 Run 档位的请求仍拒绝。
完整范围和平台限制见 [NEC-263 ADR](../docs/decisions/NEC-263/adr-001-shell-and-prompt-permissions.md)。
Windows 当前只提供文件与搜索切片，不广告尚未实现的 PowerShell 执行器。

## 修改权限后仍有工具失败

先检查新 Run 的 `permission_profile.sandbox`。设置只影响随后创建的 Run，历史及活动 Run 保留原快照。
NEC-272 的现场记录已是 `workspace_write`，但已安装 worker 仍拒绝同档位 Shell 请求，
大范围 `grep` 返回 `TOOL_EXECUTION_FAILED`；少量匹配的搜索成功。这是旧工具执行器的限制，
提高权限不能消除搜索输出上限错误。NEC-263 的执行器已允许同档位请求，并以
`truncated`、`next_offset` 返回搜索分页，避免因匹配较多丢弃全部结果。

使用包含 NEC-263 的构建，按[发布指南](../docs/operations/releasing.md)同时构建、暂存并打包
`ait-daemon` 和 `ait-worker`，退出旧应用后再打开更新后的版本。只重编前端、daemon 或修改设置
不会更新旧 worker。`TOOL_APPROVAL_REQUIRED` 表示能力/权限被拒绝；`TOOL_EXECUTION_FAILED`
还可能来自无效参数、文件读取或工具资源上限，应结合对应 ToolUse 参数判断。

如需核验某个已构建的 worker，可运行以下离线回放。将路径替换为该可信 worker 的绝对路径；
它使用临时项目、独立 SQLite 和本地 HTTP 模型夹具，不连接正在运行的 daemon，不读取真实 Provider 凭据。
旧 worker 会在同档位 Shell、写入或大范围搜索断言处失败；当前主工具循环应通过 OpenAI、DeepSeek、Gemini、MiniMax 四组验证。

```bash
AIT_TEST_WORKER_EXECUTABLE=/absolute/path/to/ait-worker cargo test -p ait-worker --test process_providers permission_change::replay_permission_change_with_external_worker -- --ignored --exact
```

真实付费 DeepSeek 验证见 [WF-11](11-deepseek-python-hello-world.md)，只能显式运行脚本或 ignored test。
离线 fixture 默认执行，不读取本机凭据，不访问付费服务。Codex 的原生工具与审批流程保持独立。
