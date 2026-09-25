# 移除 Hub、Chat 与 Loop

按用户要求移除三组共 19 个 WebSocket 接口：Hub 7、Chat 7、Loop 5。

- Rust 协议枚举和方法目录已删除这些定义，能力发布、协商与通用占位分发自动随目录收缩。
- 前端 Rust transport 的 19 个对应映射同步删除，避免客户端继续协商已移除能力。四连接预算从 61/57/43/47 收缩到 61/57/43/28。
- 请求规范名称或旧名称均返回 `method_not_found`；optional negotiation 不包含它们，required negotiation 返回 `unsupported_capability`。
- 205 项原始 Paseo fixture 保留审计用途，19 项显式排除清单仅供测试使用。集合校验保证没有误删其他接口，也不会把这三组重新注册为占位。
- 现有独立 server 的业务处理器为 **157/183（85.79%）**，剩 **26** 个占位：Plugin 15、Schedule 9、Browser 2。含自定义接口为 **164/190**。数量不表示完整行为与 Paseo 等价，既有差异仍见 [接口差异清单](server-interface-gaps.md)。

## Test coverage

- 修订：`97f1372` 加未提交工作树；macOS aarch64，默认 features。验证期间工作区出现新的 Provider 流式实现修改，因此下述已有构建产物测试不代表最新 Provider 代码已验证。
- **Workspace 行覆盖率：not measured**。首次 `cargo llvm-cov --offline --workspace --html -- --test-threads=4` 成功编译，但在旧 daemon 的 `macos_gui_path_reaches_codex_in_the_worker` 就绪等待失败后停止。使用 `cargo test --offline --target-dir target/llvm-cov-target -p ait-daemon --test codex_http macos_gui_path_reaches_codex_in_the_worker -- --exact` 单独复验通过。
- 清空 raw profile 后，重跑 `cargo llvm-cov --offline --workspace --no-clean --no-fail-fast --html -- --test-threads=4`，被验证期间新增的 Provider 编译错误阻止：`local/codex/streaming.rs` 引用的子测试文件缺失；`service/agent_manager.rs` 调用的 `Timeline::progress` 尚不存在。本轮未改动这些 Provider 实现。待其完成后应重新执行完整 workspace 检查及覆盖率。
- 已独立测量 **server-protocol 行覆盖率 100%（57 / 57）**，命令为 `cargo llvm-cov --offline -p server-protocol --no-clean --html`；9 个测试通过，无跳过，未额外排除文件，使用 llvm-cov 默认过滤，不运行 doctests。该包与上一轮均为 57/57，变化 0 个百分点；不能将此百分比解释为 workspace 覆盖率。上一轮 workspace 基线为 82.98%（46,917 / 56,540），本轮无可比较的完整值。
- [验证和逐文件覆盖率制品](server-removed-groups-validation.json)。导出命令：`cargo llvm-cov report -p server-protocol --json --summary-only --output-path /tmp/ait-remove-groups-protocol-coverage.json`。`target/llvm-cov/html/index.html` 当前是该选包报告，不是完整 workspace 报告。

执行结果另列：

- 本轮第一次成功构建的 API 测试产物：**23 通过**，包括新增的 19 项删除接口及其旧名称的协商、请求拒绝测试。直接运行 `target/llvm-cov-target/debug/deps/server_api-c3e5547176625eac --test-threads=4`，并将 `LLVM_PROFILE_FILE` 指向临时目录。
- 同次构建的生产进程 catalog 测试：**1 通过**，验证发布能力不含三组前缀、164 个实现与 26 个占位。命令：`target/llvm-cov-target/debug/deps/process-a3febe65d0a718de unix::catalog::production_registers_every_canonical_method_and_routes_each_placeholder --exact`。
- 前端 `apps/app/src/runtime/rust-server/transport.test.ts`：**12 通过**，包含缩减后的映射数量和每条连接不协商已删能力的断言；使用临时 Vitest Node 配置与已有 `/tmp/ait-paseo-client-test-deps/node_modules`，未安装新依赖。
- `python3 scripts/check-paseo-client-methods.py` 验证 186/183 映射；`python3 scripts/check-paseo-protocol.py /Users/necokeine/Documents/paseo` 验证完整原始 205 项 fixture。排除清单集合相等检查由上述 Rust 协议测试覆盖。
- 本轮修改完成、Provider 后续编辑之前，`cargo clippy --offline --target-dir target/server-audit-lint --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check` 通过。TypeScript 修改经 Prettier 格式化；最终 `git diff --check` 通过。
- 不宣称完整 workspace 测试已通过；Windows、完整 GUI 与最新 Provider 并行改动未验证。
