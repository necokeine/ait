# @getpaseo/client

Repository-local TypeScript library for building integrations on top of a Paseo daemon.

The source lives in `packages/client/src`, imported from
`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632` (`0.9.0-beta.2`).
The package name is retained for existing imports; it does not select an npm registry copy.
Consumers use explicit `file:` dependencies, and this library plus its local protocol and
relay dependencies are private workspace packages. The upstream license is retained in
[paseo/LICENSE](../../paseo/LICENSE).

## Build and use locally

Run from the repository root:

```bash
npm ci
npm run build:sdk
npm run test:sdk
```

`build:sdk` builds protocol schemas, relay/E2EE, and the client, including JavaScript and
TypeScript declarations under each package's `dist/`. App and desktop builds run this step
automatically. After editing SDK source, rebuild it; `npm run watch:client` watches client
source once the dependencies have been built. `test:sdk` builds the libraries and runs their
local tests, excluding the Wrangler service E2E and hosted relay E2E suites.

For an app under `apps/`, declare the local library as:

```json
{
  "dependencies": {
    "@getpaseo/client": "file:../../packages/client"
  }
}
```

The public library entry point remains:

```ts
import { createPaseoClient } from "@getpaseo/client";

const client = createPaseoClient({ url: "ws://127.0.0.1:6767/ws" });
await client.connect();

const agent = await client.agents.create({
  config: { provider: "codex/gpt-5.5" },
  cwd: "/Users/me/dev/storefront",
  prompt: "Review the current diff and name the riskiest change.",
});

const result = await agent.waitForFinish();
console.log(result.lastMessage);

await client.close();
```

The public API is the package root. Imports under `@getpaseo/client/internal/*` are unsupported implementation details used by Paseo's own packages.

Read the [SDK documentation](https://paseo.sh/docs/sdk) for agents, workspaces, terminals, provider discovery, events, recipes, and the API reference. Runnable TypeScript patterns also live in [`examples/`](./examples/README.md).

## Runtime

This library still speaks the Paseo daemon protocol. The repository's Rust server uses a
different wire protocol; the app currently supplies the adapter in
[`apps/app/src/runtime/rust-server`](../../apps/app/src/runtime/rust-server).
The `/ws` example above targets a compatible Paseo daemon, not the desktop-managed Rust
server. Making the SDK local does not itself add Rust relay support.

The client needs a WebSocket implementation. Modern browsers and Node.js 22 provide one globally.

Use a WebSocket URL ending in `/ws`, such as `ws://127.0.0.1:6767/ws`. Pass `password` when the daemon requires authentication.

The client advertises its supported protocol capabilities by default. Optional `capabilities`
overrides extend or override that declaration; browser hosting must be supplied by the caller.
Connecting alone does not subscribe to agent timelines or catalog events. See the
[event guide](https://paseo.sh/docs/sdk/events) for subscription lifetimes and timeline replacements.

## Stability

The high-level API exported from `@getpaseo/client` is the supported SDK surface. The SDK and daemon remain protocol-compatible across versions, but newly added capabilities can require a newer daemon.
