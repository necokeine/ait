# Paseo 桌面构建与 Rust server 启动整合

2026-09-25。范围为前端 workspace 构建和桌面管理 Rust binary；未修改 Rust 源码，未补齐
Agent/Workspace 订阅等业务接口。边界决策见 [ADR-048](../decisions/adr-048-paseo-desktop-rust-launcher.md)。

## 使用

```sh
npm ci
npm run dev:paseo
```

开发入口先通过 `install-electron` 准备 Electron 44 运行时，再构建 `server-bin`、六个共享包
与 Electron 主进程，启动 Metro，再打开桌面。
服务数据仅使用 `AIT_SERVER_DATA_DIR`，不继承旧 `PASEO_HOME`。默认服务数据为
`.tmp/paseo/server`，桌面配置为 `.tmp/paseo/electron`，均被 Git 忽略。
可通过 `AIT_SERVER_BIN` 使用指定 binary，`AIT_SERVER_DATA_DIR` 和 `AIT_SERVER_LISTEN`
覆盖数据目录与 loopback 端口，`EXPO_PORT` 覆盖 Metro 端口。

仅构建前端：`npm run build:paseo`。打包入口：`npm run build:desktop`，默认编译本机
release binary 并将其作为外部资源复制到 `bin/server`（Windows 为 `server.exe`）。
跨平台打包必须提供匹配目标平台的 `AIT_SERVER_BIN`；本轮没有验证签名、安装器或发行。

## 实现

- 导入固定 Paseo 提交 `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的 protocol、client、
  relay、highlight、plugin、expo-two-way-audio 共享包，以及前端 patches、TypeScript 和
  formatter 配置。新增根 workspace 与 lockfile；Node server/CLI 不进入依赖树。
- Plugin SDK 仅保留已有前端的编译依赖，没有恢复已删除的服务端 Plugin API。
- 替换依赖缺失 `dev-home.sh` 的开发入口，校正 Metro/Vitest relay 路径和 React 解析。
- 桌面直接管理 Rust ChildProcess。启动/停止/重启串行化；只终止自己启动的进程，
  不读取旧 Node supervisor PID 文件，也不再要求桌面和 Rust binary 具有相同版本号。
  就绪必须通过带认证的真实 WebSocket hello。
- 随机 token 只保存在主进程内存并传入子进程环境；只对当前托管 endpoint 的连接注入，
  不写入 renderer、URL、主机配置或日志。兼容 renderer 把 loopback 地址转成 localhost。
- 同一桌面进程内重启保持端口，桌面重新启动后刷新持久主机地址；稳定的托管连接 ID
  防止不断累积过期端口。停止先 SIGTERM，15 秒未结束则 SIGKILL。
- 应用退出时关闭托管服务，移除“退出后继续运行”的旧开关。旧 Node CLI 安装明确拒绝，
  daemon 状态诊断直接读取 Rust 子进程；普通浏览器认证和远程连接仍不在本轮范围。

## 验证

- `npm run build:paseo`：六个共享包及桌面主进程编译通过。
- `npm run typecheck --workspace=@getpaseo/app`：完整前端 TypeScript 检查通过。
- `npm ci --ignore-scripts --offline --dry-run`：lockfile 与 workspace 一致性检查通过。
- 前端定向 Vitest：6 个文件、139 项通过；覆盖启动服务、连接持久化、HostRuntime、
  主机探测、Rust transport 和 renderer IPC transport。
- 桌面定向 Vitest：8 个文件、43 项通过；包含真实 Rust 子进程并发启动、认证握手、
  重启、凭据更换、退出回收和数据目录争用，以及 IPC 注入、transport、退出和打包配置。
- macOS arm64 / Electron 44.2.0 实际窗口验收：renderer 自动启动 Rust server，主机进入
  online，窗口内真实 SDK 成功调用 `listProjects()`；通过桌面 IPC 重启 Rust 服务后
  自动重连并再次调用成功，renderer 异常为 0；关闭 Electron
  后验证其 Rust PID 已不存在。使用临时 server/Electron 数据目录，结束后删除。
- `prepare-server.mjs` 的 binary 资源复制已验证，复制产物可执行并报告 `server 0.0.6`。
- 定向 Oxfmt、Oxlint 和 `git diff --check` 通过。
- Rust format 与全工作区 Clippy 通过。首次全工作区回归在现有 Terminal 测试读取刚创建
  但尚未写入内容的 PID 文件时失败（`terminal.rs:327`，`ParseIntError: Empty`），
  当时已通过 576 项、忽略 5 项。该用例单独串行复验通过，随后首次未执行的十个 crate
  共 472 项全部通过；合计覆盖 1049 项不同测试，忽略 5 项。没有把首次运行标为通过。


真实窗口验收脚本为 `apps/paseo/e2e/rust-startup.e2e.mjs`。启动 Metro 后可运行：

```sh
EXPO_DEV_URL=http://localhost:8082 \
AIT_SERVER_BIN=/absolute/path/to/server \
node apps/paseo/e2e/rust-startup.e2e.mjs
```

本次首次下载在 React Native tarball 上超时/重置，完整文件进入 npm 缓存后离线安装成功。
并发构建期间曾触发测试超时，最终真实进程测试采用串行执行并明确设置等待时间。

## Test coverage

新增启动测试覆盖主进程进程所有权、凭据隔离、同地址重启、失败回收和目录锁争用；
前端测试覆盖托管地址刷新、稳定连接 ID 与手动配置保留。真实窗口测试覆盖从 renderer
启动服务到认证 RPC、重启重连与正常退出的完整路径。未测量新增 TypeScript 的行覆盖率。

本轮没有改变 Rust 行为。由于全量覆盖率运行遇到上述既有测试竞争、随后采用分段补跑，
本轮不报告可比较的完整 Rust 行覆盖率百分比。工作区检查结果及覆盖率说明记录于
[验证制品](paseo-desktop-startup-validation.json)。
Windows/ Linux 运行、跨平台打包、SIGKILL 后孤儿进程接管、完整聊天和全部上游 E2E
尚未验收。Agent/Workspace 列表订阅、部分 Session 事件与 SDK messageId 发送缺口仍保留，
不能把桌面启动成功视为全部业务功能对齐。
