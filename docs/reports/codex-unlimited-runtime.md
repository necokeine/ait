# Codex 原生任务时限与告警布局

基于 `33b318ccb5109842a0c882337937bb9fc24bd82b` 的工作区修改，设计见
[ADR-021](../decisions/adr-021-codex-unlimited-runtime.md)。

## 结果

原生 Codex writer 不再受 300 秒任务截止时间限制。API 与 Codex 辅助查询的时限保留；
手动停止、daemon shutdown、心跳失联、有限 drain 与进程树回收保持原有行为。
输出、item 与 token 预算没有放宽。

告警独立占用网格行，多个告警在有限高度内滚动；消息区和输入框保持可用。
原生中断显示 `Run interrupted`，恢复错误保留 `Workspace recovery needs review`。
原有记录和工作区未作修改，之前中断的任务不会自动重新发送。

## Test coverage

针对监督循环的 5 项回归通过，包括一天模拟运行时间、API/辅助操作保留截止时间、用户
取消、daemon shutdown、心跳失联和 drain 超时。Desktop 单元测试 147 通过，浏览器测试
87 通过；TypeScript build/typecheck、版本一致性、fmt 和 Clippy 均通过。

浏览器首轮 85 通过、2 失败，单独复跑仍失败：既有导入测试假设项目菜单第一项是
Pull from Codex，但当前菜单已有 Open project；配置测试未限定弹层，Saved Agent
同时匹配隐藏项目表单。仅修正这两处测试定位与键盘顺序，之后完整 87 项通过。
没有为通过测试修改这两个产品功能。

普通 Cargo 工作区测试 **494 通过、0 失败、5 忽略**，其中包含 1 项 doctest。
覆盖率首轮 492 通过、1 失败、5 忽略；失败目标 `ait-daemon --test codex_http`
单独复跑 **5 项全部通过**，保留首轮 profile 后重新生成全工作区报告。
失败为既有 `startup_scan_defers_an_offline_project_without_losing_its_run`，在准备数据时
遇到 `PROJECT_BUSY`。该目标以固定 `recovery-project` ID 获取宿主共享项目锁，本次普通
和覆盖率测试曾同时运行；记录表明发生了锁争用，单独复跑通过。未改动该 Rust 用例，
后续应隔离跨测试进程的项目身份或顺序执行这类完整套件。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对上次源码验证 |
| --- | ---: | ---: | ---: |
| Cargo workspace | 23,831 / 30,584 | 77.92% | -0.01 个百分点 |
| ipc | 851 / 1,324 | 64.27% | +0.40 个百分点 |
| supervisor.rs | 265 / 291 | 91.07% | 上次摘要未单列 |
| contracts | 551 / 660 | 83.48% | 无变化 |

测试通过数量与覆盖率分别统计。对比来源为
[上次源码验证摘要](codex-output-limits-coverage.json)，它包含一次补充目标运行；本次未
另建干净基线重新测量，差值不能视为修改的独立因果估计。工作区仍低于 80% 目标。
IPC 仍有握手、进程/RPC 故障路径未覆盖；真实 worker 子进程的环境过滤与强制回收导致
profile 采集不完整（worker 72/549，13.11%），后续应补充故障注入及子进程覆盖率采集。

验证命令：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --target-dir target/ait-unlimited-check -- -D warnings
cargo test --workspace --target-dir target/ait-unlimited-check --no-fail-fast -- --test-threads=1
cargo llvm-cov --workspace --html --no-fail-fast -- --test-threads=1
cargo llvm-cov -p ait-daemon --test codex_http --no-clean --html -- --test-threads=1
cargo llvm-cov report --html
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-unlimited-coverage.json
cargo build --locked --release -p ait-daemon -p ait-worker --target aarch64-apple-darwin
```

Desktop 使用 `npm test`、`npm run build`、`npm run typecheck`、`npm run verify:release`
和 `node --test test/browser/*.test.mjs`。默认 debug 目录由 IDE 后台检查占用，普通
Clippy/测试使用独立 target；覆盖率使用工具自己的 `target/llvm-cov-target`。

测量源码为上述基线加工作区修改，409 个源码文件集合的 SHA-256 为
`a7af26f79fbac1fa0ed52cdaa5a9654d43aa88119f3608edae2d16544d53d2d5`；
随变更提供的[覆盖率摘要](codex-unlimited-runtime-coverage.json)记录集合算法、各 crate、
改动文件的覆盖行数和 5 个忽略测试的名称。Rust 1.98.1、cargo-llvm-cov 0.8.4；
仅启用默认 features（新增 Tokio test-util 仅用于 dev-dependency），没有额外
排除源码；cargo-llvm-cov 默认不计入测试源文件和 doctest。HTML 位于
`target/llvm-cov/html/index.html`。

## 安装包

重新生成 0.0.6 macOS arm64 DMG，包含更新后的 release daemon/worker 与桌面文件。
37 个前端文件、挂载镜像内的 daemon/worker、应用与内置可执行文件签名、DMG 完整性、
临时数据库上的 daemon health 和正常退出均检查通过。本地构建使用现有 Developer ID
证书签名，未配置公证凭据，因此未公证。未安装、启动用户的 Electron 应用或发布 Release。
安装包、SHA-256 与包含源码身份的 build JSON 位于忽略的 `apps/desktop/release/`。

## 验证边界

时限回归使用 Tokio 暂停时钟、真实 frame 编解码和内存双向管道，不调用真实模型，
不代表已让真实模型运行一天。布局回归使用隔离 preload 的 Chromium，未改动用户数据库。
本次运行平台为 macOS arm64；其他操作系统没有实测。
