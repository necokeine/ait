import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

// Exercise the actual IPC request handler without starting Electron or a daemon.
const electron = `
  export const app = { isPackaged: false, getPath: () => process.cwd(),
    setPath() {}, setName() {}, on() {}, whenReady: () => new Promise(() => {}) };
  export const BrowserWindow = {}, dialog = {}, ipcMain = {}, shell = {};
`;
const hook = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "electron"
      ? { url: `data:text/javascript,${encodeURIComponent(electron)}`, shortCircuit: true }
      : next(specifier, context);
  },
});
const { DaemonClient } = await import("../src/main.js");
hook.deregister();

const input = { projectId: "project", sourceMessageId: "root", agentId: "agent", content: "First input",
  submissionId: "d3bfe617-97fc-4527-a129-c0a7db22b08e" };

test("fork acceptance survives subsequent project reads failing and retries do not create twice", async () => {
  const client = new DaemonClient();
  client.ensureStarted = async () => {};
  const posts: Record<string, unknown>[] = [];
  const runs: Array<{ id: string; project_id: string; session_id: string }> = [];
  const seam = client as unknown as {
    post(path: string, kind: string, body: Record<string, unknown>): Promise<unknown>;
    get(path: string, kind: string): Promise<unknown>;
    projectView(): Promise<unknown>;
  };
  seam.get = async () => runs;
  seam.post = async (_path, _kind, body) => {
    posts.push(body);
    const run = { id: `run-${posts.length}`, project_id: "project", session_id: body.id as string };
    runs.push(run);
    return run;
  };
  seam.projectView = async () => { throw new Error("Read unavailable after acceptance"); };
  const accepted = await client.request("session.fork", input);
  assert.deepEqual(accepted, { status: "accepted", selectedSessionId: input.submissionId, runId: "run-1", reusedCurrentSession: false });
  await assert.rejects(client.request("project.view", { projectId: "project" }), /Read unavailable/);
  assert.deepEqual(await client.request("session.fork", input), accepted);
  assert.equal(posts.length, 1);
  assert.equal(runs.length, 1);
});

function recoveryFixture() {
  const client = new DaemonClient();
  client.ensureStarted = async () => {};
  const seam = client as unknown as {
    post(path: string, kind: string, body: Record<string, unknown>): Promise<unknown>;
    get(path: string, kind: string): Promise<unknown>;
    unwrap(response: Response, kind: string): Promise<unknown>;
  };
  const posts: Record<string, unknown>[] = [];
  const runs: Array<{ id: string; project_id: string; session_id: string }> = [];
  seam.get = async (path, kind) => {
    assert.equal(path, "/v1/run/list?project_id=project");
    assert.equal(kind, "runs");
    return runs;
  };
  const accept = seam.post = async (path, kind, body) => {
    assert.equal(path, "/v1/session/submit-fork");
    assert.equal(kind, "run");
    posts.push(body);
    const existing = runs.find((run) => run.session_id === body.id);
    if (existing) return reject("INVALID_SESSION");
    const run = { id: `run-${posts.length}`, project_id: "project", session_id: body.id as string };
    runs.push(run);
    return run;
  };
  const reject = (code: string) => seam.unwrap(new Response(JSON.stringify({
    ok: false, error: { code, message: "Injected rejection" },
  })), "run");
  return { client, seam, posts, runs, accept, reject };
}

test("a lost POST response recovers the durable receipt", async () => {
  const f = recoveryFixture();
  f.seam.post = async (...args) => {
    await f.accept(...args);
    throw new Error("Connection closed after commit");
  };
  const receipt = await f.client.request("session.fork", input);
  assert.deepEqual(receipt, { status: "accepted", selectedSessionId: input.submissionId, runId: "run-1", reusedCurrentSession: false });
  assert.equal(f.posts.length, 1);
});

test("lost POST and recovery reads return unknown, then recover without another POST", async () => {
  const f = recoveryFixture();
  const read = f.seam.get;
  f.seam.post = async (...args) => {
    await f.accept(...args);
    f.seam.get = async () => { throw new Error("Read unavailable"); };
    throw new Error("Connection closed after commit");
  };
  assert.deepEqual(await f.client.request("session.fork", input), { status: "unknown", message: "Read unavailable" });
  f.seam.get = read;
  const receipt = await f.client.request("session.fork", { ...input, recover: true });
  assert.equal((receipt as { status: string }).status, "accepted");
  assert.equal(f.posts.length, 1);
  assert.equal(f.runs.length, 1);
});

test("an uncommitted transport failure replays the identical Session ID and payload", async () => {
  const f = recoveryFixture();
  let failedBody: Record<string, unknown> | undefined;
  f.seam.post = async (_path, _kind, body) => {
    failedBody = body;
    throw Object.assign(new Error("Connection reset"), { code: "ECONNRESET" });
  };
  assert.deepEqual(await f.client.request("session.fork", input), { status: "unknown", message: "Connection reset" });
  f.seam.post = f.accept;
  await f.client.request("session.fork", { ...input, recover: true });
  assert.deepEqual(f.posts, [failedBody]);
  assert.equal(f.runs.length, 1);
});

test("validation releases a first attempt, but cannot release an uncertain retry", async () => {
  const f = recoveryFixture();
  f.seam.post = async () => f.reject("INVALID_AGENT_CONFIGURATION");
  assert.deepEqual(await f.client.request("session.fork", input), { status: "rejected", message: "Injected rejection" });
  assert.deepEqual(await f.client.request("session.fork", { ...input, recover: true }), { status: "unknown", message: "Injected rejection" });
  f.seam.post = async () => f.reject("INTERNAL_ERROR");
  assert.deepEqual(await f.client.request("session.fork", input), { status: "unknown", message: "Injected rejection" });
});

test("overlapping requests with the same ID recover the same atomic fork", async () => {
  const f = recoveryFixture();
  const results = await Promise.all([
    f.client.request("session.fork", input),
    f.client.request("session.fork", { ...input, recover: true }),
  ]);
  assert.deepEqual(results[0], results[1]);
  assert.equal(f.runs.length, 1);
  assert.ok(f.posts.every((post) => post.id === input.submissionId));
});
