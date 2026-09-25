# App

Expo / React Native 前端，直接 TCP 连接使用独立 Rust `server` 后端。浏览器、Electron 和
原生客户端共享 Rust 协议适配器，开发依赖由仓库根 npm workspace 管理。

## 本地 Web 开发

从仓库根目录运行：

```sh
npm ci
export AIT_SERVER_TOKEN="$(openssl rand -hex 32)"
npm run dev:app
```

打开 `http://localhost:8081`，添加直接连接：Host `127.0.0.1`，端口 `7316`，访问令牌填写
当前 `AIT_SERVER_TOKEN`。macOS 可在同一终端用 `printf %s "$AIT_SERVER_TOKEN" | pbcopy`
复制令牌。不要把令牌放进 URL 或 `EXPO_PUBLIC_*` 环境变量。

入口先构建共享包与 Rust binary，再启动 server 和 Expo；Ctrl+C 同时停止两者。
server 默认数据目录为 `.tmp/app/server`，可用 `AIT_SERVER_DATA_DIR` 覆盖。支持
`AIT_SERVER_BIN`（已有 binary）、`AIT_SERVER_LISTEN` 和 `EXPO_PORT`。若修改服务端口，
连接表单也须填写对应端口。`npm run web --workspace=@getpaseo/app` 是同一入口。

## 分别启动

```sh
cargo run -p server-bin --bin server -- \
  --data-dir .tmp/app/server --listen 127.0.0.1:7316 \
  --web-origin http://localhost:8081 --web-origin http://127.0.0.1:8081
```

另一个终端：

```sh
npm run build:app-deps
npm run web:expo --workspace=@getpaseo/app -- --localhost --port 8081
```

服务端从环境读取 `AIT_SERVER_TOKEN`。浏览器先用 Bearer 换取 30 秒有效的一次性连接票据；
每次重连都会重新换票。页面来源必须匹配 `--web-origin`，原生客户端不需要这个参数。
配置也可写入 server 的非秘密 TOML：`web_origins = ["http://localhost:8081"]`。

## 原生与桌面

`npm run ios --workspace=@getpaseo/app` / `npm run android --workspace=@getpaseo/app` 构建
共享依赖后启动对应原生工程。后端仍需单独启动。iOS simulator 可直接访问宿主 loopback；
Android emulator/device 可先运行 `adb reverse tcp:7316 tcp:7316`，再连接 `127.0.0.1:7316`。
当前没有加入 LAN、公网或 Rust relay 接入，也未在实体设备上验证。

Electron 使用 `npm run dev:paseo`，由主进程负责 Rust 服务启动及 Bearer 注入。

## 验证与构建

iPhone 构建从仓库根运行 `npm run build:ios`（未签名真机归档）、
`npm run build:ios:simulator`（Release 模拟器 App）或 `npm run build:ios:ipa`（签名 IPA）。
证书、Bundle ID 和导出配置见 [Apple 构建说明](../../docs/operations/apple-builds.md)。

```sh
npm run typecheck --workspace=@getpaseo/app
npm run test --workspace=@getpaseo/app -- --project unit
npm run build:web --workspace=@getpaseo/app
APP_BROWSER_UI=1 node scripts/validate-app-rust-browser.mjs
```

最后一条从仓库根运行，需要已编译的 `target/debug/server`、Web 导出和 Playwright Chromium；
可用 `AIT_SERVER_BIN` 指定 binary。测试使用临时数据目录，覆盖实际浏览器鉴权、SDK RPC、
重连、错误令牌、页面连接及刷新恢复，结束后清理。

详细边界见 [ADR-049](../../docs/decisions/adr-049-app-rust-browser-transport.md) 和
[实施报告](../../docs/reports/app-rust-server.md)。
