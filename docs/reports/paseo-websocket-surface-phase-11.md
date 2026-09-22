# Paseo WebSocket 接口移植：第十一阶段

- 日期：2026-09-23；分支：`new`；基线：`02df2c7`。
- Paseo 固定来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 接口与统计口径

新增文件与目录分组全部 11 个规范方法。生产 capability 集合与 catalog 去重后的交集为 93 个，另有 11 个
此前独立 server 方法，共发布 104 个 capability。用户给出的 191 个来源名合并为 188 个独立方法，因此
剩余 95 个尚未接通。握手、服务端推送事件、未单独发布 capability 的旧 status 取消入口不在此计数中。
第十阶段报告的 81/110 已更正为当时的 82 个清单方法、106 个剩余方法，避免混用原始条目和独立方法。

| 规范方法 | Paseo 来源名 | 实现 |
| --- | --- | --- |
| `directory.suggestions.request` | `directory_suggestions_request` | workspace/home 搜索，kind/limit/suffix/fuzzy，显式路径读取 |
| `fs.explorer.request` | `file_explorer_request` | 目录列表，text/image/binary preview，binary stream |
| `fs.file.subscribe.request` | 同名 | 初始 ready/missing/error 版本，随后 `fs.file.update` |
| `fs.file.unsubscribe.request` | 同名 | 当前连接的幂等取消 |
| `fs.file.write.request` | 同名 | revision 优先的 CAS 检查、原子替换及权限保留 |
| `fs.entry.create.request` | 同名 | 单层目录和文件的 exclusive create |
| `fs.entry.rename.request` | 同名 | tracked Git rename、untracked filesystem rename、case-only rename |
| `fs.entry.duplicate.request` | 同名 | sibling copy 命名、目录递归复制、碰撞后编号 |
| `fs.entry.delete.request` | 同名 | 删除文件/目录/链接，禁止 workspace root |
| `fs.file.download_token.request` | `file_download_token_request` | 60 秒、一次性、固定 canonical target 的 token |
| `file.upload.request` | 同名 | connection-owned binary upload，完成后返回 uploaded attachment |

旧下划线名称不作为 wire alias。所有 request 继续使用独立 server envelope，Paseo payload 位于 `params` /
`result`，关联 ID 只使用外层 `request_id`。文件帧采用 Paseo 原始 opcode、UTF-8 ID、Begin JSON 长度和字节
布局；文件内容本身不改成 JSON/base64 上传。Binary preview 在 Begin/Chunk/End 成功路径上不额外返回 JSON
response；失败沿原 request ID 返回 explorer error。Upload 在 End 完成时返回 correlated response。

## 实现边界与对齐行为

`server-ports::files` 新增纯阻塞文件接口、version、reader 和 upload writer。`server-workspace::LocalFiles`
实现路径和文件操作；`server-application::Files` 拥有有界的一次性下载授权集合；`server-api::files` 负责
DTO 投影、connection-owned 订阅、binary 传输和 HTTP 附件下载。生产 binary 显式组装，没有复用旧 Ait 组件。

对照来源为 Paseo `file-explorer/service.ts`、`observer.ts`、`session/files/workspace-files-session.ts`、
`file-upload/index.ts`、`file-download/token-store.ts`、`utils/directory-suggestions.ts`、protocol messages
及 `binary-frames/file-transfer.ts`，对应测试同目录可定位。

- 文件路径支持绝对路径和 tilde，同时检查 lexical path 与 realpath 的 workspace 范围。缺失末端路径也检查
  已有父目录，避免目录 symlink 越界；regular-file 读取在 Unix 使用 `O_NOFOLLOW | O_NONBLOCK`。
- 文件 revision 在 Unix 保持 `dev:ino:size:mtimeNs`。UTF-8/binary 判断分块扫描完整文件、保留跨块 UTF-8
  尾部；null/control byte 和不完整 UTF-8 被识别为 binary，image extension 与 JSON MIME 按 Paseo处理。
- 文本写入拒绝 missing、binary 和超出 1 MiB 的内容，优先比较 revision，回退比较显示 mtime；临时文件继承
  权限、sync、再次检查 revision 后 rename。目录 mtime 降序，同 mtime 按名称排序。
- 新建使用 exclusive create，禁止 separator name；rename 对 tracked entry 调 `git mv`；duplicate 使用
  `name copy.ext`、`name copy 2.ext`；删除最终 symlink 时不删除其目标。
- 文件订阅返回初始状态，变化时只推送 version。订阅独立于其他连接；同 ID 替换、显式取消、通用 release、
  disconnect 和 server drain 都停止推送。取消与发布共用短临界区，确认取消后不会再入队该订阅的新事件。
- Upload 请求与帧只在同一物理连接配对。Begin 前的 chunk/end、重复 Begin、超出声明长度和短传输均失败；
  未完成文件由 RAII 清理，End 成功才保留附件。空闲时间沿用 Paseo 10 分钟，每帧/请求和空闲 timer 清理。
- Download token 60 秒有效，先消费后重新打开文件，不回收失败 token。签发时固定 canonical target 和原始
  下载名，因此改变来源 symlink 不能把 token 改指另一个文件。HTTP 只对该独立 token 免 server bearer，
  Host/Origin 校验保留，日志不记录 query，响应含 `no-store` / `nosniff`。
- Binary preview/HTTP 以 256 KiB chunk 读取同一个句柄，结束时检查 revision，文件增长、缩短或原地改写导致
  传输失败。WebSocket binary 复用现有消息数和 byte queue budget，等待背压；文件 I/O 都在 tracked blocking
  job 中运行，upload/frame 处理及 file search/entry mutation 使用现有 admission semaphore。

## 与 Paseo 尚未对齐的内容

1. 原版 file observer 共享 canonical target 的 parent native watcher，50 ms debounce，失败时 5 秒轮询。
   当前每个订阅采用 200 ms metadata polling，没有共享 native watcher；同一轮内的多次修改可能合并，繁忙时
   因全局 blocking permit 暂不可用而延迟检查。对应 watcher sharing/debounce 测试未声称通过。
2. 搜索保留默认参数、bounded BFS、workspace/home 路径形式、忽略目录和允许遍历的隐藏配置目录，但 fuzzy
   score 是当前 Rust subsequence/substring 排序，未逐行移植 Paseo `scorePathMatch` 的 tier/segment/offset
   细节、Unicode locale 比较、home 早停启发式和 8 秒 cache。Git ignored-path lookup 使用新 server 已有
   bounded Git runner（3 秒、16 KiB），失败时忽略列表为空；大 ignore 集合的 discovery 结果可能不同。
3. 当前 directory list 最多返回 20,000 项；duplicate 最多 20,000 项、64 MiB、64 层；文本 preview 上限
   512 KiB；upload 上限 64 MiB、每连接最多 8 个 pending；download grant 最多 256 个；classification 有
   30 秒预算。原版这些文件行为多数没有相同总量限制。原有 WS 1 MiB 消息上限也约束 JSON write 请求。
4. Unix revision 与原版一致；非 Unix 回退 size/modified/created token，未复刻 Windows file identity。
   本阶段在 macOS 验证，Windows symlink duplicate 明确拒绝，Windows rename/permissions 未验收。
5. Rust 标准库的 OS 错误文案、同时间文件名排序、无效 UTF-8 路径显示不与 Node/locale 保证字节级一致。
   Copy 保留文件 mtime/permissions 和 symlink 本身，未完整复制目录时间戳/所有平台扩展属性；失败的递归复制
   可能留下已创建的部分 sibling，原版也不提供整棵目录复制事务。
6. Upload 临时目录为 `uploads/<upload-id><随机后缀>/`，返回的 attachment path 为实际位置，逻辑 ID 与原版
   同为 `upload_<uuid>`。当前对重复 Begin 和 End-before-Begin 明确报错；原版 Begin 可再次截断、零字节
   End-before-Begin 可能返回不存在路径。采用较严格状态机，End-before-Begin 的差异已有进程测试；重复
   Begin 的拒绝分支尚未单独覆盖。超时清理的空闲检查最多延迟 30 秒；已完成附件无自动 TTL 删除。
7. Download 重新验证 canonical scope，并完整分类可读内容，较原版保存 absolutePath 后仅用 nofollow 打开的
   行为更严格。HTTP 错误响应目前为纯文本，原版为 `{ error }` JSON；下载名中的非 ASCII 字符会替换为
   `_`，尚未提供国际化文件名 header。不支持原版客户端 Session source 引用，只支持独立 server 物理连接
   所有权。
8. 尚未实现跨进程或外部文件写入者的锁：原子写入的最后一次 stat 与 rename 间仍有竞态窗口。祖先目录也没有
   逐级 descriptor anchoring；当前 realpath + 最终 nofollow 不构成对恶意并发目录替换的完整 OS sandbox。
   此项不宣称比原 Paseo 路径实现具备更强的隔离。

## 对应测试

移植的 service / POSIX 行为包括：文本、未知扩展名、JSON、图片与 binary 判断，跨分类块 UTF-8、后段 null
和不完整 UTF-8；read revision 在 grow/shrink/overwrite 后失效；atomic edit、mtime fallback、revision 优先、
stale/missing/binary/large edit；权限保留；create collision 和非法名称；nested copy 与 numbered siblings；
tracked/untracked/case-only rename；禁止 root/outside/delete missing；dangling/outside symlink listing、作用域内
symlink 读取与链接删除；tilde scope；mtime/name 排序；搜索 kind/blank/suffix/hidden/exact path。

协议和应用测试覆盖 camelCase、discriminated unions、null/omission、非法请求、Paseo binary layout、截断帧、
非法 opcode/metadata/size；token single-use/expiry/capacity；upload slot limit、同 ID 替换和空闲回收。
真实 binary 进程测试覆盖全部 11 个方法、持续的 ready/missing/recreated version、另一连接无法取消订阅或写
入 upload、generic release 后静默、大文件 multi-chunk、inline text/image/binary、maxBytes、stale edit、
HTTP one-use token、来源 symlink 改指、实际目标被 symlink 替换、目标删除、upload oversized/early End
及断线 partial cleanup。

## Test coverage

测量版本为 `02df2c7` 加本阶段提交中的 Rust 变更，平台为 macOS 26.6.2 / arm64、rustc 1.98.1、
cargo-llvm-cov 0.8.4。范围为整个 Cargo workspace 的默认 features、231 个生产 Rust 文件，使用工具默认的
测试文件/build script 过滤，无额外文件排除；覆盖率运行不含 doc tests。Linux 和 Windows 未运行。

| 范围 | 覆盖 / 总行数 | 行覆盖率 |
| --- | --- | --- |
| 整个 workspace | 37,641 / 46,634 | **80.72%** |
| 独立 server 的 8 个 package | 14,213 / 16,503 | **86.12%** |
| 本阶段新增可执行文件模块 | 1,519 / 1,641 | **92.57%** |
| server-bin | 363 / 376 | 96.54% |
| server-api | 4,207 / 4,866 | 86.46% |
| server-application | 2,738 / 3,122 | 87.70% |
| server-domain | 219 / 219 | 100.00% |
| server-ports | 19 / 19 | 100.00% |
| server-protocol | 622 / 743 | 83.71% |
| server-storage | 1,734 / 1,873 | 92.58% |
| server-workspace | 4,311 / 5,285 | 81.57% |

独立 server 与第十阶段同口径的 12,634 / 14,798（85.38%）相比提高 **0.75 个百分点**。第十阶段只测量了
server packages，没有可比较的整仓覆盖率基线。新增模块统计包含 API、application、port、binary framing、
LocalFiles/search/upload 的 9 个生产源文件；纯 DTO 文件 `server-protocol/src/files.rs` 没有独立可执行行，
其序列化行为通过协议和进程测试验证，未把它虚计为 100% 覆盖。

测试执行结果与覆盖率分开记录：

- 普通整仓回归包含 72 个普通 target 和 24 个 doc-test target：**788 通过、1 首次失败、5 忽略**。
  失败为未修改的旧 daemon 测试 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`，
  触发 15 秒 readiness 断言；当时覆盖率构建并行进行，构建负载是可能原因，未认定为已证实根因。
- 使用同一个已构建测试 binary 单独复跑该用例：**1 通过、0 失败**。随后完整插桩回归也通过该用例。
- 完整 workspace 覆盖率运行：**788 通过、0 失败、5 忽略**（不含另行通过的 1 个 doc test）。
  独立 server **297 项全部通过**，比第十阶段新增 **29 项**。
- 5 项忽略测试沿用已有标记，涉及真实 Codex/DeepSeek 凭据、付费模型调用或显式提供的外部 worker；
  名称及原因记录在 coverage artifact。没有新增忽略测试。
- 整仓默认 features 的严格 clippy、6 个改动 server package 的 all-features clippy、format 和 diff check
  均通过。

执行命令：

```sh
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast -j1
# 从 bins/daemon 目录复跑首次超时的现有测试 binary，保留整仓构建的 feature 组合：
/tmp/ait-phase10-workspace-target/debug/deps/codex_http-90713081ac6f2977 daemon_is_ready_and_rejects_unsent_native_recovery_without_replay --exact --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --json --summary-only --output-path /tmp/paseo-ws-phase11-workspace-coverage-raw.json --no-fail-fast -j1
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy -p server-protocol -p server-ports -p server-application -p server-workspace -p server-api -p server-bin --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
git diff --check
```

可评审的 [coverage artifact](paseo-websocket-surface-phase-11-coverage.json) 随提交保存各 crate / 新增模块的
原始行数、基线、命令、测试结果和限制。完整本机 HTML 位于
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`，原始 JSON 位于
`/tmp/paseo-ws-phase11-workspace-coverage-raw.json`。

已查看未覆盖行：主要剩余 OS I/O 失败和并发目标替换分支、classification 超时、大目录/复制限额耗尽、
duplicate Begin、upload 根目录 symlink 拒绝、HTTP 多余参数/服务不可用/流中断及非 ASCII 文件名替换。
后续应补故障注入和持续 binary 流下的取消/队列压力用例，并在移植 native watcher、完整搜索排序时补齐
Paseo 对应测试。当前行覆盖率不代表这些未移植行为或其他平台已验收。
