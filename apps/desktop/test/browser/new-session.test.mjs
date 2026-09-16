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
