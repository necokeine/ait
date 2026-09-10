import assert from "node:assert/strict";
import test from "node:test";

import {
  composeDesktopState,
  emptyProjectView,
  eventBelongsToProject,
  projectReadPaths,
  refreshVisibleSlices,
  resolveInitialProjectId,
} from "../src/desktop-slices.js";
import type { AgentCatalog, DesktopProject, ProjectCatalog, ProjectView } from "../src/types.js";

const project = (id: string): DesktopProject => ({
  id,
  name: id,
  workdir: `/${id}`,
  description: "",
  baseCommit: "a".repeat(40),
  defaultAgentId: null,
});

const projectCatalog: ProjectCatalog = {
  protocolVersion: 1,
  revision: 1,
  projects: [project("project-a"), project("project-b")],
};

const agentCatalog: AgentCatalog = {
  protocolVersion: 1,
  revision: 2,
  agents: [],
  providers: [],
};

function projectView(projectId: string): ProjectView {
  return {
    ...emptyProjectView(),
    projectId,
    revision: 3,
    sessions: [{
      id: `session-${projectId}`,
      projectId,
      name: "",
      title: projectId,
      description: "",
      titleGenerationStarted: false,
      currentMessageId: `message-${projectId}`,
      agentId: "agent",
      version: 1,
      active: false,
      activeRunId: null,
      updatedAt: 1,
    }],
  };
}

test("startup chooses a remembered Project only while it remains in the catalog", () => {
  assert.equal(resolveInitialProjectId([], "deleted"), undefined);
  assert.equal(resolveInitialProjectId([project("only")], undefined), "only");
  assert.equal(resolveInitialProjectId(projectCatalog.projects, "project-b"), "project-b");
  assert.equal(resolveInitialProjectId(projectCatalog.projects, "deleted"), "project-a");
});

test("renderer composition keeps the global Project catalog but only one Project data slice", () => {
  const stateA = composeDesktopState(projectCatalog, agentCatalog, projectView("project-a"));
  assert.deepEqual(stateA.projects.map(({ id }) => id), ["project-a", "project-b"]);
  assert.deepEqual(stateA.sessions.map(({ projectId }) => projectId), ["project-a"]);

  const stateB = composeDesktopState(projectCatalog, agentCatalog, projectView("project-b"));
  assert.deepEqual(stateB.projects.map(({ id }) => id), ["project-a", "project-b"]);
  assert.deepEqual(stateB.sessions.map(({ projectId }) => projectId), ["project-b"]);
});

test("every Project data read path carries the same encoded project_id", () => {
  const paths = projectReadPaths("project/a b");
  assert.equal(paths.length, 4);
  assert.ok(paths.every((path) => path.endsWith("?project_id=project%2Fa%20b")));
  assert.deepEqual(paths.map((path) => path.split("?")[0]), [
    "/v1/session/list",
    "/v1/message/list",
    "/v1/run/list",
    "/v1/run/progress",
  ]);
});

test("Project events cannot invalidate another selected Project", () => {
  assert.equal(eventBelongsToProject({ project_id: "project-a" }, "project-a"), true);
  assert.equal(eventBelongsToProject({ project_id: "project-b" }, "project-a"), false);
  assert.equal(eventBelongsToProject({ reason: "cursor reset" }, "project-a"), true);
});

test("ordinary Project refreshes do not reread either global catalog", async () => {
  const calls: string[] = [];
  const result = await refreshVisibleSlices(
    async () => { calls.push("project-a"); return true; },
    {
      projects: async () => { calls.push("projects"); return projectCatalog; },
      agents: async () => { calls.push("agents"); return agentCatalog; },
    },
    false,
  );

  assert.deepEqual(calls, ["project-a"]);
  assert.deepEqual(result, { projectAccepted: true });
});

test("event reconnect and cursor reset refresh only the visible slice and global catalogs", async () => {
  const calls: string[] = [];
  const result = await refreshVisibleSlices(
    async () => { calls.push("project-a"); return true; },
    {
      projects: async () => { calls.push("projects"); return projectCatalog; },
      agents: async () => { calls.push("agents"); return agentCatalog; },
    },
    true,
  );

  assert.deepEqual(calls.toSorted(), ["agents", "project-a", "projects"]);
  assert.equal(result.projectAccepted, true);
  assert.equal(result.projects, projectCatalog);
  assert.equal(result.agents, agentCatalog);
});
