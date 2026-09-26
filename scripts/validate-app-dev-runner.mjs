/** Verify the real dev entry point and that stopping it releases both listeners. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const data = await mkdtemp(path.join(os.tmpdir(), "ait-app-dev-"));
const token = randomBytes(32).toString("hex");
async function freePort() {
  const listener = createServer().listen(0, "127.0.0.1");
  await once(listener, "listening");
  const port = listener.address().port;
  await new Promise((resolve) => listener.close(resolve));
  return port;
}
async function responds(url) {
  try {
    return (await fetch(url, { signal: AbortSignal.timeout(1_000) })).ok;
  } catch {
    return false;
  }
}
const expoPort = await freePort();
const serverPort = await freePort();
const urls = [`http://127.0.0.1:${expoPort}`, `http://127.0.0.1:${serverPort}/healthz`];
const child = spawn(process.execPath, [path.join(root, "apps/app/scripts/dev-server.mjs")], {
  cwd: root,
  env: {
    ...process.env,
    AIT_SERVER_TOKEN: token,
    AIT_SERVER_DATA_DIR: data,
    AIT_SERVER_BIN: process.env.AIT_SERVER_BIN ?? path.join(root, "target/debug/server"),
    AIT_SERVER_LISTEN: `127.0.0.1:${serverPort}`,
    EXPO_PORT: String(expoPort),
    EXPO_NO_TELEMETRY: "1",
    BROWSER: "none",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let output = "";
child.stdout.on("data", (chunk) => {
  output = (output + chunk).slice(-12_000);
});
child.stderr.on("data", (chunk) => {
  output = (output + chunk).slice(-12_000);
});
const exited = once(child, "exit");
try {
  const deadline = Date.now() + 180_000;
  let ready = false;
  while (Date.now() < deadline && child.exitCode === null) {
    if ((await Promise.all(urls.map(responds))).every(Boolean)) {
      ready = true;
      break;
    }
    await delay(250);
  }
  assert(ready, `Dev processes did not become ready: ${output.replaceAll(token, "[redacted]")}`);
  assert(!output.includes(token), "Dev output exposed the server token");
  const ticket = await fetch(`http://127.0.0.1:${serverPort}/v1/auth/ws-ticket`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, Origin: `http://localhost:${expoPort}` },
  });
  assert.equal(ticket.status, 200);
  child.kill("SIGTERM");
  const timeout = setTimeout(() => child.kill("SIGKILL"), 25_000);
  const [code, signal] = await exited;
  clearTimeout(timeout);
  assert.equal(signal, null);
  assert.equal(code, 0);
  assert.deepEqual(await Promise.all(urls.map(responds)), [false, false]);
  console.log(
    JSON.stringify(
      {
        entrypoint: "dev:app",
        frontend: "ready",
        backend: "ready",
        configuredOrigin: "accepted",
        tokenInOutput: false,
        shutdown: "both listeners closed",
      },
      null,
      2,
    ),
  );
} finally {
  if (child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
    await exited;
  }
  await rm(data, { recursive: true, force: true });
}
