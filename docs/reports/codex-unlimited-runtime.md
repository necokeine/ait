# Codex 原生任务时限与告警布局

变更已 rebase 到远端 `main` 的 `fb54a3eb675a3b05a4b9d7fdc00e61f70b175fee`，设计见
[ADR-021](../decisions/adr-021-codex-unlimited-runtime.md)。本报告为 rebase 后源码的验证结果。

## 结果

原生 Codex writer 不再受 300 秒任务截止时间限制。API 与 Codex 辅助查询的时限保留；
手动停止、daemon shutdown、心跳失联、有限 drain 与进程树回收保持原有行为。
输出、item 与 token 预算没有放宽。

告警独立占用网格行，多个告警在有限高度内滚动；消息区和输入框保持可用。
原生中断显示 `Run interrupted`，恢复错误保留 `Workspace recovery needs review`。
既有 Run/Message 不被重写，之前中断的任务不会自动重新发送。

## Test coverage

监督循环的 5 项回归覆盖一天模拟运行时间、API/辅助操作保留截止时间、用户取消、
daemon shutdown、心跳失联和 drain 超时。普通 Cargo 工作区测试 **492 通过、0 失败、5 忽略**，
包含 1 项 doctest；覆盖率运行 **491 通过、0 失败、5 忽略**，不计 doctest。
两套完整 Rust 测试顺序执行。Desktop 单元测试 **147 通过**、浏览器测试 **87 通过**；
TypeScript build/typecheck、版本一致性、workspace build、fmt 和 Clippy 均通过。

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| Cargo workspace | 23,428 / 30,131 | 77.75% |
| ipc | 851 / 1,324 | 64.27% |
| supervisor.rs | 265 / 291 | 91.07% |
| contracts | 488 / 589 | 82.85% |

与最近上游的[同平台默认 features 摘要](codex-import-session-agent-coverage.json)相比，
workspace +0.0133 个百分点。该摘要未单列 ipc，
与更早的[输出预算变更摘要](codex-output-limits-coverage.json)相比，ipc
+0.3966 个百分点。未另建干净基线重新测量；
上游 archive 删除及导入改动已改变源码集合，历史差值仅供参考，不能视为本次修改的独立效果。

工作区仍低于 80% 目标。IPC 的握手与进程/RPC 故障路径仍有缺口；真实 worker 子进程的
环境过滤与强制回收导致 profile 采集不完整（worker 72 / 549，
13.11%），后续应补充故障注入及子进程覆盖率采集。
测试通过数量与覆盖率分别统计，Desktop 数量不代表 JavaScript 行覆盖率。

验证命令：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --target-dir target/ait-unlimited-check -- -D warnings
cargo test --workspace --target-dir target/ait-unlimited-check --no-fail-fast -- --test-threads=1
cargo build --workspace --target-dir target/ait-unlimited-check
cargo llvm-cov --workspace --html --no-fail-fast -- --test-threads=1
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-rebase-coverage.json
```

Desktop 使用 `npm test`、`npm run build`、`npm run typecheck`、`npm run verify:release`
和 `node --test test/browser/*.test.mjs`。普通 Rust 构建和测试使用独立 target，
覆盖率使用工具自己的 `target/llvm-cov-target`。

测量源码提交为 `964a289252dd606c7bbda8a8adcfb8df95f02706`，后续仅更新验证文档。
407 个源码文件集合的 SHA-256 为
`297eaa796d531bf0b9467848bb209f60bbe60821b9f5b52c257ef243a6008a66`。
可共享的[覆盖率摘要](codex-unlimited-runtime-coverage.json)记录集合算法、各 crate、改动文件的
覆盖行数和 5 个忽略测试的名称。Rust 1.98.1、cargo-llvm-cov 0.8.4；启用默认 features，
Tokio test-util 仅用于 dev-dependency。没有手工排除源码；cargo-llvm-cov 默认不计入测试源文件
和 doctest。HTML 在本机 `target/llvm-cov/html/index.html`。

## 验证边界

时限回归使用 Tokio 暂停时钟、真实 frame 编解码和内存双向管道，不调用真实模型，
不代表已让真实模型运行一天。5 个显式忽略的测试涉及真实模型/API 凭据或外部 worker，
没有运行。布局回归使用隔离 preload 的 Chromium，未改动用户数据库。
测量平台为 macOS arm64，Linux 与 Windows 未在本地实测。
本次 rebase 未重建 DMG；此前的 0.0.6 安装包基于 rebase 前源码，不能代表本次提交。
