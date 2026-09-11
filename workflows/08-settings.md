# WF-08：修改设置并处理并发更新

用户目标：保存偏好，在其他客户端先修改时避免覆盖，并可恢复默认值。
前置条件：使用演练 daemon，设置影响整个本地工作空间，不局限于某个 Project。

## 操作

```bash
ait settings get > "$WF_ROOT/settings.json"
REVISION="$(jq -r '.result.value.revision' "$WF_ROOT/settings.json")"
jq '.result.value.values + {"interface.theme":"dark"}' \
  "$WF_ROOT/settings.json" > "$WF_ROOT/settings-values.json"
ait settings set --expected-revision "$REVISION" --input "$WF_ROOT/settings-values.json"
ait settings get
# 预期拒绝：重复使用已过期的 revision
ait settings set --expected-revision "$REVISION" --input "$WF_ROOT/settings-values.json"
# 重置全部设置
ait settings reset
ait settings get
```

## 验收与失败恢复

`get_settings` 返回 Rust 提供的 schema、完整 values 和 settings revision。
`save_settings` 是带 revision 的**完整替换**，不能只传一个字段当作 patch；从读取结果修改目标字段再提交。
保存成功 theme 为 `dark` 且 revision 递增；服务重启后仍保持。

旧 revision、缺少 schema 要求的 key、未知 key、非法枚举或范围均返回 `INVALID_CONFIGURATION`，
退出码 2，不覆盖成功保存的设置。冲突后重新读取完整文档并重新合并用户改动；不要只增加 revision 硬写旧内容。

`reset_settings` 恢复全部 Rust 默认值并递增 revision，重启后仍为默认值。
它不是“仅重置 theme”；该例仅适合专用演练数据库。
schema 中的 `restartRequired` 表示相应配置是否需要重启生效，保存值成功不等于所有运行时组件已即时应用。

自动化：[`wf08_save_reset_and_recover_settings`](../bins/cli/tests/workflows.rs)，
覆盖完整保存、旧 revision、缺 key、非法 theme，以及保存和重置后分别重开数据库。

## 在代码写入前设置权限

默认 `permissions.sandbox=read_only`、`permissions.approval=on_request`。
需要 Codex 写代码时，在发送第一条输入前读取最新 settings 并保存：

```bash
ait settings get > "$WF_ROOT/settings.json"
REVISION="$(jq -r '.result.value.revision' "$WF_ROOT/settings.json")"
jq '.result.value.values + {"permissions.sandbox":"workspace_write","permissions.approval":"on_request"}' \
  "$WF_ROOT/settings.json" > "$WF_ROOT/settings-values.json"
ait settings set --expected-revision "$REVISION" --input "$WF_ROOT/settings-values.json"
```

`--input` 仅含完整 values 对象；`--expected-revision` 单独传入，不能传响应信封。
也支持 `--input -` 从 stdin 读取完整对象。这里的 jq 仅编辑复杂设置文档。
权限在新 Run 准入时固定，受 daemon 管理员上限约束，修改不会扩大已有 Run 的权限。
`strict` 是 `read_only` 兼容值；`full_access` 只在明确选择且管理员允许时生效。
旧值 `approval=always` 无法映射当前 Codex 协议，会拒绝 Codex Run 准入；使用 `on_request` 或 `untrusted_only`。
普通 API Provider 当前只生成文本，即使选择 `workspace_write` 也不会获得尚未实现的宿主工具。
