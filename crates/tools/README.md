# Default API tool set

Codex uses a separate native profile described below; none of its instructions
or tools are added to this API catalog.

`ait-tools` owns provider-neutral function definitions and the Ait system prompt.
`ToolSet::default()` supplies 28 functions on each host; Windows selects `pwsh`,
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
| User interaction and planning | `ask_user_question`, `exit_plan_mode`, `todo_write` |
| Skills | `skill` |
| Goals | `get_goal`, `create_goal`, `update_goal` |
| Delegation | `subagent`, `subagent_fork`, `list_subagent_models`, `list_agents`, `send_message`, `interrupt_agent` |
| Orchestration | `workflow`, `ralph` |
| Web | `web_search`, `web_fetch` |

## Source mapping

The parameter schemas in `catalog/default.json` are adapted from these files
at the pinned upstream commit:

- `packages/preset/agent-presets/presets/standard/agent.cordis.yml`: enabled Standard packages and options.
- `snapshots/web/fresh-round-trip/tool-schemas.expected.json`: Standard function arguments.
- `snapshots/sdk/subagent-dsh-sdk-dynamic-route/tool-schemas.expected.json`: `list_subagent_models` and model-selection fields on `subagent`.
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

This first stage implements **prompt, tool contracts and request assembly**.
Definitions do not install executors, grant access, or turn a single completion
into an agent loop. A host enabling calls must supply the named implementations,
enforce workspace/approval/model capabilities, persist ToolUse and ToolResult,
and continue the Run through its existing termination barrier. `read_image`
specifically requires image support. Cross-field rules (such as `provider` plus
`model`, or editor command-specific arguments) are executor responsibilities.

`LLMClient::prompt`, `text_request`, and the current text-only
`RigProviderGateway` send the system prompt with **no function catalog**. The
gateway cannot handle structured tool results yet. Rich API callers use
`complete(completion_request(...))` and receive tool calls as data. Schema
presence must never be described as proof of an implemented tool loop.

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
