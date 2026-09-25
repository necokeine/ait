# Skills 接口实施报告

基于 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`，本轮实现 Skills 全部五个接口；独立 server 的 Paseo 规范方法处理器从 152 增至 **157/202（77.72%）**，占位从 50 减至 **45**。加上七个自定义方法，生产处理器为 **164/209**。数量表示有业务实现，不表示所有上游细节均等价。

## 已实现行为

| 接口 | 行为 |
| --- | --- |
| `agent.skills.get_status.request` | 返回 selection、available、installed、ops 与 not-installed / up-to-date / drift 状态 |
| `agent.skills.reconcile.request` | 修复三个目标中的缺失或变化文件，不执行待确认的删除 |
| `agent.skills.uninstall.request` | 删除当前 bundle 与四个上游 legacy 名称对应的已安装目录，保留选择和无关技能 |
| `agent.skills.save_selection.request` | 规范化选择，返回待确认删除名单；确认后同步文件、持久化选择、返回新快照 |
| `agent.skills.import_legacy_selection.request` | 仅在尚未保存选择时导入；去空白、去重、排序，不安装文件 |

默认选择 all 不自动写盘；custom 的未知名字保留在选择中，但不当作路径或安装项。缺失目标优先于内容差异，安装状态与操作列表按名称排序。普通 status 不创建安装目录；若存在中断事务，status 会先执行恢复。

安装保留目标中的用户额外文件；与 Paseo 一样，bundle 内的同名文件会覆盖目标内容。`.paseo-managed-files.json` 记录 SHA-256；旧 managed 文件仅在内容仍匹配旧哈希时删除，用户修改的过期文件保留。更新保留既有文件和目录的普通权限位。卸载或确认移除会删除整项技能目录，包含其中的额外文件。

选择与事务日志使用临时文件、同步和原子替换写入。安装先写目标旁暂存目录，再备份并发布；未提交事务在下次请求恢复旧选择与目录，已提交事务仅清理备份。发布前检查目录内容指纹；外部修改导致无法安全恢复时保留日志与备份并报错。

## 宿主配置

- `AIT_SERVER_SKILLS_BUNDLE`：绝对路径，默认 `<data-dir>/skills-bundle`。其直接子目录是技能名称；无目录时 catalog 为空。本轮没有附带技能包。
- `AIT_SERVER_SKILLS_HOME`：绝对路径，默认 `HOME`；HOME 缺失时使用 `<data-dir>/agent-home`。三个目标为该目录下 `.agents/skills`、`.claude/skills`、`.codex/skills`。
- 持久状态在 `<data-dir>/skills-state`，目标旁暂存目录名为 `.ait-skills-<uuid>`。不要在未检查日志与备份前手工删除冲突恢复现场。
- 生产进程测试使用临时 HOME，并清除继承的 Skills 配置变量，未写入开发者真实技能目录。

## 上游对应测试与差异

对照 `packages/server/src/server/orchestration-skills/internal/` 的 controller、operations、sync、selection-store、paths 实现与测试，移植了以下行为组：

| 上游行为组 | 本轮回归 |
| --- | --- |
| 三目标状态、缺失优先、次级目标漂移、无关技能 | local/skills/tests.rs |
| 保存确认、默认 all、custom 空集合/未知项、卸载后恢复选择、导入一次 | local/skills/tests.rs、真实 WS 测试 |
| 用户文件、managed manifest、过期文件、越界路径 | local/skills/tests.rs |
| 删除/更新/新增中断、提交后清理、旧选择恢复、重复恢复 | local/skills/transaction/tests.rs |
| 外部修改、新删除项在发布时出现、恶意恢复记录 | local/skills/transaction/tests.rs |
| 严格参数验证、存储错误传播 | service/skills/tests.rs |

没有声称移植了上游每一个测试。尚未等价的行为：

1. **配置存储与资源发现**：Paseo 使用 daemon `agents.skills.selection` 并从源码/打包布局发现自带技能；本实现使用专属 JSON 和显式 bundle 根目录，不接入通用 daemon config，也不附带依赖 Paseo CLI 的原技能文本。
2. **启动收敛**：Paseo 可在启动时更新已有安装；这里不自动执行，需调用 reconcile。事务恢复发生于下一次 Skills 请求，而非启动期间。
3. **外部并发恢复**：Paseo 有有限重规划重试、文件合并与 quarantine；这里拒绝变化中的计划，冲突时保留现场并返回 resource_exhausted/registry_io，需要处理冲突后重试。未移植自动合并、隔离与相关测试。路径检查与 rename 不是防恶意同机进程的原子沙箱，存在检查后竞态；无跨宿主锁。
4. **容量与路径**：每个技能树最多 32 MiB、4096 项、64 层，bundle 最多 256 项。拒绝符号链接和特殊文件，包括用户额外文件；Paseo 的文件遍历通常跳过这些额外项。本轮没有覆盖全部容量极限和所有底层 I/O 故障注入。
5. **文件元数据**：保留字节与普通权限，不保证 inode、mtime、ACL、扩展属性或硬链接关系；过期 managed 文件的空父目录暂时保留。文件/目录类型冲突报错，不自动替换。目录未做 fsync，不承诺突然断电后的磁盘事务保证。
6. **协议包装**：使用独立 server 统一 request/result/error 信封，字段放在 params/result 中；错误是统一安全错误码，不复刻 Paseo 的自由文本异常。

## Test coverage

- 修订：`97f1372` 加当前未提交工作树（包含此前用户修改）；平台 macOS aarch64，默认 features。
- 精确命令：`cargo llvm-cov --offline --workspace --html -- --test-threads=4`。整个 workspace 单元与集成测试 **1,003 通过、0 失败、5 ignored**；未额外排除文件或手工屏蔽测试，使用 llvm-cov 默认过滤。本命令不运行 doctests，其他操作系统未验证。
- workspace 行覆盖率 **82.98%（46,917 / 56,540）**；同口径 Push 基线为 **82.82%（46,260 / 55,858）**，提高 **0.16 个百分点**。
- 本轮 Skills 模块合计 **96.43%（622 / 645）**；这是模块覆盖率，不是接口等价率。
- 可审阅制品：[逐文件覆盖率 JSON](server-skills-coverage.json)。导出命令：`cargo llvm-cov report --json --summary-only --output-path /tmp/ait-skills-workspace-coverage.json`。HTML 位于 `target/llvm-cov/html/index.html`，仅为本地制品。

| 相关包 / 模块 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-filesystem | 7,245 / 8,537 | 84.87% |
| server-api | 879 / 906 | 97.02% |
| server-bin | 499 / 522 | 95.59% |
| Skills service | 82 / 82 | 100.00% |
| Skills protocol | 22 / 22 | 100.00% |
| Skills local adapter | 145 / 149 | 97.32% |
| Skills transaction | 154 / 161 | 95.65% |
| Skills tree | 219 / 231 | 94.81% |

执行结果与覆盖率分开说明：

- 新增 **17 个单元测试和 1 个真实 WebSocket 进程测试**，完整 workspace 运行全部通过；生产 catalog 回归确认 164 个处理器、45 个占位，并逐一验证占位响应。
- `cargo clippy --offline --target-dir target/server-audit-lint --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 均通过。
- `python3 scripts/check-paseo-protocol.py /Users/necokeine/Documents/paseo` 核对固定上游 205 个原始入站名称成功。
- 5 个 ignored 是仓库原有的真实 Codex/DeepSeek 或外部 worker 用例，需要凭据、付费调用或 `AIT_TEST_WORKER_EXECUTABLE`；本轮未启用。
- 初次 Skills 定向测试发现 serde 的 unit variant 接受 all 选择中的额外字段；已改为严格的空对象 variant，最终非法参数回归通过。最终全量没有失败用例。
- 未覆盖主要是容量上限、部分底层 I/O 失败及并发窗口错误分支；Windows、突然断电和恶意同机进程竞态未测试。后续需补受控故障注入、跨进程协调，以及前述 Paseo quarantine/自动合并机制。
