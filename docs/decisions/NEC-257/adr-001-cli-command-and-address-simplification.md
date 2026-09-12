# ADR-001：CLI 命令与连接参数简化

- 状态：Accepted
- 日期：2026-09-13
- 关联：NEC-257
- 修订：NEC-241 类型化实体 CLI

## 决策

CLI 统一以下入口，不保留旧入口的隐藏别名：

| 旧入口 | 当前入口 |
| --- | --- |
| `events [--after <cursor>]` | `event list [--after <cursor>]` |
| `agent-provider <动作>` | `agent provider <动作>` |
| `--endpoint <url>` | `--host <host> --port <port>` |

`agent provider` 支持 `list`、`save`、`discover-models`、`refresh-models`，沿用原有参数、
Command 映射、凭据 stdin 输入与脱敏规则。事件仍使用已有 cursor 回放与 SSE 输出。

`--host` 和 `--port` 是全局参数，可放在任意子命令层级或分别指定，默认值为 `127.0.0.1` 和 `7314`。
CLI 始终构造 `http://<host>:<port>` 作为 daemon 地址；host 接受域名、IPv4、带或不带方括号的 IPv6，
拒绝协议、端口、路径、凭据、query、fragment 和空白；port 只接受 1–65535 的整数。
无效值由 clap 在 I/O 前拒绝。Provider 远端连接的 `--url` 继续支持 HTTP(S)，与 daemon 地址独立。

本次仅调整 CLI 边界；HTTP 路由、application Command、领域模型与持久化格式保持原有契约。
顶层 `export` / `import` 快捷命令继续可用。

## 验证

递归帮助与 Command 映射测试覆盖新命令层级；旧命令和 `--endpoint` 必须解析失败。
地址测试覆盖默认值、单项覆盖、各层级全局参数、IPv4/IPv6、端口边界与非法 host。
真实 CLI 子进程的 HTTP/SQLite 流程全部改用 host/port，继续验证 Provider 保存/发现/刷新/列表、
凭据保护、事件回放/重启恢复及业务错误退出码。真实模型验收脚本的 CLI 参数同步迁移。
