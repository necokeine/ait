# Codex Thread Session Agent 导入验证报告

## 结果

Codex Thread 同步现在会保留原生 model/reasoning 配置。配置与回退命名 Agent 不同时，Session
绑定稳定的自有 Agent；对应模型或推理等级缺失时，Codex Provider catalog 会在同一事务中补录。
重复同步复用 Session、Agent 和模型记录。

## 验证

- `cargo test -p ait-application --test codex_history`：15 passed；覆盖原生 Agent 配置、Provider
  模型补录、重复同步、同一 Agent revision 更新，以及 Codex 历史同步与恢复回归。
- `cargo build --workspace`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo test --workspace --no-fail-fast -- --test-threads=1`：490 passed、0 failed、5 ignored。
- `cargo fmt --all --check`：通过。

默认并发的 workspace 与首次 coverage 各一次在既有
`daemon_http_generates_an_assistant_response_through_codex` readiness 10 秒窗口超时；相同测试单独
1/1、`codex_http` target 5/5、完整串行 workspace 与最终串行 coverage 均通过。失败发生在 daemon
ready 之前，本次变更路径尚未执行，因此未扩大 NEC-345 范围修改该并发 flaky。

## Test coverage

在 base `33b318ccb5109842a0c882337937bb9fc24bd82b` 的工作树执行
`cargo llvm-cov --workspace --html -- --test-threads=1`，default features、无手工源码排除，coverage
测试为 489 passed、0 failed、5 ignored（llvm-cov 不计 doctest）。结果：

- workspace line coverage：77.9860%，24,015 / 30,794；
- `ait-application` line coverage：82.2987%，10,089 / 12,259；
- `control/codex_history.rs`：86.7188%，999 / 1,152；
- 相对最近同口径基线 `codex-output-limits-coverage.json`，workspace +0.0596 pp，application
  +0.1226 pp；源码总体变化使该比较仅作参考。

[coverage 摘要](codex-import-session-agent-coverage.json)记录 source fingerprint、命令、测试数量、
ignored live tests 与并发 flaky 诊断。HTML 已在本机 `target/llvm-cov/html/index.html` 生成；可共享的
审查 artifact 为仓库内摘要。当前未覆盖的重要分支是确定性 Session Agent ID 与其他 owner 冲突的
拒绝路径；正常创建、重复同步、原生配置变化、null effort 和 Provider 补录均有离线回归。
