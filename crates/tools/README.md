# Default API tool set

Codex uses a separate native profile described below; none of its instructions
or tools are added to this API catalog.

`ait-tools` owns provider-neutral function definitions and the Ait system prompt.
`ToolSet::default()` supplies 27 functions on each host; Windows selects `pwsh`,
other hosts select `bash`. Definitions are sorted by name for stable requests.
`ToolSetRegistry` supports exact `(provider, model)` overrides with a shared
default fallback. It does not infer capabilities from model-name substrings.

The reference is [DeepSeek Harness at d347e703](https://github.com/deepseek-ai/deepseek-harness/tree/d347e703908d0406b7a7ef80e3a0e594d86b2215).
The baseline is the shipped **Standard** composition, including its dynamic
model-discovery function, plus Minimal's `str_replace_editor` contract.
Optional LSP, terminal, schedule, PTC, Cordis authoring, and externally installed
Codex/Claude plugins are outside that default composition.

| Capability | Functions |
| --- | --- |
| Shell | `bash` / `pwsh` |
| Files and search | `read`, `read_image`, `write`, `edit`, `glob`, `grep`, `str_replace_editor` |
| Background jobs | `job_list`, `job_output`, `job_kill` |
| User interaction and planning | `question`, `plan_exit`, `todowrite` |
| Skills | `skill` |
| Goals | `get_goal`, `create_goal`, `update_goal` |
| Delegation | `task`, `list_subagent_models`, `list_agents`, `send_message`, `interrupt_agent` |
| Orchestration | `workflow`, `ralph` |
| Web | `websearch`, `webfetch` |

## Source mapping

The parameter schemas in `catalog/default.json` are adapted from these files
at the pinned upstream commit:

- `packages/preset/agent-presets/presets/standard/agent.cordis.yml`: enabled Standard packages and options.
- `snapshots/web/fresh-round-trip/tool-schemas.expected.json`: Standard function arguments.
- `snapshots/sdk/subagent-dsh-sdk-dynamic-route/tool-schemas.expected.json`: `list_subagent_models` and model-selection fields adapted for `task`.
- `snapshots/web/minimal-preset/tool-schemas.expected.json`: `str_replace_editor`.
- `snapshots/session/pwsh-tool-turn/tool-schemas.expected.json`: Windows shell variant.
- `packages/core/system-prompt/src/index.ts` and `snapshots/web/fresh-round-trip/system-prompt.expected.md`: ordered instructions, separate schemas, and tool-use guidance.

The upstream MIT license is retained in `catalog/DEEPSEEK-LICENSE`. Ait's
descriptions and prompt remove DSH UI paths, environment variables, automatic
completion notices, and other deployment-specific promises. Numeric line,
timeout, revision and round arguments use positive integers; web search encodes
its documented one-to-four non-empty query constraint in JSON Schema.

## Request assembly and execution boundary

`LLMClient::completion_request` selects the configured tool set and produces
`[system, current user]`; `completion_request_with_history` produces
`[system, existing history..., current user]`. User text is preserved verbatim,
never interpolated into the system prompt. Existing project instruction
snapshots are retained in history. Function schemas live in the API's `tools`
field, rather than being pasted into user content. DeepSeek uses Chat
Completions; OpenAI uses Responses, with Rig mapping leading system messages to
`instructions` before the remaining input.

```rust
use ait_tools::{ToolSet, ToolSetRegistry};

let mut profiles = ToolSetRegistry::default();
let catalog = ToolSet::default();
let read_only = ToolSet::new(
    "Inspect the project with the supplied read tool and explain your findings.",
    vec![catalog.get("read").unwrap().clone()],
).unwrap();
profiles.insert("deepseek", "my-model-id", read_only);
assert_eq!(profiles.resolve("deepseek", "my-model-id").tools().len(), 1);
// Assign profiles to LLMClientConfig.tool_sets before constructing the client.
```

The public OpenAI/DeepSeek/Gemini/MiniMax Session path now uses the existing `RunCoordinator`
with `HostToolFactory`. After exact provider/model selection, the adapter advertises
only executable functions. The local family contains `read`, `grep`, `glob`, `write`,
`edit`, `skill`, `todowrite`, `webfetch`, `websearch`, and `bash` where the OS
backend is available. The Agent extension adds `question`, `plan_exit`, and `task`,
for 13 executable functions on a Unix host with a usable shell (12 without one).
Unsupported options are removed from schemas and rejected by the executor; the
former snake-case and subagent spellings are not aliases.

`skill` reads a named `SKILL.md` without following symlinks from Project-local
`.agents/skills`, `.opencode/skills`, `.ait/skills`, or `skills`. `todowrite`
returns the complete validated replacement list in its persisted ToolResult.
`webfetch` and `websearch` accept bounded text only, pin public DNS resolutions,
reject credentials and local/private/link-local targets on every redirect, and mark
all returned material as external untrusted input. Search currently uses DuckDuckGo's
public HTML endpoint and therefore has no provider SLA.

Questions and plan reviews are durable Run records. They cross the private worker
protocol, appear in Session and Runs views, expire with the Run/worker deadline, and
resume exactly one waiting ToolUse after an explicit desktop response. Foreground
`task` uses a self-contained prompt and the current provider/model route, runs at
most eight model rounds and 16 nested tool calls, and charges known nested
token/cost/tool usage to the parent Run even when it later fails, is cancelled, or
hits a limit. Recursive, inherited-context, and background delegation
are not advertised. Cross-provider/model child routing and durable background child
jobs remain intentionally unsupported until Ait has a first-class child-Run aggregate.

Structured file tools use workspace-relative, non-hidden capability directory
handles without following symlinks. Writes atomically replace files with a 64 KiB
input cap. `read` streams line windows or lists directories; `grep` supports `path`,
`include`, and `output_mode: "count"` for per-file matching-line counts without
returning contents. `glob` discovers scoped paths. Results are paginated and mark
truncation/skipped files explicitly; `count_complete` identifies incomplete counts.
Hidden paths, `target`, and `node_modules` are excluded from these inspection tools.

`bash` executes shell syntax in the Session workspace. Readonly forbids writes;
Workspace Write permits workspace writes. Both can read only the Session and
explicit OS runtime directories; host homes, other Projects and host temporary
files remain inaccessible, including through workspace symlinks. Both restrict
networking and use a fixed system PATH. macOS uses
Seatbelt, Linux requires system bubblewrap (`bwrap`) with working user namespaces.
Full Access explicitly removes the OS sandbox. All modes keep the admitted Run
and administrator ceilings. Same/lower `sandbox_permissions` requests execute;
higher requests receive a persisted denial. Windows has no shell executor yet;
missing or unusable sandbox backends never fall back to unrestricted execution.
Each Run probes the actual isolation command with a bounded, reaped shell startup
before advertising Bash, so an installed binary alone does not confer capability.

Shell commands default to 10 seconds, capped at 120 seconds. Both output streams
are captured and truncated with explicit markers; nonzero exit status and stderr
remain available. The host clears inherited environment/startup configuration,
reaps the process group on completion/cancellation, and does not expose background
jobs. Shell calls run serially because they may write. See
[NEC-263](../../docs/decisions/NEC-263/adr-001-shell-and-prompt-permissions.md) for the
platform boundary and the Prompt permission selector.

The host persists intent before execution and a unique user ToolResult afterward.
Up to four safe calls can run concurrently; results append in proposal order.
Filesystem calls run on tracked blocking workers with cancellation checks during
traversal, chunked I/O and before atomic publication. Cancellation/deadline drains
all workers before terminal state or Session release; an OS call already in flight
may delay that acknowledgment, but cannot outlive it. Commands are killed and reaped.
Successful tool rounds keep the same attempt. Unknown crash outcomes are never
replayed. API changes remain uncommitted for member review. See
[NEC-247](../../docs/decisions/NEC-247/adr-001-api-provider-host-tool-loop.md),
[NEC-313](../../docs/decisions/NEC-313/adr-001-aligned-api-agent-tools.md), and
[WF-13](../../workflows/13-api-provider-tool-loop.md).

`LLMClient::prompt` and `text_request` remain explicit text-only helpers.
`complete` remains one SDK request; it does not own a Run or execute a tool.

## Verification

```bash
cargo test -p ait-tools -p ait-agent-adapters --all-targets
```

Tests validate every schema (including both shell variants), valid and invalid
arguments, default coverage, exact model overrides, and real Rig serialization
against local DeepSeek/OpenAI HTTP fixtures. The DeepSeek fixture also covers a
tool call followed by a host-supplied result with the matching call id and
preserved reasoning. A narrow HTTP compatibility layer accepts null assistant
content and absent tool-call indices before Rig 0.42's stricter deserializer.
See the [DeepSeek API contract](https://api-docs.deepseek.com/api/create-chat-completion/).

An optional live smoke test sends one bounded request to the official DeepSeek
API. It requires `DEEPSEEK_API_KEY` and `DEEPSEEK_MODEL` already set in the local
environment and incurs API usage; it is ignored by default. Never commit them.

```bash
cargo test -p ait-agent-adapters --test llm_client deepseek_live_default_catalog -- --ignored --exact
```

## Codex native profile

`codex::CodexToolSet` uses the installed codex-core through app-server. It has no
conversion to the API `ToolSet`: core owns both tool definitions and execution,
including native patches, shell/exec sessions, planning, images, and configured
MCP/hosted tools. Availability depends on the actual model, platform, and local
Codex configuration; this profile does not enable optional capabilities.

`prompts/codex.md` supplies the Ait host layer. The adapter sends it followed by
the Project instruction snapshot as `developerInstructions`, on both start and
resume, leaving core's base prompt intact. User content stays in the turn input.
Only a Codex Provider invokes this adapter, regardless of model names used by API
providers. `ToolSet::default()` and all existing model overrides are unchanged.

See [ADR-012](../../docs/decisions/adr-012-codex-native-tool-set.md) for pinned
upstream references, boundaries, and the real two-turn Python smoke test.
