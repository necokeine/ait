# Codex Thread Session Agent 导入验证报告

## 结果

Codex Thread 同步现在会保留原生 model/reasoning 配置。配置与回退命名 Agent 不同时，Session
绑定稳定的自有 Agent；对应模型或推理等级缺失时，先在全局 Codex Provider catalog 幂等补录，
再重读并提交 Project 历史。重复同步复用 Session、Agent 和模型记录。

## 验证

- `cargo test -p ait-application --test codex_history`：18 passed；新增生产 Portable 存储覆盖缺失
  模型/effort、失败重试与事件一致性，以及相同/另一有效 fallback 下重复同步私有 Agent。
- `cargo build --workspace`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo test --workspace --no-fail-fast -- --test-threads=1`：487 passed、0 failed、5 ignored。
- `cargo fmt --all --check`：通过。

默认并发的 workspace 与首次 coverage 各一次在既有
`daemon_http_generates_an_assistant_response_through_codex` readiness 10 秒窗口超时；相同测试单独
1/1、`codex_http` target 5/5、完整串行 workspace 与最终串行 coverage 均通过。失败发生在 daemon
ready 之前，本次变更路径尚未执行，因此未扩大 NEC-345 范围修改该并发 flaky。

## Test coverage

在合入 base `5ef106bd04956c245b1c11597aea3c65c08c4970` 的工作树执行
`cargo llvm-cov --workspace --html -- --test-threads=1`，default features、无手工源码排除，coverage
测试为 486 passed、0 failed、5 ignored（llvm-cov 不计 doctest）。结果：

- workspace line coverage：77.7405%，23,417 / 30,122；
- `ait-application` line coverage：81.9809%，9,618 / 11,732；
- `control/codex_history.rs`：87.0462%，1,055 / 1,212；
- 相对最近同口径基线 `codex-output-limits-coverage.json`，workspace -0.1859 pp，application
  -0.1952 pp；同期 NEC-344 删除 archive 源码及测试，使源码总体变化，该比较仅作参考。

[coverage 摘要](codex-import-session-agent-coverage.json)记录 source fingerprint、命令、测试数量、
ignored live tests 与并发 flaky 诊断。HTML 已在本机 `target/llvm-cov/html/index.html` 生成；可共享的
审查 artifact 为仓库内摘要。当前未覆盖的重要分支是确定性 Session Agent ID 与其他 owner 冲突的
拒绝路径；正常创建、重复同步、原生配置变化、null effort 和 Provider 补录均有离线回归。
