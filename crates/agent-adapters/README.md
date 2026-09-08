# Agent adapters

This crate is the integration boundary for external agent harnesses and
Rig-backed LLM clients.
The first adapter is `CodexAppServerAdapter`, a native Rust client for the
official `codex app-server` stdio JSONL protocol.

The Codex adapter supports:

- `initialize` / `initialized` negotiation;
- new and resumed threads;
- streamed turns and agent message deltas;
- rich raw item lifecycle events for commands, file changes, MCP calls, etc.;
- token usage normalization;
- command/file/permission approval routing through an `ApprovalHandler`;
- cancellation via `turn/interrupt` and child-process cleanup;
- safe defaults: workspace-write, on-request approvals, deny-all handler;
- protocol forward compatibility through `RawNotification` and `Raw` approval responses.

`CodexWorkspaceAgent` is the higher-level local execution boundary used by the
daemon. It collects the final assistant result and commits changes produced in
the Project Git root. It requires a clean worktree before invocation so it can
never fold pre-existing user changes into the generated commit.

Codex authentication remains owned by the local Codex installation. The
adapter does not accept, persist, or log an API key or ChatGPT token.

Codex alone uses `ait_tools::codex::CodexToolSet`. The installed core provides
native tools and their executors. Start and resume send Ait instructions plus
the Project system snapshot in `developerInstructions`, leaving the base prompt
and native tool catalog to Codex. The conversation remains separate user input.
See [ADR-012](../../docs/decisions/adr-012-codex-native-tool-set.md) for the
construction contract and an opt-in Python Hello World test that checks actual
patch/command events and resumes the thread for a second edit and verification.

## Rig LLM client

`LLMClient` wraps Rig 0.42's native OpenAI or DeepSeek client. OpenAI uses
`POST /responses`; DeepSeek uses `POST /chat/completions`. Both use Rig's
`GET /models` support to fetch models visible to the configured API key.
The returned catalog can include models for capabilities other than text generation.

```rust,no_run
use ait_agent_adapters::{LLMClient, LLMClientConfig, LLMProvider};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let key = std::env::var("DEEPSEEK_API_KEY")?;
let mut config = LLMClientConfig::new(LLMProvider::DeepSeek, key);
// For OpenAI, select LLMProvider::OpenAI and pass OPENAI_API_KEY instead.
// Optional: config.base_url = Some("https://gateway.example/v1".into());
config.timeout = std::time::Duration::from_secs(120);
let client = LLMClient::new(config)?;

let models = client.list_models().await?;
let model = std::env::var("LLM_MODEL")?; // choose an available text-generation model
let text = client.prompt(&model, "Say hello.").await?;

// Keep Rig's structured content (including tool calls/reasoning) and token usage.
let mut request = client.completion_request(&model, "Explain ownership in Rust.");
request.max_tokens = Some(256);
client.apply_reasoning_effort(&mut request, "high")?;
let response = client.complete(request).await?;
# Ok(())
# }
```

`complete` accepts a Rig `CompletionRequest` with an explicit `model` and returns
Rig's `CompletionResponse`. These types, `Message`, `AssistantContent`, `Model`
and `ModelList` are re-exported from `ait_agent_adapters::llm`.
`prompt` extracts text blocks and reports a protocol error if no text is returned.
`completion_request` now includes Ait's default system prompt and function
catalog; `completion_request_with_history` places system instructions before
history and appends the current user message last. `LLMClientConfig.tool_sets`
accepts exact provider/model overrides. See [the tool catalog](../tools/README.md)
for the pinned DeepSeek Harness baseline and integration boundary.
`prompt` and `text_request` omit tools for callers that consume only text.
`apply_reasoning_effort` preserves other provider parameters and maps the
model-catalog value to the selected API dialect. OpenAI receives
`reasoning.effort`; DeepSeek receives `thinking: enabled` plus
`reasoning_effort`, except the adapter-owned `off` choice becomes
`thinking: disabled` without an invalid `reasoning_effort: off`. Omitting the
method preserves the provider default.
DeepSeek response normalization accepts null content and omitted tool-call
indices before Rig deserialization; reasoning and call ids remain intact.
Neither method executes tools, retries requests, or owns Message/Session/Run state.
Dropping the operation's future cancels it; requests have a finite timeout.

Configuration and clients are not serializable; their `Debug` output hides keys.
Errors use the existing `AdapterError` classification, retain HTTP status in `code`,
and omit raw SDK errors and response bodies. A `retryable` flag is advisory for
the caller, not an automatic retry. Persist only credential references outside
this adapter; resolve them before construction. No key is read implicitly.

Protocol references: [Rig](https://docs.rs/rig-core/0.42.0/rig_core/),
[OpenAI model listing](https://developers.openai.com/api/reference/resources/models/methods/list),
[DeepSeek model listing](https://api-docs.deepseek.com/api/list-models).

Run verification:

```bash
cargo test -p ait-agent-adapters --all-targets
cargo clippy -p ait-agent-adapters --all-targets -- -D warnings
cargo fmt --all --check
```

The tests use an in-memory fake app-server transport and local HTTP fixtures, without an
account, network access, or API spend. The implementation was checked against
schemas generated by `codex-cli 0.151.0`.

Minimal construction:

```rust,no_run
use ait_agent_adapters::{
    AgentAdapter, AgentRunRequest, ApprovalPolicy, SandboxMode,
    codex::{CodexAppServerAdapter, CodexAppServerConfig},
};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let adapter = CodexAppServerAdapter::new(CodexAppServerConfig::default())?;
let stream = adapter.run(AgentRunRequest {
    request_id: "run-message-1".into(),
    model: None, // use the local Codex default
    reasoning_effort: None, // use the model's advertised default
    project_instructions: None, // optional immutable Project instruction snapshot
    prompt: "Inspect this project and summarize its architecture.".into(),
    cwd: PathBuf::from("/absolute/path/to/project"),
    resume_thread_id: None,
    sandbox: SandboxMode::ReadOnly,
    approval_policy: ApprovalPolicy::Never,
    output_schema: None,
    cancellation: CancellationToken::new(),
}).await?;
# drop(stream);
# Ok(())
# }
```

Official protocol references:

- <https://developers.openai.com/codex/app-server>
- <https://developers.openai.com/codex/codex-sdk>

## Provider gateway

`RigProviderGateway` implements the application-facing `AgentProviderGateway` port.
It resolves immutable credential references through the operating system credential
store, lists models through `LLMClient`, and executes a text-history completion
using the Run's fixed Agent configuration. Secrets are never written into the
control snapshot, events or Project export. Model reasoning levels are catalog
metadata configured by the user; model discovery preserves existing levels.
Draft discovery can use `list_models_with_secret` without storing the credential;
persisting the selected catalog and credential remains a separate application operation.
This text-only port prepends the default system prompt but does not expose
function tools until the host implements a persisted tool execution loop.
