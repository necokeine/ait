/**
 * Validate the imported frontend adapter against the pinned Paseo SDK and a real Rust server.
 * Dependencies: esbuild, vitest, zod ^4.4.3, tweetnacl, base64-js, semver and ws (or Playwright).
 * PASEO_SOURCE_ROOT: pinned upstream checkout; PASEO_TEST_DEPS: test node_modules directory.
 * AIT_SERVER_BIN: compiled Rust server; PASEO_VALIDATION_REPORT: optional JSON output.
 * No npm install, credentials, real projects or provider sessions are used by this script.
 */
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawn, spawnSync, execFileSync } = require("node:child_process");
const { once } = require("node:events");
const assert = require("node:assert/strict");
const repo = path.resolve(__dirname, "..");
const upstream =
    process.env.PASEO_SOURCE_ROOT || path.resolve(repo, "../paseo");
const deps = process.env.PASEO_TEST_DEPS || path.join(repo, "node_modules");
const serverBinary =
    process.env.AIT_SERVER_BIN || path.join(repo, "target/debug/server");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "ait-paseo-validation-"));
const pin = "2c8e8a826810337492cc5a38bb0bbd705b6fb632";
const resolve = (name) =>
    require.resolve(name, { paths: [deps, path.join(repo, "apps/desktop")] });
const esbuild = require(resolve("esbuild"));
let wsPath;
try {
    wsPath = resolve("ws");
} catch {
    wsPath = path.join(
        path.dirname(resolve("playwright-core/package.json")),
        "lib/utilsBundle.js",
    );
}
const wsModule = require(wsPath);
const WebSocket = wsModule.WebSocket || wsModule.ws || wsModule;
let DaemonClient,
    WSOutboundMessageSchema,
    createRustServerTransportFactory,
    createDesktopDaemonTransportFactory,
    createLocalTransportManager;
const plugin = {
    name: "local-upstream",
    setup(build) {
        build.onResolve(
            {
                filter: /^(@\/desktop\/host|.*local-daemon-transport-rpc|electron)$/,
            },
            ({ path: name }) => ({ path: name, namespace: "test-bridge" }),
        );
        build.onLoad(
            { filter: /.*/, namespace: "test-bridge" },
            ({ path: name }) => ({
                contents:
                    name === "electron"
                        ? "export const BrowserWindow={getAllWindows:()=>[]};"
                        : name === "@/desktop/host"
                          ? "export const isElectronRuntime=()=>true;"
                          : "export const defaultLocalDaemonTransportRpc={};",
                loader: "js",
            }),
        );
        build.onResolve({ filter: /^ws$/ }, () => ({
            path: path.join(work, "ws.cjs"),
        }));
        build.onResolve({ filter: /^@getpaseo\// }, ({ path: name }) => {
            let [pkg, ...rest] = name.slice("@getpaseo/".length).split("/");
            if (rest[0] === "internal") rest.shift();
            return {
                path: path.join(
                    upstream,
                    "packages",
                    pkg,
                    "src",
                    (rest.join("/") || "index") + ".ts",
                ),
            };
        });
        build.onResolve(
            { filter: /^(zod|tweetnacl|base64-js|semver)$/ },
            ({ path: name }) => ({
                path: require.resolve(name, {
                    paths: [deps, path.join(repo, "apps/desktop/node_modules")],
                }),
            }),
        );
        build.onLoad({ filter: /\/validation\/ws-outbound\.ts$/ }, () => ({
            contents:
                'import { WSOutboundMessageSchema } from "../messages.js"; export const validateWSOutboundMessage = (value) => WSOutboundMessageSchema.safeParse(value);',
            loader: "ts",
        }));
    },
};
async function buildFixtures() {
    fs.writeFileSync(
        path.join(work, "sdk-entry.ts"),
        `export * from '${upstream}/packages/client/src/daemon-client.ts';\nexport { createWebSocketTransportFactory } from '${upstream}/packages/client/src/daemon-client-websocket-transport.ts';\nexport { WSOutboundMessageSchema, SessionOutboundMessageSchema } from '${upstream}/packages/protocol/src/messages.ts';`,
    );
    for (const [name, entry] of [
        ["sdk", path.join(work, "sdk-entry.ts")],
        [
            "adapter",
            path.join(repo, "apps/app/src/runtime/rust-server/transport.ts"),
        ],
        [
            "renderer",
            path.join(
                repo,
                "apps/app/src/desktop/daemon/desktop-daemon-transport.ts",
            ),
        ],
        ["main", path.join(repo, "apps/paseo/src/daemon/local-transport.ts")],
    ]) {
        await esbuild.build({
            entryPoints: [entry],
            bundle: true,
            platform: "node",
            format: "cjs",
            outfile: path.join(work, `${name}.cjs`),
            plugins: [plugin],
            tsconfigRaw: { compilerOptions: { target: "ES2022" } },
        });
    }
}
const results = { checks: [], invalidMessages: [] };
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
async function runIntegration() {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ait-rust-frontend-"));
    const token = crypto.randomBytes(32).toString("hex");
    const env = Object.fromEntries(
        Object.entries(process.env).filter(
            ([k]) => !k.startsWith("AIT_SERVER_"),
        ),
    );
    const proc = spawn(
        serverBinary,
        ["--listen", "127.0.0.1:0", "--data-dir", dir],
        {
            env: { ...env, AIT_SERVER_TOKEN: token },
            stdio: ["ignore", "ignore", "pipe"],
        },
    );
    let log = "";
    proc.stderr.on("data", (b) => (log += b));
    const exited = once(proc, "exit");
    let client;
    try {
        for (
            let i = 0;
            i < 150 && !/listen=(127\.0\.0\.1:\d+)/.test(log);
            i++
        ) {
            if (proc.exitCode !== null) throw Error(log);
            await wait(50);
        }
        const address = log.match(/listen=(127\.0\.0\.1:\d+)/)[1];
        const listeners = new Set();
        const manager = createLocalTransportManager({
            resolveEndpoint: async (target) => ({
                url: target.url,
                close() {},
                failureDetail: () => null,
            }),
            createWebSocket: (url, options) => new WebSocket(url, options),
            scheduleTimeout: (fn, ms) => {
                const timer = setTimeout(fn, ms);
                return () => clearTimeout(timer);
            },
            emitEvent: (event) => {
                for (const listener of listeners) listener(event);
            },
        });
        const base = createDesktopDaemonTransportFactory({
            openSession: async (input) => manager.open(input),
            listenToEvents: async (fn) => {
                listeners.add(fn);
                return () => listeners.delete(fn);
            },
            sendMessage: (input) => manager.send(input),
            closeSession: async (id) => manager.close(id),
        });
        const factory = createRustServerTransportFactory(base);
        const checkedFactory = (options) => {
            const t = factory(options);
            t.onMessage((data, binary) => {
                if (binary) return;
                const value = JSON.parse(String(data));
                const check = WSOutboundMessageSchema.safeParse(value);
                if (!check.success)
                    results.invalidMessages.push({
                        message: value,
                        issues: check.error.issues,
                    });
            });
            return t;
        };
        client = new DaemonClient({
            url: `ws://${address}/v1/ws`,
            clientId: "rust-sdk-test",
            clientType: "mobile",
            password: token,
            transportFactory: checkedFactory,
            reconnect: { enabled: false },
            logger: { debug() {}, info() {}, warn() {}, error() {} },
            suppressSendErrors: false,
        });
        async function check(name, fn, expectedCode) {
            let timer;
            try {
                const value = await Promise.race([
                    fn(),
                    new Promise(
                        (_, rej) =>
                            (timer = setTimeout(
                                () => rej(Error("test deadline")),
                                3500,
                            )),
                    ),
                ]);
                results.checks.push({
                    name,
                    status: expectedCode ? "unexpected_success" : "passed",
                    keys:
                        value && typeof value === "object"
                            ? Object.keys(value)
                            : [],
                    ...(value?.error ? { error: value.error } : {}),
                });
                return value;
            } catch (e) {
                results.checks.push({
                    name,
                    status: expectedCode === e.code ? "expected_gap" : "failed",
                    error: e.message,
                    code: e.code,
                });
            } finally {
                clearTimeout(timer);
            }
        }
        await check("connect", () => client.connect());
        assert(client.getLastServerInfoMessage(), "no server info");
        for (const [name, fn, expectedCode] of [
            ["projects", () => client.listProjects()],
            ["workspaces", () => client.fetchWorkspaces({ timeout: 2000 })],
            ["agents", () => client.fetchAgents({ timeout: 2000 })],
            ["daemon status", () => client.getDaemonStatus()],
            ["daemon config", () => client.getDaemonConfig()],
            ["providers", () => client.listAvailableProviders()],
            ["provider snapshot", () => client.getProvidersSnapshot()],
            ["workspace labels", () => client.listWorkspaceLabels()],
            ["voice mode off", () => client.setVoiceMode(false)],
            ["liveness ping", () => client.livenessPing({ timeoutMs: 2000 })],
            [
                "unsupported schedules",
                () => client.scheduleList(),
                "not_implemented",
            ],
            [
                "agents subscribe",
                async () => {
                    const sub = client.observeAgents();
                    try {
                        return await sub.ready;
                    } finally {
                        await sub.release();
                    }
                },
                "unsupported_capability",
            ],
            [
                "workspace labels subscribe/release",
                async () => {
                    const sub = client.observeWorkspaceLabels();
                    try {
                        return await sub.ready;
                    } finally {
                        await sub.release();
                    }
                },
            ],
        ])
            await check(name, fn, expectedCode);
        const projectDir = path.join(dir, "project");
        fs.mkdirSync(projectDir);
        fs.writeFileSync(path.join(projectDir, "hello.txt"), "hello");
        await check("add project", () => client.addProject(projectDir));
        const workspace = await check("open workspace", () =>
            client.openProject(projectDir),
        );
        await check("nonempty workspaces", () =>
            client.fetchWorkspaces({ timeout: 2000 }),
        );
        await check("list files", () => client.listDirectory(projectDir, "."));
        await check("list terminals", () => client.listTerminals(projectDir));
        if (workspace?.workspace?.id)
            await check("assign workspace label", () =>
                client.setWorkspaceLabel({
                    workspaceId: workspace.workspace.id,
                    label: { name: "probe", color: "blue" },
                    assigned: true,
                }),
            );
        await check(
            "workspace subscribe",
            async () => {
                const sub = client.observeWorkspaces();
                try {
                    return await sub.ready;
                } finally {
                    await sub.release();
                }
            },
            "unsupported_capability",
        );
        await check("supported session events", async () => {
            const sub = client.observeEvents(["status.daemon_config_changed"]);
            try {
                return await sub.ready;
            } finally {
                await sub.release();
            }
        });
        await check(
            "unsupported session events",
            async () => {
                const sub = client.observeEvents(["agent_permission_request"]);
                try {
                    return await sub.ready;
                } finally {
                    await sub.release();
                }
            },
            "unsupported_capability",
        );
        await check(
            "send message with SDK messageId",
            () => client.sendAgentMessage("missing-agent", "isolated test"),
            "unsupported_capability",
        );
        await check("label update event", async () => {
            const sub = client.observeWorkspaceLabels();
            try {
                await sub.ready;
                const event = new Promise((resolve) =>
                    sub.subscribe({
                        snapshot() {},
                        update(message) {
                            resolve(message);
                        },
                    }),
                );
                await client.updateWorkspaceLabel({
                    name: "probe",
                    newName: "updated",
                });
                return await event;
            } finally {
                await sub.release();
            }
        });
    } finally {
        await client?.close();
        proc.kill("SIGTERM");
        const stop = setTimeout(() => proc.kill("SIGKILL"), 18000);
        try {
            await exited;
        } finally {
            clearTimeout(stop);
            fs.rmSync(dir, { recursive: true, force: true });
        }
    }
    assert.equal(results.invalidMessages.length, 0);
    assert(
        results.checks.every((item) =>
            ["passed", "expected_gap"].includes(item.status),
        ),
    );
}

function runUnitTests() {
    const aliases = [
        "daemon-endpoints",
        "connection-offer",
        "ssh-transport",
    ].map((name) => ({
        find: "@getpaseo/protocol/" + name,
        replacement: path.join(upstream, "packages/protocol/src", name + ".ts"),
    }));
    aliases.push(
        {
            find: "@getpaseo/client/internal/daemon-client",
            replacement: path.join(work, "sdk.cjs"),
        },
        {
            find: "@getpaseo/client/internal/daemon-client-websocket-transport",
            replacement: path.join(
                upstream,
                "packages/client/src/daemon-client-websocket-transport.ts",
            ),
        },
        { find: "@", replacement: path.join(repo, "apps/app/src") },
        { find: "zod", replacement: resolve("zod") },
        {
            find: "vitest",
            replacement: path.join(
                path.dirname(resolve("vitest/package.json")),
                "dist/index.js",
            ),
        },
        { find: "ws", replacement: path.join(work, "ws.cjs") },
        { find: "electron", replacement: path.join(work, "electron.mjs") },
    );
    fs.writeFileSync(
        path.join(work, "electron.mjs"),
        "export const BrowserWindow = { getAllWindows: () => [] };",
    );
    fs.writeFileSync(
        path.join(work, "setup.mjs"),
        `import { vi } from 'vitest';
vi.mock('@/utils/client-id', () => ({ getOrCreateClientId: async () => 'test-client' }));
vi.mock('@/utils/app-version', () => ({ resolveAppVersion: () => null }));
vi.mock('@/constants/platform', () => ({ isWeb: true }));
vi.mock('@/desktop/host', () => ({ isElectronRuntime: () => false }));
vi.mock('@/desktop/daemon/local-daemon-transport-rpc', () => ({ defaultLocalDaemonTransportRpc: {} }));`,
    );
    const config = {
        root: repo,
        tsconfig: false,
        oxc: { tsconfig: false },
        resolve: { alias: aliases },
        test: {
            setupFiles: [path.join(work, "setup.mjs")],
            include: [
                "apps/app/src/runtime/rust-server/transport.test.ts",
                "apps/app/src/utils/test-daemon-connection.test.ts",
                "apps/app/src/desktop/daemon/desktop-daemon-transport.test.ts",
                "apps/paseo/src/daemon/local-transport.test.ts",
            ],
            environment: "node",
            pool: "forks",
            maxWorkers: 2,
        },
    };
    const configPath = path.join(work, "vitest.config.mjs");
    fs.writeFileSync(configPath, "export default " + JSON.stringify(config));
    const cli = path.join(
        path.dirname(resolve("vitest/package.json")),
        "vitest.mjs",
    );
    const checked = spawnSync(
        process.execPath,
        [cli, "run", "--config", configPath],
        { cwd: repo, stdio: "inherit" },
    );
    assert.equal(checked.status, 0, "frontend unit tests failed");
}

(async () => {
    try {
        assert.equal(
            execFileSync("git", ["-C", upstream, "rev-parse", "HEAD"], {
                encoding: "utf8",
            }).trim(),
            pin,
        );
        assert(
            fs.existsSync(serverBinary),
            "Build server-bin or set AIT_SERVER_BIN",
        );
        fs.writeFileSync(
            path.join(work, "ws.cjs"),
            `const mod=require(${JSON.stringify(wsPath)});exports.WebSocket=mod.WebSocket||mod.ws||mod;`,
        );
        await buildFixtures();
        ({ DaemonClient, WSOutboundMessageSchema } = require(
            path.join(work, "sdk.cjs"),
        ));
        ({ createRustServerTransportFactory } = require(
            path.join(work, "adapter.cjs"),
        ));
        ({ createDesktopDaemonTransportFactory } = require(
            path.join(work, "renderer.cjs"),
        ));
        ({ createLocalTransportManager } = require(
            path.join(work, "main.cjs"),
        ));
        runUnitTests();
        await runIntegration();
        results.upstreamRevision = pin;
        results.schema =
            "Pinned upstream original Zod WSOutboundMessageSchema (AOT build output not imported)";
        results.scope =
            "Real SDK, renderer IPC transport, main-process transport manager and isolated Rust server; no Electron UI";
        if (process.env.PASEO_VALIDATION_REPORT)
            fs.writeFileSync(
                process.env.PASEO_VALIDATION_REPORT,
                JSON.stringify(results, null, 2) + "\n",
            );
        console.log(JSON.stringify(results, null, 2));
    } finally {
        fs.rmSync(work, { recursive: true, force: true });
    }
})().catch((error) => {
    console.error(error);
    process.exitCode = 1;
});
