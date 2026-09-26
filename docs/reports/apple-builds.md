# Apple 构建验证

日期：2026-09-26。环境：macOS arm64、Xcode 26.6、iOS SDK 26.5、CocoaPods 1.16.2、
Electron 44.2.0 / electron-builder 26.8.1、Expo 54 / React Native 0.81.5。

## 实现

- 根 workspace 提供 `build:dmg`、`build:dmg:release`、`build:ios:simulator`、
  `build:ios` 和 `build:ios:ipa`。详细参数见 [操作说明](../operations/apple-builds.md)。
- DMG 包含 release Rust server、Electron 主进程和 Electron 专用 Web 页面；Rust binary
  纳入 macOS 签名。本地 DMG 使用 ad-hoc 签名，不带上游自动更新源。
- iOS 从 Expo 配置生成原生工程，构建 Terminal WebView、安装 Pods，再执行 Release
  编译或 archive；不依赖运行中的 Metro。IPA 导出要求显式的 Team、Bundle ID、导出配置。
  最低版本调整为 iOS 16，匹配 Skia 静态二进制；Metro 默认限制为 2 个 worker，Xcode
  不输出 shell script 环境变量。
- EAS 增加 simulator / preview profile，移除固定的上游 Expo project、owner 和 ascAppId；
  通过环境变量绑定自己的账号。签名入口缺少配置会在构建前失败，IPA 导出拒绝 upload destination。
- 本轮没有修改 Rust 源码或服务端网络边界。按用户选择，先验证构建，正式签名稍后配置。

## 实测结果

- macOS arm64 DMG 成功：`apps/paseo/release/Paseo-0.9.0-beta.2-local-arm64.dmg`，
  140,508,407 bytes（约 134 MiB）。SHA-256：
  `ed2705f9e964a9787d782af1327ff0ec8a8c5175ae4b70ebdb7d7c30a01571f2`。
- `hdiutil verify`、只读挂载、`codesign --verify --deep --strict` 通过。挂载后内置
  `server --version` 为 `server 0.0.6`；Web 资源存在，未携带上游 updater 配置。
- 实际打包 App 启动成功：自动启动内置 Rust server、鉴权 RPC 项目列表、重启重连、
  正常退出后子进程回收全部通过；rendererErrors 为 0。验证中发现 Node inspector 参数
  被错误转入旧 CLI 路径，已修复并增加 2 项回归用例。
- App 与桌面 TypeScript 检查、定向 Oxfmt / Oxlint 通过。桌面打包和参数解析 24 项通过，
  App native version / Rust transport 定向 24 项通过。
- App 完整 unit suite：602 个文件通过、3 个文件失败；5,267 项通过、2 项失败、7 项跳过。
  失败涉及缺失根 `CHANGELOG.md`、live draft hook 超时以及时间测试假定英文但当前环境为
  中文；Node 26 下还报告 localStorage 缺失的未处理异常。没有把完整 suite 记为通过。
- iOS 模拟器最终 Release 构建成功：`apps/app/release/ios/simulator/DerivedData/Build/Products/Release-iphonesimulator/Paseo.app`，
  bundle 为 90,400,571 bytes，内含约 40 MB 的 Hermes bundle。`Info.plist` 和 Mach-O
  均确认 iOS Simulator / arm64、最低 iOS 16.0、SDK 26.5；版本 0.9.0 / build 9000002。
  在独立 iPhone 17 Pro / iOS 26.5 模拟器安装并启动成功，欢迎页完整显示，进程保持运行；
  没有启动 Metro。验证结束后删除临时模拟器，截图保存在 `.tmp/paseo-ios-release.png`。
- Rust fmt 与 workspace Clippy（`-D warnings`）通过。完整 workspace 共 103 个结果块，
  汇总 1,054 项通过、3 项失败、5 项忽略，命令退出码 101。其中两项 Codex 进程用例
  超时，使用该轮原测试二进制单独复验，2 项全部通过。
- 另外一项 `unix::checkout::binary_serves_checkout_reads_and_connection_owned_diff_updates`
  在 `bins/server/tests/process/checkout.rs:113` 断言刷新结果时得到 `Null`，单独复验仍失败。
  本轮没有修改 checkout 实现/测试；保留该失败，未将 workspace 整体记为通过。
- iPhone 真机归档成功：`apps/app/release/ios/unsigned/Paseo.xcarchive`，包含
  `Products/Applications/Paseo.app` 和 dSYM。App bundle 为 76,103,619 bytes；Mach-O
  明确为 **iOS / arm64**，最低 iOS 16.0、SDK 26.5，与模拟器二进制区分。Hermes bundle
  已打入 App；未嵌入 provisioning profile，未执行 IPA 签名或实体 iPhone 安装。

产物元数据、SHA-256 和检查汇总见 [机器可读验证记录](apple-builds-validation.json)。
Apple 公证、签名 IPA 导出、实体 iPhone 安装及真机连接 server 均未在本轮执行。

## Test coverage

本轮无 Rust 行为变更，Rust 覆盖率不适用；不把上一轮测量作为本轮覆盖率。
对构建变更使用真实打包、产物完整性检查和启动验证。
