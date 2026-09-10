# WF-07：迁移 Project 的消息与 Session

用户目标：把 Project 的历史分支与 Session 引用带到另一个本地工作空间。
前置条件：完成 WF-01，可先运行 WF-02/WF-03 产生历史；导出文件放在 Project 工作目录外。

## 操作

在终端 A 导出：

```bash
ait project export --project-id p1 --output "$WF_ROOT/project-archive.json"
jq '{format_version,project,sessions,messages}' "$WF_ROOT/project-archive.json"
mkdir -p "$WF_ROOT/imported-project"
```

在第三个终端进入仓库，将 `演练目录` 替换为本次绝对路径，启动独立目标数据库：

```bash
target/debug/ait-daemon --database '演练目录/target.sqlite3' --listen 127.0.0.1:17315
```

该进程就绪后，在终端 A 指向目标 endpoint：

```bash
"$AIT_REPO/target/debug/ait-cli" --endpoint http://127.0.0.1:17315 \
  project import --input "$WF_ROOT/project-archive.json" --workdir "$WF_ROOT/imported-project"
"$AIT_REPO/target/debug/ait-cli" --endpoint http://127.0.0.1:17315 \
  session list --project-id p1
"$AIT_REPO/target/debug/ait-cli" --endpoint http://127.0.0.1:17315 \
  message list --project-id p1
```

演练结束后在第三个终端按 Ctrl-C 停止目标服务。

## 验收与失败恢复

- `export` 成功退出码 0，stdout 为空，目标文件是 archive JSON 而非 Response 信封。
  当前 `format_version=3`，保存 Project、引用到的 Agents 和无凭证 Providers、Messages 和 Sessions；兼容导入格式 2。
- 导入保留 Message ID、parent 边、内容、Session 指针和版本、Agent revision，以及默认 Agent。
  `workdir` 改为目标规范化目录，Project `base_commit` 取目标仓库 HEAD。
  历史 user Message 的 `git_commit` 仍保持原值。
- 归档里的 Session `active_run_id=null`；导出不取消源 Run。
  目标工作空间不恢复源 Run、Cron、运行 attempt 或凭据。
- 归档不携带 Git 工作文件、提交对象或附件字节；如需后续访问原文件/commit，用户还需另外准备仓库及附件存储。
  导入到空目录不等于已经迁移了代码。
- 未知 Project 的 export 返回 `INVALID_PROJECT`、退出码 2，不能覆盖已有输出文件。
  成功 export 会覆盖同名文件；需要保留旧版时先选一个新文件名。
- 重复导入同一个 Project ID、身份冲突或不支持的 format version 返回 `INVALID_PROJECT`，
  不部分写入工作空间。检查目标是否已导入，或换新的隔离数据库；不要手动篡改 Message ID 来避冲突。
- JSON 损坏或输入文件不存在属于本地错误，退出码 1，详情在 stderr。

自动化：[`wf07_export_and_import_project_archive`](../bins/cli/tests/workflows.rs)，
通过真实文件和两个隔离服务验证分支往返、活动引用清理、失败导出保护文件、重复/非法归档拒绝和源记录不变。
