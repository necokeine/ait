# Paseo SDK 本地 library

2026-09-26。本次将已有 SDK 的依赖声明固定为本地路径，并完善独立构建与验证入口。

## 源码与依赖

`packages/client`、`packages/protocol`、`packages/relay` 已在 ADR-048 实施时导入，
来源为 `getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`，版本 `0.9.0-beta.2`。
本次检查前，npm lockfile 和 Node 实际解析就已经指向本地 workspace；没有重复复制源码。

- App 对 client/protocol、Electron 对 protocol、client 对 protocol/relay 的依赖改为
  显式 `file:` 路径。Plugin SDK 的开发依赖也改为本地路径，peer 范围允许本地版本演进。
- 三个 SDK 包标记为 private，移除公开发布配置；保留包名、公共导出和 TypeScript 声明。
- `npm run build:sdk` 按 protocol、relay、client 的顺序构建，旧 `build:client` 保留为别名。
  `build:app-deps` 使用同一入口，清空三个包的 dist 后也能独立构建。
- `npm run test:sdk` 先构建，再运行三个包的本地测试。排除需要 Wrangler 服务的
  `src/e2e.test.ts` 和访问托管服务的 `src/live-relay.e2e.test.ts`。
- 补齐 SDK 现有垃圾回收测试使用但未声明的 `tsx` 开发依赖，并同步 lockfile。
  SDK 本身使用本仓库源码，其他第三方 npm 依赖仍由 lockfile 管理。

使用方式见 [SDK README](../../packages/client/README.md)。本次没有修改 Rust 源码、
领域边界或 SDK 业务协议。Rust 协议转换仍由 `apps/app/src/runtime/rust-server` 提供，
relay 的 Rust 接入仍需后续实现。

## 验证

- 清空 protocol、relay、client 的 dist 后，`npm run test:sdk` 成功重建并完成
  **76 个测试文件、969 项测试**：protocol 706、client 216、relay 47。
- lockfile 中三个 SDK 包均为本地链接；从 App、Electron、client、plugin 四个调用位置
  解析 client 公共入口、protocol messages、relay E2EE，12 次真实路径检查全部指向
  仓库内 dist。公共入口实际 import、创建客户端并关闭成功。
- `npm run build:paseo` 通过：三个 SDK 包、其他前端共享包和 Electron 主进程构建成功。
- protocol、client、relay、SDK examples、App 和 Electron 的完整类型检查通过。
- SDK 源码 Oxlint 检查通过，163 个文件、0 warnings、0 errors。
- `npm ci --ignore-scripts --offline --dry-run --no-audit --no-fund` 安装规划检查通过。
- 定向 Oxfmt、`git diff --check`、`cargo fmt --all --check` 和全工作区 Clippy
  `--all-targets --offline -- -D warnings` 通过。
- Rust 全工作区测试已执行，但**未完成**，不能标记为全量通过。在允许本机监听的环境中
  运行超过十分钟后停止，已完整执行 15 个测试目标：181 passed、0 failed、2 ignored；
  停止时正在执行 `ait-application` 的 `provider_catalog` 测试目标。
  调用栈采样显示慢速等待位于 `reqwest::ClientBuilder::build` →
  `hyper_util::client::proxy::matcher::mac::with_system` 的 macOS 系统代理配置读取。
  已完成的三个目标分别耗时约 188、129、186 秒。本次不修改无关 Rust 实现来绕过该等待。

首次测试暴露缺失的 tsx；补齐后 SDK 全部通过。沙箱内执行中继转发与 Rust 测试时，
本机监听和部分进程操作被权限限制，未将该次执行计为通过；中继测试已在允许本机
监听的环境复验成功。两次未完成的 Rust 运行及其当前测试进程均已停止。
没有访问托管 relay 或执行设备配对 E2E。

## Test coverage

未新增业务代码测试；运行现有 SDK 全量本地测试，覆盖公共 API、订阅释放、传输、
协议解析、E2EE 和中继路由。SDK 行覆盖率未测量。Rust 行覆盖率不适用：本轮没有
修改 Rust 行为，工作区测试用于检查集成回归。
