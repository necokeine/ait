# Claude Code

在运行 Rust server 的机器上安装并登录 Claude Code：

```sh
claude --version
claude auth login
claude auth status
```

启动应用后，在新建 Agent 时选择 **Claude Code**。模型和推理等级来自本机 CLI。
默认使用 `default` 审批模式；工具授权和 Claude 的提问会出现在同一会话中。

如果桌面进程找不到命令，在启动 server 或 `npm run dev:paseo` 前设置：

```sh
export AIT_SERVER_CLAUDE_BIN=/absolute/path/to/claude
```

可用 `CLAUDE_CONFIG_DIR` 选择 Claude 自身的配置目录。认证信息继续由 Claude Code 保存；
无需在 Ait 配置 API key。默认加载 Claude 用户、项目和本地设置以及 CLAUDE.md、技能和 MCP。

通过 WebSocket 创建时使用 `config.provider: "claude"`，例如：

```json
{"config":{"provider":"claude","cwd":"/absolute/project","model":"sonnet","modeId":"default"}}
```

先通过 `workspace.open.request` 打开工作目录，再发送 `agent.create.request`。
后续发送、等待、取消、审批与历史查询沿用已有 Agent 接口。
可选 `modeId`：`default`、`plan`、`acceptEdits`、`auto`、`bypassPermissions`。
省略 `model` 和 `thinkingOptionId` 时使用 Claude 默认配置。

模型/模式切换在下一轮生效；服务重启或取消后继续使用同一原生 Session UUID。
原生模型别名按 CLI 的 `resolvedModel` 展示 fast mode 和关闭思考等能力。

运行中发送消息默认中断当前轮次后提交新输入；选择 `activeTurnBehavior: "steer"` 可向
当前轮次追加输入。`messageId` 用于幂等重试，必须在重试时保持内容和投递方式不变。
语音占用期间，新文本不会打断语音；已接收消息的重试仍返回原有结果。

支持图片、文本附件、MCP 配置、精确工具预授权、原生持久授权和提问。
令牌、费用与上下文用量来自 CLI，账户额度通过已有 OAuth 登录只读查询；API key 登录
或额度接口不可用时显示 unavailable，不估算订阅余量，也不自动刷新凭据。

`agent.rewind.request` 的 `mode` 可为 `conversation`、`files` 或 `both`。会话回退会创建
原生分支并保留原始历史；文件回退只恢复 Claude 原生检查点跟踪的文件。`both` 先恢复
文件再切换会话，后一步失败时文件恢复不会自动撤销。`/rewind [message-id]` 可恢复指定
检查点的文件，省略 ID 时选择最近的可用检查点。

子 Agent、后台任务和 Workflow 使用独立时间线，父轮次完成后仍可接收子任务输出。
子任务由原生父子关系确认身份，关闭或丢失进程后不会继续显示为运行中。

完整边界见 [ADR-052](../decisions/adr-052-native-provider-capabilities.md)，当前验证状态见
[能力补齐清单](../plans/provider-parity.md)和[验证报告](../reports/provider-parity.md)。
