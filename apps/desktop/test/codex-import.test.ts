import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";

const electron = `export const app = { isPackaged: false, getPath: () => process.cwd(),
  setPath() {}, setName() {}, on() {}, whenReady: () => new Promise(() => {}) };
  export const BrowserWindow = {}, dialog = {}, ipcMain = {}, shell = {};`;
const hook = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "electron"
      ? { url: `data:text/javascript,${encodeURIComponent(electron)}`, shortCircuit: true }
      : next(specifier, context);
  },
});
const { DaemonClient } = await import("../src/main.js");
hook.deregister();

function fixture() {
  const client = new DaemonClient();
  client.ensureStarted = async () => {};
  const seam = client as unknown as {
    get(path: string, kind: string): Promise<unknown>;
    post(path: string, kind: string, body: unknown): Promise<unknown>;
  };
  return { client, seam };
}

test("native discovery is scoped at HTTP and returns minimal summaries with bound Agents", async () => {
  const { client, seam } = fixture();
  const reads: string[] = [];
  seam.get = async (path, kind) => {
    reads.push(path);
    if (kind === "sessions") return [{ id: "bound", project_id: "project", agent_id: "original", active_run_id: "run" }];
    assert.equal(kind, "codex_threads");
    return [{ thread_id: "thread", name: "Native", preview: "Hello", cwd: "/repo", updated_at: 10,
      archived: true, session_id: "bound", project_id: "project", native_metadata: { secret: "not for renderer" } }];
  };
  assert.deepEqual(await client.request("project.codex-threads", { projectId: "project", providerId: "codex" }), [{
    threadId: "thread", title: "Native", preview: "Hello", cwd: "/repo", updatedAt: 10_000,
    archived: true, sessionId: "bound", agentId: "original", activeRunId: "run",
  }]);
  assert.deepEqual(reads, ["/v1/codex/thread/list?project_id=project&provider_id=codex", "/v1/session/list?project_id=project"]);
});

test("discovery rejects cross-Project bindings and local summaries", async () => {
  const { client, seam } = fixture();
  seam.get = async (_path, kind) => kind === "sessions" ? [] : [{ project_id: "other" }];
  await assert.rejects(client.request("project.codex-threads", { projectId: "project", providerId: "codex" }), /another Project/);
  seam.get = async (_path, kind) => kind === "sessions" ? [{ project_id: "other" }] : [];
  await assert.rejects(client.request("project.codex-threads", { projectId: "project", providerId: "codex" }), /Project/i);
});

test("unbound native summaries accept omitted fields and explicit nulls, returning a normalized bridge shape", async () => {
  const { client, seam } = fixture();
  // CodexThreadView uses skip_serializing_if for its optional Ait bindings and name.
  const unbound = { thread_id: "new", preview: "Native preview", cwd: "/repo", updated_at: 10, archived: false };
  seam.get = async (_path, kind) => kind === "sessions" ? [] : [
    unbound,
    { ...unbound, thread_id: "nulls", name: null, session_id: null, project_id: null },
  ];
  assert.deepEqual(await client.request("project.codex-threads", { projectId: "project", providerId: "codex" }),
    ["new", "nulls"].map((threadId) => ({
      threadId, title: "Native preview", preview: "Native preview", cwd: "/repo", updatedAt: 10_000,
      archived: false, sessionId: null, agentId: null, activeRunId: null,
    })));
});

test("sync returns a receipt without loading conversations or starting model execution", async () => {
  const { client, seam } = fixture();
  seam.get = async () => { throw new Error("Views unavailable"); };
  const posts: unknown[] = [];
  seam.post = async (path, kind, body) => {
    posts.push({ path, kind, body });
    return { id: "session", project_id: "project" };
  };
  const input = { projectId: "project", providerId: "codex", threadId: "thread", agentId: "original" };
  assert.deepEqual(await client.request("project.sync-codex-thread", input), { projectId: "project", sessionId: "session" });
  assert.deepEqual(posts, [{ path: "/v1/codex/thread/sync", kind: "session", body: {
    project_id: "project", provider_id: "codex", thread_id: "thread", agent_id: "original",
  } }]);
  seam.post = async () => ({ id: "session", project_id: "other" });
  await assert.rejects(client.request("project.sync-codex-thread", input), /Project/i);
  await assert.rejects(client.request("project.sync-codex-thread", { ...input, threadId: "" }), /Thread/);
});
