# Codex 输出预算与超限诊断

实现基于 `d17cef718643c106eaa96510779e8ce26c2338c2` 的工作区修改，设计见
[ADR-020](../decisions/adr-020-codex-output-limits.md)。

## 结果

原生 Codex 分析不再共享 API 工具的 64 KiB 输出限制。daemon/worker 默认使用独立的
8 MiB 上限，分别检查整轮累计文本和单个原生 item JSON。step、token、wall-clock
与 API 工具的限制没有变更；旧 bootstrap 缺少新字段时保留原调用方的较小输出限制。

超限返回四种明确指标：`native_items`、`item_bytes`、`text_bytes`、`tokens`。
可读错误包含实际值、上限与单位，原生 adapter/worker 的结构化 details 保存相同数值。
application 对外错误仍使用现有 code/message/retryable 字段，数值随 message 持久化。
计量器保留首个原因，
取消、权威历史读取、进程回收和后续同步不覆盖本地超限结论，不重放输入或触发自动提交。
JSON 大小通过只计数的 writer 计算，不分配完整序列化副本。

更大的原生内容通过已有历史分块协议传递；实时 delta 拆成最多 4 KiB 的 UTF-8 片段，
实时 Message 预览超过 64 KiB 时标记截断，避免越过 IPC frame 边界。

## Test coverage

`cargo test --workspace --no-fail-fast -- --test-threads=1` 通过：489 项通过、
0 失败、5 忽略，包含 1 项 doctest；覆盖率运行不计入该 doctest。

`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo build --workspace`、Desktop `npm run typecheck` 和 `git diff --check` 通过。
Desktop `npm test` 为 147 通过、0 失败。

覆盖率首轮执行为 487 通过、1 失败、5 忽略；失败目标
`ait-workspace-local --lib` 单独复跑 30 项全部通过。初次失败为本次未修改的
`workspace::deadline_tests::git_timeout_after_worktree_side_effect_reports_uncertain_creation_state`：
错误表示前置操作已耗尽 2 秒预算，尚未进入测试预期的 worktree 创建后阻塞阶段。
没有修改该测试或生产时限；复跑通过，但不能据此断言并发是失败的确定根因。
后续应将这个依赖真实执行速度的用例改为更确定的故障注入。

测试执行数量与以下行覆盖率分别统计。复跑保留初次测量的 profile，最终重新生成整个
workspace 的 HTML 与 JSON。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对 ADR-018 历史测量 |
| --- | ---: | ---: | ---: |
| Cargo workspace | 23,826 / 30,575 | 77.93% | +0.11 个百分点 |
| agent-adapters | 2,081 / 3,404 | 61.13% | +0.94 个百分点 |
| contracts | 551 / 660 | 83.48% | +0.28 个百分点 |
| worker | 72 / 549 | 13.11% | -0.34 个百分点 |
| application | 9,894 / 12,040 | 82.18% | 无变化 |
| workspace-local | 1,218 / 1,512 | 80.56% | +0.40 个百分点 |

修改后的预算检查 `crates/agent-adapters/src/codex/budget.rs` 为 **85/85（100%）**，
覆盖四种超限、阈值相等、首错保留、UTF-8/JSON 字节数、计数饱和与缓存 token 不重复计入。
worker contract 文件为 135/141（95.74%），包含独立默认上限、旧 bootstrap 与非法上限回归。
真实 worker 用例验证 1,200,000 字节中文文本经过有界进度 frame 与完整历史传输，
以及 item/text/token 超限的具体数值、取消与进程回收。桌面测试验证可读错误完整展示。

测量命令：

```sh
CARGO_BUILD_JOBS=2 cargo llvm-cov --workspace --html --no-fail-fast -- --test-threads=1
CARGO_BUILD_JOBS=2 cargo llvm-cov -p ait-workspace-local --lib --no-clean --html -- --test-threads=1
cargo llvm-cov report --html
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-codex-output-coverage.json
```

范围为 macOS arm64、默认 features、完整 Cargo workspace，Rust 1.98.1、
cargo-llvm-cov 0.8.4。没有额外排除源码；工具默认不计入测试源文件和 doctest。
4 项需要真实模型/凭据的测试及 1 项需要外部 worker 配置的测试保持忽略，具体名称见
[可审查的覆盖率摘要 JSON](codex-output-limits-coverage.json)。HTML 位于
`target/llvm-cov/html/index.html`，摘要随本次源码变更提供，不仅依赖本机 HTML 路径。

测量源码为本报告开头的基线提交加工作区修改，SHA-256 为
`8cd397ebdd2175c004e54f9f8790c436e5727d6da2890a82cf5dba66ccd00380`；计算范围、算法、
407 个源码文件的集合口径、各 crate 和修改文件结果均记录于摘要。
对比来源为 [ADR-018 历史测量](adr-018-coverage-summary.json)，它也包含一次补充测量；
没有重新测量本次基线的干净检出，差值不是本次修改的独立因果估计。

工作区仍低于 80% 目标。原生 adapter 文件为 189/276（68.48%），仍缺少部分协议/进程
I/O 失败组合。`bins/worker/src/codex.rs` 为 0/239：现有 worker 环境白名单不传递
`LLVM_PROFILE_FILE`，进程组强制回收也阻止正常退出时刷写 profile，因此真实 worker
集成用例通过不等同于这些行已被覆盖率工具记录。后续需完善子进程采集并补充针对性的
错误注入；本次没有排除这些文件来提高百分比。

## 验证边界

模型运行使用离线 app-server fixture 与真实 worker 子进程，不调用付费模型。
macOS arm64 为本次执行平台；其他平台未实测。Desktop 使用单元测试与类型检查，未启动
真实 Electron GUI 验收。

原生完整历史能传过 worker，不表示 Ait 会展示全部 8 MiB 文本。既有 ADR-016 的
ProviderItem 投影仍将字符串限制为 20,000 字符、payload 限制为 256 KiB；不在本次修改
中改变规范化版本或重写不可变历史。worker 子进程覆盖率受现有环境过滤与强制回收影响，
集成测试通过数量与工具实际记录的行覆盖率分开报告。
