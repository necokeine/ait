# ADR-004: Electron desktop uses daemon as its sole state boundary

- Status: Accepted
- Date: 2026-09-04
- Issue: NEC-162

## Context

Ait needs an Electron workspace for sessions, immutable message branches, agents, and settings. The repository now has a daemon HTTP control plane backed by the SQLite `ControlStore`; introducing a desktop-specific process or file store would create a second source of truth and duplicate transaction rules.

## Decision

- Electron renderer remains sandboxed (`contextIsolation`, no Node integration) and receives only a narrow preload API.
- Electron main is a daemon client. It reuses the loopback daemon on `127.0.0.1:7314`, or starts the packaged `ait-daemon` sidecar when none is available.
- The daemon and its `LocalControlService` are the sole entry point for workspace and settings persistence. Desktop owns no state file and never opens SQLite directly.
- Settings use versioned daemon commands and are stored in the same durable control snapshot.
- `ForkSession` creates a session at an immutable message and sends the first user input within one state transition and one optimistic SQLite commit. Failure commits neither half.
- Electron stops only a daemon process it started; an already-running daemon remains independently owned.

The current transport is loopback HTTP because that is the implemented daemon API. A future local-socket transport may replace HTTP without changing the application commands or persistence ownership.

## Consequences

- CLI and desktop observe the same workspace and revisions.
- Branch and settings behavior is testable below Electron through application and HTTP contracts.
- Packaging must include and sign `ait-daemon` in `resources/bin` and verify sidecar discovery and shutdown on each platform.
- Renderer-facing projections remain intentionally narrower than the daemon wire model and contain no filesystem or credential authority.
