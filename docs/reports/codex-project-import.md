# 项目菜单中的 Codex 会话导入

本次实现基于 `939d5fd` 工作区，设计补充见
[ADR-017](../decisions/adr-017-unified-native-codex-worker.md#项目菜单中的历史发现与同步2026-09-19)。
首次交付后发现的原生列表重复 ID 问题及后续验证见
[重复 ID 修复报告](codex-thread-list-deduplication.md)；随后发现的 Desktop 可选字段误判及
真实 HTTP 集成验证见[未绑定会话修复报告](codex-import-optional-bindings.md)。下文保留首次交付的测量结果。

## 行为

Project 的 `•••` 菜单现在包含 `Pull from Codex…` 和 `Project settings`。
拉取入口先展示匹配当前项目的原生会话，显示标题、预览、工作目录、更新时间、归档和导入状态。
用户选择后再导入或同步；普通导航不请求 Codex 原生列表。

新导入可选择该 Codex Provider 的启用全局 Agent，已导入会话保留自己的 Agent。
活跃 Ait Run 和不可用的绑定 Agent 显示原因并禁止勾选。批次逐项执行，保留成功结果；
失败项可以再次提交，成功项不会在同一批次重试中重新同步。关闭弹窗停止剩余尚未发出的请求，
已发出请求仍可完成。结果刷新原目标 Project，不导航或替换正在查看的其他会话。

Desktop 经 typed preload IPC 调用现有 HTTP 历史接口，后端仍通过 `ait-worker` 访问
Codex app-server。此入口不发送输入、不调用 `turn/start`、不创建 Run、不生成 Git commit。
返回成功回执后独立刷新视图，避免把刷新失败误报成导入失败。

## 项目归属

新增可选 `project_id` 列表参数，由 application 复用同步时的归属规则。
已绑定 Thread 遵循原 Project；未绑定 Thread 的 canonical cwd 必须唯一属于一个已注册
Project 的根目录或其后代，或精确匹配已注册 Session 的 workdir。
无归属、无法规范化或同时匹配多个项目的会话不显示。路径判断按目录边界进行并解析符号链接。
列表中的匹配仅用于发现，真正同步时仍重新校验绑定和 writer 状态。
不传 `project_id` 的 CLI/HTTP 调用继续返回完整列表。

## 回归范围

- Rust application：目录后代、相似前缀目录、消失目录、嵌套项目歧义、符号链接、已绑定会话
  cwd 变化、跨项目隔离、未知 Project 在调用 Codex 前拒绝；CLI 覆盖可选参数解析。
- Desktop IPC：HTTP 请求范围、绑定 Agent/Run 投影、跨项目响应拒绝、最小回执、无额外视图
  读取或执行请求。
- 浏览器：按需发现、选择导入、保留 Agent、部分失败重试、非当前项目刷新、空列表和发现错误、
  关闭/重新打开/切换 Provider 的陈旧响应、批次关闭后的完成、繁忙会话、键盘与焦点恢复。
  原有项目设置与其他桌面流程也纳入完整浏览器回归。

## Test coverage

`cargo test --workspace --no-fail-fast`：455 通过、0 失败、5 忽略；`cargo fmt --all --check`、
`cargo build --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
Desktop `npm run typecheck`、`npm run build` 通过；`npm test`：140 通过、0 失败；
`npm run test:browser`：69 通过、0 失败，其中本功能新增 8 个浏览器用例。

`cargo llvm-cov --workspace --html`：454 通过、0 失败、5 忽略；与普通测试相差的 1 项为
doctest。以下数据是行覆盖率，不是用例通过率。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对 ADR-017 参考值 |
| --- | ---: | ---: | ---: |
| Cargo workspace | 21,447 / 27,720 | 77.37% | +0.08 个百分点 |
| application | 9,582 / 11,526 | 83.13% | +0.19 个百分点 |
| api-http | 717 / 929 | 77.18% | -0.08 个百分点 |
| cli | 531 / 550 | 96.55% | -0.68 个百分点 |
| contracts | 552 / 661 | 83.51% | 不变 |

修改的 `control/codex_history.rs` 整体为 802/931（86.14%）。新增用例覆盖本次项目筛选的
主要归属分支；CLI 参数解析和 Desktop IPC 请求范围已有断言，但 CLI 发出带项目参数的
HTTP 请求、HTTP 原生列表 handler 的串联路径没有在 Rust 覆盖率运行中被直接命中。
这些薄转发路径仍是验证缺口，不能用类型检查或浏览器 fixture 代替端到端测量。

范围为 macOS arm64、默认 features、完整 Cargo workspace；Rust 1.98.1、
cargo-llvm-cov 0.8.4。没有手动排除源码；工具不包含测试文件和 doctest，Desktop TypeScript
也不计入 Rust 覆盖率。Linux/Windows 分支未实测。4 项真实模型用例与 1 项需要指定外部
worker 的用例保持忽略，名称列于摘要。workspace 仍低于 80% 目标；既有 worker 强制结束
导致 profile 未刷写的限制仍然存在，未通过排除源码或调整百分比修饰结果。

测量基线为 `939d5fd304afa23113f661ed8d9cef177ac88453` 上的当前修改，源码 SHA-256 为
`05c1811c3b3a076011cde5ffcafd34f923da1340d32b9e641e8c21635ecf8686`。
[覆盖率摘要 JSON](codex-project-import-coverage.json)包含计算范围、各 crate、修改文件、测试数
及忽略用例，可随代码审查；HTML 明细位于 `target/llvm-cov/html/index.html`。
导出命令为 `cargo llvm-cov report --json --summary-only --output-path /tmp/ait-codex-import-coverage.json`。
比较使用 [ADR-017 历史摘要](adr-017-coverage-summary.json)，没有重新测量干净基线；
代码及行数总体已有变化，差值仅作为参考。

## 验证边界

浏览器测试使用隔离 preload fixture，并检查了弹窗截图；没有运行真实 Electron 与本地 Codex
账号的端到端人工操作，也没有拉取或修改用户的原生会话。本次不改变 app-server 协议适配，
真实协议/子进程行为由仓库既有回归覆盖，付费模型测试仍保持忽略。
