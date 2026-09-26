# ADR-052: Native provider capability completion

Status: Accepted; implementation tracked in [the parity checklist](../plans/provider-parity.md).

## Context

The independent Rust server already delegates native session execution to Codex app-server
and Claude Code stream-json. Its initial text-only port did not expose several capabilities
used by the imported Paseo client. The comparison baseline is Paseo
`2c8e8a826810337492cc5a38bb0bbd705b6fb632`.

## Decisions

- Keep native tools, credentials, provider history and permission-rule persistence in their
  respective CLI. The host validates intent, coordinates native sessions and projects UI facts.
  This does not change ADR-001 v4's Message tree or introduce provider dependencies into domain.
- Validate provider options and MCP definitions before launching native processes. Map the
  common MCP and exact tool policy contracts into each CLI's own configuration. Unknown
  fields and ambiguous wildcard preapprovals are rejected. Configuration changes take effect
  at the next turn by resuming the same identity in a new process when required.
- Permission responses distinguish one-call approval, session approval and explicitly selected
  native persistent rules. Native suggested amendments are retained exactly; a generic Allow
  action cannot acquire the authority of a persistent amendment. Claude's explicit SDK rule
  updates retain their destination. Credentials and MCP configuration are not public snapshots.
- Primitive MCP forms use the existing question UI and return typed, validated values. Unsupported
  nested schemas and URL elicitation are declined without terminating an otherwise usable CLI.
- Usage events contain provider facts, not billing estimates. Store the latest complete usage
  snapshot under runtime-info `extra.lastUsage`, expose `lastUsage` in public snapshots and
  publish `usage_updated` after persistence. Repeated observations replace values rather than
  incrementing totals. Context occupancy comes from the current request or native compaction,
  never cumulative session token usage. Preserve usage across native resume and runtime refresh.
- Negotiate Codex planning with `collaborationMode/list`; check native version for auto-review
  and goal support. The installed CLI's **experimental** schema contains `collaborationMode`.
  ADR-041's conclusion based on the ordinary generated schema is superseded on this point.
  Clearing a previously enabled plan mode sends an explicit default collaboration mode.
- Retain existing bounded frame, event and history processing. New rich-input and scheduling
  capabilities must validate complete intent before native admission, preserve voice turn
  ownership, and never automatically replay uncertain native submissions.
- Rich prompts preserve context/text/image/attachment ordering. Decode native image results into
  private content-addressed files under `agents/provider-images`; live output and historical
  projection use the same directory and source identity. Store markdown references in timelines,
  not the image base64. Structured tool previews retain bounded commands, results and file diffs.
- Claude steering writes `priority: next` into the active SDK input stream with user replay enabled.
  Admission means a completed stdin write, not an inference acknowledgement. Native echoes or
  command lifecycle observations drain unread steers before terminal completion; cancellation
  closes the query so unread inputs cannot resume an interrupted turn. Native permission withdrawal
  resolves the corresponding public card and clears permission attention only when no requests remain.
- Timeline schema v4 adds independent input receipts with payload/policy fingerprints and a FIFO
  of unsubmitted rich prompts. Commit a claim before native submission, then its outcome. A crash
  after claim leaves an uncertain receipt that is never replayed; only unclaimed queued work can
  resume. Ordinary send defaults to interrupt-and-deliver, and definite steer rejection can use
  that fallback. Explicit cancel withdraws queued inputs. Voice admission remains exclusive.
- Session-scoped Codex asynchronous questions survive turn completion and restart in opaque native
  resume metadata. Answers use separate immediate-admission receipts: a definitive refusal can be
  retried, but it never interrupts the current task or enters the fallback queue. An uncertain write
  remains fenced. Resolutions are separate immutable display items; ordinary native tool approvals
  still expire with their transport.
- Native subagent ports carry verified ancestry and independent child progress. Timeline schema v5
  retains child display descriptors; child timelines use a separate scope and publish updates to
  observers of their registered parent. Native discovery and live announcements supply identity;
  arbitrary foreign-thread output cannot enroll a child. Claude task aliases retain the first tool
  call identity, and explicit background tasks may continue after the parent foreground turn ends.
  Read-only discovery can ignore an unfinished final JSONL fragment from an actively written child;
  complete malformed records and oversized frames remain errors.
- Codex `/compact` and `/goal` use native control RPCs with immediate-admission receipts and do
  not interrupt foreground input. Enable native goals only after the CLI version probe succeeds.
  Provider-originated `turn/started` events establish autonomous foreground turn ownership in the
  host. Bounded control-result notes persist in opaque resume metadata and replay alongside native
  history. Native custom prompt expansion reads the CLI's prompt directory and substitutes quoted
  positional/named arguments without evaluating a shell.
- Codex plan proposals produce durable review requests. Explicit approval disables plan mode and
  admits one implementation prompt through the same immediate receipt mechanism as asynchronous
  answers. Rejecting or replacing a proposal records an immutable resolution; it does not submit
  implementation work. A native accepted goal/compaction that has not yet announced its turn is
  pending work for finish-wait and message admission.
- Claude CLI model aliases inherit capabilities only from the native `resolvedModel` field.
  Runtime initialization can update the actual model without dropping persisted usage. Arbitrary
  host client message IDs are mapped to native UUIDs in bounded opaque metadata, and both live
  projections and replay preserve the original correlation ID.
- Native task tools become immutable todo snapshots, including Claude TaskCreate/TaskUpdate/
  TaskList and Codex plan progress. Claude restores task and usage observations before new input.
  Native root output after a completed turn establishes a distinct autonomous turn; child output
  remains in its own scope. Direct stream-JSON transport has no JavaScript SDK iterator to recover
  after an interrupt exception; canceled queries are closed, and queued unsubmitted input resumes
  through a new query against the same native history. Uncertain submitted input is never replayed.
- Child recovery reads provider-verified ancestry. Claude restores native task/tool aliases and
  outstanding tool observations; Codex buffers bounded early child events until native spawn
  provenance arrives. Workflow summaries require an original Workflow tool-result/run link and
  use bounded regular-file readers for summaries/results and recursively discover bounded Workflow transcripts. Nonterminal persisted Workflow runs
  are failed on replay, not resurrected as running. Closed/lost processes retire cached running
  child descriptors. Child inspection never registers a new host Agent.
- Claude diagnostics use `auth status --json` and project only authentication state/method.
  Quota reads existing OAuth credentials from the selected credential file or the default macOS
  keychain, performs a bounded HTTPS GET with redirects disabled, and never refreshes or writes
  credentials. Native model/surface quota identities, zero usage and unknown usage stay distinct.
- Existing receipts are checked before new voice-ownership restrictions: an already admitted
  retry is acknowledged without taking another turn. A queued admission failure is persisted for
  its own Agent and does not prevent independent Agents from draining their queues. Rewind and
  cancellation withdraw unsubmitted inputs before changing the native conversation.
- Claude final structured output absent from native JSONL is retained in bounded opaque result notes,
  so history reconciliation does not discard an acknowledged result. Repeated Workflow results are
  content-deduplicated while changed results remain visible.
- A provider's history inspection may return opaque resume metadata for intentional empty branches.
  This avoids treating an arbitrary missing native transcript as an empty successful rewind.
- Conversation rewinds must retain immutable host history generations. Native file checkpoint
  rewind remains a provider capability; Codex has no file rewind in the reference implementation
  and must not advertise it or emulate a whole-workspace restore.
- Claude conversation rewind follows the SDK fork format: write a new private transcript with
  remapped transcript UUIDs and parent links, preserving original history. Native file checkpoints
  are restored through `rewind_files`; forks start without the source's undo history. Rewinding
  both restores files before changing the conversation, so a later fork failure cannot promise that
  file restoration was rolled back. Reference: [official SDK fork transform](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/session_mutations.py).

## Consequences

Capabilities are projected from implemented/native-supported behavior. Offline native peers
verify request parameters and event sequences; installed-CLI checks and inference checks are
reported separately. The checklist and final coverage report are the authority for implementation
and validation status; this ADR does not assert completion of unchecked work.
