import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { registerHooks } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";

test("owned legacy daemon failure can be discarded through the desktop bridge and restarted", { timeout: 60_000 }, async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ait-desktop-startup-"));
  const server = createServer((_request, response) => response.end("ready"));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  const databasePath = join(root, "ait-development.sqlite3");
  const old = new DatabaseSync(databasePath);
  old.exec("PRAGMA user_version=2; CREATE TABLE old_config (value TEXT); INSERT INTO old_config VALUES ('old data');");
  old.close();
  const original = await readFile(databasePath);
  await writeFile(join(root, "ait.sqlite3"), "production database");
  await mkdir(join(root, "project/.ait"), { recursive: true });
  await writeFile(join(root, "project/.ait/project.sqlite3"), "project history");
  const state = globalThis.aitStartupTest = { root, dialogs: [], confirm: async () => ({ response: 0 }) };
  const electron = `
    export const app = { isPackaged: false, getPath: () => globalThis.aitStartupTest.root,
      setPath() {}, setName() {}, on() {}, whenReady: () => new Promise(() => {}) };
    export const BrowserWindow = { getAllWindows: () => [] };
    export const dialog = { showMessageBox(options) {
      globalThis.aitStartupTest.dialogs.push(options);
      return globalThis.aitStartupTest.confirm();
    } };
    export const ipcMain = {}, shell = {};`;
  process.env.AIT_DESKTOP_DEV_PORT = String(port);
  const hook = registerHooks({ resolve(specifier, context, next) {
    return specifier === "electron" ? { url: `data:text/javascript,${encodeURIComponent(electron)}`, shortCircuit: true } : next(specifier, context);
  } });
  let DaemonClient;
  try { ({ DaemonClient } = await import("../../dist/main.js")); } finally { hook.deregister(); }
  const client = new DaemonClient();
  t.after(async () => {
    const child = client.ownedProcess;
    const closed = child ? once(child, "close") : Promise.resolve();
    client.stop();
    await closed;
    server.closeAllConnections();
    if (server.listening) await new Promise((resolve) => server.close(resolve));
    delete globalThis.aitStartupTest;
    await rm(root, { recursive: true, force: true });
  });

  assert.equal(await client.request("startup.recovery", {}), null);
  await assert.rejects(client.request("startup.reset-database", {}), /only available/);
  assert.equal(state.dialogs.length, 0);
  await assert.rejects(client.request("project.list", {}), /LEGACY_RECOVERY_REQUIRED/);
  assert.deepEqual(await client.request("startup.recovery", {}), { databasePath });
  await assert.rejects(client.request("startup.retry", {}), /LEGACY_RECOVERY_REQUIRED/);

  assert.equal(await client.request("startup.reset-database", {}), false);
  assert.deepEqual(await readFile(databasePath), original);
  assert.equal(state.dialogs[0].defaultId, 0);
  assert.equal(state.dialogs[0].cancelId, 0);
  assert.match(state.dialogs[0].detail, /No backup/);
  assert.ok(state.dialogs[0].detail.includes(databasePath));

  let finishConfirmation;
  state.confirm = () => new Promise((resolve) => { finishConfirmation = resolve; });
  const pending = client.request("startup.reset-database", {});
  while (!finishConfirmation) await new Promise((resolve) => setTimeout(resolve, 10));
  await assert.rejects(client.request("startup.reset-database", {}), /already in progress/);
  await assert.rejects(client.request("project.list", {}), /recovery is in progress/);
  await assert.rejects(client.request("startup.retry", {}), /recovery is in progress/);
  // A daemon appearing while confirmation is open invalidates the deletion.
  await new Promise((resolve) => server.listen(port, "127.0.0.1", resolve));
  finishConfirmation({ response: 1 });
  await assert.rejects(pending, /daemon is running/);
  assert.deepEqual(await readFile(databasePath), original);
  const unowned = new DaemonClient();
  await assert.rejects(unowned.ensureStarted(), /unverified daemon/);
  assert.equal(await unowned.request("startup.recovery", {}), null);
  await assert.rejects(unowned.request("startup.reset-database", {}), /only available/);
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));

  state.confirm = async () => ({ response: 1 });
  await mkdir(databasePath + "-journal");
  await assert.rejects(client.request("startup.reset-database", {}), /non-file/);
  assert.deepEqual(await readFile(databasePath), original);
  assert.deepEqual(await client.request("startup.recovery", {}), { databasePath });
  await rm(databasePath + "-journal", { recursive: true });
  assert.equal(await client.request("startup.reset-database", { databasePath: join(root, "ait.sqlite3") }), true);
  assert.deepEqual((await readdir(root)).sort(), ["ait-development.sqlite3.lock", "ait.sqlite3", "project"]);
  assert.equal(await readFile(join(root, "ait.sqlite3"), "utf8"), "production database");
  assert.equal(await readFile(join(root, "project/.ait/project.sqlite3"), "utf8"), "project history");
  assert.equal(await client.request("startup.recovery", {}), null);
  await assert.rejects(client.request("startup.reset-database", {}), /only available/);

  await client.request("startup.retry", {});
  assert.deepEqual((await client.request("project.list", {})).projects, []);
  assert.ok((await client.request("agent.catalog", {})).agents.length > 0);
  await assert.rejects(client.request("startup.reset-database", {}), /only available/);
});
