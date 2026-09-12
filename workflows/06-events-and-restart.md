# WF-06：续读事件并在重启后恢复视图

用户目标：中断查看后可以从上次位置继续，并能在服务重启后找回最终状态。
前置条件：完成 WF-01，记录本次 daemon 的数据库文件和监听端口。

## 操作

```bash
ait event list --after 0 | tee "$WF_ROOT/events.sse"
CURSOR="$(awk '/^id:/ { cursor=$2 } END { print cursor+0 }' "$WF_ROOT/events.sse")"
ait session create --id s-events --project-id p1 --agent-id agent-demo
ait session send --session-id s-events --text '保存重启前的状态'
ait event list --after "$CURSOR" | tee "$WF_ROOT/events-after.sse"
{
  ait project list
  ait agent list
  ait session list --project-id p1
  ait message list --project-id p1
  ait run list --project-id p1
} > "$WF_ROOT/before-restart.jsonl"
```

在运行本次 daemon 的终端 B 按 Ctrl-C，然后使用同一数据库路径和端口重新运行原启动命令。
看到监听提示后回到终端 A：

```bash
{
  ait project list
  ait agent list
  ait session list --project-id p1
  ait message list --project-id p1
  ait run list --project-id p1
} > "$WF_ROOT/after-restart.jsonl"
diff "$WF_ROOT/before-restart.jsonl" "$WF_ROOT/after-restart.jsonl"
ait event list --after "$CURSOR"
```

## 验收与失败恢复

SSE 帧包含 `id`（cursor）、`event`（事件种类）和 `data`（JSON）。数据里的 cursor 与帧 ID 相同，
按 cursor 严格递增。`--after` 为排他起点，续读不会返回已确认 cursor 本身。
保存收到的最后一个 `id`；连接失败后以它重试，不能把“发送请求时刻”当游标。

当前每次命令有限回放默认最多 256 条后退出，没有持续监听。
历史较多时反复以本批最后一个 cursor 续读，直到输出为空；空结果不是错误。
这组自动化覆盖小批次的排他续读，不宣称覆盖超过 256 条的全量分页或网络半帧中断。

重启后，在没有其他写入时，各实体查询相同，已保存 Run 可查，同一旧 cursor 的后续事件仍可回放。
事件是恢复视图的辅助，最终状态以对应的实体 list command / `get_run` 为准。
若实体列表意外为空，先检查是否启动了错误数据库或连接了错误的主机或端口，避免重新注册已有数据。

此处恢复的是持久化数据，不代表中断中的 worker、工具调用或模型请求已自动恢复执行。

自动化：[`wf06_replay_events_and_reopen_workspace`](../bins/cli/tests/workflows.rs)，
关闭 HTTP 服务和 store 后重开同一 SQLite 文件，验证实体记录、Run 和游标续读保持一致。
