# Provider contracts and built-in adapters

`ait-providers` implements the Agent/Provider boundary from ADR-001 v4 and
ADR-002. It owns one provider invocation at a time; Session, Message
persistence, and the Run termination barrier stay in the host runtime.

Included:

- append-only Agent revisions and exact revision pinning for Run creation;
- provider-neutral messages, streaming events, tool calls, usage, stop reasons,
  errors, retry directives, and cancellation;
- capability validation before network I/O;
- `credential_ref` resolution with redacted secret formatting;
- a deterministic local `ScriptedProvider`;
- an OpenAI-compatible remote streaming adapter;
- `contract::verify_stream_contract`, reusable by every future adapter.

Run the checks with:

```bash
cargo test -p ait-providers --all-targets
cargo clippy -p ait-providers --all-targets -- -D warnings
```

The in-memory Agent catalog and credential resolver are deterministic fixtures.
Production composition should replace them with the Store port and an OS
keychain-backed resolver.
