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

## 构建整合范围

这次导入是两个客户端的原始源码快照，现有 `apps/desktop` 和 Rust Cargo workspace
继续保持独立。上游 package.json、构建脚本及路径引用尚未完成 workspace 整合。

2026-09-25 起，TCP 连接层已增加本地 Rust server 适配；源码不再是逐字不变的上游快照。
新增和修改范围、测试及剩余后端差异见[前端适配报告](../docs/reports/paseo-client-rust-adapter.md)。

两个项目依赖上游 npm workspace，当前还不能作为独立项目直接构建或启动。后续整合需要：

- 提供共享包：app 使用 client、protocol、relay、highlight、plugin、expo-two-way-audio；
  desktop 使用 CLI、server 和 protocol。
- 接入上游根级构建脚本、TypeScript 配置、依赖补丁和锁文件。
- 校正测试与打包配置里的其余 workspace 路径；desktop 的 `../app` 引用已对应 `apps/app`。
- 将桌面内置 daemon 的启动和关闭改接 Rust binary，并使用已适配的 Rust TCP 连接层。

导入校验和测试范围见[导入报告](../docs/reports/paseo-client-import.md)。
