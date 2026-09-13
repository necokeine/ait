# ADR-001：Prompt 权限入口与默认值

- 状态：Proposed，待 NEC-269 验收
- 日期：2026-09-13
- 修订：NEC-192 的新建/重置设置默认值，以及 NEC-263 的 Prompt 工具栏排列

Prompt 工具栏移除尚无行为的附件加号，按权限、Agent 配置、推理等级排列。
权限选择器继续保存全局设置，使用现有 revision/CAS 流程。

Rust settings schema 将 `permissions.sandbox` 默认值设为 `workspace_write`，
用于新建设置和 Restore defaults；`permissions.approval` 仍为 `on_request`。
Desktop 从后端读取设置，HTML 初始选项与该默认值一致。已持久化的 Readonly、
Full Access 或历史 strict 设置不迁移；正在运行和已保存的 Run 保留原权限快照。

管理员上限、缺失/非法设置的准入拒绝，以及旧 Run 缺少权限快照时的只读兼容值保持原语义。
当管理员上限为 Readonly 时，新默认值不能通过准入，用户需显式选择 Readonly。

验证覆盖 CLI 保存/重启/重置、HTTP 新建默认值到 Codex/OpenAI/DeepSeek Run 与 Codex
native sandbox 映射、应用层重置后的 Run 权限，以及显式 Readonly 的写入拒绝与 CAS 重检。
