# Native provider parity work

Reference: Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`. Scope: the independent
Rust server used by `apps/app`, with native Codex and Claude Code sessions. Preserve
the native providers' ownership of authentication, tools, history and approvals.

Completion requires implementation, focused regression coverage, workspace checks,
an ADR documenting boundary changes, and a coverage report. This is a working
checklist, not a claim that unchecked features are implemented.

- [x] Validated provider options, MCP transports and exact tool preapprovals
- [x] Claude fast mode and native model capability validation
- [x] Codex plan mode and auto-review when supported by the native protocol
- [x] Rich prompt attachments and image input/output
- [x] Streaming and persisted token/cost/context usage
- [x] Native permission scopes and MCP/question interactions, durable plan approval
- [x] Claude steering, queued input and interruption semantics
- [x] Host busy queue, client message IDs and idempotent admission
- [x] Claude conversation/file/both rewind with immutable host history
- [x] Claude/Codex subagent discovery/history, live events, aliases and Workflow replay
- [x] Codex compact/goal/custom prompt commands
- [x] Rich tool details, diffs, task snapshots and image rendering
- [x] Claude authentication and quota inspection
- [x] Native resume/session identity, input correlation and recovery handling
- [x] Protocol/host integration and capability projections
- [x] ADR, operations documentation, parity matrix and coverage artifact
- [x] Formatting, strict workspace lint, workspace tests and LLVM coverage

Codex file rewind is unsupported by the reference provider itself. Do not
advertise it or emulate it by destructively restoring the whole workspace.
Native version-dependent features must be negotiated or explicitly rejected,
never accepted and silently ignored. Keep unavailable live/platform validation
separate from missing implementation.

## Completion

All applicable capabilities against the pinned Paseo version are implemented and documented in the
[capability matrix and delivery report](../reports/provider-parity.md). At initial completion, sequential workspace
tests passed **1696 tests, 0 failed, 8 ignored**; format, diff checks and strict lint passed.
Installed Codex 0.153.4 inference/history and Claude 2.1.221 model inspection passed. Claude online
inference remains unverified because native OAuth refresh fails; this is recorded separately from
implementation and offline validation.

## Test coverage

Historical full-workspace line coverage: **85.24% (57,553/67,518)**. Scope, exact commands,
source fingerprint, per-crate/production results, ignored tests and platform limitations are in the
[delivery report](../reports/provider-parity.md) and [reviewable JSON artifact](../reports/provider-parity-coverage.json).
Validation after merging PR #109 is recorded separately in the
[consolidation report](../reports/local-workspace-consolidation.md).
