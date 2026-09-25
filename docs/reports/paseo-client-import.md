# Paseo desktop 与 app 源码导入

本报告记录初始源码快照；后续本地连接层修改见
[前端 Rust 适配报告](paseo-client-rust-adapter.md)，原始导入摘要继续作为来源基线保留。

按指定路径导入本地 Paseo 仓库的两个项目：

| 上游目录 | 导入目录 | 上游文件数 |
| --- | --- | ---: |
| `packages/desktop` | [`apps/paseo`](../../apps/paseo) | 205 |
| `packages/app` | [`apps/app`](../../apps/app) | 2,564 |

来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`，
版本 `0.9.0-beta.2`。导入时源仓库工作树干净，仅复制受版本管理的文件。
逐文件核对 Git blob、可执行权限及目录内容摘要，2,769 个原始文件全部一致。
两个目录共 26,193,611 字节，另补存上游根许可证。

源码、测试、原生模块和资源保持上游内容；原有 `apps/desktop`、Cargo workspace
和 Rust 服务端实现保持独立。来源、摘要算法和目录映射保存在
[导入清单](../../paseo/import-manifest.json)。构建依赖及待整合路径见
[导入说明](../../paseo/README.md#构建整合范围)。

## 验证

app 路径更正为 `apps/app` 后，重新校验全部 2,564 个 app 文件的内容、可执行权限和
目录摘要，结果一致；重新检查忽略规则和文档链接。下面的语法检查及 Rust 回归结果
来自首次导入，本次仅移动客户端目录及更新导入元数据，没有重复运行。

- 2,769 个文件的内容、可执行权限和目录摘要与上游一致；两份补存许可证逐字一致。
- 新文件均未被仓库忽略规则意外隐藏。
- 使用本仓库现有 TypeScript 5.9.3 的 `createSourceFile` 检查 2,573 个 JS/TS/JSX/TSX
  文件，语法错误为 0。这是语法检查，不等于依赖解析、完整类型检查或构建通过。
- 格式、Clippy `-D warnings` 和 diff 检查通过。完整 Rust 工作区测试：
  **1004 passed、0 failed、5 ignored**，99 个目标。
  [验证制品](paseo-client-import-validation.json)保存范围、命令、导入摘要和测试日志哈希。
  Rust 验证针对当前工作树（包含此前未提交的实现），不是单独测量上游客户端功能。

Rust 工作区验证命令：

```sh
cargo fmt --all --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
git diff --check
```

## Test coverage

Rust 行覆盖率：**not applicable — 本轮没有修改 Rust 行为**，没有将之前的覆盖率结果
作为本轮测量。新增前端源码覆盖率：**not measured**，这是上游原始源码导入，依赖的
npm workspace 共享包、根配置和构建脚本尚未迁入，因此未运行客户端完整类型检查、
构建、功能测试或覆盖率。后续接通 workspace 后再验证 Electron、Web、iOS 和 Android。
