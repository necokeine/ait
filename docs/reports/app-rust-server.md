# App 接入 Rust server

2026-09-26；基线 `0b6a6c3709e55d35c36af11fa54fc38f7359f8e0` 加本次工作区改动。

## 实现

`apps/app` 的普通浏览器现在能通过既有 Rust 协议适配器连接独立 `server`。
主机探测与长期 HostRuntime 都使用同一连接配置，四个功能连接各自完成 HTTP 换票及
WebSocket 升级。错误令牌、网络/来源错误会传回连接流程；断线取消未完成的换票。
Electron 和原生客户端保留 Bearer header 路径。

Rust server 新增 `POST /v1/auth/ws-ticket`、显式 `--web-origin`/TOML `web_origins`。
票据在内存保存，30 秒有效、单次使用且绑定 Origin，最多 256 个；默认仍仅监听 loopback。
领域和业务 RPC 没有变化，详见 [ADR-049](../decisions/adr-049-app-rust-browser-transport.md)。

新增 `npm run dev:app`，统一构建共享依赖并运行 Rust server 与 Expo Web。
入口要求环境中的 `AIT_SERVER_TOKEN`，不给 Expo 子进程传入该变量。默认数据隔离在
`.tmp/app/server`；退出时关闭自己创建的服务。直接连接默认 `127.0.0.1:7316`，中英文
表单明确访问令牌用途。iOS/Android 启动脚本构建完整共享依赖，操作方法见
[App README](../../apps/app/README.md)。

## 验证

- 前端 TypeScript 检查、定向 Oxfmt/Oxlint、Rust fmt、工作区 Clippy（`-D warnings`）通过。
- 前端定向 Vitest：6 个文件 122 项通过，含浏览器 transport、Rust 协议、HostRuntime、
  主机探测、桌面 transport 与翻译 key。新浏览器 transport 测试覆盖 HTTP 鉴权、二进制
  透传、401/403/429、异常票据、网络失败、取消及缺失令牌。
- `npm run build:web --workspace=@getpaseo/app` 的共享包构建通过；Expo 首次因沙箱禁止写
  `~/.expo` 中的遥测状态而停止。随后 `EXPO_NO_TELEMETRY=1 npm exec --workspace=@getpaseo/app -- expo export --platform web` 成功导出完整 Web 应用。
- [真实浏览器验证](../../scripts/validate-app-rust-browser.mjs)：Chromium 经真实 CORS
  preflight 换票，8 条实际 WebSocket 使用 8 个独立票据；SDK 项目列表、liveness ping、
  关闭后重新连接、错误令牌拒绝通过。完整导出页面从欢迎页直接连接，默认值检查、提交
  表单及刷新后主机恢复通过，页面未捕获异常为 0。测试使用临时服务数据、随机令牌并清理。
- [开发入口验证](../../scripts/validate-app-dev-runner.mjs)：实际启动 Expo 与 Rust server，
  两者就绪、配置来源可换票、输出不含令牌；向启动器发送 SIGTERM 后正常退出，两个
  监听端口均释放。
- 最终 `server-api` 全部 31 项测试通过，覆盖默认 Bearer 兼容、显式来源、CORS、
  一次性票据、过期/重放、限额、升级和关闭；server-bin 的 9 项单元测试与 10 项依赖
  边界测试通过。普通工作区首轮汇总 1,051 项通过、6 项失败、5 项忽略；失败来自
  Codex 取消（1）、旧 daemon（4）、修复前的无凭据升级状态码（1）。六项均在独立
  复验或最终覆盖率轮次通过；没有声称普通首轮命令整体通过。最终覆盖率轮次的
  server-bin 全部 42 项真实进程测试通过。

复现核心命令：

```sh
npm run typecheck --workspace=@getpaseo/app
npm run test --workspace=@getpaseo/app -- --project unit --maxWorkers 1 --testTimeout 60000 \
  src/runtime/host-runtime.test.ts src/runtime/rust-server src/utils/test-daemon-connection.test.ts \
  src/i18n/locales.test.ts src/desktop/daemon/desktop-daemon-transport.test.ts
APP_BROWSER_UI=1 node scripts/validate-app-rust-browser.mjs
node scripts/validate-app-dev-runner.mjs
cargo fmt --all --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -- --test-threads=1
```

并行编译期间前端动态导入曾超过测试超时；最终以单 worker 重跑完整定向集合通过。
首次 Rust 定向测试处于沙箱中，loopback/子进程权限导致失败；最终结果采用允许本机
网络、PTY 和测试子进程的环境。工作区初次普通测试的
`cancellation_during_handshake_reaps_the_owned_child` 在等待子进程时超时；同一测试单独
复验通过，覆盖率轮次也通过。并发的普通/覆盖率轮次中，旧 daemon 使用固定 Project ID
的用例出现锁争用及超时；随后单独运行的覆盖率轮次中该测试文件 5 项全部通过。
初轮工作区运行还暴露了无凭据 WebSocket 的状态码从 401 变为 403 的兼容回归，
已调整鉴权检查顺序，最终 server-api 全部测试通过。server 进程集成测试也出现过
3 秒响应超时，Forge 单项复验通过。最终汇总区分首次
执行、复验和覆盖率结果，没有把中间失败记作通过。

## Test coverage

最终工作区 Rust 行覆盖率 **83.33%（49,242 / 59,096 行）**。

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | --- | --- |
| server-api | 1,048 / 1,093 | 95.88% |
| server-bin | 769 / 819 | 93.89% |
| 新增 browser_auth.rs | 82 / 82 | 100.00% |

可审查制品：[覆盖率摘要及源码 SHA-256](app-rust-server-coverage.json)、
[验证结果](app-rust-server-validation.json)。完整 HTML 生成在
`target/llvm-cov/html/index.html`。测量使用上述基线加本次最终源码、默认 features、macOS
arm64；覆盖率统计没有额外源码排除，沿用 cargo-llvm-cov 默认的测试代码排除规则。
没有直接可比的改动前测量，因此不报告增减值。

首次 `cargo llvm-cov --workspace --html --offline -- --test-threads=1` 因 daemon 测试失败
中断，保留 profile，以以下命令完成剩余包并重跑 daemon；其中排除的五个包已经在首轮
完整执行通过，仅从第二次测试执行排除，仍在最终报告范围中：

```sh
cargo llvm-cov --workspace --no-clean --html --offline --no-fail-fast \
  --exclude-from-test ait-agent-adapters --exclude-from-test ait-api-http \
  --exclude-from-test ait-application --exclude-from-test ait-cli \
  --exclude-from-test ait-contracts -- --test-threads=1
```

第二轮有一项既有 worker 心跳测试把启动延迟判作握手超时；使用同一个 instrumented
binary 单独重跑通过，并写入同目录 profile。具体命令在 JSON 制品中。测试通过数与行
覆盖率分别记录，测量合并首轮、完成轮次和该复验的数据。最后按 `cargo llvm-cov show-env`
给出的 instrumentation 环境执行 `cargo test --workspace --no-run --offline`，更新最终源码
的所有覆盖率映射，再执行 `cargo llvm-cov report --html` 和
`cargo llvm-cov report --json --summary-only --output-path /tmp/ait-app-coverage-summary-final.json`。

新增鉴权模块的行覆盖率为 100%，仍有未命中的错误分支：Mutex poison、底层 URI 解析
失败和部分异常 Header 编码。全工作区仍有 9,854 行未覆盖；本次没有为了提高覆盖率
改动无关模块。五项既有忽略测试涉及真实 Codex/付费 DeepSeek 或显式外部 worker，未
启用；Windows/Linux、真实移动设备和真实 Provider 后续应在相应环境验证。

前端行覆盖率未测量；122 项测试通过不是覆盖率百分比。未验证真实 iOS/Android 设备、
HTTPS/远程部署、真实模型执行、语音或全部业务操作。此次接通浏览器 transport，不改变
既有 RPC 行为差异，包括带服务器时间戳的 SDK session ping 尚不支持；liveness ping 已验证。
