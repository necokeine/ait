import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { chmod, copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../../../../", import.meta.url));
const shellQuote = (value) => `'${value.replaceAll("'", "'\\''")}'`;

test("Desktop discovers unbound Rust summaries and imports/syncs without losing Project isolation", {
  skip: process.platform === "win32" ? "Uses the repository's Unix worker fixture harness" : false,
  timeout: 60_000,
}, async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), "ait-desktop-codex-wire-")));
  let daemon;
  let closed;
  t.after(async () => {
    if (daemon && daemon.exitCode === null && daemon.signalCode === null) {
      daemon.kill("SIGTERM");
      const timer = setTimeout(() => daemon.kill("SIGKILL"), 10_000);
      timer.unref();
      try { await closed; } finally { clearTimeout(timer); }
    }
    await rm(root, { recursive: true, force: true });
  });
  const bin = join(root, "bin");
  await mkdir(bin);
  await copyFile(new URL("./fixtures/codex-history.py", import.meta.url), join(bin, "codex"));
  await chmod(join(bin, "codex"), 0o700);
  // The worker uses a login shell on macOS. Its temporary home must resolve our fixture,
  // without loading the user's shell configuration or Codex credentials.
  await writeFile(join(root, ".zshrc"), `export PATH=${shellQuote(bin)}:$PATH\n`);
  await writeFile(join(root, ".zprofile"), `export PATH=${shellQuote(bin)}:$PATH\n`);
  daemon = spawn(join(repo, "target/debug/ait-daemon"), [
    "--database", join(root, "catalog.sqlite3"), "--listen", "127.0.0.1:0",
    "--worker-binary", join(repo, "target/debug/ait-worker"),
  ], { cwd: root, env: { ...process.env, HOME: root, PATH: `${bin}:${process.env.PATH ?? ""}` }, stdio: ["ignore", "ignore", "pipe"] });
  closed = once(daemon, "close");
  const endpoint = await new Promise((resolve, reject) => {
    let log = "";
    const timer = setTimeout(() => reject(new Error(`Daemon readiness timed out: ${log}`)), 15_000);
    timer.unref();
    daemon.once("error", (error) => { clearTimeout(timer); reject(error); });
    daemon.once("exit", () => { clearTimeout(timer); reject(new Error(`Daemon exited before readiness: ${log}`)); });
    daemon.stderr.on("data", (chunk) => {
      log = (log + chunk.toString()).slice(-8_192);
      const match = log.match(/AIT daemon listening on (http:\/\/127\.0\.0\.1:\d+)/);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
  });
  const request = async (path, body) => {
    const response = await fetch(`${endpoint}${path}`, body ? {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
    } : {});
    const value = await response.json();
    assert.equal(response.ok && value.ok, true, JSON.stringify(value.error));
    return value.result.value;
  };
  for (const id of ["project-one", "project-two"]) {
    await mkdir(join(root, id));
    await request("/v1/project/register", { id, name: id, workdir: join(root, id) });
  }
  await request("/v1/agent/register", { id: "import-agent", name: "Codex fixture", config: {
    provider_id: "builtin-codex", model: "gpt-5.6-sol",
  } });
  const raw = await request("/v1/codex/thread/list?provider_id=builtin-codex&project_id=project-one");
  assert.equal(raw.length, 1);
  assert.equal(raw[0].thread_id, "native-project-one");
  for (const field of ["name", "project_id", "session_id"]) assert.equal(Object.hasOwn(raw[0], field), false);

  process.env.AIT_DESKTOP_DEV_PORT = new URL(endpoint).port;
  const electron = `export const app = { isPackaged: false, getPath: () => process.cwd(),
    setPath() {}, setName() {}, on() {}, whenReady: () => new Promise(() => {}) };
    export const BrowserWindow = {}, dialog = {}, ipcMain = {}, shell = {};`;
  const hook = registerHooks({ resolve(specifier, context, next) {
    return specifier === "electron" ? { url: `data:text/javascript,${encodeURIComponent(electron)}`, shortCircuit: true } : next(specifier, context);
  } });
  let DaemonClient;
  try { ({ DaemonClient } = await import("../../dist/main.js")); } finally { hook.deregister(); }
  const client = new DaemonClient();
  client.ensureStarted = async () => {};
  const input = { projectId: "project-one", providerId: "builtin-codex" };
  const discovered = await client.request("project.codex-threads", input);
  assert.deepEqual(discovered, [{ threadId: "native-project-one", title: "New native conversation",
    preview: "New native conversation", cwd: join(root, "project-one"), updatedAt: 2_000,
    archived: false, sessionId: null, agentId: null, activeRunId: null }]);

  const sync = { ...input, threadId: discovered[0].threadId, agentId: "import-agent" };
  const imported = await client.request("project.sync-codex-thread", sync);
  assert.equal(imported.projectId, input.projectId);
  assert.ok(imported.sessionId);
  assert.deepEqual(await client.request("project.sync-codex-thread", sync), imported);
  const bound = await client.request("project.codex-threads", input);
  assert.equal(bound[0].sessionId, imported.sessionId);
  assert.equal(bound[0].agentId, "import-agent");
  const other = await client.request("project.codex-threads", { ...input, projectId: "project-two" });
  assert.deepEqual(other.map((thread) => thread.threadId), ["native-project-two"]);
  await assert.rejects(client.request("project.sync-codex-thread", { ...sync, projectId: "project-two" }), /another Project/);
  assert.equal((await request("/v1/session/list?project_id=project-one")).length, 1);
  assert.deepEqual(await request("/v1/run/list?project_id=project-one"), []);
  assert.deepEqual(await request("/v1/session/list?project_id=project-two"), []);
  const methods = (await readFile(join(root, "methods.jsonl"), "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  assert.ok(methods.includes("thread/list") && methods.includes("thread/read"));
  assert.ok(methods.every((method) => ["initialize", "initialized", "thread/list", "thread/read", "thread/turns/list"].includes(method)));
});
