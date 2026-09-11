# ADR-001：仅名称创建 Project 的目录分配边界

- 状态：Accepted
- 日期：2026-09-11
- 来源：NEC-195
- 修订：补充 NEC-150 v4、NEC-149、ADR-006 与 NEC-224；不修改 Project 持久化结构。

## 决策

`register_project` / `POST /v1/project/register` 的 `workdir` 改为可选字符串：
省略或 JSON `null` 表示新建 `<Documents>/<name>`；显式字符串仍表示注册已有目录。
显式空字符串仍按原路径校验拒绝，不触发默认目录。`id`、`name`、`repo_url` 的原有
校验先于目录分配。目录命名约束仅作用于省略 workdir 的路径，不限制已有目录的展示名称。

实际公开调用链为 Desktop renderer → preload IPC → Electron main → daemon HTTP →
`LocalControlService` → `ControlStore::apply`。CLI 使用 NEC-241 的实体入口
`project register --id <id> --name <name> [--workdir <path>]`，同样走 HTTP；省略 `--workdir`
构造 `None`，不恢复已退役的通用 `command` 入口。Rust IPC 共享 `Command` DTO。
较早的 `ProjectService` / `ProjectRegistration` 是显式目录服务，
此次不改变其 API，也不将它误认为公开控制入口。

`ports::ProjectDirectoryCreator` 定义一次性、不可复用的目录分配能力。
`project-local::DocumentsProjectDirectory` 实现平台路径解析与文件名校验；daemon 装配
该实现。application 只依赖端口，不依赖 dirs、Electron 或特定平台 API。domain 仅增加
稳定错误码，不新增依赖。测试通过 `with_resolver` 注入临时路径，不修改全局环境或接触真实 Documents。

平台解析采用 [`dirs::document_dir`](https://docs.rs/dirs/6.0.0/dirs/fn.document_dir.html)：
Linux 遵循 XDG user directories（支持自定义/本地化位置），macOS 使用用户的 Documents，
Windows 使用 `FOLDERID_Documents`（支持已重定向的 Known Folder）。解析失败、返回相对路径、
父目录不存在/不是目录/无法访问时拒绝；不回退到 cwd、HOME 或临时目录，也不创建 Documents 本身。
父目录先 canonicalize，可接受操作系统配置的目录链接；当前字符串传输契约不支持非 UTF-8 父路径。

名称原样作为一个组件使用，不替换字符或重命名。拒绝空名、`.`/`..`、斜杠/反斜杠、
控制字符、Windows 特殊字符及设备名、前后空白、尾随点和超过 255 UTF-8 字节的名称。
合法 Unicode 和内部空格保留。上述可移植目录规则位于本地适配器，domain 不理解平台文件名。

目标使用单次 `create_dir` 独占分配；禁止 `exists → create_dir_all`、复用、覆盖或递归清理。
已有目录、普通文件、链接（包括悬空链接）均返回 `PROJECT_PATH_ALREADY_EXISTS`。
同名并发请求只有一个能成功分配。

分配后沿用已有 Git-root 校验、`git init`、manager-owned 空初始提交与 HEAD 捕获流程。
只有 Git 准备成功才在一次 SQLite 事务中提交 Project、根 Message 和注册事件。
CAS 重试保留本请求分配的路径，不重复 mkdir；其他请求不能获得这种复用资格。
SQLite schema、Project/Message 结构、archive 版本和事件结构不变。

## 失败与恢复

| 失败阶段 | 结果 | 文件系统与持久化 |
| --- | --- | --- |
| id/name/repo_url 或名称组件无效 | `INVALID_PROJECT` | 分配前拒绝，无新增目录或记录 |
| Documents 无法解析或不可用 | `PROJECT_DEFAULT_DIRECTORY_UNAVAILABLE` | 不创建父目录，不选择替代位置 |
| 目标已有条目 | `PROJECT_PATH_ALREADY_EXISTS` | 不执行 Git，不改写目录、文件或链接，不注册 Project |
| mkdir 权限/I/O 失败 | `PROJECT_DIRECTORY_CREATION_FAILED` | 不进行 Git 或持久化 |
| Git 初始化/根校验失败 | `PROJECT_GIT_INIT_FAILED` | 保留本次目录及其全部内容，不注册 |
| 初始提交/HEAD 失败 | `PROJECT_GIT_HEAD_UNAVAILABLE` | 保留目录和可能的部分 Git 元数据，不注册 |
| SQLite 写入失败或 CAS 重试耗尽 | 原存储错误码 | 原子回滚 Project/Message/事件；保留已准备的 Git 目录 |

文件系统与 SQLite 无跨资源事务。分配成功后的错误包含 `Directory retained at <path>` 与恢复提示，
并设置 `retryable=false`，防止客户端盲目重试并误认为可以覆盖。用户应检查保留目录，选择另一个名称，
或明确指定该目录恢复注册。绝不自动删除可能被其他进程写入的内容。进程崩溃可能留下未注册目录；
若数据库已提交而响应丢失，目录仍保留，用户先查询 Project 列表再决定恢复操作。

Desktop 只负责可选路径输入、展示提示和转发后端错误；不在 Electron 创建目录或执行 Git。
保留原有“选择目录后自动取目录名”的交互。默认 Agent 设置仍是注册成功后的独立请求；
注册失败不会发送该请求，注册成功后默认 Agent 设置失败的既有恢复行为未改变。

## 验证与限制

- `ait-project-local`：临时目录下成功分配、Unicode/空格、文件/目录/链接冲突、并发分配、
  无效名称、不可用 Documents、Unix 权限失败；真实 Git 与 SQLite 验证注册、显式目录兼容、
  Git init/HEAD 故障、CAS 重试/耗尽和存储失败后的无半注册/内容保留。
- HTTP 测试覆盖 `null` 与稳定错误信封；CLI WF-01 覆盖省略路径、冲突无副作用与重启恢复；
  Desktop 测试覆盖表单输入到 HTTP body 的省略语义、显式路径保留与错误传播后停止后续写入。
- 继承 NEC-149 的非对抗性本机路径访问基线：普通 canonical path + Git subprocess 不能抵御
  恶意本地进程在检查后替换祖先目录的 TOCTOU；未引入跨平台目录句柄沙箱。
- 自动化使用注入路径验证默认目录边界，不调用真实用户 Documents。原生 XDG/macOS/Windows
  resolver 与打包桌面 GUI 需要在各目标系统另做验收。
