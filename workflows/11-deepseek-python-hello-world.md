# WF-11：使用 DeepSeek 默认 Agent 生成并验证单文件 Python Hello World

用户目标：在临时目录接入 `example-project`，从 `.env` 读取 DeepSeek API key，创建并选择
DeepSeek 默认 Agent，让它生成 `hello.py`，随后独立运行程序并校验代码逻辑。

本流程使用原生 `kind: "deepseek"` Provider，模型默认 `deepseek-v4-flash`，
请求发送到 `https://api.deepseek.com`。模型和地址见
[DeepSeek 官方文档](https://api-docs.deepseek.com/)。
配置与执行遵循 [ADR-009](../docs/decisions/adr-009-session-exclusion-and-agent-providers.md)。
公共路径使用 NEC-247 的宿主工具循环。流程将新 Run 权限设置为 `workspace_write`，
让模型通过 `write` 创建文件、通过 `read` 核对，再独立校验磁盘文件；测试不代写源码。
默认离线覆盖见 [WF-13](13-api-provider-tool-loop.md)。

## 前置条件与运行入口

- macOS/Linux、Rust stable、Git、Python 3，以及 daemon 可使用的操作系统凭据库。
- 本流程不要求 Codex 登录；Session 会提前命名，跳过内置 Codex 标题生成。
- 可访问 DeepSeek API，账号有可用额度。测试会真实调用模型，因此默认 `ignored`，普通 CI 不消耗额度。
- `.env` 中有一条非空 `DEEPSEEK_API_KEY=...`。可复制根目录的 [`.env.example`](../.env.example)
  到 `.env` 后在本机编辑；现有 `.env` 可直接用路径参数指定，不需要复制。
- key 必须是由字母、数字、`-`、`_` 组成的字面值；允许外围单/双引号、可选 `export`、
  CRLF 和独立注释行。不支持变量展开、命令替换、多行值或行尾注释；重复 key 会报错。
  其他变量不会导入进程。加载器不执行 `.env`，解析失败也不显示文件内容。

在仓库根目录执行：

```bash
./test_with_deepseek.sh
# 或指定已有 .env（路径按调用脚本时的当前目录解析）
./test_with_deepseek.sh '/path/to/.env'
# 可选：使用账号支持的其他 DeepSeek 模型
AIT_DEEPSEEK_MODEL=deepseek-v4-pro ./test_with_deepseek.sh '/path/to/.env'
```

脚本先构建 `ait-cli` 与 `ait-daemon`，再精确运行
[`wf11_real_deepseek_python_hello_world`](../bins/cli/tests/deepseek_workflow.rs)。
直接用 Cargo 运行时，可将 `AIT_DEEPSEEK_ENV_FILE` 设置为 `.env` 的绝对路径；
缺省路径始终是仓库根目录的 `.env`。`AIT_WORKFLOW_DAEMON_BIN` 可覆盖 daemon 路径，
否则查找当前 Cargo 构建的 CLI 同级目录中的 daemon。

普通、不调用模型的验证：

```bash
cargo test -p ait-cli --test deepseek_workflow
```

这会测试凭据解析和 Python 逻辑校验器，并明确显示真实调用测试被跳过。
另有 `cargo test -p ait-cli --test workflows wf11_` 验证 CLI 标准输入和错误诊断不泄露凭据。
这些测试通过不代表 DeepSeek 端到端验收通过。

## 五步用户流程

### 1. 创建临时 example-project

测试创建全新的 `ait-wf11-*` 临时目录，在随机 loopback 端口启动真实 daemon，
确认数据库为空，再创建 `example-project` 子目录并执行：

```bash
ait project register --id example-project --name example-project --workdir "$WF_ROOT/example-project"
```

以下示例中的 `ait` 是连接本次 daemon 的 CLI 包装函数（见公共演练准备）；测试自动填入运行时路径和 ID。
注册按现有契约初始化 Git 并创建空初始提交。
测试只在示例仓库设置 Git 身份。数据库、日志和响应文件均位于项目之外。

### 2. 从 .env 输入凭据并创建 DeepSeek Agent

Rust 测试从 `.env` 读取 key，仅通过子进程 stdin 传给 `--secret-stdin`。
普通 Provider 字段使用 flags，模型目录使用无凭据 JSON 文件：

```bash
cat > "$WF_ROOT/models.json" <<'JSON'
[{"id":"deepseek-v4-flash","name":"deepseek-v4-flash","reasoning_efforts":[]}]
JSON
# 从安全来源重定向原始 key（文件内容仅为 key，可有末尾换行；不是整个 .env）
ait agent-provider save --id deepseek --name DeepSeek --kind deepseek \
  --url https://api.deepseek.com --input "$WF_ROOT/models.json" \
  --secret-stdin < /path/to/protected-key-file
```

现有 `.env` 应通过 `test_with_deepseek.sh` 的 Rust 加载器读取；不要 source 它，也不要把 key
粘贴到命令行或使用 echo 字面量。`--secret-stdin` 不接受 key 参数，直接连终端会被拒绝以避免回显。
`--input -` 可从 stdin 读取模型列表，但不能与 `--secret-stdin` 共用 stdin；此时请使用模型文件。
`agent-provider discover-models` 接受同样的 flags，可以在保存前预览模型；
`agent-provider refresh-models --provider-id deepseek` 使用已保存连接刷新目录。

daemon 将 Secret 写入操作系统凭据库；SQLite 仅保留引用，响应只显示 `has_secret=true`。
本次目录、数据库和对应凭据项会保留以便复核、重开该工作空间；删除临时目录不会自动删除
系统凭据库中的条目（服务名 `ait.agent-provider`）。本流程不覆盖凭据回收。
随后创建可复用的命名 Agent：

```bash
ait agent create --id deepseek --name DeepSeek --provider-id deepseek --model deepseek-v4-flash
```

API key 不进入 Agent JSON、命令行参数、prompt 或项目文件；Provider 保存请求通过管道传输，
不写入响应文件。实体 JSON 解析错误仅报告行列位置；专用 secret stdin 的内容不会进入参数诊断或响应。
本流程使用独立 daemon 和全新数据库，不修改已有 Provider 或 Agent。

### 3. 设置并使用项目默认 Agent

```bash
ait project set-default-agent --project-id example-project --agent-id deepseek
```

确认响应 `default_agent_id=deepseek`，用该响应中的值创建 Session：

```bash
ait session create --id hello-world --project-id example-project --agent-id deepseek
ait session rename --session-id hello-world --name 'DeepSeek Hello World'
```

当前 `create_session` 必须显式传 `agent_id`，因此测试读取 Project 默认值后填入，
不声称已经支持省略字段时自动继承。提前命名 Session 会按现有契约跳过 AI 标题生成，
避免该辅助功能使用内置 OpenAI 标题模型。

### 4. 让 DeepSeek 生成一个 Python 文件

真实工作流测试先在 Project 目录外创建 `prompt with spaces.txt`，写入多行 UTF-8 指令，
随后实际执行 `ait session send --session-id hello-world --text-file "$WF_ROOT/prompt with spaces.txt"`。
手工执行时也应先准备该文件；或者选择 `--text-stdin`。
发送前读取 `ait settings get`，将完整 values 文档的 `permissions.sandbox` 改为
`workspace_write`，用 `ait settings set --expected-revision <revision> --input <file>` 保存。
指令要求工具创建并读取 `hello.py`：无参数 `main()` 只打印字面值 `Hello, world!`，
仅在 `if __name__ == "__main__"` 中调用；无依赖、导入或其他行为。
模型仅返回源码文本不能通过验收；必须有成功的 write/read ToolExecution 和真实文件。
不要求额外 Git 提交，改动留待审阅。

测试同步等待 `session send`，最多 600 秒。必须同时满足 `ok=true`、
`status=completed`、`error=null`；queued、failed、超时或只有文字回复均不算通过。
失败的 Run 不得生成成功报告，不回退到模拟模式、Codex 或其他模型。

### 5. 调用程序并校验代码逻辑

验收检查：

- Project 默认 Agent 和 Session Agent 都为 `deepseek`，Run 固定同一 Agent revision/config，
  `run.provider.kind=deepseek`。
- assistant 输出非空；Session 指向 Run 最终 Message，`active_run_id=null`。
- 项目除 `.git` 外恰好只有普通文件 `hello.py`，且存在成功的 write/read ToolExecution；由独立逻辑校验器核对内容。
- 历史仍只有初始空提交，HEAD 等于 Project 的 `base_commit`；
  user Message 的 `git_commit` 也等于该基线。
- [逻辑校验器](../bins/cli/tests/fixtures/verify_hello.py) 先用 AST 检查约定结构，
  排除导入、额外调用、错误输出和错误 main guard，再验证导入无输出、连续两次 `main()`
  各输出一行且返回 `None`。允许注释、docstring 和 `-> None` 注解。
- 最后独立执行 `python3 -I -B hello.py`，要求退出码 0、stdout 严格为
  `Hello, world!\n`、stderr 为空；验证后 Git 状态仅为 `?? hello.py`，没有额外文件。

只有所有断言通过才生成 `verification.json`。该报告包含 provider、model、Run ID、
base_commit、文件列表、源码来源、stdout 与逻辑验证结果，不包含 key。

## 结果与失败恢复

测试打印本次临时目录，成功或失败都保留 daemon 日志、各步响应 JSON、数据库和示例项目，
用于人工复核。启动限时 10 秒，普通 CLI/Git/Python 命令限时 20 秒，生成限时 600 秒。
退出时回收本次 daemon 及其进程组，失败或超时同样执行清理；不会停止其他 daemon。

| 现象 | 处理 |
| --- | --- |
| `.env` 不存在、key 为空或格式错误 | 修复本机文件后重新执行；测试失败，不静默跳过或回退到其他模型 |
| Python/daemon 不存在 | 安装缺失程序；daemon 使用脚本构建；检查 PATH 或 binary 覆盖值 |
| 系统凭据库不可用或锁定 | 解锁并配置本机凭据服务，再重新执行；不回退成明文保存 |
| DeepSeek 认证、额度、模型或协议失败 | 检查 `run.json` 和本机服务配置；只有 Run completed 才继续程序验收 |
| 生成额外文件、逻辑不符或输出不符 | 验收失败，保留原始产物；不要手工改成 Hello World 后标记模型成功 |
| 需要重试 | 重新运行脚本，使用新的临时目录；不要复用失败的 Run 或数据库 |

本流程覆盖真实 DeepSeek 经宿主工具循环创建/读取文件及独立验收，不覆盖任意 shell、
模型发现、网络重试、执行中恢复或凭据回收。没有真实 `verification.json` 时，应记录“未完成真实验收”。
