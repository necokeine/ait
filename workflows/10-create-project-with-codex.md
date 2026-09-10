# WF-10：从空目录创建项目，让 Codex 生成并提交 Rust Hello World

用户目标：创建一个独立的 AIT 工作空间，在其中接入 `example-project`，通过 `ait-cli`
调用真实 Codex 创建可运行的 Rust Hello World，最终得到一个可追溯的 Git 提交。
本流程独立执行，不依赖 WF-01，也不使用已有 daemon 或数据库。

## 前置条件

- macOS 或 Linux，已安装 Rust stable、Cargo、Git 和支持 `app-server` 的 Codex CLI。
- 当前环境的 Codex 已配置可用凭据并能访问执行模型；默认沿用 AIT 的 `gpt-5.6-sol`，
  可用 `AIT_WORKFLOW_MODEL` 显式指定本机可用模型。首次 Session 元数据生成还使用
  AIT 内置的 `gpt-5.6-luna`；元数据生成失败不代表代码执行成功或失败，应检查 Run。
- 自动化测试不模拟 Codex，会使用当前 Codex 配置、联网调用模型并消耗额度。
  因此它标记为 `ignored`，普通 `cargo test --workspace` 只编译它，不执行真实调用。
- 下列手工命令需要 Bash/Zsh 和 jq。Rust 测试本身不依赖 jq。

## 一条测试命令复现

在仓库根目录执行：

```bash
./test_with_codex.sh
```

根目录的 [`test_with_codex.sh`](../test_with_codex.sh) 会先构建 `ait-cli` 和 `ait-daemon`，
再显式运行当前 WF-10 测试并显示输出；构建或测试失败时返回非零退出码。
也可以从其他目录通过脚本路径调用，它会自动切换到所在仓库。

测试：[`wf10_create_project_with_real_codex_and_commit`](../bins/cli/tests/project_creation.rs)。
测试使用本次 Cargo 构建的 CLI，默认从其同级目录查找 `ait-daemon`；使用不同 target/profile
时先构建对应 daemon，或通过 `AIT_WORKFLOW_DAEMON_BIN` 指定它的路径。

测试先确认新临时目录为空，再以它为 cwd 启动真实 daemon，使用 `127.0.0.1:0`
让操作系统分配端口，从启动日志取得实际地址并通过 CLI 检查各实体列表为空。
随后依次执行下面五步。启动最多等 10 秒，普通命令 20 秒，真实 Codex 请求 600 秒，
最后独立 Cargo 验证 120 秒。成功、断言失败或超时时都会停止并回收本次 daemon，
同时终止其独立进程组中的 Codex 子进程。

终端打印本次临时目录；成功和失败都保留它，便于检查 `daemon.log`、各步 JSON 响应、
`example-project` 仓库和数据库。只有全部断言通过才生成 `verification.json`。
验收后可自行删除该次目录；不要把数据库、日志或凭据加入 AIT 源码仓库。

## 手工操作

### 1. 构建空临时目录

在终端 A 的 AIT 仓库根目录执行：

```bash
cargo build -p ait-cli -p ait-daemon
export AIT_REPO="$PWD"
export WF_ROOT="$(mktemp -d)"
export AIT_ENDPOINT="http://127.0.0.1:17315"
set -o pipefail
ait() { "$AIT_REPO/target/debug/ait-cli" --endpoint "$AIT_ENDPOINT" "$@"; }
printf '演练目录：%s\n' "$WF_ROOT"
```

### 2. 在临时目录下启动 daemon

终端 B 将两个路径替换为终端 A 的实际值：

```bash
cd '演练目录'
'/AIT仓库绝对路径/target/debug/ait-daemon' --database ait.sqlite3 --listen 127.0.0.1:17315
```

终端 A 依次执行下列命令，确认返回数组均为空。若端口占用，修改本次 `--listen` 和
`AIT_ENDPOINT`，重新启动。

```bash
ait project list
ait agent list
ait session list --project-id example-project
```

### 3. 创建 example-project 和第一个空提交

终端 A 执行：

```bash
mkdir "$WF_ROOT/example-project"
git -C "$WF_ROOT/example-project" init
git -C "$WF_ROOT/example-project" config user.name 'AIT Workflow'
git -C "$WF_ROOT/example-project" config user.email 'workflow@localhost'
git -C "$WF_ROOT/example-project" config commit.gpgsign false
git -C "$WF_ROOT/example-project" commit --allow-empty --no-gpg-sign -m 'Initial empty commit'
export INITIAL_COMMIT="$(git -C "$WF_ROOT/example-project" rev-parse HEAD)"
git -C "$WF_ROOT/example-project" ls-tree --name-only HEAD
```

最后一条命令无输出，提交历史只有一个空提交。Git 身份只配置在该示例仓库中。

### 4. 使用 ait-cli 接入项目

```bash
ait command "$(jq -nc --arg workdir "$WF_ROOT/example-project" \
  '{type:"register_project",id:"example-project",name:"example-project",workdir:$workdir}')" \
  | tee "$WF_ROOT/project.json"
jq -e --arg initial "$INITIAL_COMMIT" \
  '.ok == true and .result.value.base_commit == $initial' "$WF_ROOT/project.json"
```

这里的“导入”是注册已有 Git 目录，使用 `register_project`。
`ait-cli import` 专用于导入 AIT Project 归档，不适用于这个空 Git 仓库。
注册应保留已有 HEAD，不再增加初始提交；响应包含规范化工作目录和根 Message ID。

### 5. 通过 ait-cli 调用 Codex，生成 Rust 程序并提交

```bash
ait command "$(jq -nc --arg model "${AIT_WORKFLOW_MODEL:-gpt-5.6-sol}" \
  '{type:"register_agent",id:"codex",name:"Codex",config:{provider_id:"builtin-codex",model:$model,reasoning_effort:"high"}}')"
ait command '{"type":"create_session","id":"hello-world","project_id":"example-project","agent_id":"codex"}' \
  | tee "$WF_ROOT/session.json"
ait command "$(jq -nc \
  '{type:"send_message",session_id:"hello-world",
    text:"Create a minimal Rust binary package named example-project at the repository root, with Cargo.toml, Cargo.lock, src/main.rs and .gitignore ignoring /target/. Use no external dependencies. cargo run --offline --quiet must print exactly Hello, world! followed by a newline. Verify it. Do not create a Git commit; AIT will commit your changes."}')" \
  | tee "$WF_ROOT/run.json"
jq -e '.ok == true and .result.value.status == "completed" and .result.value.error == null' "$WF_ROOT/run.json"
ait session list --project-id example-project | tee "$WF_ROOT/final-sessions.json"
ait message list --project-id example-project | tee "$WF_ROOT/final-messages.json"
ait run list --project-id example-project | tee "$WF_ROOT/final-runs.json"
git -C "$WF_ROOT/example-project" log --oneline
git -C "$WF_ROOT/example-project" status --porcelain=v1
(
  cd "$WF_ROOT/example-project"
  cargo run --offline --locked --quiet
)
```

`send_message` 当前同步等待执行结果。`ok=true` 表示命令成功返回，还必须检查 Run 的
`status=completed` 和 `error=null`。真实生成后的提交由 AIT 宿主执行，遵循
[Codex 执行与 Git 提交 ADR](../docs/decisions/NEC-174/adr-001-codex-session-execution.md)。
AIT 通过 [Codex app-server](https://developers.openai.com/codex/app-server) 的 stdio 协议执行，
使用项目目录和 `workspace-write`，沿用现有 adapter 的审批策略。

## 验收

- 初始实体列表为空；daemon 数据库和日志位于示例项目之外。
- Project 的 `base_commit` 等于手动创建的空提交；注册不移动 Git HEAD。
- Codex 的 Run 完成，包含非空 assistant 输出；Session 指向最终 Message 且 `active_run_id=null`。
- user Message 的 `git_commit` 等于初始提交；assistant Message 的
  `data.codex.commit_id` 等于生成后的 Git HEAD。
- 历史恰好两个提交；第二个以初始提交为父，subject 以 `ait: ` 开头，包含
  `Cargo.toml`、`Cargo.lock`、`src/main.rs`、`.gitignore`，没有提交 `target` 构建产物。
- 独立执行 `cargo run --offline --locked --quiet` 的 stdout 严格为 `Hello, world!\n`。
  验证前后工作树均干净，不能只依据模型的文字回复判定成功。

手工演练结束后，在终端 B 按 Ctrl-C 停止本次 daemon。

## 失败恢复与边界

- 找不到 daemon：先执行构建命令，并确认 profile/target 或显式 binary 路径一致。
- 找不到 Codex、未登录、模型不可用或配额不足：检查 `run.json` 中的错误和当前 Codex 环境；
  修复后重新执行测试，新运行会使用新的临时目录。不要把失败的 Run 当作通过。
- `PROJECT_GIT_DIRTY`：检查示例项目内的未跟踪文件；响应、数据库、日志应保存在其父目录。
  不在已有真实项目中清理改动来满足这个演练。
- Codex 失败、超时或生成内容不满足验收：保留目录、响应和 Git 状态用于诊断；
  测试应失败，不以模拟模式替代后宣称真实执行通过。
- 本流程验证成功路径，不覆盖网络重试、执行中重启或完整崩溃恢复。
