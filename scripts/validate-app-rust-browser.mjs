/** Real Chromium + app transport + SDK + isolated Rust server; no provider execution. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const work = await mkdtemp(path.join(os.tmpdir(), "ait-app-browser-"));
const token = randomBytes(32).toString("hex");
let backend;
let browser;
let bundle;
const mime = {
  ".js": "text/javascript",
  ".css": "text/css",
  ".html": "text/html",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".ttf": "font/ttf",
  ".woff2": "font/woff2",
  ".ico": "image/x-icon",
};
const frontend = createServer(async (request, response) => {
  const pathname = new URL(request.url, "http://localhost").pathname;
  if (pathname === "/fixture.js") {
    response.setHeader("Content-Type", "text/javascript");
    response.end(bundle);
  } else if (pathname === "/fixture") {
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><script src="/fixture.js"></script>');
  } else {
    const dist = path.join(root, "apps/app/dist");
    const file = path.resolve(dist, `.${pathname}`);
    try {
      if (!file.startsWith(dist + path.sep)) throw new Error("index");
      const contents = await readFile(file);
      response.setHeader("Content-Type", mime[path.extname(file)] ?? "application/octet-stream");
      response.end(contents);
    } catch {
      try {
        response.setHeader("Content-Type", "text/html");
        response.end(await readFile(path.join(dist, "index.html")));
      } catch {
        response.writeHead(404).end();
      }
    }
  }
});
try {
  const compiled = await build({
    stdin: {
      contents: `export { DaemonClient } from '@getpaseo/client/internal/daemon-client'; export { buildRustClientConfig } from './apps/app/src/runtime/rust-server/connection';`,
      resolveDir: root,
      loader: "ts",
    },
    bundle: true,
    write: false,
    format: "iife",
    globalName: "fixture",
    platform: "browser",
    tsconfig: path.join(root, "apps/app/tsconfig.json"),
    plugins: [
      {
        name: "browser-platform",
        setup(builder) {
          builder.onResolve({ filter: /^@\/constants\/platform$/ }, () => ({
            path: "platform",
            namespace: "browser",
          }));
          builder.onLoad({ filter: /.*/, namespace: "browser" }, () => ({
            contents: "export const isWeb = true;",
          }));
        },
      },
    ],
  });
  bundle = compiled.outputFiles[0].text;
  frontend.listen(0, "127.0.0.1");
  await once(frontend, "listening");
  const origin = `http://127.0.0.1:${frontend.address().port}`;
  backend = spawn(
    process.env.AIT_SERVER_BIN ?? path.join(root, "target/debug/server"),
    ["--data-dir", path.join(work, "server"), "--listen", "127.0.0.1:0", "--web-origin", origin],
    { env: { ...process.env, AIT_SERVER_TOKEN: token }, stdio: ["ignore", "ignore", "pipe"] },
  );
  const endpoint = await new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(
      () =>
        reject(
          new Error(
            `Rust startup timed out: ${output.slice(-1000).replaceAll(token, "[redacted]")}`,
          ),
        ),
      90_000,
    );
    backend.once("error", reject);
    backend.once("exit", () => {
      clearTimeout(timeout);
      reject(new Error("Rust exited before ready"));
    });
    backend.stderr.on("data", (chunk) => {
      output += chunk.toString();
      const match = output.match(/server ready\s+listen=(127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
  });
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "reduce",
  });
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message.replaceAll(token, "[redacted]")));
  await page.addInitScript(() => {
    window.sockets = [];
    const Original = window.WebSocket;
    window.WebSocket = class extends Original {
      constructor(url, protocols) {
        super(url, protocols);
        window.sockets.push({ url, protocols });
      }
    };
  });
  await page.goto(`${origin}/fixture`);
  const result = await page.evaluate(
    async ({ endpoint, token }) => {
      const { DaemonClient, buildRustClientConfig } = window.fixture;
      const config = (password) => ({
        ...buildRustClientConfig({ endpoint, password }, null),
        clientId: "app-browser-validation",
        reconnect: { enabled: false },
        suppressSendErrors: true,
      });
      const outcomes = [];
      for (let attempt = 0; attempt < 2; attempt++) {
        const client = new DaemonClient(config(token));
        try {
          await client.connect();
          const projects = await client.listProjects();
          await client.livenessPing({ timeoutMs: 5_000 });
          outcomes.push({
            serverId: client.getLastServerInfoMessage().serverId,
            projects: projects.projects.length,
          });
        } finally {
          await client.close();
        }
      }
      const invalid = new DaemonClient(config("invalid-token"));
      let denied = false;
      try {
        await invalid.connect();
      } catch {
        denied = true;
      } finally {
        await invalid.close();
      }
      return {
        outcomes,
        denied,
        connections: window.sockets.length,
        bearerLeaked: window.sockets.some(({ url, protocols }) =>
          `${url} ${protocols}`.includes(token),
        ),
        tickets: window.sockets.map(({ protocols }) => protocols[0]),
      };
    },
    { endpoint, token },
  );
  assert.equal(result.outcomes.length, 2);
  assert.equal(result.outcomes[0].serverId, result.outcomes[1].serverId);
  assert.equal(result.outcomes[0].projects, 0);
  assert.equal(result.connections, 8);
  assert.equal(new Set(result.tickets).size, 8);
  assert.equal(result.denied, true);
  assert.equal(result.bearerLeaked, false);
  let ui = "not requested";
  if (process.env.APP_BROWSER_UI === "1") {
    await page.goto(origin);
    await page
      .getByTestId("welcome-direct-connection")
      .click({ timeout: 60_000 })
      .catch(async (error) => {
        await page.screenshot({ path: "/tmp/ait-app-browser-failure.png" });
        console.error(await page.locator("body").innerText());
        throw error;
      });
    assert.equal(await page.getByTestId("direct-host-input").inputValue(), "127.0.0.1");
    assert.equal(await page.getByTestId("direct-port-input").inputValue(), "7316");
    await page.getByTestId("direct-port-input").fill(endpoint.split(":")[1]);
    await page.getByTestId("direct-password-input").fill(token);
    await page.getByTestId("direct-host-submit").click();
    await page.getByTestId("add-host-modal").waitFor({ state: "hidden", timeout: 15_000 });
    await page.reload();
    await page.getByTestId("sidebar-hosts-trigger").click({ timeout: 30_000 });
    await page
      .getByTestId(`sidebar-host-row-${result.outcomes[0].serverId}`)
      .waitFor({ timeout: 15_000 });
    ui = "connected and persisted across reload";
  }
  assert.deepEqual(pageErrors, []);
  const report = {
    pageErrors: pageErrors.length,
    browser: "Chromium",
    sdkConnections: result.connections,
    independentTickets: 8,
    projects: "passed",
    ping: "passed",
    reconnect: "passed",
    wrongToken: "rejected",
    bearerInWebSocket: false,
    ui,
  };
  console.log(JSON.stringify(report, null, 2));
  if (process.env.APP_BROWSER_REPORT)
    await writeFile(process.env.APP_BROWSER_REPORT, JSON.stringify(report, null, 2) + "\n");
} finally {
  await browser?.close();
  if (backend && backend.exitCode === null && backend.signalCode === null) {
    backend.kill("SIGTERM");
    const timer = setTimeout(() => backend.kill("SIGKILL"), 17_000);
    await once(backend, "exit");
    clearTimeout(timer);
  }
  await new Promise((resolve) => frontend.close(resolve));
  await rm(work, { recursive: true, force: true });
}
