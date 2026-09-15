# API Provider 工具权限审批

OpenAI、DeepSeek 等普通 API Provider 使用 AIT HostTools。Settings 的 Sandbox 是新 Run 的
权限基线；修改它不改变已有 Run。Approval mode 请选择 **on_request**：已在基线内的操作
直接执行，需要更高权限且可验证的操作显示审批卡。API Run 不支持 untrusted_only 或 always，
选择这些值会在新 Run 开始前报告配置错误。Codex 继续使用自己的原生审批规则。

## 使用

1. 在 Session 对话查看审批卡；定时或无 Session 的 Run 从 **Runs → View Run · Approvals** 打开。
2. 审阅 Provider/Agent、工具、文件目标或完整命令、工作目录、理由、权限范围和有效期。
3. **Allow once** 仅批准这一调用，**Deny** 保存拒绝结果并让 Provider 继续，**Cancel Run** 取消整次运行。
4. 同一 Run 的下一次升级仍需新的决定。全局设置、其他 Session 和 Run 原权限都不会改变。

Shell 的 Workspace Write 授权仍有 OS 沙箱；Full Access 表示该条命令和子进程可访问宿主文件
与网络，应按完整命令的范围审阅。结构化 write/edit 始终限制在工作目录内，不接受符号链接、
父路径和隐藏路径。管理员上限之外、缺少安全执行后端或无法确定授权目标的请求直接拒绝。

审批最多等待两分钟，并受 Run/worker 剩余总时间限制。卡片显示的期限后不能批准。关闭或刷新
页面不会默认批准；重连后重新读取持久化状态。worker/daemon 丢失使旧授权失效；未知执行结果
不自动重放。`consumed` 表示授权已在执行前消费；操作是否成功请查看 ToolResult 和最终回复。

## 安全、离线、可重复的演示

在 macOS 开发环境，仓库根目录执行：

```sh
cargo build -p ait-daemon -p ait-worker
npm ci --prefix apps/desktop
npm run test:gui:approvals --prefix apps/desktop
```

夹具启动真实 Electron、它拥有的 daemon、真实 worker，以及临时 SQLite/空 Git 项目。
离线 OpenAI/DeepSeek HTTP 夹具请求写入 `approved.txt`；测试真实点击 Allow once / Deny，
覆盖 Session 和无 Session Cron、刷新恢复、最终回复、唯一 ToolResult 与文件副作用。
另外杀死精确的测试 daemon PID 后重新打开临时数据库，验证旧请求过期且不重放。
截图输出到 `target/approval-gui`；无付费模型或真实敏感文件。

GUI 夹具在 macOS Keychain 中短暂保存随机引用的合成凭据，结束时只删除该临时 catalog 中的
测试引用，并清理临时目录/自身进程。Linux GUI 需另行配置 secret service；该夹具会明确标为
未执行，不能当作跨平台 GUI 验收。普通 Rust 进程夹具使用内存测试凭据。

```sh
AIT_REQUIRE_SHELL_SANDBOX=1 cargo test -p ait-tools --test grants --test shell_permissions
cargo test -p ait-worker --test process_providers tool_approval
```

Shell grant 测试必须启动真实后端，不能通过跳过后端检查而宣称成功。macOS 使用 Seatbelt；
Linux 需要可启动的 bwrap/user namespace/seccomp。Windows 当前拒绝单次升级。

开发版可通过 `AIT_DESKTOP_DEV_PORT=空闲端口` 隔离 daemon，范围 1024–65535；绑定固定为
127.0.0.1，仍由 Desktop 创建并拥有 daemon，不复用未验证进程。打包版忽略此开发覆盖。
