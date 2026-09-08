import assert from "node:assert/strict";
import test from "node:test";

import { ProjectViewLoader } from "../src/project-view-loader.js";

interface TestView {
  sessions: Array<{ id: string; projectId: string }>;
  messages: Array<{ projectId: string }>;
  runs: Array<{ projectId: string }>;
}

interface Deferred<Value> {
  promise: Promise<Value>;
  resolve(value: Value): void;
}

function deferred<Value>(): Deferred<Value> {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((accept) => { resolve = accept; });
  return { promise, resolve };
}

function projectView(
  projectId: string,
  sessions: Array<{ id: string; projectId: string }> = [],
): TestView {
  return {
    sessions,
    messages: [{ projectId }],
    runs: [{ projectId }],
  };
}

test("a terminal refresh during a Project switch cannot restore the previous Project", async () => {
  const switchResponse = deferred<TestView>();
  const refreshResponse = deferred<TestView>();
  const responses = [switchResponse, refreshResponse];
  const requestedProjects: Array<string | undefined> = [];
  let backendQueue = Promise.resolve();
  let rememberedProject = "project-a";

  const read = (projectId?: string): Promise<TestView> => {
    requestedProjects.push(projectId);
    const response = responses.shift();
    assert.ok(response, "the test configured one deferred response per view request");
    const result = backendQueue.then(async () => {
      rememberedProject = projectId ?? "";
      return response.promise;
    });
    backendQueue = result.then(() => undefined, () => undefined);
    return result;
  };

  const views = new ProjectViewLoader(read);
  views.replace("project-a", projectView("project-a"));

  const switching = views.select("project-b");
  assert.equal(views.selectedProjectId, "project-b", "the target changes before the read settles");
  const terminalRefresh = views.refresh();

  switchResponse.resolve(projectView("project-b"));
  assert.equal(await switching, false, "the later refresh invalidates the first response");
  refreshResponse.resolve(projectView("project-b"));
  assert.equal(await terminalRefresh, true);

  assert.deepEqual(requestedProjects, ["project-b", "project-b"]);
  assert.equal(views.selectedProjectId, "project-b");
  assert.equal(views.projectId, "project-b");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-b"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-b"]);
  assert.equal(rememberedProject, "project-b");
});

test("a Project switch discards an older refresh response", async () => {
  const refreshResponse = deferred<TestView>();
  const switchResponse = deferred<TestView>();
  const responses = [refreshResponse, switchResponse];
  let rememberedProject = "project-a";

  const read = (projectId?: string): Promise<TestView> => {
    const response = responses.shift();
    assert.ok(response, "the test configured one deferred response per view request");
    rememberedProject = projectId ?? "";
    return response.promise;
  };

  const views = new ProjectViewLoader(read);
  views.replace("project-a", projectView("project-a"));

  const terminalRefresh = views.refresh();
  const switching = views.select("project-b");

  refreshResponse.resolve(projectView("project-a"));
  assert.equal(await terminalRefresh, false);
  assert.equal(views.selectedProjectId, "project-b");

  switchResponse.resolve(projectView("project-b"));
  assert.equal(await switching, true);
  assert.equal(views.projectId, "project-b");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-b"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-b"]);
  assert.equal(rememberedProject, "project-b");
});

test("a Session creation response cannot restore its Project after a later selection", async () => {
  const createResponse = deferred<TestView>();
  const switchResponse = deferred<TestView>();
  const catalogRefreshResponse = deferred<TestView>();
  const viewResponses = [switchResponse, catalogRefreshResponse];
  const requestedProjects: Array<string | undefined> = [];
  let rememberedProject = "project-a";

  const read = (projectId?: string): Promise<TestView> => {
    requestedProjects.push(projectId);
    const response = viewResponses.shift();
    assert.ok(response, "the test configured one deferred response per view request");
    rememberedProject = projectId ?? "";
    return response.promise;
  };
  const views = new ProjectViewLoader(read);
  views.replace("project-a", projectView("project-a"));

  const mutation = views.beginMutation("project-a");
  const creating = (async () => {
    const created = await createResponse.promise;
    if (views.commitMutation(mutation, created)) return "accepted";
    return await views.refresh() ? "refreshed" : "superseded";
  })();
  const switching = views.select("project-b");

  const sessions = [{ id: "session-in-a", projectId: "project-a" }];
  rememberedProject = "project-a";
  createResponse.resolve(projectView("project-a", sessions));
  await Promise.resolve();

  switchResponse.resolve(projectView("project-b"));
  assert.equal(await switching, false, "the catalog refresh supersedes the earlier B read");
  assert.equal(views.selectedProjectId, "project-b");

  catalogRefreshResponse.resolve(projectView("project-b", sessions));
  assert.equal(await creating, "refreshed");

  assert.deepEqual(requestedProjects, ["project-b", "project-b"]);
  assert.equal(views.selectedProjectId, "project-b");
  assert.equal(views.projectId, "project-b");
  assert.deepEqual(views.view?.sessions, sessions, "the current view refreshes the global Session catalog");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-b"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-b"]);
  assert.equal(rememberedProject, "project-b");
});
