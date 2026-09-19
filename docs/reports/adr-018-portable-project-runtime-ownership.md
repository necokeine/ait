# ADR-018 实现与验证

基于 `94e1c89c7450f365d3906a524b4798e8f9206b74` 的工作区修改，实现
[ADR-018](../decisions/adr-018-portable-project-runtime-ownership.md)。

## 使用行为

添加已有目录会打开其原 Project ID，保留根消息、Session、Run 与不可变 Message。
目录锁和操作系统账户公共 Project ID 锁共同排除另一后端及同身份目录副本。
关闭项目先拒绝新 Run/worker，再取消、排空执行并释放锁；关闭保留目录和最近项目注册。
退出或强制终止 daemon 后 OS 释放锁，下一实例提升 owner epoch；陈旧请求不能借 CAS 重试
获得新执行身份。

Desktop 项目菜单提供 Open project / Close project；项目设置可把保存的 Agent 引用明确连接
到本 catalog 的命名 preset。HTTP 对应 `/v1/project/close`、`/v1/project/bind-agent`；CLI 对应
`ait project close --project-id ...`、`ait project bind-agent --project-id ... --source-agent-id ... --agent-id ...`。
`x-ait-project-owner` 和 CLI `--project-owner` 接收项目响应中的 owner JSON，校验预期接管代次；
Desktop 自动保存并附带该上下文。兼容的不带此头的请求使用服务器当前上下文，不能表示旧客户端
的预期代次。协议支持与认证是不同边界。

## 存储与执行

生产 daemon 使用 `PortableSqliteControlStore`，全局和项目 SQLite 均为 format 3。
项目元信息、私有 Agent、Cron、配置来源/快照/绑定、历史、Run、progress、事件与提交回执在
项目库内保存。全局只保留共享配置及可重建注册、定位和 feed 投影；新生产写路径拒绝任意
跨项目或全局/项目混合提交。

`ControlVersion` 表达 catalog 身份/配置版本及实际读取的项目版本、实例和 epoch。
项目提交不会改变全局配置 revision；依赖全局配置时仍做配置 CAS。提交批次与本地事件、版本、
内容指纹回执在同一 SQLite 事务生效。相同已提交批次可重读原版本；这不是通用 HTTP 输入
exactly-once 协议，模型输入仍由已有 Run/input intent/worker receipt 协议保证不自动重发。

目录初始化失败整体回滚，不留下可被误认为完整项目的半份记录。投影失败不把已提交业务变成
失败输入；下一次读取/订阅会重试投影。全局事件表保存定位信息，项目事件正文仍在项目库。
订阅包含 catalog/feed namespace，切换命名空间触发 reset。当前保留全部项目 outbox 与提交回执，
没有新增自动清理/压缩策略。

worker 协议为 3.0，要求 `project-owner-v1`。监督器在 bootstrap 前持久化 PID/进程组声明，
只有正常 ExitReport 和监督器回收均成功才删除。崩溃或超时后，PGID 消失不足以证明
另建进程组的工具子进程已经结束；同一次宿主启动期间保留执行阻断。macOS 以
`kern.bootsessionuuid`、Linux 以 `/proc/sys/kernel/random/boot_id` 确认宿主重启后才清理
未确认回执（测试注入旧启动标识验证这条分支，没有实际重启机器）。这类项目仍允许已提交
历史读取，拒绝新的执行和业务写入，不会用时间到期或 PID 猜测来授权接管。
正常目录打开、后台标题、历史同步、恢复、Git 重试和
Run 监督器保留原始 owner fence。Unix 还核对根目录、`.ait`、数据库和锁文件的 inode/device。

启动扫描非阻塞尝试取得项目管理权；无恢复工作的临时 guard 立即释放，忙项目不阻断其他项目。
外来 catalog 不自动恢复旧 Run，Cron 保留定义但需本机明确启用。保存的 Agent/Provider 引用
不会仅因本机存在同 ID 配置而自动生效；绑定冻结选中的 Agent/Provider，修改它们会使绑定失效。

## 旧格式升级

仅支持已完成先前切换的 format 2。停止所有使用原 catalog 的旧 daemon 和 worker，然后执行：

```sh
ait-daemon --database /absolute/path/to/original/ait.sqlite3 --upgrade-storage
```

转换先恢复旧协议已决定的提交并重新枚举项目，再为全局及项目数据库建立一致备份；全局格式
屏障在任何项目转换前提交。每个项目保存独立完成回执，保持历史身份和内容，中断后运行相同
命令继续。转换全程离线；清单未完成时新 daemon 拒绝启动，旧版本也拒绝重新打开新格式。
验证发生在临时数据库，没有对用户正在使用的 catalog 或项目库执行升级。

必须使用原 catalog，不能靠删除 coordinator 字段抢救旧 prepared batch。更早格式、未知
格式、原 catalog 丢失、忙项目均明确报错。备份文件保留，Unix 新备份权限为 0600。转换不会
重跑 ADR-017 的历史清理；不会删除 `.ait`、原生 rollout 或工作区文件。

## 原生 Codex 与平台边界

所有 Codex 调用仍经过 ait-worker。公共 native binding reservation 独立于 Ait catalog，
项目内另有绑定回执；同 Thread 不能在本机登记给不同 Project/Session。当前不能验证跨
catalog 的原生存储来源，因此已保存 Codex Session 的重新绑定明确返回
`NATIVE_SOURCE_UNRESOLVED`，历史继续可读。未知来源共用 Thread ID 仲裁空间，可能产生保守冲突。
公共 registry 丢失但初始化标记仍在时返回 `BINDING_UNKNOWN`，不会用空表默许重新归属；
需要保留/恢复这份本机元数据，不承诺任意删除公共数据后的自动发现与修复。

普通打开不提供备份恢复、新身份复制或历史合并。已知 event stream 的 revision 回退会被拒绝；
恢复旧备份需要另行建立 stream generation 并核对外部副作用。项目内绝对 Session/cwd、凭据、
原生 rollout、Git worktree 等外部资源不会因为项目库便携而自动改写或重建。

验证平台为 macOS arm64、本地文件系统。Linux/Windows 及网络文件系统未在本轮验证。
非 Unix 崩溃残留 worker 声明采用保守阻断，尚无 Windows 的旧进程树核验器；不能据此声称
Windows 异常接管已完成。没有增加强制解锁、强制接管原生 writer 或自动重放未知输入。

## Test coverage

新增回归覆盖的主要行为如下；这是行为验收范围，不是覆盖率百分比。

| 范围 | 验证内容 |
| --- | --- |
| 两个真实 daemon | 不同 catalog 竞争同目录、显式关闭再打开、稳定 Project/根消息 ID、旧 owner HTTP 请求被拒绝、跨 catalog 事件游标 reset、空闲 daemon 分别正常退出和被 SIGKILL 后接管 |
| 项目存储 | 同 ID 的目录副本互斥、inode 替换拒绝、catalog 丢失后的索引重建、项目版本与配置版本分别校验 |
| 提交与投影 | 响应丢失后的相同批次去重、初始化失败回滚、项目提交成功而全局投影失败、索引缺失返回 rebuilding、跨项目 payload 拒绝 |
| 配置与原生绑定 | 外来配置/Cron 不自动启用、显式采用与配置变化后的失效、相同 Provider ID 不误用旧 endpoint、私有 Agent 项目内保存、原生归属冲突与未验证来源拒绝 |
| 排空与异常 worker | 拒绝新 Run/worker、允许已有 Run 收尾、真实进程组遗留阻断、进程组消失仍不误判清理完成、旧宿主启动标识清理分支 |
| 离线升级 | v2 历史保留、格式屏障后的中断恢复、项目提交成功但 catalog 确认丢失、重复执行不覆盖已转换的历史 |

测试执行结果（与覆盖率分开统计）：

| 命令 | 结果 |
| --- | --- |
| `cargo test --workspace --no-fail-fast -- --test-threads=1` | 481 passed，0 failed，5 ignored；包含文档测试 |
| `cargo test -p ait-storage-sqlite portable::tests -- --test-threads=1` | 最后调整后的 14 项专项复验通过 |
| `cargo test -p ait-daemon --test portable_projects -- --test-threads=1` | 包括事件游标 reset、正常退出和 SIGKILL 接管的真实双后端测试通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all --check`、`git diff --check` | 通过 |
| Desktop `npm run typecheck`、`npm test` | 通过；144 passed，0 failed |

5 项 ignored 是已有的真实 Codex/DeepSeek 凭据、API 用量和外部 worker 测试，没有新增跳过规则。
本轮没有执行 GUI 视觉验收、真实付费模型调用、实际操作系统重启或用户数据库升级。

测量范围为 macOS arm64、workspace 默认 features，没有手动排除源码。覆盖率不包含
doctest；插桩全量运行 480 passed、0 failed、5 ignored，另有 1 项正常退出接管补充运行通过。
完整常规回归包含 1 项额外 doctest，因此为 481 passed。最终补充只调整了验收测试，生产
源码保持不变。测量使用 Rust 1.98.1、cargo-llvm-cov 0.8.4，命令为：

```sh
cargo llvm-cov --workspace --html --no-fail-fast -- --test-threads=1
cargo llvm-cov -p ait-daemon --test portable_projects --no-clean --html -- --test-threads=1
cargo llvm-cov report --html
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-adr018-coverage.json
```

| 范围 | 行覆盖率 | covered / total |
| --- | ---: | ---: |
| Workspace | **77.82%** | **23,736 / 30,501** |
| 新增 portable 存储模块 | **87.57%** | **1,783 / 2,036** |
| storage-sqlite | 88.85% | 3,300 / 3,714 |
| application | 82.18% | 9,894 / 12,040 |
| api-http | 73.72% | 749 / 1,016 |
| ipc | 63.35% | 833 / 1,315 |
| ports | 64.34% | 175 / 272 |
| contracts | 83.21% | 545 / 655 |
| domain | 80.24% | 1,003 / 1,250 |
| CLI | 96.72% | 561 / 580 |
| daemon | 83.33% | 100 / 120 |
| worker | 13.46% | 72 / 535 |

基线 commit 为本文开头的 `94e1c89`，测量对象是本次未提交工作区。403 个源码/构建清单文件
按路径、长度和内容计算的 SHA-256 为
`f0df7e917777af67c147f1336f3fd84dd1223deefc31e2afe98da96aa300e663`。
精确算法、逐 crate/变更文件统计和跳过测试清单随本次修改提供在
[覆盖率 JSON 摘要](adr-018-coverage-summary.json)，可直接随代码审查；本地完整 HTML 为
`target/llvm-cov/html/index.html`，没有上传外部 CI。

没有针对当前基线 commit 的新测量，不能给出严格的前后差值。历史
[ADR-017 摘要](adr-017-coverage-summary.json) 为 77.29%（21,389 / 27,673），本轮高约
0.53 个百分点；两次源码和行数总体不同，仅供参考。Workspace 尚未达到 80% 目标。

剩余覆盖缺口主要包括应用层 Agent 绑定 HTTP 路径、活动 worker 排空超时、owner 校验失败的
部分回调分支、公共 native registry 损坏以及升级中的底层 I/O 故障组合。已有存储层配置绑定
测试不等于完整 UI/HTTP 绑定验收。后续应补这些场景及 Linux/Windows 平台验收。
worker 启动时会清理继承环境、不转发插桩环境变量，强制终止的进程也可能不写出 profile；实测 13.46%
反映已收集的数据，不能用进程测试已通过来替代覆盖率。正常退出 daemon 的补充验收既验证
运行锁释放，也让该进程写出 profile；没有为覆盖率修改生产环境变量白名单或终止规则。
