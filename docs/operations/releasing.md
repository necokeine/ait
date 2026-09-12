# Ait desktop 发布操作指南

GitHub Release 是 Ait desktop 的唯一正式发布入口。推送语义化版本标签后，
`.github/workflows/release.yml` 在原生 GitHub-hosted runner 上分别构建 Rust daemon 与
Electron 桌面外壳，所有平台成功后才创建 Release。

## 发布产物

| 平台 | 架构 | 文件 | 用途 |
| --- | --- | --- | --- |
| Linux | x86_64 | `Ait-desktop-VERSION-linux-x86_64.AppImage` | 免安装运行包 |
| Linux | x86_64 | `Ait-desktop-VERSION-linux-x86_64.tar.gz` | 解压运行包 |
| macOS | Apple Silicon (arm64) | `Ait-desktop-VERSION-macos-arm64.dmg` | 图形化安装镜像 |
| macOS | Apple Silicon (arm64) | `Ait-desktop-VERSION-macos-arm64.zip` | 压缩应用包 |
| 全部 | — | `SHA256SUMS` | 上述四个文件的 SHA-256 校验值 |

GitHub 还会自动提供当前标签的源码 ZIP 与 tarball。Release Note 根据合并的 PR 自动生成；
`.github/release.yml` 按 breaking change、feature、fix 和其他变更分组，带
`skip-changelog` 标签的 PR 不进入说明。

当前 macOS 产物**没有签名或公证**。首次打开时 macOS 会显示开发者身份警告；在项目接入
Apple Developer 证书与 notarization 凭据之前，这些文件只适合内部测试分发。

## 准备版本

发布提交中的三处版本必须一致：Git 标签、根 `Cargo.toml` 的
`workspace.package.version`、`apps/desktop/package.json` 的 `version`。

以准备 `0.0.2` 为例：

```bash
# 修改 Cargo.toml 中的 workspace.package.version 后：
cd apps/desktop
npm version 0.0.2 --no-git-tag-version --ignore-scripts
cd ../..
cargo check --workspace
node apps/desktop/scripts/verify-release-version.mjs v0.0.2
```

提交更新后的 `Cargo.toml`、`Cargo.lock`、`apps/desktop/package.json` 与
`apps/desktop/package-lock.json`，通过 PR 合并到 `main`。不要从未合并的工作分支发布。

## 创建 Release

从最新 `main` 创建并推送标签：

```bash
git switch main
git pull --ff-only
git tag -a v0.0.2 -m "Ait desktop v0.0.2"
git push origin v0.0.2
```

`Release Ait desktop` 工作流会执行以下门禁：

1. 标签与 Rust、desktop 版本完全一致；
2. 用 `Cargo.lock` 构建 release 模式的 `ait-daemon`；
3. 把对应架构的 daemon 放入应用的 `resources/bin/ait-daemon`；
4. 生成四个预期包并确认没有缺失；
5. 写出 `SHA256SUMS`，用 GitHub 自动生成的 Release Note 发布全部文件。

工作流也支持手动重跑：在 Actions → **Release Ait desktop** → **Run workflow** 输入已经存在的
标签。手动运行不会创建标签；它检出该标签并重建产物。如果 Release 已存在，工作流会保留
Release Note，并用本次构建覆盖同名产物。

## 本地验证打包

必须在目标操作系统和目标架构上构建；不要把其他架构的 daemon 放入桌面包。

Linux x86_64：

```bash
cargo build --locked --release -p ait-daemon -p ait-worker --target x86_64-unknown-linux-gnu
cd apps/desktop
npm ci
npm run stage:daemon -- ../../target/x86_64-unknown-linux-gnu/release/ait-daemon
npm run package:linux
```

Apple Silicon：

```bash
cargo build --locked --release -p ait-daemon -p ait-worker --target aarch64-apple-darwin
cd apps/desktop
npm ci
npm run stage:daemon -- ../../target/aarch64-apple-darwin/release/ait-daemon
npm run package:mac
```

产物写入 `apps/desktop/release/`，暂存的 daemon 写入
`apps/desktop/release-resources/`；两者都已忽略，不应提交。

下载 Release 后，在 Linux 上运行 `sha256sum -c SHA256SUMS`，或在 macOS 上运行
`shasum -a 256 -c SHA256SUMS`，确认下载内容与 GitHub 上的校验文件一致。

## 失败恢复

- **版本检查失败**：删除错误标签，修正三个版本与 lockfile，在新提交上重新创建并推送标签。
  已公开使用的标签不要移动，应发布新的补丁版本。
- **任一平台构建失败**：Release job 不会运行，因此不会发布残缺的 Release。修复后发布新补丁版；
  仅当标签从未对外使用且提交未变化时，才使用手动工作流重跑原标签。
- **发布阶段失败**：用手动工作流输入相同标签。已有 Release 的同名文件会被覆盖，缺失文件会补齐。
- **校验失败**：不要运行下载文件；重新下载后仍失败时删除本地文件，并在仓库中报告该 Release。
