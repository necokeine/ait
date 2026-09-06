# WF-11：使用 DeepSeek 默认 Agent 生成并验证单文件 Python Hello World

用户目标：在临时目录接入 `example-project`，从 `.env` 读取 DeepSeek API key，创建并选择
DeepSeek 默认 Agent，让它生成 `hello.py`，随后独立运行程序并校验代码逻辑。

本流程使用 **DeepSeek 模型 + Codex 执行器**。AIT 当前的 `mode: "codex"` 指执行器，
模型配置为 `deepseek-v4-flash`，请求发送到 `https://api.deepseek.com`。
DeepSeek 官方支持通过 Responses API 接入 Codex，见
[DeepSeek 接入说明](https://api-docs.deepseek.com/quick_start/agent_integrations/codex/)。
本流程没有新增独立的 `deepseek` mode，也没有实现 AIT 的通用凭据管理界面。

## 前置条件与运行入口

- macOS/Linux、Rust stable、Git、Python 3，以及支持自定义 Responses provider 的 Codex CLI。
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
这些测试通过不代表 DeepSeek 端到端验收通过。

## 五步用户流程

### 1. 创建临时 example-project

测试创建全新的 `ait-wf11-*` 临时目录，在随机 loopback 端口启动真实 daemon，
确认数据库为空，再创建 `example-project` 子目录并执行：

```json
{"type":"register_project","id":"example-project","name":"example-project","workdir":"<本次临时目录>/example-project"}
```

所有 JSON 都通过真实 `ait-cli --endpoint <本次地址> command '<JSON>'` 发送；
尖括号代表运行时值，测试自动填入。注册按现有契约初始化 Git 并创建空初始提交。
测试只在示例仓库设置 Git 身份。数据库、日志和响应文件均位于项目之外。

### 2. 从 .env 输入凭据并创建 DeepSeek Agent

Rust 测试读取 key，仅给本次 daemon 的子进程环境设置 `DEEPSEEK_API_KEY`，
并为其 Codex 子进程准备独立配置目录。配置中只有环境变量引用：

```toml
model = "deepseek-v4-flash"
model_provider = "deepseek"

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com"
wire_api = "responses"
env_key = "DEEPSEEK_API_KEY"
requires_openai_auth = false
```

完整配置由测试生成，包括请求超时、关闭重试，以及工具子进程的环境变量白名单。
配置目录仅通过该子进程的 `CODEX_HOME` 指定；用户当前 Codex 配置和登录信息无需修改。
`env_key` 和配置目录机制见 [Codex 官方配置文档](https://developers.openai.com/codex/config-advanced)。
随后通过 AIT 注册：

```json
{"type":"register_agent","id":"deepseek","name":"DeepSeek","model":"deepseek-v4-flash","mode":"codex"}
```

API key 不进入 Agent JSON、命令行参数、prompt 或项目文件。
本次 daemon 专用于 DeepSeek；当前 provider 由执行器进程配置，尚不支持在同一 daemon 中
给多个 Agent 分别保存不同 endpoint/credential_ref。不要把这个测试配置用于已有生产 daemon。

### 3. 设置并使用项目默认 Agent

```json
{"type":"set_project_default_agent","project_id":"example-project","agent_id":"deepseek"}
```

确认响应 `default_agent_id=deepseek`，用该响应中的值创建 Session：

```json
{"type":"create_session","id":"hello-world","project_id":"example-project","agent_id":"deepseek"}
{"type":"rename_session","session_id":"hello-world","name":"DeepSeek Hello World"}
```

当前 `create_session` 必须显式传 `agent_id`，因此测试读取 Project 默认值后填入，
不声称已经支持省略字段时自动继承。提前命名 Session 会按现有契约跳过 AI 标题生成，
避免该辅助功能使用内置 OpenAI 标题模型。

### 4. 让 DeepSeek 生成一个 Python 文件

发送输入时，从最新 Session 响应读取 `version` 作为 `expected_version`。
指令要求只生成 `hello.py`：无参数 `main()` 只打印字面值 `Hello, world!`，
仅在 `if __name__ == "__main__"` 中调用；无依赖、导入或其他行为。
生成者可自行验证程序，但不创建提交，提交由 AIT 宿主负责。

测试同步等待 `send_message`，最多 600 秒。必须同时满足 `ok=true`、
`status=completed`、`error=null`；queued、failed、超时或只有文字回复均不算通过。
不得把模型返回的 Markdown 抽取成文件来补救失败，也不得由测试写入目标 `hello.py`。

### 5. 调用程序并校验代码逻辑

验收检查：

- Project 默认 Agent 和 Session Agent 都为 `deepseek`，Run 固定同一 Agent revision。
- assistant 输出非空；Session 指向 Run 最终 Message，`active_run_id=null`。
- 项目除 `.git` 外恰好只有普通文件 `hello.py`，Git 中也只有它。
- 历史恰好两个提交；新提交父节点是 Project 的 `base_commit`，
  assistant 的 `data.codex.commit_id` 等于当前 HEAD。
- [逻辑校验器](../bins/cli/tests/fixtures/verify_hello.py) 先用 AST 检查约定结构，
  排除导入、额外调用、错误输出和错误 main guard，再验证导入无输出、连续两次 `main()`
  各输出一行且返回 `None`。允许注释、docstring 和 `-> None` 注解。
- 最后独立执行 `python3 -I -B hello.py`，要求退出码 0、stdout 严格为
  `Hello, world!\n`、stderr 为空；验证前后 Git 工作树均干净。

只有所有断言通过才生成 `verification.json`。该报告包含 provider、model、Run ID、
commit、文件列表、stdout 与逻辑验证结果，不包含 key。

## 结果与失败恢复

测试打印本次临时目录，成功或失败都保留配置、daemon 日志、各步 JSON、数据库和示例项目，
用于人工复核。启动限时 10 秒，普通 CLI/Git/Python 命令限时 20 秒，生成限时 600 秒。
退出时回收本次 daemon 及其进程组，失败或超时同样执行清理；不会停止其他 daemon。

| 现象 | 处理 |
| --- | --- |
| `.env` 不存在、key 为空或格式错误 | 修复本机文件后重新执行；测试失败，不静默跳过或回退到其他模型 |
| Codex/Python/daemon 不存在 | 安装缺失程序；daemon 使用脚本构建；检查 PATH 或 binary 覆盖值 |
| DeepSeek 认证、额度、模型或协议失败 | 检查 `run.json` 和本机服务配置；只有 Run completed 才继续程序验收 |
| 生成额外文件、逻辑不符或输出不符 | 验收失败，保留原始产物；不要手工改成 Hello World 后标记模型成功 |
| 需要重试 | 重新运行脚本，使用新的临时目录；不要复用失败的 Run 或数据库 |

本流程覆盖一轮真实生成和独立验收，不覆盖通用 provider 配置 UI、网络重试、
执行中恢复或多 provider 混用。没有真实 `verification.json` 时，应记录“未完成真实验收”。
