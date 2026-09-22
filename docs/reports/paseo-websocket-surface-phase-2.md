# Paseo WebSocket 接口移植：第二阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`2b7f428`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段把 daemon/config/diagnostics 分组的 9 个 catalog 条目全部加入生产 capability。累计已接通
24 个规范方法，剩余 167 个仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `daemon.get_status.request` | 同名 | server/version/pid/executable/start/listen；relay null、providers 空数组 |
| `daemon.get_pairing_offer.request` | 同名 | 无 relay 时返回 Paseo 的空 offer 形状 |
| `daemon.config.reload.request` | 同名 | 重读 `config.json`，验证后分类 live/restart/override 路径 |
| `daemon.update.request` | 同名 | 返回 Paseo update result；当前 standalone 安装明确失败 |
| `diagnostics.request` | 同名 | 返回不含凭据的进程、系统与 capability 文本报告 |
| `daemon.config.get.request` | `get_daemon_config_request` | 返回 normalized mutable config |
| `daemon.config.set.request` | `set_daemon_config_request` | 验证、深合并、provider 删除、原子持久化 |
| `server.restart.request` | `restart_server_request` | correlated ack 后 drain，释放锁并在同进程重新 bind/组装 |
| `server.shutdown.request` | `shutdown_server_request` | correlated ack 后 drain 并正常退出 |

legacy 下划线名称仍只保留在 method catalog 中，不参与路由。配置 DTO 复制 Paseo mutable config
的 relay、MCP、browser tools、host/CORS/proxy、Git、app、provider model、metadata generation、
profiles、skills 和 plugin 字段；Paseo 允许 passthrough 的层级也保留未知字段。patch 只采纳 Paseo
store 明确支持的字段，未知顶层字段不会变成设置入口。

## 持久化与生命周期

`server-ports::daemon::DaemonConfigStore` 定义 application 的出站边界；
`server-storage::daemon_config::FileDaemonConfigStore` 在 `<data-dir>/config.json` 保存配置，限制
4 MiB、拒绝 symlink、使用同目录临时文件、fsync 和原子替换。写入成功后才发布新的内存值；
无效外部编辑不会替换当前值。`removeProviders` 同时删除 provider override 和 metadata generation
候选，匹配 Paseo store 的行为。

WebSocket restart/shutdown 记录第一个 lifecycle intent 并关闭接纳。restart 完成所有已接纳连接与
blocking job 的 drain，drop application 和 instance lease，然后由 binary 主循环重新 bind。
稳定 `server_id` 保留，`instance_id` 每次变化。这个路径与 SIGINT/SIGTERM 使用相同关闭流程。

## 明确的 Paseo 差异

1. Paseo status 从 pid lock、Node runtime、relay manager 和 provider manager 聚合；Rust server 的
   `nodePath` 字段放当前 Rust executable，start time 是本次 composition 时间，尚无 relay/provider
   adapter，因此对应值是 null/空数组。
2. Paseo pairing offer 可生成 relay URL 和 QR；新 server 没有 relay keypair/registration，固定返回
   `{url:"", qr:null, relayEnabled:false}`，与 Paseo 的 relay-disabled 分支一致。
3. Paseo `config.json` 是更大的 persisted config，并映射出 mutable projection；当前新 server 直接
   持久化 normalized mutable projection。字段形状和 patch 行为对齐，但磁盘根结构不能与 Paseo
   home 互换。terminal profile、agent profile、skills 和 plugin source 的嵌套 schema 当前只验证
   JSON 容器，尚未复制对应 protocol 子模块的全部字段约束。
4. Paseo reload 对 live owner 做 transactional apply/rollback，并能报告 CLI/env override 控制字段；
   当前没有这些 runtime owner 或启动 override，reload 发布验证后的值，`overrideControlledPaths`
   始终为空。路径分类覆盖当前 mutable 字段；未知根字段和 plugin source 变化归为 restart-required。
5. Paseo diagnostics 还包含内存、负载、磁盘、Agent、Project/Workspace、provider/tool、WebSocket、
   observation 和 Hub 指标。当前报告只有进程身份、OS/architecture/CPU、生命周期和 capability，
   且用固定 provider 0 值明确表示尚未接入。
6. Paseo npm-global updater 会发送 progress、安装最新版并发 restart intent；Rust standalone binary
   没有可靠的安装来源或包管理器 adapter，因此返回 `success:false`、安全错误、previousVersion 和
   null newVersion。统一 RPC envelope 当前也没有 Paseo 的独立 progress push frame。
7. Paseo restart/shutdown 向 session 广播 status，再把 intent 交给外部 supervisor。新 binary 返回
   correlated lifecycle result，并在进程内 restart；没有跨连接广播。监听端口为 0 时重启后的实际
   端口可能变化，固定配置端口会重新绑定同一地址。

## 测试执行

移植了 Paseo daemon-session/config-store/self-update/lifecycle 测试中适用于当前安装边界的断言：
status payload、relay-disabled pairing、config get/patch/reload、unknown patch 丢弃、provider 删除、
原子文件与 symlink 拒绝、diagnostics 脱敏、unsupported updater result、restart identity 和 shutdown。
新增 14 个测试：protocol 4、application 2、storage 4、API 2、真实 binary WebSocket 2。

阶段性验证已通过：

```text
cargo test -p server-bin --test process                         8 passed
cargo test -p server-protocol -p server-storage ... --lib      52 passed
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

新 server 的 12 个普通测试目标共 **118 passed、0 failed、0 ignored**。其中本阶段新增
14 个测试全部通过；真实 binary 测试确认 config 写入/reload、legacy 名称拒绝、restart 重组与
shutdown 退出。

`cargo test --workspace --no-fail-fast -j 1` 的 72 个普通测试目标结果为 **607 passed、
2 failed、5 ignored**。第一个失败是未修改的旧 daemon 用例
`daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`，仍超过固定 15 秒 readiness
窗口，与第一阶段结果一致。第二个失败是 `server-workspace` 身份锁用例一次返回 `Busy`；该用例
随后精确复跑为 **1 passed、0 failed、13 filtered**，并在覆盖率全组运行中再次通过，因此记录为
一次测试进程释放时序抖动。本批修改的 server 测试全部通过。

普通测试完成后，第一个旧 crate 的 `rustdoc` 再次处于无 CPU、无输出状态，因而手动终止
doc-test 尾段；这与第一阶段的环境现象相同。格式、diff whitespace 和默认 feature 的全 workspace
Clippy 均通过。

## Test coverage

测量命令：

```sh
cargo llvm-cov \
  -p server-bin -p server-api -p server-application -p server-domain \
  -p server-ports -p server-protocol -p server-storage -p server-workspace \
  --json --summary-only --output-path /tmp/paseo-ws-phase2-coverage-raw.json \
  --no-fail-fast -j 1
```

覆盖率运行的 12 个测试目标为 **118 passed、0 failed、0 ignored**。测量范围只包含全部新
server package 的默认 features 和生产 Rust 源；test/build 源按 cargo-llvm-cov 默认规则过滤。

| 范围 | Covered / total lines | 行覆盖率 | 相对第一阶段 |
| --- | ---: | ---: | ---: |
| 全部新 server package | 5,191 / 5,819 | 89.21% | -0.14 个百分点 |
| server-bin | 320 / 332 | 96.39% | +0.24 个百分点 |
| server-api | 1,353 / 1,572 | 86.07% | +1.36 个百分点 |
| server-application | 898 / 980 | 91.63% | -0.06 个百分点 |
| server-domain | 213 / 213 | 100.00% | +0.00 个百分点 |
| server-ports | 16 / 16 | 100.00% | +0.00 个百分点 |
| server-protocol | 477 / 559 | 85.33% | -7.60 个百分点 |
| server-storage | 1,381 / 1,492 | 92.56% | +0.09 个百分点 |
| server-workspace | 533 / 655 | 81.37% | +0.00 个百分点 |

本阶段四个 daemon 主模块合计覆盖 **606 / 691 行（87.70%）**：API 125 / 128、application
59 / 65、protocol 109 / 161、storage 313 / 337。protocol 百分比下降主要来自新增的完整配置
DTO 字段面；行为分支由 schema、patch、reload 和真实 WebSocket 测试覆盖，尚未逐字段构造所有
可选 profile/skill/plugin passthrough 组合。

完整本地 HTML 位于 `target/llvm-cov/html/index.html`；可评审摘要保存在
[覆盖率 artifact](paseo-websocket-surface-phase-2-coverage.json)，包含命令、测试数、crate 汇总、
本阶段模块行计数和第一阶段基线。
