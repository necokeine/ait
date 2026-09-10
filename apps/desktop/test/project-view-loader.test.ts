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
  reject(reason: unknown): void;
}

function deferred<Value>(): Deferred<Value> {
  let resolve!: (value: Value) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<Value>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
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

  catalogRefreshResponse.resolve(projectView("project-b"));
  assert.equal(await creating, "refreshed");

  assert.deepEqual(requestedProjects, ["project-b", "project-b"]);
  assert.equal(views.selectedProjectId, "project-b");
  assert.equal(views.projectId, "project-b");
  assert.deepEqual(views.view?.sessions, [], "the current view excludes the previous Project's Sessions");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-b"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-b"]);
  assert.equal(rememberedProject, "project-b");
});

test("a failed cross-Project Session creation keeps the previous Project", async () => {
  const createResponse = deferred<TestView>();
  const recoveryResponse = deferred<TestView>();
  let rememberedProject = "project-b";
  const views = new ProjectViewLoader((projectId) => {
    rememberedProject = projectId ?? "";
    return recoveryResponse.promise;
  });
  views.replace("project-b", projectView("project-b"));

  const mutation = views.beginMutation("project-a");
  const creating = (async () => {
    try {
      await createResponse.promise;
      return "unexpected success";
    } catch {
      views.discardMutation(mutation);
      return await views.refresh() ? "recovered" : "superseded";
    }
  })();

  assert.equal(views.selectedProjectId, "project-b", "a pending mutation does not change the target");
  rememberedProject = "project-a";
  createResponse.reject(new Error("daemon unavailable"));
  await Promise.resolve();
  recoveryResponse.resolve(projectView("project-b"));
  assert.equal(await creating, "recovered");

  assert.equal(views.selectedProjectId, "project-b");
  assert.equal(views.projectId, "project-b");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-b"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-b"]);
  assert.equal(rememberedProject, "project-b");
});

test("a failed Session creation cannot restore its Project after a later selection", async () => {
  const createResponse = deferred<TestView>();
  const switchResponse = deferred<TestView>();
  const recoveryResponse = deferred<TestView>();
  const responses = [switchResponse, recoveryResponse];
  let rememberedProject = "project-b";
  const views = new ProjectViewLoader((projectId) => {
    rememberedProject = projectId ?? "";
    const response = responses.shift();
    assert.ok(response, "the test configured one deferred response per view request");
    return response.promise;
  });
  views.replace("project-b", projectView("project-b"));

  const mutation = views.beginMutation("project-a");
  const creating = (async () => {
    try {
      await createResponse.promise;
      return "unexpected success";
    } catch {
      views.discardMutation(mutation);
      return await views.refresh() ? "recovered" : "superseded";
    }
  })();
  const switching = views.select("project-c");

  rememberedProject = "project-a";
  createResponse.reject(new Error("storage failure"));
  await Promise.resolve();
  assert.equal(views.selectedProjectId, "project-c");

  switchResponse.resolve(projectView("project-c"));
  assert.equal(await switching, false, "the recovery refresh supersedes the earlier C read");
  recoveryResponse.resolve(projectView("project-c"));
  assert.equal(await creating, "recovered");
  assert.equal(views.selectedProjectId, "project-c");
  assert.equal(views.projectId, "project-c");
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-c"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-c"]);
  assert.equal(rememberedProject, "project-c");
});

test("a same-Project refresh does not prevent selecting a newly created Session", async () => {
  const createResponse = deferred<{ view: TestView; selectedSessionId: string }>();
  const refreshResponse = deferred<TestView>();
  const finalResponse = deferred<TestView>();
  const responses = [refreshResponse, finalResponse];
  let rememberedProject = "project-a";
  const views = new ProjectViewLoader((projectId) => {
    rememberedProject = projectId ?? "";
    const response = responses.shift();
    assert.ok(response, "the test configured one deferred response per view request");
    return response.promise;
  });
  views.replace("project-a", projectView("project-a", [{ id: "old-session", projectId: "project-a" }]));
  let selectedSessionId = "old-session";

  const mutation = views.beginMutation("project-a");
  const creating = (async () => {
    const result = await createResponse.promise;
    if (!views.commitMutation(mutation, result.view)) return "superseded";
    selectedSessionId = result.selectedSessionId;
    return await views.select("project-a") ? "selected" : "stale";
  })();
  const terminalRefresh = views.refresh();

  const sessions = [
    { id: "old-session", projectId: "project-a" },
    { id: "new-session", projectId: "project-a" },
  ];
  createResponse.resolve({
    view: projectView("project-a", sessions),
    selectedSessionId: "new-session",
  });
  await Promise.resolve();

  refreshResponse.resolve(projectView("project-a", sessions));
  assert.equal(await terminalRefresh, false, "the committed mutation supersedes the older refresh response");
  finalResponse.resolve(projectView("project-a", sessions));
  assert.equal(await creating, "selected");

  assert.equal(selectedSessionId, "new-session");
  assert.equal(views.selectedProjectId, "project-a");
  assert.equal(views.projectId, "project-a");
  assert.deepEqual(views.view?.sessions, sessions);
  assert.deepEqual(views.view?.messages.map(({ projectId }) => projectId), ["project-a"]);
  assert.deepEqual(views.view?.runs.map(({ projectId }) => projectId), ["project-a"]);
  assert.equal(rememberedProject, "project-a");
});
