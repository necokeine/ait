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

The Project switcher can register local directories, persist a default Agent backend per Project, and switch between their isolated Session lists. Create a Session with the `+` button, send from the composer, then move between Projects without losing either conversation. The development control-plane currently provides a deterministic built-in Codex profile so this full flow is testable without network access.

## Packaging

A packaged application expects a prebuilt `ait-daemon` binary at `resources/bin/ait-daemon` (or `.exe` on Windows). There is no desktop-specific persistence adapter: daemon and its SQLite control store are the only state interaction boundary.
