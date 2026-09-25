# Apple 本地构建

在仓库根目录执行命令。需要 macOS、完整 Xcode（含 iOS SDK）、CocoaPods、Node/npm 和
Rust stable；先运行 `npm ci`。首次构建会下载 Electron、Pods 和 React Native 预编译依赖。
原生工程、签名文件和产物不提交到 Git。建议预留至少 20 GB 磁盘空间。
iOS 最低版本为 16.0，与当前 Skia 二进制依赖一致。

## macOS DMG

```sh
npm run build:dmg
# 同时验证内置 Rust server 的启动、鉴权 RPC、重启重连和退出清理：
PASEO_DESKTOP_SMOKE=1 npm run build:dmg
```

流程：共享 TypeScript 包 → Electron 专用 Web 导出 → Rust `server` release binary →
Electron 主进程 → electron-builder DMG。当前命令只构建宿主架构：Apple Silicon 是 arm64，
Intel 是 x64。Rust binary 和 Electron 必须同架构，不支持直接生成 universal 包。
`AIT_SERVER_BIN` 可指定已有的同架构 binary；默认从当前仓库编译。

输出：`apps/paseo/release/Paseo-<version>-local-<arch>.dmg`，其中包含 `Paseo.app`。
Rust server 位于 App 的 `Contents/Resources/bin/server`，Web 页面位于 `app-dist`。
本地构建使用 ad-hoc 签名，关闭 notarization 和上游自动更新源；适合本机验证，不等同于
通过 Gatekeeper 公证的正式分发包。构建始终传入 `--publish never`。

正式分发使用 Developer ID Application 证书和 Apple 公证凭据，例如已保存在钥匙串中的
notarytool profile：

```sh
export CSC_NAME='YOUR NAME (TEAMID)'
export APPLE_KEYCHAIN_PROFILE='your-notary-profile'
npm run build:dmg:release
```

也支持 electron-builder 的 `CSC_LINK` 和 `CSC_KEY_PASSWORD`，以及完整的 Apple ID 或
App Store Connect API key 公证环境变量。正式命令会提交给 Apple 公证服务，不上传到 GitHub。
`CSC_NAME` 填写证书名称中冒号之后的部分，不包含 `Developer ID Application:` 前缀。
它使用 `electron-builder.yml` 的发布配置；对外分发前应把其中上游 `getpaseo/paseo` 更新源
换成自己维护的发布源。Rust 可执行文件也列入签名范围。

## iOS 模拟器 App

```sh
npm run build:ios:simulator
```

流程：共享包 → Terminal WebView bundle → Expo prebuild → CocoaPods → Xcode Release build。
不依赖 EAS 云构建，也不需要 Apple 账号或 Metro 开发服务。
默认产物：`apps/app/release/ios/simulator/DerivedData/Build/Products/Release-iphonesimulator/Paseo.app`。

```sh
xcrun simctl boot 'iPhone 17 Pro'  # 使用本机已有的模拟器名称
xcrun simctl install booted apps/app/release/ios/simulator/DerivedData/Build/Products/Release-iphonesimulator/Paseo.app
xcrun simctl launch booted sh.paseo
```

模拟器 App 不能安装到实体 iPhone。

## iPhone 归档与 IPA

```sh
npm run build:ios
```

生成 arm64 真机 Release 归档：`apps/app/release/ios/unsigned/Paseo.xcarchive`。
该命令关闭签名，可验证原生编译和 JS 打包，但未经签名的归档不能直接安装或提交 App Store。
手机端只包含客户端，Rust server 运行在电脑/服务器上。

导出可安装或可提交的 IPA，需要自己的 Bundle ID、Apple Team、签名证书和匹配的
provisioning profile。在 Xcode 的 Signing & Capabilities 配置自己的 Team，按用途导出一次
ExportOptions.plist（development / ad-hoc / App Store Connect），保存在 Git 之外。
然后运行：

```sh
export APPLE_TEAM_ID='YOURTEAMID'
export IOS_BUNDLE_IDENTIFIER='com.yourcompany.paseo'
export IOS_EXPORT_OPTIONS_PLIST='/absolute/path/to/ExportOptions.plist'
# 如需 Xcode 联系 Apple 更新 provisioning profile，可显式启用：
# export IOS_ALLOW_PROVISIONING_UPDATES=1
npm run build:ios:ipa
```

产物在 `apps/app/release/ios/signed/`。ExportOptions 的 destination 应为 `export`，
这个步骤只导出 IPA；TestFlight/App Store 上传需单独执行。Development/ad-hoc 签名需要
包含目标设备；App Store 签名产物通过 TestFlight/App Store 安装。证书、profile、Apple
密码和 API key 不得提交到仓库。

`IOS_BUILD_JOBS` 默认 4，Metro 默认 2 个 worker，可降低以控制内存；
`EXTRA_PACKAGER_ARGS` 可覆盖 Metro 参数；`APP_VARIANT=development` 生成 Paseo Debug。
脚本每次重新应用 Expo 配置，不使用 `prebuild --clean`。原生目录是生成物，请把持久配置
放在 `app.config.js` 或 Expo config plugin 中。

如选择 EAS，先设置自己的 `EXPO_OWNER`、`EAS_PROJECT_ID` 和 `IOS_BUNDLE_IDENTIFIER`，
再从 `apps/app` 运行 `eas build --platform ios --profile simulator|preview|production`。
项目不再默认绑定上游 Expo project 或 App Store app。EAS 会使用账号服务及对应签名流程。

## server 连接范围

桌面包自动启动内置 Rust server。iOS 模拟器可连接宿主 `127.0.0.1:7316`，填写 server 的
访问令牌。实体 iPhone 的 `127.0.0.1` 指向手机自身；当前 Rust server 只接受 loopback
监听，真机访问电脑仍需要另行配置网络入口/安全隧道。本次打包不改变服务端网络边界。

参考：[electron-builder v26 macOS 配置](https://www.electron.build/v26/docs/mac/)、
[Expo 本地 Release 构建](https://docs.expo.dev/guides/local-app-production/)、
[Apple 注册设备分发](https://developer.apple.com/documentation/xcode/distributing-your-app-to-registered-devices)。
