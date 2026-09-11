# WF-09：在脚本中判断成功与错误

用户目标：让脚本可靠判断参数错误、服务不可达、业务拒绝和运行失败，避免误报完成或重复写入。
前置条件：完成公共演练准备；失败示例只针对本次演练数据。

## 操作

```bash
ait --help
ait session send --help
ait run get --run-id missing > "$WF_ROOT/error.json"
COMMAND_STATUS=$?
printf 'exit=%s\n' "$COMMAND_STATUS"
cat "$WF_ROOT/error.json"

ait cron trigger --cron-id cron-daily --scheduled-at invalid > "$WF_ROOT/invalid.out" 2> "$WF_ROOT/invalid.err"
COMMAND_STATUS=$?
printf 'exit=%s\n' "$COMMAND_STATUS"
cat "$WF_ROOT/invalid.err"
```

## 当前输出约定

| 情况 | 退出码 | stdout | stderr / 下一步 |
| --- | --- | --- | --- |
| `--help` | 0 | 帮助文本 | 空；按当前帮助构造参数 |
| 成功实体操作 / project import | 0 | `ok=true` JSON 信封 | 空；继续检查业务 payload |
| 成功 export | 0 | 空 | 空；从指定文件读 archive |
| 成功 event list | 0 | SSE，可为空 | 空；按事件 cursor 续读 |
| 业务拒绝 | 2 | `ok=false`、稳定 error code | 空；根据 code 修正输入或冲突 |
| 未知/缺失子命令、缺少必填 flag、非法 enum/时间戳/游标 | 2 | 空 | 参数诊断和用法 |
| 实体 `--input` JSON 损坏或形状错误 | 1 | 空 | 解析诊断；修正 JSON |
| import 文件缺失/损坏、export 本地写入失败 | 1 | 空 | I/O 或解析诊断；修正路径或文件 |
| endpoint 不可达或 HTTP 失败 | 1 | 空 | 传输诊断；检查本次服务和 endpoint |

业务错误例：不存在的 Run 当前为 `INVALID_RUN`，旧 Session version 为
`SESSION_POINTER_CONFLICT`，不支持的 Agent 配置为 `INVALID_AGENT_CONFIGURATION`。
错误信封保证 `code/message/retryable`；测试不匹配 reqwest/clap 的整段诊断文案。

退出码 2 同时用于参数错误和业务拒绝，脚本还要检查输出通道与 JSON `ok`。
`ok=true` 下 `result.kind=run` 时，应读取 Run `status/error`；failed、cancelled、waiting_approval
都有明确意义，不能统一显示“任务完成”。

管道调用使用 `set -o pipefail`，或先落盘、立即保存 `$?` 再运行 jq，否则管道末端的成功可能掩盖 CLI 失败。
网络中断可能发生在服务已提交写入之后；先用相应实体 list command 或 `get_run` 核对状态，不能无条件重发创建型命令。
当前 CLI 的网络请求没有显式用户可配置超时；自动化 fixture 的 20 秒上限只保护测试进程。

自动化：[`wf09_cli_diagnostics_do_not_mutate_workspace`](../bins/cli/tests/workflows.rs)，
覆盖帮助、未知/缺失命令、非法实体 JSON、enum/时间戳、缺失/损坏 import、业务错误信封和停止服务后的连接失败。
失败输入前后相关实体记录一致；export 文件保护由 WF-07 覆盖。
