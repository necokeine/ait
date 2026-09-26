# Codex / Claude Code provider 能力补齐

比较基准是 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`；范围是 `bins/server` 与
`crates/server-provider` 构成的独立 Rust server。此次不更改 ADR-001 v4 的 domain 边界。
此报告替代初版 Claude 接入报告中的能力缺口说明；初版测量仍保留为历史记录。

以下测试与覆盖率记录来自整合提交前的完整工作区快照，revision 和源码指纹见下文。
合并 PR #109 后的提交前复验另见[整合验证报告](local-workspace-consolidation.md)，
不得将本报告的历史数值当作该复验的测量结果。

## 能力矩阵

| 能力 | Codex | Claude Code |
| --- | --- | --- |
| 原生创建、恢复、列举、导入、关闭与历史 | 已实现，app-server | 已实现，stream-json / 原生 JSONL |
| 流式文本、推理、工具、任务列表和图片 | 已实现 | 已实现，含结构化最终结果恢复 |
| 模型、思考等级、fast、模式 | 原生目录；协商 plan / auto-review | 原生目录及 resolvedModel 别名；含 off / ultracode |
| MCP、系统提示、工具策略、provider options | 校验后映射原生配置 | 校验后映射原生配置 |
| 图片、附件、上下文、输出 schema | 已实现 | 已实现 |
| 运行中追加、中断、排队、幂等重试 | 原生 steer；明确拒绝可中断后重投 | SDK priority next；关闭取消的 query 后恢复 |
| 审批、原生持久授权、提问、MCP 表单 | 已实现；异步问题与计划审批可重启恢复 | 已实现；授权规则保留原生目标范围 |
| 用量、费用、上下文与账户额度 | 原生事件与账户接口 | 原生事件与只读 OAuth usage |
| 子 Agent、后台任务、独立历史和实时输出 | 原生父子关系及 spawn 事件 | 原生工具/任务身份、别名、嵌套子任务及 Workflow |
| 会话回退 | 原生 fork / rollback | SDK 格式分支；保留原始历史 |
| 文件回退及同时回退 | 参考实现不支持，不声明该能力 | 原生 rewind_files；文件成功后再创建会话分支 |
| 原生命令 | compact、goal、技能和自定义 prompt | CLI 命令、技能、检查点 rewind |
| 自主后续轮次、崩溃恢复、队列隔离 | 已实现 | 已实现 |

版本不支持的原生能力会明确拒绝。非阻塞原生问题通过独立的持久会话问题路径处理；
普通工具审批不会在原生进程结束后伪装为仍可响应。MCP 的嵌套 schema 和 URL 交互按现有
客户端支持范围明确 decline。未知交互、格式错误和超出资源上限的输入不会被静默接受。

## 验证

- `cargo fmt --all --check`、`git diff --check` 及严格 workspace / all-targets Clippy 通过。
- 最终串行工作区测试：**1696 通过、0 失败、8 忽略**。
- 独立执行的覆盖率测试：**1695 通过、0 失败、8 忽略**。
  覆盖率命令使用 `--tests`，不计普通工作区运行中的 doctest。
- provider：**370 通过、0 失败、3 忽略**；server：9 项单元测试、10 项架构检查、54 项进程测试通过。
- Codex 0.153.4 的真实 app-server 最小对话及原生历史重读通过。
- Claude Code 2.1.221 的真实控制协议握手和模型发现通过；在线推理未通过。
  直接运行原生 CLI 同样返回 `OAuth session expired and could not be refreshed`。
  需要更新本机 Claude 登录后重跑 opt-in 推理/历史检查；没有将其计为成功。
- 初次测试中，Homebrew 的 Codex 0.157.1 连 `--version` 都未正常返回；成功的在线验证
  使用同机可正常启动的 0.153.4 可执行文件。未更改用户安装或凭据。

早期并行执行普通/覆盖率工作区测试导致旧 daemon 用例争用全局项目锁。另一个新增测试误读
重启前的 registry 缓存，已修正测试。最终测试和覆盖率在相同源码上依次完整执行并全部通过；
早期失败不并入最终覆盖率，摘要与日志哈希保存在 JSON 中，便于审查。

可在已登录的机器上重跑在线检查：

```sh
AIT_SERVER_CLAUDE_BIN=/absolute/path/to/claude cargo test -p server-provider --offline installed_claude -- --ignored --test-threads=1
AIT_SERVER_CODEX_BIN=/absolute/path/to/codex cargo test -p server-provider --offline installed_codex -- --ignored --test-threads=1
```

## Test coverage

本次工作区行覆盖率 **85.24%（57,553/67,518）**。

| 测量范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| server-provider | 14,403/15,547 | 92.64% |
| server-provider 生产代码 | 14,371/15,515 | 92.63% |
| Claude adapter 生产代码 | 3,551/3,854 | 92.14% |
| Codex adapter 生产代码 | 3,443/3,679 | 93.59% |
| server-bin | 776/827 | 93.83% |

测量 revision：`5e9fc8a759c886fb78e3212ec681391ed97318e7` 加当前未提交工作区；Rust/Cargo/测试 fixture 的源码
指纹为 `ed2168a310393833bf9cb24f89d65bd3f13380f5f685240cf8aefa45291f6b21`，测量前后相同。范围是整个 Cargo workspace、默认 features、
macOS arm64；使用 cargo-llvm-cov 默认源码筛选，没有追加文件排除。生产代码分组排除测试及
测试辅助文件；完整逐文件统计与源码哈希见[可审查 JSON 产物](provider-parity-coverage.json)。

没有对本轮改动前的同一工作区进行独立测量，因此没有可直接比较的基线增量。
初版 Claude 报告的覆盖率包含不同代码，不能当作本轮测量或直接计算改善幅度。

```sh
CARGO_TARGET_DIR=/tmp/ait-provider-parity-check cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test --workspace --offline --no-fail-fast -- --test-threads=1
cargo llvm-cov --workspace --html --offline --no-fail-fast -- --test-threads=1
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-provider-parity-coverage-summary.json
cargo llvm-cov report --lcov --output-path /tmp/ait-provider-parity-coverage.lcov
```

本地 HTML 为 `target/llvm-cov/html/index.html`；共享可审查产物是上方 JSON，不只依赖本地路径。
所有忽略的测试名称及原因均列在 JSON 中，包括 3 项需要已安装/已认证 CLI 的 provider 测试，
它们的单独执行结果不混入离线工作区计数或覆盖率。

尚未实测 Linux/Windows、本机真实工具执行和真实 OAuth 额度服务；离线原生协议及 HTTP peers
覆盖相应参数、事件、失败和恢复语义。未覆盖区域主要包括平台专有进程/凭据分支、异常
文件系统或进程终止路径、默认端口的拒绝回退、Workflow 的部分历史记录形状，以及部分
资源上限边界。后续应在这些平台和有效账户上执行相同
验证；这些验证限制不被描述为已通过。
