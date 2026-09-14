import assert from "node:assert/strict";
import test from "node:test";
import { isActiveRunStatus, loadActiveRuns } from "../src/active-runs.js";
import { renderActiveRunRows } from "../src/runs-page.js";

function run(project: string, id: string, status = "running", session: string | null = "session") {
  return {
    id, project_id: project, session_id: session, agent_id: "agent", config: { model: "pinned-model" },
    status, phase: "calling_agent", trigger: session ? "manual" : "cron",
    native_approvals: [{ status: "pending" }, { status: "approved" }],
    execution: { private_payload: "must not reach renderer" },
    provider: { url: "must not reach renderer" },
  };
}

test("activity includes every nonterminal lifecycle status and excludes every terminal status", () => {
  for (const status of ["queued", "running", "waiting_approval", "retry_wait", "settling", "cancelling"]) {
    assert.equal(isActiveRunStatus(status), true, status);
  }
  for (const status of ["completed", "failed", "cancelled", "limit_exceeded", "interrupted", "unknown"]) {
    assert.equal(isActiveRunStatus(status), false, status);
  }
});

test("aggregates all Projects including Sessionless Runs with a narrow, scoped projection", async () => {
  const calls: string[] = [];
  const catalog = await loadActiveRuns(async (path, kind) => {
    calls.push(path);
    if (kind === "projects") return [{ id: "p/a b", name: "Code" }, { id: "p2", name: "Scheduled" }];
    if (kind === "runs") return path.includes("p2")
      ? [run("p2", "scheduled", "retry_wait", null)]
      : [run("p/a b", "active"), run("p/a b", "complete", "completed")];
    if (kind === "sessions") return [{ id: "session", project_id: "p/a b", name: "Member name", title: "AI title" }];
    throw new Error(`Unexpected ${path}`);
  });
  assert.deepEqual(catalog.runs, [
    { id: "active", projectId: "p/a b", projectName: "Code", sessionId: "session", sessionTitle: "Member name", agentId: "agent", model: "pinned-model", status: "running", phase: "calling_agent", trigger: "manual", pendingApprovals: 1 },
    { id: "scheduled", projectId: "p2", projectName: "Scheduled", sessionId: null, sessionTitle: null, agentId: "agent", model: "pinned-model", status: "retry_wait", phase: "calling_agent", trigger: "cron", pendingApprovals: 1 },
  ]);
  assert.deepEqual(catalog.unavailableProjects, []);
  assert.deepEqual(calls.toSorted(), ["/v1/project/list", "/v1/run/list?project_id=p%2Fa%20b", "/v1/run/list?project_id=p2", "/v1/session/list?project_id=p%2Fa%20b"].toSorted());
});

test("an unavailable Project does not hide other Projects or imply an empty workspace", async () => {
  const catalog = await loadActiveRuns(async (_path, kind) => {
    if (kind === "projects") return [{ id: "broken", name: "Broken" }, { id: "ok", name: "Working" }];
    if (_path.includes("broken")) throw new Error("offline");
    return [run("ok", "scheduled", "queued", null)];
  });
  assert.equal(catalog.runs[0]?.id, "scheduled");
  assert.deepEqual(catalog.unavailableProjects.map((project) => project.projectId), ["broken"]);
});

test("Session lookup failures retain active Runs with a fallback title and a visible warning", async () => {
  const catalog = await loadActiveRuns(async (_path, kind) => {
    if (kind === "projects") return [{ id: "p", name: "Project" }];
    if (kind === "runs") return [run("p", "active")];
    throw new Error("Session lookup failed");
  });
  assert.equal(catalog.runs[0]?.sessionTitle, "Session session");
  assert.equal(catalog.unavailableProjects.length, 1);
});

test("unexpected Project records cannot be attributed to the requested Project", async () => {
  const catalog = await loadActiveRuns(async (_path, kind) => kind === "projects"
    ? [{ id: "p", name: "Project" }] : [run("wrong-project", "active")]);
  assert.equal(catalog.runs.length, 0);
  assert.equal(catalog.unavailableProjects[0]?.projectId, "p");
});

test("the catalog limits concurrent Project reads and skips Session reads for idle Projects", async () => {
  let inFlight = 0;
  let peak = 0;
  let reads = 0;
  const catalog = await loadActiveRuns(async (_path, kind) => {
    if (kind === "projects") return Array.from({ length: 12 }, (_, id) => ({ id: String(id), name: String(id) }));
    assert.equal(kind, "runs");
    reads += 1;
    peak = Math.max(peak, ++inFlight);
    await new Promise((resolve) => setImmediate(resolve));
    inFlight -= 1;
    return [];
  });
  assert.equal(reads, 12);
  assert.equal(peak, 4);
  assert.deepEqual(catalog, { runs: [], unavailableProjects: [] });
});

test("global catalog errors propagate so a failed read cannot masquerade as no active Runs", async () => {
  await assert.rejects(loadActiveRuns(async () => { throw new Error("disconnected"); }), /disconnected/);
});

test("Runs rendering escapes labels and offers navigation only for Runs with a Session", async () => {
  const catalog = await loadActiveRuns(async (_path, kind) => {
    if (kind === "projects") return [{ id: 'p"<', name: '<img src=x onerror="bad()">' }];
    if (kind === "runs") return [run('p"<', "interactive"), run('p"<', "scheduled", "queued", null)];
    return [{ id: "session", project_id: 'p"<', title: "<script>alert(1)</script>" }];
  });
  const html = renderActiveRunRows(catalog.runs, []);
  assert.ok(!html.includes("<script>"));
  assert.ok(!html.includes("<img"));
  assert.match(html, /&lt;script&gt;/);
  assert.equal((html.match(/data-run-id=/g) ?? []).length, 1);
  assert.match(html, /Waiting for approval/);
  assert.match(html, /Scheduled Run/);
  assert.match(html, /No Session/);
});
