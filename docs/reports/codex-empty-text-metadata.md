# Codex 用户消息的空元数据卡片

导入的文本输入会将 Codex `text_elements` 保留为
`application/vnd.openai.codex.text-elements+json` 结构化 sub-message。即使其值是空数组，
Desktop 原先也会展示通用 JSON 卡片，导致正常输入下面出现 MIME 标题和 `[]`。

本次仅修改展示层：构建消息展示分段时跳过这个 MIME 类型的空数组，避免生成空卡片或空消息
外框。持久化数据、投影数据和输入顺序不变；已导入消息重新渲染即可生效，无需再次同步。
非空、格式异常的 metadata 仍可查看；普通文本中的 `[]` 和其他结构化数据不受影响。

## Test coverage

本轮 **not measured** 行覆盖率：修改仅涉及 Desktop TypeScript，Rust 行覆盖率不能衡量
该显示行为，未重新执行 llvm-cov 或采集 TypeScript 行覆盖率。回归以实际持久化消息形状经过
`projectMessage` 和渲染函数验证；后续覆盖率工作应为 Desktop 配置单独的测量与报告。

基于 `84b47827474a12d9c4284d3cab06ef6667172cc6` 的工作区，macOS arm64 验证：

- 新增 2 项回归：空 metadata 消失且历史不变；非空/异常 metadata、普通文本与其他 JSON
  空数组保留。修复前复现空卡片断言失败，修复后通过。
- `npm run typecheck`、`npm run build` 通过；`npm test`：143 通过、0 失败、0 跳过。
- `npm run test:browser`：69 通过、0 失败、0 跳过。
- 使用已有浏览器 harness 对截图中的中文输入与空 metadata 做额外显示验收，确认气泡仅含
  输入文字；已检查截图 `/tmp/ait-codex-empty-metadata-fixed.png`。此项不计入持久化测试数。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和
  `git diff --check` 通过。
- `cargo test --workspace --no-fail-fast`（默认 features）：459 通过、1 失败、5 忽略。
  失败为 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay` 的 15 秒启动
  时限断言。4 项真实模型测试与 1 项外部 worker 测试沿用既有忽略配置。
- 随后单独运行 `cargo test -p ait-daemon --test codex_http`：5 通过、0 失败、0 忽略，
  测试执行耗时 15.99 秒。首次全量运行的启动耗时失败未在此复跑中重现；未改动该测试或
  后端代码，也没有将复跑结果记成全量一次通过。若再次出现，应单独调查 daemon 启动耗时。

浏览器使用隔离 preload fixture；未访问真实 Codex 账号或修改用户会话。Linux/Windows 未实测。
