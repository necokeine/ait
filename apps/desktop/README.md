# Ait Desktop

Electron desktop shell for the Ait daemon. The renderer is sandboxed and can only call the narrow preload API; Electron main translates those calls to the daemon's loopback HTTP API.

## Development

```sh
npm ci
npm run typecheck
npm test
npm run dev
```

Main first reuses a daemon already listening on `127.0.0.1:7314`. Otherwise it starts `cargo run -p ait-daemon` with a SQLite database under Electron's per-user `userData` directory. Only a daemon started by this Electron process is stopped on application exit.

The sidebar keeps every Project and its isolated Session list visible at once. Use the `+` beside Projects to register a local directory, choose that Project's default Agent backend, and use the `+` on a Project row to create a Session. Starting the desktop with an empty workspace leaves this list empty until the user explicitly creates a Project. The built-in Codex profile uses the locally installed and authenticated `codex app-server`; deterministic adapters remain available to the Rust test suite without network access.

The composer Agent selector is available for every idle Session. Selecting a
different Agent immediately rebinds that Session with a version check; an
active Session remains locked to the Agent revision already pinned by its Run.
Legacy echo-mode profiles are displayed as `Echo · echo` so they cannot be
confused with the real Codex app-server profile.

## Packaging

A packaged application expects a prebuilt `ait-daemon` binary at `resources/bin/ait-daemon` (or `.exe` on Windows). There is no desktop-specific persistence adapter: daemon and its SQLite control store are the only state interaction boundary.
