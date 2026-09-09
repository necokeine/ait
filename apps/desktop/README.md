# Ait Desktop

Electron desktop shell for the Ait daemon. The renderer is sandboxed and can only call the narrow preload API; Electron main translates those calls to the daemon's loopback HTTP API.

## Development

```sh
npm ci
npm run typecheck
npm test
npm run dev
```

Main owns the daemon it starts and refuses to reuse an unverified process already listening on its
configured loopback port. Stop the process occupying that port before reopening Ait. Only the daemon
started by this Electron process is stopped on application exit.

The desktop development launcher adds the non-default `dev-mock-provider` daemon feature. Together
with Rust debug assertions this exposes the local deterministic Mock provider for UI development;
packaged/release daemons do not contain that provider even if the feature is passed accidentally.
Development uses `127.0.0.1:7315` and `<Electron userData>/ait-development.sqlite3`; packaged builds
use `127.0.0.1:7314` and `<Electron userData>/ait.sqlite3`. These defaults keep development Mock data
out of the production store. To reset only the development profile, quit Ait and remove
`ait-development.sqlite3` plus its optional `-wal` and `-shm` companions from the `userData`
directory. Never rename or copy that database to `ait.sqlite3`.

The sidebar keeps every Project and its isolated Session list visible at once. Use the `+` beside Projects to register a local directory, choose that Project's default Agent backend, and use the `+` on a Project row to create a Session. Starting the desktop with an empty workspace leaves this list empty until the user explicitly creates a Project. The built-in Codex profile uses the locally installed and authenticated `codex app-server`; deterministic adapters remain available to the Rust test suite without network access.

The composer Agent selector is available for every idle Session. Selecting a
different Agent immediately rebinds that Session with a version check; an
active Session remains locked to the Agent revision already pinned by its Run.

Conversation code fences render as rounded cards with a language label, a wrap
toggle, and a copy button that copies the code itself. User input stays literal
inside a single right-aligned bubble. Messages show their persisted creation
date and time in the local timezone, including system and tool messages. Older
records that never stored a timestamp display `Time unavailable`; loading them
does not invent or rewrite historical times.

Sending a message uses the daemon's asynchronous submission route. Electron
main keeps one cursor-based progress stream for all windows and forwards only
the fixed `ait:run-event-frame` IPC channel. A new document must announce a
unique ready generation before main sends it events; frames and ACKs are scoped
to that generation, and a missing ACK times out into checkpoint resync. Each
window has at most one acknowledged frame in flight and a 512-update/1 MiB
main-process buffer; the renderer uses the same limits while a view refresh is in
flight. Overflow converges through a fresh checkpoint instead of accumulating
IPC messages. Active Sessions render each frame once, preserving commentary as
process output until an explicit final phase arrives. Refresh and reconnect
recover from the daemon without restarting the Run. Closing a renderer
subscription does not cancel execution. A disconnected stream is shown
separately from Run failure, and the final immutable Message replaces the
transient projection after Ait has finished saving it.

## Packaging

A packaged application expects a prebuilt `ait-daemon` binary at `resources/bin/ait-daemon` (or `.exe` on Windows). It rejects a pre-existing listener on its daemon port, and its trusted Electron main boundary strips a development Mock provider and any Agent that references it before data reaches Settings, Agents, or the composer. There is no desktop-specific persistence adapter: daemon and its SQLite control store are the only state interaction boundary.

## Providers and Agent presets

Settings → Models manages shared Codex/OpenAI/DeepSeek provider connections, API
keys, model discovery and per-model reasoning levels. DeepSeek discovery supplies
the adapter-owned `off`, `low`, `high`, `max` levels, so a selected DeepSeek model
shows the same conversation reasoning control as a reasoning-capable Codex model.
Settings → Agents saves
named presets for Projects and Sessions. Changing Provider, Model or Reasoning
in a Session immediately saves a private Agent configuration. Running Sessions
reject new messages and configuration changes; no client version is sent.
