# Codex 列表重复 Thread ID 修复

## 原因与改动

`Pull from Codex` 报 `Codex thread/list returned a duplicate Thread id`，是 Ait 协议适配器
把重复身份视为错误并中止整个发现列表。ADR-016 已规定必须允许重复页：一次分页不是一致性
快照，会话更新或在未归档/归档目录间移动都可能造成重叠。截图本身不能确定这次重复来自哪种
重叠，但足以触发原实现的错误分支。

适配器现在按 Thread ID 合并，保留首次出现的位置并采用最后一次观察到的摘要、归档状态和
原生 metadata，不因时间戳相同或回退而忽略后续摘要。重复项不阻止继续分页，整页都重复时
也继续使用返回的 cursor。空身份、无效响应以及重复 cursor 仍然报错。

分页改用 `created_at` 正序，减少活跃会话 `updatedAt` 变化引起的位置变化；application
仍按更新时间排序显示。创建时间排序减少波动，并不提供快照一致性。完整历史读取和身份绑定
规则没有改变，列表摘要也不会发布为完整 Turn。

本机 `codex-cli 0.153.4` 生成的契约包含 `sortKey`、`sortDirection`、不透明 cursor 和
`archived` 两组过滤；同时核对了 [OpenAI Docs 的 app-server 文档](https://developers.openai.com/zh-Hans/docs/app-server)。
文档支持这些请求参数，但没有承诺分页列表全局唯一；重复记录兼容策略来自 ADR-016 以及本次
观察到的实际错误，不能把它描述成服务端提供了跨页一致性保证。

## 回归

新增 5 项协议 fixture，覆盖同页重复且时间戳不变、跨页重叠/整页重复且时间戳回退、扫描期间
归档移动、重复记录不能掩盖游标循环、空身份不能被去重吞掉。原有来源筛选、归档状态和完整
历史读取测试继续保留。针对性的 `cargo test -p ait-agent-adapters --test codex_protocol`
为 27 通过、0 失败。

另用新的本机 debug 构建启动临时 Ait daemon（独立临时 catalog），调用 HTTP 原生列表接口，
通过真实 worker 访问本机 Codex 历史：返回 168 条会话、168 个唯一 ID，包含 1 条归档记录。
只保留数量，不记录原生 ID、标题或内容；没有注册用户项目、导入会话、修改 Ait 原数据或启动
模型回合。临时 daemon 已停止，临时 catalog 已删除。此验证证明修复后的真实发现链路可用，
不证明每次动态扫描都能穷尽所有会话。

## Test coverage

普通工作区 `cargo test --workspace --no-fail-fast`：460 通过、0 失败、5 忽略。
`cargo build --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo fmt --all --check` 和 `git diff --check` 通过。
本次没有修改 Desktop 源码，不重复执行其 140 个单元测试和 69 个浏览器测试；这些数量是
前一轮项目菜单功能的通过结果，不计为本次重新运行的结果。

`cargo llvm-cov --workspace --html`：459 通过、0 失败、5 忽略；与普通运行的差额为
1 个 doctest。行覆盖率与测试通过率分别统计：

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对修复前同工作区测量 |
| --- | ---: | ---: | ---: |
| Cargo workspace | 21,458 / 27,727 | 77.39% | +0.02 个百分点 |
| agent-adapters | 2,016 / 3,349 | 60.20% | +0.17 个百分点 |

修改文件 `codex/protocol.rs` 为 1,150/1,458（78.88%）。测量范围为 macOS arm64、默认
features、完整 Cargo workspace；Rust 1.98.1、cargo-llvm-cov 0.8.4。无手动源码排除，
工具不计测试文件和 doctest，也不含 Desktop TypeScript。Linux/Windows 未实测；4 项
需要真实模型的测试和 1 项需要外部 worker 的测试仍忽略，名称列在摘要中。

基线为 `939d5fd304afa23113f661ed8d9cef177ac88453` 上的当前修改，源码 SHA-256 为
`a09741244be770a74afbbb1ae17956576c1f59436b18b1141cc04c2a6824ea3e`。
[覆盖率摘要 JSON](codex-thread-list-deduplication-coverage.json)包含范围、源码指纹算法、所有
crate 与修改文件的行数、测试结果及忽略列表。HTML 位于 `target/llvm-cov/html/index.html`；
导出命令为 `cargo llvm-cov report --json --summary-only --output-path /tmp/ait-codex-list-fix-coverage.json`。
比较使用[修复前项目菜单测量](codex-project-import-coverage.json)，对应相同基线提交上的前一版
工作区；没有重新测量干净 HEAD，行数总体也已改变。

workspace 与 adapter 整体仍低于 80% 目标。尚未系统测量持续并发归档/写入的服务端组合，
也未补齐其他协议操作的全部 I/O 异常分支；后续可做并发扫描及故障注入验收。既有 worker
强制退出导致 profile 未刷写的限制仍在，真实子进程测试和本机列举通过不能代替这些行的
覆盖率记录。未排除相关源码或调整百分比。
