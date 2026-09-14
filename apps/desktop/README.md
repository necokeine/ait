# Ait

Electron desktop shell for the Ait daemon. The renderer is sandboxed and can only call the narrow preload API; Electron main translates those calls to the daemon's loopback HTTP API.

## Development

```sh
pnpm install
pnpm run typecheck
pnpm test
pnpm run dev
```

`pnpm run dev` builds both `ait-daemon` and `ait-worker` before opening Electron,
then Electron launches the resulting debug daemon directly. This keeps a first
Rust build from being mistaken for a daemon startup timeout.

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

The sidebar keeps the Project catalog visible and loads Sessions only for the selected Project. Electron
uses separate Project catalog, global Agent/Provider catalog, and explicit Project-scoped data calls;
it never transfers an all-Workspace view to the renderer. Use the `+` beside Projects to register a
local directory, choose that Project's default Agent backend, and use the `+` on a Project row to create
a Session. Starting the desktop with an empty workspace leaves this list empty until the user explicitly
creates a Project. The built-in Codex profile uses the locally installed and authenticated
`codex app-server`; deterministic adapters remain available to the Rust test suite without network access.

Runs in the workspace navigation (also available as **Open Runs** in the command
palette) lists active Runs across every registered Project. Queued, running,
waiting-for-approval, retrying, finishing and cancelling Runs remain visible until
they reach a terminal state. Each row shows its Project, Session, Agent, pinned
model and lifecycle phase. **Open Session** loads the Run's Project and conversation;
scheduled Runs without a Session are also listed.

Electron main builds this narrow activity summary from the existing Rust daemon's
Project-scoped Run and Session APIs, with at most four Projects read concurrently.
It does not load Message history for the Runs page or transfer full execution
records to the renderer. Global lifecycle events refresh the visible list, and a
five-second refresh recovers missed events and temporarily unavailable Projects.
Refreshes stop when leaving Runs. Connection loss, failed refreshes and unavailable
Projects have explicit notices; a partial result is not presented as an empty
workspace. The current daemon API returns each Project's Run history before main
filters it, so the read cost still grows with retained Runs.

The composer Agent selector is available for every idle Session. Selecting a
different Agent immediately rebinds that Session with a version check; an
active Session remains locked to the Agent revision already pinned by its Run.

The composer toolbar starts with Run permissions, followed by Agent configuration
and reasoning effort. New installations and Restore defaults use Workspace Write;
saved permission choices are preserved. Permission changes apply to new Runs,
while active Runs keep their original permission snapshot.

Conversation code fences render as rounded cards with a language label, a wrap
toggle, and a copy button that copies the code itself. User input stays literal
inside a single right-aligned bubble. Messages show their persisted creation
date and time in the local timezone. The initial system messages are hidden from
the conversation, while remaining available in the Message tree. Consecutive
reasoning, tool calls, tool results and process output share one collapsed Events
disclosure with an event count, even across types and Message boundaries. Expand
it to see each section's type, details and saved Message timestamps. Ordinary
messages and final answers remain visible between groups. Live output uses the
same grouping and preserves manually opened groups during streaming updates.
Message rows have no leading avatars. Older
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

The application name is **Ait**. The editable brand source is `logo.svg` at the
repository root; `logo.png` is its committed 512×512 export. After editing the
SVG, run `npm run generate:icons` in this directory and commit both files. The
generator uses the icon toolset from the pinned electron-builder version (it
downloads the toolset on first use). Ordinary builds copy the committed assets
to `dist`; electron-builder converts the SVG to a macOS ICNS (up to 1024×1024)
and uses the PNG for Linux packaging.

The npm package name `@ait/desktop` and app ID `dev.ait.desktop` remain stable.
Main resolves and pins the existing Electron `userData` and `sessionData` paths
before setting the display name, so the rename retains existing catalogs,
settings, and browser storage. Development and packaged database filenames
remain separate as described above.

A packaged application expects a prebuilt `ait-daemon` binary at `resources/bin/ait-daemon` (or `.exe` on Windows). It rejects a pre-existing listener on its daemon port, and its trusted Electron main boundary strips a development Mock provider and any Agent that references it before data reaches Settings, Agents, or the composer. There is no desktop-specific persistence adapter: daemon and its SQLite control store are the only state interaction boundary.

On macOS, the Codex adapter launches through `/bin/zsh -lic`, so model discovery,
title generation, and Runs use the environment from `.zprofile` and `.zshrc` even
when Ait opens from Finder or the Dock. Codex must be installed and authenticated
locally. Shell startup files must leave stdout quiet for the app-server JSONL
protocol. This also applies to development launches; other platforms launch Codex
directly.

macOS direct-distribution builds are signed with the personal `Developer ID
Application: Dong Shan (SVS7GV79T9)` identity, use Hardened Runtime, and sign
the bundled daemon and worker before notarization. Local builds discover that
identity in the login keychain. The GitHub Actions release job imports the
certificate and notarizes with repository secrets; configure
`MAC_CSC_LINK`, `MAC_CSC_KEY_PASSWORD`, `APPLE_ID`,
`APPLE_BUILD_APP_SECRET` (mapped to `APPLE_APP_SPECIFIC_PASSWORD`), and
`APPLE_TEAM_ID` (`SVS7GV79T9`). Never add
the `.p12`, its password, or an Apple app-specific password to the repository.

## Providers and Agent presets

Settings → Models manages shared Codex/OpenAI/DeepSeek provider connections, API
keys, model discovery and per-model reasoning levels. DeepSeek discovery supplies
the adapter-owned `off`, `low`, `high`, `max` levels, so a selected DeepSeek model
shows the same conversation reasoning control as a reasoning-capable Codex model.
Settings → Agents saves
named presets for Projects and Sessions. Changing Provider, Model or Reasoning
in a Session immediately saves a private Agent configuration. Running Sessions
reject new messages and configuration changes; no client version is sent.
