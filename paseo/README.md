# Paseo 客户端源码导入

来源为 `getpaseo/paseo`，版本 `0.9.0-beta.2`，提交
`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。

| 上游目录 | 本仓库目录 | 上游文件数 |
| --- | --- | ---: |
| `packages/desktop` | [`apps/paseo`](../apps/paseo) | 205 |
| `packages/app` | [`apps/app`](../apps/app) | 2,564 |

两个目录最初按上游受 Git 管理的源码、测试、资源和可执行权限导入。来源与初始内容摘要见
[import-manifest.json](import-manifest.json)。上游许可证保存在 [LICENSE](LICENSE)
和 [apps/paseo/LICENSE](../apps/paseo/LICENSE)；项目内第三方许可证随原文件保留。

## 构建与启动

根 npm workspace 已接入 `apps/app`、`apps/paseo` 及 protocol、client、relay、highlight、
plugin、expo-two-way-audio 六个共享包；共享源码取自上述固定提交。Plugin 仅保留前端
编译依赖，Rust 服务端没有恢复 Plugin API。Node server 和 Node CLI 不进入 workspace。

```sh
npm ci
npm run dev:paseo
```

开发入口构建共享包、Electron 主进程及 `server-bin`，启动 Metro 和桌面。默认开发数据
位于 `.tmp/paseo/server`，桌面配置位于 `.tmp/paseo/electron`。桌面主进程生成随机 token，
启动 loopback Rust server，认证握手成功后自动登记主机；退出应用时停止自己启动的服务。

可设置 `AIT_SERVER_BIN` 使用指定的已编译 binary；`AIT_SERVER_DATA_DIR`、
`AIT_SERVER_LISTEN`、`EXPO_PORT` 可覆盖数据目录和端口。不要在环境中设置
`ELECTRON_RUN_AS_NODE`。仅构建前端可运行 `npm run build:paseo`，打包入口为
`npm run build:desktop`（默认构建当前平台的 release Rust binary）。

完整实现边界和验证见[启动报告](../docs/reports/paseo-desktop-startup.md)与
[ADR-048](../docs/decisions/adr-048-paseo-desktop-rust-launcher.md)。原始导入清单仅对应最初
两个客户端目录；共享包为后续新增。
