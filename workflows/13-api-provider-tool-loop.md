# WF-13：远程 Provider 创建并验证项目文件

公共 `Session → RunCoordinator → API Provider → HostTool` 路径的默认离线验收：

```bash
cargo test -p ait-application --test api_tool_loop
cargo test -p ait-tools --test host
cargo test -p ait-runtime --test run_coordinator
```

HTTP fixture 使用实际 Rig OpenAI Responses / DeepSeek Chat Completions adapter。
首轮提出 `write(hello.py)`，第二轮提出 `read` 与 `grep`，第三轮收到原 call id 的结果后给出最终答复。
宿主持久化 ToolExecution intent 后执行；测试程序不代写文件。读取与搜索可以并发，结果仍按提案顺序追加。

独立验证文件内容、Python 执行结果、Git `?? hello.py`、Run/Message 身份、parent/run_seq、
Agent revision、单一 attempt、usage、唯一 ToolResult，以及重新打开 SQLite 后的查询结果。
故障回归另外验证真实文件读取重叠、发布前取消/deadline 等待 worker 清理、取消后崩溃快照
恢复及 store error/task panic 的原子终态结算。取消请求可先显示 cancelling；只有工作线程
退出后才确认 cancelled 并释放 Session，慢磁盘上的已开始系统调用可能延迟这一确认。
API 文件变化保留在 Project 工作区供成员审阅，没有自动 Git 提交；确认后自行提交，再发送下一条需要干净 Git 基线的输入。

默认 `read_only` 只广告读取、搜索与受控命令。创建/编辑需先通过设置选择 `workspace_write`。
范围外路径、隐藏目录/文件、符号链接、未知工具、无效参数或审批升级均在副作用前拒绝。
`full_access` 仍受管理员上限约束；首版文件能力仍限制在 Project 内。

首版 `bash` 只接受 `echo`、`printf`、`sleep`，使用清空环境的固定可执行文件，不解释 shell 表达式。
无网络、任意程序、后台任务或权限升级。未实现的目录工具不发送给模型。
Windows 当前只提供文件与搜索切片，不广告尚未实现的 PowerShell 执行器。

真实付费 DeepSeek 验证见 [WF-11](11-deepseek-python-hello-world.md)，只能显式运行脚本或 ignored test。
离线 fixture 默认执行，不读取本机凭据，不访问付费服务。Codex 的原生工具与审批流程保持独立。
