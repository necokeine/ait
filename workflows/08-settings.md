# WF-08：修改设置并处理并发更新

用户目标：保存偏好，在其他客户端先修改时避免覆盖，并可恢复默认值。
前置条件：使用演练 daemon，设置影响整个本地工作空间，不局限于某个 Project。

## 操作

```bash
ait command '{"type":"get_settings"}' > "$WF_ROOT/settings.json"
jq -c '{type:"save_settings",expected_revision:.result.value.revision,values:(.result.value.values + {"interface.theme":"dark"})}' \
  "$WF_ROOT/settings.json" > "$WF_ROOT/save-settings.json"
ait command "$(cat "$WF_ROOT/save-settings.json")"
ait command '{"type":"get_settings"}'
# 预期拒绝：重复使用已过期的 revision
ait command "$(cat "$WF_ROOT/save-settings.json")"
# 重置全部设置
ait command '{"type":"reset_settings"}'
ait command '{"type":"get_settings"}'
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
