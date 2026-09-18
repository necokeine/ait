// Real Electron renderer + preload + main + owned daemon + worker + SQLite.
// Only the remote model is replaced. No real model credentials or requests.
import assert from "node:assert/strict";
import { test } from "node:test";
import { _electron as electron } from "playwright";
import { createServer } from "node:http";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";

const desktop = fileURLToPath(new URL("../../", import.meta.url));
const repo = resolve(desktop, "../..");
const artifacts = join(repo, "target/approval-gui");
async function listen(server) {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return server.address().port;
}
async function eventually(check, message) {
  for (let n = 0; n < 200; n++) {
    const value = await check();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(message);
}
function modelReply(kind, call, final) {
  if (kind === "openai") return {
    id: "resp_fixture", object: "response", created_at: 0, status: "completed", model: "fixture-model", tools: [],
    output: final ? [{ type: "message", id: "msg_final", status: "completed", role: "assistant", content: [{ type: "output_text", annotations: [], text: "Approval fixture finished" }] }]
      : [{ type: "function_call", id: "fc_one", call_id: "call-one", name: "write", arguments: JSON.stringify(call), status: "completed" }],
    usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 },
  };
  return {
    id: "chatcmpl_fixture", object: "chat.completion", created: 0, model: "fixture-model",
    choices: [{ index: 0, finish_reason: final ? "stop" : "tool_calls", message: final
      ? { role: "assistant", content: "Approval fixture finished" }
      : { role: "assistant", content: null, reasoning_content: "Use the host tools.", tool_calls: [{ id: "call-one", type: "function", function: { name: "write", arguments: JSON.stringify(call) } }] } }],
    usage: { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 },
  };
}

test("API tool approval GUI: both providers, Session and Cron, approve and deny, reload and view sync", {
  timeout: 180_000,
  skip: process.platform !== "darwin" ? "This GUI fixture uses macOS Keychain for synthetic credentials; Linux requires a configured secret service." : false,
}, async () => {
  await mkdir(artifacts, { recursive: true });
  const directory = await mkdtemp(join(tmpdir(), "ait-approval-gui-"));
  const profile = join(directory, "profile");
  await mkdir(profile);
  const calls = new Map();
  const model = createServer(async (req, res) => {
    const scenario = req.url.split("/").filter(Boolean)[0];
    const count = calls.get(scenario) ?? 0;
    let input = "";
    for await (const chunk of req) input += chunk;
    if (req.method === "GET") {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify({ data: [{ id: "fixture-model", object: "model", owned_by: "fixture" }] }));
    }
    calls.set(scenario, count + 1);
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify(modelReply(scenario.startsWith("openai") ? "openai" : "deepseek", {
      file_path: "approved.txt", content: "synthetic approved content", sandbox_permissions: "workspace-write", justification: "Create this synthetic approval demonstration file",
    }, count > 0)));
  });
  const modelPort = await listen(model);
  const reserve = createServer();
  const port = await listen(reserve);
  await new Promise((resolve) => reserve.close(resolve));
  const endpoint = `http://127.0.0.1:${port}`;
  const api = async (path, body) => {
    const response = await fetch(endpoint + path, body === undefined ? {} : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    const data = await response.json();
    assert.equal(data.ok, true, JSON.stringify(data));
    return data.result.value;
  };
  // Configure only profile paths before importing the unchanged production entry point.
  const bootstrap = join(directory, "bootstrap.cjs");
  await writeFile(bootstrap, `const { app } = require('electron'); app.setPath('userData', ${JSON.stringify(profile)}); app.setPath('sessionData', ${JSON.stringify(profile)}); import(${JSON.stringify(pathToFileURL(join(desktop, "dist/main.js")).href)});`);
  let app;
  let restarted;
  try {
    app = await electron.launch({ args: [bootstrap], cwd: desktop, env: { ...process.env, AIT_DESKTOP_DEV_PORT: String(port) } });
    const page = await app.firstWindow();
    page.setDefaultTimeout(15_000);
    await page.locator("#app:not(.is-loading)").waitFor();
    assert.equal(await app.evaluate(({ app }) => app.getPath("userData")), profile);
    const settings = await api("/v1/settings");
    await api("/v1/settings/save", { expected_revision: settings.revision, values: { ...settings.values, "permissions.sandbox": "read_only", "permissions.approval": "on_request" } });
    for (const kind of ["openai", "deepseek"]) {
      for (const sessionBound of [true, false]) {
        for (const action of ["approve", "deny"]) {
          const id = `${kind}-${sessionBound ? "session" : "cron"}-${action}`;
          const workdir = join(directory, id);
          await mkdir(workdir);
          await api("/v1/agent-provider/save", { provider: { id, name: `${kind} offline fixture`, kind, url: `http://127.0.0.1:${modelPort}/${id}`, models: [{ id: "fixture-model", name: "Fixture", reasoning_efforts: [] }] }, secret: "offline-gui-fixture-key" });
          await api("/v1/agent/register", { id, name: `Approval Agent ${id}`, config: { provider_id: id, model: "fixture-model", reasoning_effort: null } });
          await api("/v1/project/register", { id, name: id, workdir });
          const session = await api("/v1/session/create", { id, project_id: id, agent_id: id });
          let cronResult;
          let cronPromise;
          let cronSession;
          let run;
          if (sessionBound) {
            run = await api("/v1/session/submit-message", { session_id: id, text: "Create a synthetic file with approval." });
          } else {
            await api("/v1/cron/create", { id, name: id, project_id: id, base_message_id: session.current_message_id, agent_id: id, schedule: "0 0 0 1 1 *", timezone: "UTC" });
            cronPromise = api("/v1/cron/trigger", { cron_id: id, scheduled_at: Date.now() }).then((value) => { cronResult = value; });
            run = await eventually(async () => (await api(`/v1/run/list?project_id=${id}`))[0], "Cron must create a Run");
            assert.equal(typeof run.session_id, "string");
            cronSession = await eventually(async () => (await api(`/v1/session/list?project_id=${id}`)).find((candidate) => candidate.id === run.session_id), "Cron Session must be visible");
          }
          run = await eventually(async () => {
            const value = await api("/v1/run/get", { run_id: run.id });
            assert.ok(!["completed", "failed", "cancelled"].includes(value.status), `${id} terminated before approval: ${JSON.stringify(value)}; model requests=${calls.get(id)}`);
            return value.tool_approvals?.some((a) => a.status === "pending") && value;
          }, `${id}: Request must be persisted before clicking; model requests=${calls.get(id)}`);
          await page.reload(); // Recover cards from durable state, not a remembered event.
          await page.locator("#app:not(.is-loading)").waitFor();
          if (sessionBound) {
            await page.locator(`[data-project-id="${id}"]`).click();
            await page.locator(`[data-session-id="${id}"]`).click();
          } else {
            await page.locator("#runs-nav").click();
            await page.locator(`[data-run-detail="${run.id}"]`).click();
          }
          const card = page.locator(`.tool-approval[data-run-id="${run.id}"]`).filter({ visible: true });
          await card.locator('[data-approval-action="approve"]').waitFor();
          assert.match(await card.innerText(), /approved\.txt/);
          assert.match(await card.innerText(), /Readonly → Workspace Write/);
          assert.match(await card.innerText(), /This operation only/);
          await page.screenshot({ path: join(artifacts, `${id}-pending.png`) });
          await card.locator(`[data-approval-action="${action}"]`).click();
          const completed = await eventually(async () => {
            const value = await api("/v1/run/get", { run_id: run.id });
            return value.status === "completed" && value;
          }, "Provider must continue to final output after GUI decision");
          assert.equal(completed.tool_approvals[0].status, action === "approve" ? "consumed" : "denied");
          assert.equal(completed.permission_profile.sandbox, "read_only");
          if (cronPromise) { await cronPromise; assert.equal(cronResult.status, "completed"); }
          if (sessionBound) {
            await page.locator("#conversation").getByText("Approval fixture finished", { exact: true }).waitFor();
          } else {
            await page.locator(".run-detail-result").filter({ hasText: "Approval fixture finished" }).waitFor();
            await page.locator(".run-detail-status").filter({ hasText: "Completed" }).waitFor();
          }
          const output = join(sessionBound ? session.workdir : cronSession.workdir, "approved.txt");
          if (action === "approve") assert.equal(await readFile(output, "utf8"), "synthetic approved content");
          else await assert.rejects(readFile(output), { code: "ENOENT" });
          assert.equal(calls.get(id), 2);
          const messages = await api(`/v1/message/list?project_id=${id}`);
          assert.equal(messages.filter((m) => m.message_kind === "tool_result" || m.kind === "tool_result").length, 1);
          await page.screenshot({ path: join(artifacts, `${id}-completed.png`) });
          console.log(`PASS ${id}: GUI decision → worker → unique ToolResult → final reply`);
        }
      }
    }
    // Crash the exact owned daemon while a request is pending, then reopen its SQLite catalog.
    const id = "openai-daemon-restart";
    const workdir = join(directory, id);
    await mkdir(workdir);
    await api("/v1/agent-provider/save", { provider: { id, name: id, kind: "openai", url: `http://127.0.0.1:${modelPort}/${id}`, models: [{ id: "fixture-model", name: "Fixture", reasoning_efforts: [] }] }, secret: "offline-gui-fixture-key" });
    await api("/v1/agent/register", { id, name: id, config: { provider_id: id, model: "fixture-model", reasoning_effort: null } });
    await api("/v1/project/register", { id, name: id, workdir });
    const session = await api("/v1/session/create", { id, project_id: id, agent_id: id });
    const run = await api("/v1/session/submit-message", { session_id: id, text: "Wait for a member decision." });
    const waiting = await eventually(async () => {
      const value = await api("/v1/run/get", { run_id: run.id });
      return value.tool_approvals?.some((a) => a.status === "pending") && value;
    }, "Recovery fixture must reach pending");
    const children = execFileSync("pgrep", ["-P", String(app.process().pid)], { encoding: "utf8" }).trim().split(/\s+/);
    const daemonPids = children.filter((pid) => execFileSync("ps", ["-p", pid, "-o", "comm="], { encoding: "utf8" }).trim() === join(repo, "target/debug/ait-daemon"));
    assert.equal(daemonPids.length, 1, "Only our exact Electron-owned daemon may be terminated");
    process.kill(Number(daemonPids[0]), "SIGKILL");
    await app.close();
    app = undefined;
    restarted = spawn(join(repo, "target/debug/ait-daemon"), ["--database", join(profile, "ait-development.sqlite3"), "--listen", `127.0.0.1:${port}`], { stdio: "ignore" });
    const recovered = await eventually(async () => {
      try { const value = await api("/v1/run/get", { run_id: run.id }); return value.status === "completed" && value; } catch { return false; }
    }, "Restart must expire old authority and continue without side effects");
    assert.equal(recovered.tool_approvals[0].status, "expired");
    assert.ok(recovered.lease_epoch > waiting.lease_epoch);
    await assert.rejects(readFile(join(session.workdir, "approved.txt")), { code: "ENOENT" });
    const stale = await fetch(endpoint + "/v1/run/tool-approval/resolve", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ run_id: run.id, approval_id: waiting.tool_approvals[0].grant.request_id, action: "approve" }) });
    assert.equal(stale.status, 409);
    assert.equal(calls.get(id), 2);
    console.log("PASS daemon crash/restart: pending expired, stale decision rejected, no tool effect replayed");
  } finally {
    await app?.close();
    if (restarted && restarted.exitCode === null) { restarted.kill("SIGTERM"); await once(restarted, "exit"); }
    await new Promise((resolve) => model.close(resolve));
    // Remove only the synthetic credential references from this disposable catalog.
    try {
      const refs = JSON.parse(execFileSync("python3", ["-c", "import sqlite3,json,sys; c=sqlite3.connect(sys.argv[1]); print(json.dumps([json.loads(r[0]) for r in c.execute('SELECT body_json FROM provider_credentials')]))", join(profile, "ait-development.sqlite3")], { encoding: "utf8" }));
      for (const reference of refs) execFileSync("security", ["delete-generic-password", "-s", "ait.agent-provider", "-a", reference], { stdio: "ignore" });
    } catch (error) { console.error("Synthetic Keychain cleanup failed:", error.message); }
    await rm(directory, { recursive: true, force: true });
  }
});
