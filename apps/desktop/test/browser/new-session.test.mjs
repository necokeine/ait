import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

async function openDraft(page, projectId = "a") {
  await page.locator(`[data-new-session-project-id="${projectId}"]`).click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "New Session");
}

test("an empty Project can open, repeat and cancel a new Session without persisting anything", async (t) => {
  const page = await openFixture(t, () => { window.fixture.sessions = []; window.fixture.runs = []; });
  for (let i = 0; i < 2; i++) {
    await openDraft(page);
    assert.equal(await page.locator("#message-input").isDisabled(), false);
    assert.equal(await page.locator("#send-button").isDisabled(), true);
    await page.locator("#message-input").fill("   ");
    await page.locator("#message-input").press("Control+Enter");
    assert.equal(await page.locator("#tree-list [data-message-id]").count(), 1);
    assert.equal(await page.locator('#tree-list [data-message-id="root-a"]').count(), 1);
    assert.equal(await page.locator("#conversation .message").count(), 0);
    await page.locator("#clear-branch").click();
    assert.equal(await page.locator("#session-title").textContent(), "No session selected");
  }
  assert.deepEqual(await page.evaluate(() => ({ forks: window.fixture.forks, sessions: window.fixture.sessions, runs: window.fixture.runs })),
    { forks: [], sessions: [], runs: [] });
});

test("a draft inherits the global Default Agent when the Project has no override", async (t) => {
  const page = await openFixture(t, () => {
    window.fixture.projects[0].defaultAgentId = null;
    const settings = window.ait.settings;
    window.ait.settings = async () => {
      const response = await settings();
      response.values["agents.default_agent"] = "alternate";
      return response;
    };
  });
  await openDraft(page);
  assert.equal(await page.locator("#composer-agent").inputValue(), "alternate");
  assert.deepEqual(await page.evaluate(() => window.fixture.created), []);
  await page.locator("#message-input").fill("Use the global default");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.equal(await page.evaluate(() => window.fixture.created[0].agentId), "alternate");
});

test("the first input creates one Session from the Project root even while another Session runs", async (t) => {
  const page = await openFixture(t);
  const original = await page.evaluate(() => structuredClone(window.fixture.sessions[1]));
  await openDraft(page, "b");
  assert.equal(await page.locator("#composer-agent").inputValue(), "agent");
  await page.locator("#composer-config-trigger").click();
  await page.locator("#composer-agent").selectOption("alternate");
  await page.locator("#composer-config-trigger").click();
  await page.evaluate(() => window.fixture.emit([{ type: "resync" }]));
  await page.waitForFunction(() => window.fixture.projectReads.filter((id) => id === "b").length >= 2);
  assert.equal(await page.locator("#composer-agent").inputValue(), "alternate");
  assert.equal(await page.evaluate(() => window.fixture.sessions.length), 2);
  await page.locator("#message-input").fill("First input");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.deepEqual(await page.evaluate(() => window.fixture.forks), [{ projectId: "b", sourceMessageId: "root-b", agentId: "alternate", content: "First input" }]);
  assert.equal(await page.evaluate(() => window.fixture.sessions.length), 3);
  assert.deepEqual(await page.evaluate(() => window.fixture.sessions[1]), original);
  assert.equal(await page.locator("#message-input").isDisabled(), true, "the accepted Run now locks its real Session");
});

test("a rejected first message retains the draft, text and Agent for retry without an empty Session", async (t) => {
  const page = await openFixture(t, () => { window.fixture.forkFailure = true; });
  await openDraft(page);
  await page.locator("#message-input").fill("Keep this input");
  await page.locator("#send-button").click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.locator("#session-title").textContent(), "New Session");
  assert.equal(await page.locator("#message-input").inputValue(), "Keep this input");
  assert.equal(await page.locator("#message-input").isDisabled(), false);
  assert.equal(await page.evaluate(() => window.fixture.sessions.length), 2);
  await page.evaluate(() => { window.fixture.forkFailure = false; });
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
});

test("repeated submit is blocked and a late response cannot replace a Session opened afterward", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      window.fixture.submissions = (window.fixture.submissions ?? 0) + 1;
      await new Promise((resolve) => { window.fixture.releaseFork = resolve; });
      return fork(input);
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Delayed first input");
  await page.locator("#send-button").click();
  assert.equal(await page.locator("#send-button").isDisabled(), true);
  await page.evaluate(() => document.querySelector("#composer").dispatchEvent(new Event("submit", { cancelable: true })));
  assert.equal(await page.evaluate(() => window.fixture.submissions), 1);
  await page.locator('[data-session-id="session-a"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session A");
  await page.locator("#message-input").fill("Input for the original Session");
  await page.evaluate(() => window.fixture.releaseFork());
  await page.locator('[data-session-id="created"]').waitFor();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  assert.equal(await page.locator("#message-input").inputValue(), "Input for the original Session");
});

test("the command palette opens a draft and navigating away discards it without creating a Session", async (t) => {
  const page = await openFixture(t);
  await page.keyboard.press("Control+k");
  await page.locator('[data-command="new-session"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "New Session");
  await page.locator("#message-input").fill("Discard this draft");
  await page.locator('[data-project-id="b"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  assert.equal(await page.locator("#message-input").inputValue(), "");
  assert.deepEqual(await page.evaluate(() => window.fixture.forks), []);
  assert.equal(await page.evaluate(() => window.fixture.sessions.length), 2);
});

test("an unavailable initial system Message preserves the previous Session", async (t) => {
  const page = await openFixture(t, () => { window.fixture.projects[1].rootMessageId = "missing"; });
  await page.locator('[data-new-session-project-id="b"]').click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  assert.deepEqual(await page.evaluate(() => window.fixture.forks), []);
});

test("a Project with multiple roots uses its declared initial system Message", async (t) => {
  const page = await openFixture(t, () => {
    const view = window.fixture.view;
    window.fixture.view = (projectId) => {
      const result = view(projectId);
      result.messages.unshift({ ...result.messages[0], id: "other-root" });
      return result;
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Use the initial root");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => window.fixture.forks.length === 1);
  assert.equal(await page.evaluate(() => window.fixture.forks[0].sourceMessageId), "root-a");
});

test("late loading of a new draft cannot override subsequent navigation", async (t) => {
  const page = await openFixture(t, () => {
    const project = window.ait.project;
    window.ait.project = async (id) => {
      if (id === "b") await new Promise((resolve) => { window.fixture.releaseProject = resolve; });
      return project(id);
    };
  });
  await page.locator('[data-new-session-project-id="b"]').click();
  await page.waitForFunction(() => typeof window.fixture.releaseProject === "function");
  await page.locator("#agents-nav").click();
  await page.evaluate(() => window.fixture.releaseProject());
  await page.waitForFunction(() => !document.querySelector('[data-new-session-project-id="b"]').disabled);
  assert.equal(await page.locator("#agents-page").isVisible(), true);
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  assert.deepEqual(await page.evaluate(() => window.fixture.forks), []);
});

test("a concurrent rename cannot make an accepted draft submit again", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      await new Promise((resolve) => { window.fixture.releaseFork = resolve; });
      return fork(input);
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Accept once");
  await page.locator("#send-button").click();
  await page.locator('[data-session-id="session-a"]').click({ button: "right" });
  await page.locator("#session-rename-action").click();
  await page.locator("#rename-session-name").fill("Renamed A");
  await page.locator("#rename-session-submit").click();
  await page.locator("#rename-session-dialog.is-hidden").waitFor({ state: "attached" });
  await page.evaluate(() => window.fixture.releaseFork());
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  await page.evaluate(() => document.querySelector("#composer").dispatchEvent(new Event("submit", { cancelable: true })));
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
  assert.equal(await page.locator("#message-input").inputValue(), "");
});

test("failed navigation during acceptance cannot unlock the consumed draft", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      await new Promise((resolve) => { window.fixture.releaseFork = resolve; });
      return fork(input);
    };
    const project = window.ait.project;
    window.ait.project = async (id) => {
      if (id === "b") throw new Error("Navigation failed");
      return project(id);
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Accept once");
  await page.locator("#send-button").click();
  await page.locator('[data-project-id="b"]').click();
  await page.locator("#toast.is-error").waitFor();
  await page.evaluate(() => window.fixture.releaseFork());
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
  assert.equal(await page.locator("#send-button").isDisabled(), true);
});

test("accepted first input retries only the view after reads fail", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      const receipt = await fork(input);
      window.fixture.viewFailure = true;
      return receipt;
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Persist exactly once");
  await page.locator("#send-button").click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.locator("#message-input").isDisabled(), true);
  assert.equal(await page.locator("#send-button").getAttribute("aria-label"), "Open Session");
  // Even another failed recovery must not call the creation endpoint again.
  await page.locator("#send-button").click();
  assert.equal(await page.evaluate(() => window.fixture.forks.length), 1);
  await page.evaluate(() => { window.fixture.viewFailure = false; });
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.deepEqual(await page.evaluate(() => ({
    sessions: window.fixture.created.length,
    messages: window.fixture.createdMessages.length,
    runs: window.fixture.runs.filter((run) => run.sessionId === "created").length,
    posts: window.fixture.forks.length,
  })), { sessions: 1, messages: 1, runs: 1, posts: 1 });
  assert.equal(await page.locator("#message-input").inputValue(), "");
});

test("an unknown receipt freezes the original intent and recovers using the same identifier", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.fixture.attempts = [];
    window.ait.fork = async (input) => {
      window.fixture.attempts.push(input);
      if (window.fixture.receipt) return window.fixture.receipt;
      window.fixture.receipt = await fork(input);
      throw new Error("IPC response lost");
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Original input");
  await page.locator("#send-button").click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.locator("#message-input").isDisabled(), true);
  assert.equal(await page.locator("#composer-agent").isDisabled(), true);
  assert.equal(await page.locator("#send-button").getAttribute("aria-label"), "Check submission");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  const attempts = await page.evaluate(() => window.fixture.attempts);
  assert.ok(attempts[0].submissionId);
  assert.deepEqual(attempts[1], { ...attempts[0], recover: true });
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
  assert.equal(await page.evaluate(() => window.fixture.createdMessages.length), 1);
});

test("acceptance cannot cancel a later navigation that is still loading", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      await new Promise((resolve) => { window.fixture.releaseFork = resolve; });
      return fork(input);
    };
    const project = window.ait.project;
    window.ait.project = async (id) => {
      if (id === "b") await new Promise((resolve) => { window.fixture.releaseNavigation = resolve; });
      return project(id);
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("One accepted input");
  await page.locator("#send-button").click();
  await page.locator('[data-project-id="b"]').click();
  await page.waitForFunction(() => typeof window.fixture.releaseNavigation === "function");
  await page.evaluate(() => window.fixture.releaseFork());
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  await page.evaluate(() => window.fixture.releaseNavigation());
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
});

test("a quickly completed first Run cannot let automatic titles cancel later navigation", async (t) => {
  const page = await openFixture(t, () => {
    const fork = window.ait.fork;
    window.ait.fork = async (input) => {
      await new Promise((resolve) => { window.fixture.releaseFork = resolve; });
      const receipt = await fork(input);
      window.fixture.finish(receipt.runId, "completed", false);
      window.fixture.sessions.find((s) => s.id === receipt.selectedSessionId).titleGenerationStarted = false;
      return receipt;
    };
    const project = window.ait.project;
    window.ait.project = async (id) => {
      if (id === "b") await new Promise((resolve) => { window.fixture.releaseNavigation = resolve; });
      return project(id);
    };
    window.ait.setSessionTitle = async ({ projectId }) => window.fixture.view(projectId);
    window.ait.generateSessionTitle = async ({ projectId }) => {
      window.fixture.titleGenerated = true;
      return window.fixture.view(projectId);
    };
  });
  await openDraft(page);
  await page.locator("#message-input").fill("Fast first Run");
  await page.locator("#send-button").click();
  await page.locator('[data-project-id="b"]').click();
  await page.waitForFunction(() => typeof window.fixture.releaseNavigation === "function");
  await page.evaluate(() => window.fixture.releaseFork());
  await page.waitForFunction(() => window.fixture.titleGenerated);
  await page.evaluate(() => window.fixture.releaseNavigation());
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  assert.equal(await page.evaluate(() => window.fixture.created.length), 1);
});
