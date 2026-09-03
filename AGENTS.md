# Repository guidance

Before changing domain boundaries, read `docs/README.md` and the authoritative ADR-001 v4.

- Rust is the fixed implementation language; keep the root Cargo workspace buildable.
- `domain` must remain free of Tokio, SQLx, HTTP, IPC, UI, and provider dependencies.
- Dependencies point inward: adapters implement ports; application coordinates domain behavior.
- Message history is immutable. Session is a movable pointer into the Message tree, not the tree itself.
- ToolUse is an assistant sub-message; ToolResult is a user Message.
- A Run is complete only after retry, compaction recovery, and newly queued work are all drained.
- Never commit credentials, provider tokens, local SQLite databases, or runtime artifacts.
- Run format, lint, and workspace tests before handing off changes.
- Record durable boundary changes as an ADR and update `docs/README.md`.
