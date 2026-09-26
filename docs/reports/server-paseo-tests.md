# Server Paseo 测试扩展

日期：2026-09-26。范围为独立 Rust server；没有改变 crate 边界、增加依赖或新增 RPC 方法。
基线为 `5e9fc8a759c886fb78e3212ec681391ed97318e7`，上游固定为
`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。

本轮新增 529 个 Rust 测试函数，未删除旧测试。其中 224 个为上游行为场景的适配，
305 个为这些契约衍生的容量、取消、持久化、隔离和故障回归。多个 Rust 测试可以对应同一
上游场景，参数表不另计测试数；这些数字不代表 Paseo 功能实现比例。

[逐测试来源清单](server-paseo-tests-manifest.json)包含 Rust 文件、函数、行号，上游文件、
真实测试标题、行号，以及 adapted / derived 分类。所有上游标题已在固定源码中校验，
共关联 59 个上游文件中的 313 个不同测试标题；关联不等于逐一完整移植。

| 范围 | 新增 | 主要验证内容 |
| --- | ---: | --- |
| server-provider | 146 | Codex transport、审批、恢复、流式生命周期、原生 session 导入、timeline 游标与搜索、fork context |
| server-filesystem | 127 | Git diff/commit/discard、PR/Forge/CLI、文件搜索和上传、Skills、Worktree 创建与恢复、轮询争用与取消 |
| server-terminal | 63 | 独立订阅、恢复边界、尾部输出和退出、尺寸所有权、真实 PTY 与短读写 |
| server-metadata | 52 | Workspace 自动化、目录、注册表、Labels 事务和并发归档 |
| server-schedule | 50 | 修改与执行并发、持久化故障、重启恢复、截止次数、跨时区和 DST |
| server-browser | 29 | Host/Tab 路由、租约隔离、超时/取消、容量与结果 schema |
| server-api | 23 | 连接级请求/订阅所有权、能力协商、WebSocket 分片与控制帧、Origin |
| server-voice | 21 | 单次转写提交、静音、取消、晚到输出、Dictation finish 与回放缓存 |
| server-model | 8 | 物理发送队列预算、背压、取消、字节许可释放与前后台任务预算隔离 |
| server-bin process | 10 | 真实 server 的多连接 Terminal / Timeline / File Upload 测试 |

## 测试驱动的修复

- 相同 Terminal 查询现在创建独立订阅 ID 和输出 slot；每个订阅计入预算，释放一个 ID
  保留其他观察者。专用 unsubscribe 仍释放当前连接内的全部匹配订阅。契约修订记录在
  [ADR-033](../decisions/adr-033-server-terminal.md)。
- Timeline 的 before 游标越过尾部时返回尾部窗口，after 游标超出保留历史时明确 reset。
  Codex 已完成项在容量边界仍幂等；Session 列表排除嵌套来源的子 Agent；Skills 拒绝无效 cwd。
- Fork context 合并相邻、同原生消息且连续的 assistant 片段，保留内部空白和 Unicode；
  遇用户、工具或独立消息即保留顺序边界。完整附件连同头尾标签一起计入 512 KiB 限制。
  已存储的原生历史不变。交叉复审发现并纠正了中间实现跨消息合并造成的重排。
- 未产生首个 commit 的 Git 仓库可以生成 diff，并使用仓库自身的 SHA-1/SHA-256 空树标识；
  含空格路径的 diff 去除 Git 的尾部元数据分隔符；路径 suffix 搜索按完整路径段匹配。
- 多个 diff 订阅使用独立、容量为 1 的后台执行许可并公平等待，避免反复跳过同一个
  观察者；排队取消立即撤销等待，已开始的 Git 操作继续被追踪，结束后检查取消并丢弃快照。
  前台请求保留原有许可，不被后台等待队列占满；同一 Checkout 服务的锁仍会串行操作。
  运行资源修订记录在 [ADR-037](../decisions/adr-037-server-model-context.md)。
- Worktree 路径比较在子路径尚不存在时仍解析真实前缀，统一 `/var` 别名，并正确处理
  指向现存外部目录的符号链接及 `..`。已存在的完整路径保持原来的快速路径。
- Labels 提交只合并 labels 和更新时间，保留并发修改的其他字段；在注册表锁内取得回滚前
  状态并重新检查 assignment 目标，包含 no-op assignment。Catalog rename/delete 仍可处理
  archived 工作区。Catalog 和 prepared/committed journal 均在首次写入前验证完整 4 MiB 限制，
  防止成功写出重启拒读的数据。Catalog 同时验证规范化后的非空、唯一标签名。

回归证据包含旧实现失败、新实现通过；锁内 journal before-image 的测试使用实际提交失败和
锁内回调断言，不声称复现了随机线程调度。中间的断言、编译和 lint 问题均以最终验证为准。
Diff 公平排队用例通过手动 poll 确认等待位置，再逐个释放受控读取；前台隔离单测验证独立
预算，subscribe 使用该预算的接线另经源码复审。执行中取消测试验证最终静默、任务 drain
和 permit 回收，不声称 cancel 与同步 send 之间存在可撤销已入队消息的线性化屏障。

## 范围与差距

外部能力通过 fixture-owned Python/CLI、临时本地 Git 仓库、SQLite、loopback WebSocket/HTTP、
真实本地 PTY 验证；测试的 Git 写入仅作用于临时仓库，真实付费模型和 GitHub 账户不参与。

- Rust 当前的前台文件与目录请求共享单个 blocking admission slot。上传隔离测试同时保持多个上传打开，
  但逐帧使用 ping fence 等待处理完成；不声称支持 Paseo 的并发磁盘写入队列。
- Rust Origin 策略信任监听地址及 localhost；不同 loopback 地址需要显式许可。
- Fork context 的旧 checkpoint 投影变化校验、同一工具调用的进度/最终状态合并，以及按投影
  message 计数仍与 Paseo 有差异。新增测试不等于完成整个 projected timeline。
- Forge resolver registry / SSH alias 探测、状态和 timeline cache、批量轮询及合并策略事实
  校验未因本轮测试而新增。低层 Worktree 测试不表示已对齐首个 Agent 自动命名等服务能力。
  指向不存在目标的 dangling symlink 仍保留既有的词法回退边界，本轮路径修复没有覆盖它。
- Voice 仍是有界整段转写；未实现原生流式 STT 分段、置信度过滤和 Paseo 的分段拼接语义。
  Browser 测试覆盖现有 broker/protocol，不代表真实浏览器扩展或 Agent 工具注入已联调。

## Test coverage

当前全 workspace 行覆盖率 **84.2707%（49,911 / 59,227 行）**，
较可比较历史基线提高 **0.9453 个百分点**。
其中 `bins/server` 和所有 `server-*` crate 合计
**91.0194%（26,483 / 29,096 行）**，
较基线提高 1.8428 个百分点。

[可评审覆盖率 artifact](server-paseo-tests-coverage.json)记录逐 crate 计数、变更文件覆盖率、
测试执行统计、源文件哈希和基线；[HTML](../../target/llvm-cov/html/index.html)为本机生成的逐行报告。
可共享的数据保存在前面的 JSON 中，不依赖本机 HTML 路径。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 较基线（百分点） |
| --- | ---: | ---: | ---: |
| server-provider | 6,767 / 7,264 | 93.16% | +0.85 |
| server-filesystem | 7,519 / 8,566 | 87.78% | +2.91 |
| server-metadata | 5,889 / 6,609 | 89.11% | +1.02 |
| server-terminal | 1,289 / 1,436 | 89.76% | +2.07 |
| server-api | 1,049 / 1,093 | 95.97% | +0.09 |
| server-browser | 707 / 726 | 97.38% | +9.50 |
| server-schedule | 802 / 820 | 97.80% | +3.78 |
| server-voice | 1,218 / 1,275 | 95.53% | +2.04 |
| server-model | 296 / 310 | 95.48% | +0.66 |
| server-bin | 769 / 819 | 93.89% | +0.00 |

测量基于 `5e9fc8a759c886fb78e3212ec681391ed97318e7 + working tree`；最终源文件集合的 SHA-256 为
`a32843dd80ec523139d2f50c85975d311b72316ffcc2028d425bdd6e09200abe`，完整输入列表和每文件哈希在 artifact 中。
范围为 macOS arm64 上的完整 Cargo workspace，默认 features，未添加 source exclusion。
使用 cargo-llvm-cov 默认文件过滤；其统计仍包含两个 `test_support.rs` helper 文件。
Doctest 由普通 `cargo test` 运行，没有启用不稳定的 doctest instrumentation。
Linux / Windows 未执行。

精确验证命令：

```sh
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline -p server-api -p server-bin -p server-browser -p server-domain -p server-filesystem -p server-metadata -p server-model -p server-protocol -p server-provider -p server-schedule -p server-terminal -p server-voice --no-fail-fast -- --test-threads=1
cargo llvm-cov --offline --workspace --html --no-fail-fast -- --test-threads=1
cargo llvm-cov report --html
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-server-tests-coverage-summary.json
```

测试执行与覆盖率分开统计：所有 12 个 server package 的普通运行 **1,094 passed / 0 failed / 0 ignored**；
instrumented workspace 运行 **1,585 passed / 0 failed / 5 ignored**。
完整 workspace 由上述 cargo llvm-cov 调用 cargo test 验证。Rustfmt 和严格 Clippy 检查通过。
5 个原有 ignored 测试分别依赖真实 Codex、付费 DeepSeek 或
外部构建 worker；名称与原因均保留在 artifact 中，没有增加 ignored 测试。

并发修复前启动的一轮普通 workspace 回归已被最终验证替代，未完成结果不计为通过；
记录保留在 artifact。首次 instrumented workspace 中的 diff observer 超时已精确复现，
通过独立后台预算和取消修复处理，并增加 6 个确定性用例；没有扩大原真实 Git 用例的等待上限。

比较基线：[app-rust-server-coverage.json](app-rust-server-coverage.json)，
49,242 / 59,096 行（83.3254%），同平台、默认 features 和测量范围。
这是历史测量，本轮未重新运行旧版本；其记录的五个生产文件哈希与本轮起始 HEAD 相同。
覆盖率增量包含修复引起的源码行数变化，不仅是新增测试命中的旧行。

已有代码中，Directory RPC（71.29%）、Project icon 存储（63.21%）和 Forge RPC（73.36%）
仍有未覆盖分支；后续优先补排序/分页、GIF/JPEG/SVG 与图标发现、嵌套检查结果和错误投影，
以及 Git pull 冲突清理的针对性 fixture。

仍未覆盖真实模型服务的协议变化、审批后断连恢复、Windows 进程树和 PTY 行为。
Codex UTF-8 fixture 验证逐字节写入和 CRLF 的往返结果；pipe 可能合并读取，未声称
确定性覆盖每个 read boundary。其余功能差距见上文，不以覆盖率替代功能对齐结论。
