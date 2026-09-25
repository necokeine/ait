import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { _electron as electron } from "playwright";

const require = createRequire(import.meta.url);
const desktop = fileURLToPath(new URL("..", import.meta.url));
const root = path.resolve(desktop, "../..");
const temporary = mkdtempSync(path.join(os.tmpdir(), "ait-paseo-desktop-smoke-"));
const packagedApp = process.env.PASEO_PACKAGED_APP;
const env = {
  ...process.env,
  AIT_SERVER_DATA_DIR: path.join(temporary, "server"),
  AIT_SERVER_BIN:
    process.env.AIT_SERVER_BIN ||
    path.join(root, "target/debug", process.platform === "win32" ? "server.exe" : "server"),
  PASEO_ELECTRON_USER_DATA_DIR: path.join(temporary, "electron"),
};
if (packagedApp) delete env.AIT_SERVER_BIN;
delete env.ELECTRON_RUN_AS_NODE;
let app;
let page;
let pid;
try {
  app = await electron.launch({
    executablePath: packagedApp
      ? path.join(packagedApp, "Contents/MacOS/Paseo")
      : require("electron"),
    args: packagedApp ? [] : [desktop],
    env,
    timeout: 60000,
  });
  page = await app.firstWindow();
  const errors = [];
  page.on("pageerror", (error) => {
    errors.push(error.message);
    console.error("renderer error:", error.message);
  });
  await page.waitForFunction(() => typeof window.paseoDesktop?.invoke === "function");
  // The renderer must bootstrap the daemon itself. This test never sends start.
  const deadline = Date.now() + 90_000;
  let status;
  do {
    status = await page.evaluate(() => window.paseoDesktop.invoke("desktop_daemon_status"));
    if (status.status === "running") break;
    if (status.status === "errored") throw new Error(status.error);
    await page.waitForTimeout(250);
  } while (Date.now() < deadline);
  assert.equal(status.status, "running", JSON.stringify(status));
  assert(status.serverId && status.ownedByDesktop && status.listen);
  assert(!("token" in status) && !("bearerToken" in status));
  pid = status.pid;
  await page.waitForFunction(
    (id) => globalThis.__paseoHostRuntimeStore?.getSnapshot(id)?.connectionStatus === "online",
    status.serverId,
    { timeout: 30000 },
  );
  const projects = await page.evaluate(async (id) => {
    const runtime = globalThis.__paseoHostRuntimeStore;
    return await runtime.getSnapshot(id).client.listProjects();
  }, status.serverId);
  assert(Array.isArray(projects.projects), "Authenticated renderer RPC did not return projects");
  const restarted = await page.evaluate(() => window.paseoDesktop.invoke("restart_desktop_daemon"));
  assert.equal(restarted.serverId, status.serverId);
  assert.equal(restarted.listen, status.listen);
  assert.notEqual(restarted.pid, pid);
  assert.throws(() => process.kill(pid, 0), "Previous Rust process survived restart");
  pid = restarted.pid;
  await page.waitForFunction(
    (id) => globalThis.__paseoHostRuntimeStore?.getSnapshot(id)?.connectionStatus === "online",
    status.serverId,
    { timeout: 30000 },
  );
  const afterRestart = await page.evaluate(async (id) => {
    return await globalThis.__paseoHostRuntimeStore.getSnapshot(id).client.listProjects();
  }, status.serverId);
  assert(Array.isArray(afterRestart.projects), "Renderer did not reconnect after Rust restart");

  await page.waitForTimeout(2000);
  assert.equal(errors.length, 0, errors.join("\n"));
  if (process.env.PASEO_SMOKE_SCREENSHOT)
    await page.screenshot({ path: process.env.PASEO_SMOKE_SCREENSHOT });
  console.log(
    JSON.stringify({
      status: "passed",
      serverId: status.serverId,
      listen: status.listen,
      rendererErrors: errors,
      title: await page.title(),
    }),
  );
  await app.close();
  app = null;
  assert.throws(() => process.kill(pid, 0), "Rust child survived normal Electron quit");
  pid = null;
} catch (error) {
  if (page && !page.isClosed()) {
    console.error("Renderer body:", (await page.locator("body").innerText()).slice(0, 5000));
    if (process.env.PASEO_SMOKE_SCREENSHOT)
      await page.screenshot({ path: process.env.PASEO_SMOKE_SCREENSHOT });
  }
  throw error;
} finally {
  await app?.close();
  if (pid) {
    try {
      process.kill(pid, "SIGTERM");
    } catch {}
  }
  rmSync(temporary, { recursive: true, force: true });
}
