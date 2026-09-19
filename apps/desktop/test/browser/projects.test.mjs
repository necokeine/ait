import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

const sessions = (page, id) => page.locator(`[data-project-group="${id}"] .project-sessions`);
const select = (page, id) => page.locator(`[data-project-id="${id}"]`).click();

test("Projects expand on click and keep independent disclosure states across navigation and refresh", async (t) => {
  const page = await openFixture(t);
  assert.equal(await sessions(page, "a").isVisible(), false);
  assert.equal(await sessions(page, "b").isVisible(), false);
  await select(page, "a");
  await sessions(page, "a").locator('[data-session-id="session-a"]').waitFor();
  await select(page, "b");
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  assert.equal(await sessions(page, "a").isVisible(), true);
  assert.equal(await sessions(page, "b").isVisible(), true);
  await sessions(page, "a").locator('[data-session-id="session-a"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session A");
  await page.locator('[data-project-toggle="b"]').click();
  await page.evaluate(() => window.fixture.emit([{ type: "resync" }]));
  await page.waitForFunction(() => window.fixture.projectReads.length >= 5);
  assert.equal(await sessions(page, "a").isVisible(), true);
  assert.equal(await sessions(page, "b").isVisible(), false);
  await select(page, "a");
  assert.equal(await sessions(page, "a").isVisible(), false);
});

test("offscreen Session summaries refresh without loading another conversation, and retry errors", async (t) => {
  const page = await openFixture(t);
  await page.locator('[data-project-toggle="b"]').click();
  await sessions(page, "b").locator('[data-session-id="session-b"]').waitFor();
  assert.deepEqual(await page.evaluate(() => window.fixture.projectReads), ["a"]);
  await page.evaluate(() => {
    const f = window.fixture;
    f.sessions.push({ ...f.sessions[1], id: "b-new", title: "New B Session" });
    f.finish("run-b");
  });
  await sessions(page, "b").locator('[data-session-id="b-new"]').waitFor();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  await page.evaluate(() => { window.fixture.sessionFailure = true; window.fixture.emit([{ type: "resync" }]); });
  await sessions(page, "b").locator('[role="alert"]').waitFor();
  await page.evaluate(() => { window.fixture.sessionFailure = false; });
  await sessions(page, "b").getByRole("button", { name: "Retry" }).click();
  await sessions(page, "b").locator('[role="alert"]').waitFor({ state: "detached" });
  await sessions(page, "b").locator('[data-session-id="session-b"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  await page.locator("#message-input").fill("Message for B");
  await page.locator("#send-button").click();
  assert.equal(await page.evaluate(() => window.fixture.sent.at(-1).projectId), "b");
});

test("editing saves name and default Agent together, and new Sessions use the new default", async (t) => {
  const page = await openFixture(t);
  await page.locator('[data-project-settings-id="a"]').click();
  await page.locator("#project-settings-action").click();
  assert.equal(await page.locator("#project-settings-name").inputValue(), "Project A");
  await page.locator("#project-settings-name").fill("  Renamed Project  ");
  await page.locator("#project-backend").selectOption("alternate");
  await page.locator("#project-backend-save").click();
  await page.locator("#project-settings-dialog.is-hidden").waitFor({ state: "attached" });
  assert.equal(await page.locator('[data-project-id="a"] strong').textContent(), "Renamed Project");
  assert.deepEqual(await page.evaluate(() => window.fixture.edits), [{ projectId: "a", name: "Renamed Project", agentId: "alternate" }]);
  assert.equal(await page.evaluate(() => window.fixture.sessions[0].agentId), "agent");
  await page.locator('[data-new-session-project-id="a"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "New Session");
  assert.deepEqual(await page.evaluate(() => window.fixture.created), []);
  assert.equal(await page.locator("#composer-agent").inputValue(), "alternate");
  await page.locator("#message-input").fill("First message");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Created Session");
  assert.deepEqual(await page.evaluate(() => window.fixture.created), [{ projectId: "a", agentId: "alternate", sourceMessageId: "root-a", content: "First message" }]);
});

test("cancel and failed saves retain persisted Project data, with drafts available for retry", async (t) => {
  const page = await openFixture(t);
  await page.locator('[data-project-settings-id="b"]').click();
  await page.locator("#project-settings-action").click();
  await page.locator("#project-settings-name").fill("Cancelled");
  await page.locator("#project-settings-cancel").click();
  assert.deepEqual(await page.evaluate(() => window.fixture.edits), []);
  await page.locator('[data-project-settings-id="b"]').click();
  await page.locator("#project-settings-action").click();
  assert.equal(await page.locator("#project-settings-name").inputValue(), "Project B");
  await page.locator("#project-settings-name").fill("   ");
  await page.locator("#project-backend-save").click();
  assert.deepEqual(await page.evaluate(() => window.fixture.edits), []);
  await page.locator("#project-settings-name").fill("Retry B");
  await page.evaluate(() => { window.fixture.updateFailure = true; });
  await page.locator("#project-backend-save").click();
  await page.waitForFunction(() => document.querySelector("#toast").textContent.includes("Project update failed"));
  assert.equal(await page.locator("#project-settings-dialog").isVisible(), true);
  assert.equal(await page.locator('[data-project-id="b"] strong').textContent(), "Project B");
  await page.evaluate(() => { window.fixture.updateFailure = false; });
  await page.locator("#project-backend-save").click();
  await page.waitForFunction(() => document.querySelector('[data-project-id="b"] strong').textContent === "Retry B");
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
});

test("Project names can be edited without an available Agent", async (t) => {
  const page = await openFixture(t, () => { window.fixture.agents = []; });
  await page.locator('[data-project-settings-id="a"]').click();
  await page.locator("#project-settings-action").click();
  assert.equal(await page.locator("#project-backend").isEnabled(), true);
  assert.equal(await page.locator("#project-backend").inputValue(), "agent");
  await page.locator("#project-settings-name").fill("Name only");
  await page.locator("#project-backend-save").click();
  await page.waitForFunction(() => document.querySelector('[data-project-id="a"] strong').textContent === "Name only");
  assert.deepEqual(await page.evaluate(() => window.fixture.edits), [{ projectId: "a", name: "Name only" }]);
  assert.equal(await page.evaluate(() => window.fixture.projects[0].defaultAgentId), "agent");

  await page.locator('[data-project-settings-id="a"]').click();
  await page.locator("#project-settings-action").click();
  await page.locator("#project-backend").selectOption("");
  await page.locator("#project-backend-save").click();
  await page.waitForFunction(() => window.fixture.edits.length === 2);
  assert.deepEqual(await page.evaluate(() => window.fixture.edits[1]), {
    projectId: "a", name: "Name only", agentId: "",
  });
  assert.equal(await page.evaluate(() => window.fixture.projects[0].defaultAgentId), null);
});

test("renaming an offscreen Session preserves the visible conversation", async (t) => {
  const page = await openFixture(t);
  await page.locator('[data-project-toggle="b"]').click();
  await sessions(page, "b").locator('[data-session-id="session-b"]').click({ button: "right" });
  await page.locator("#session-rename-action").click();
  await page.locator("#rename-session-name").fill("Renamed B Session");
  await page.locator("#rename-session-submit").click();
  await page.waitForFunction(() => document.querySelector('[data-session-id="session-b"] strong').textContent === "Renamed B Session");
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
});

test("archives a Session from its context menu and restores it from Settings", async (t) => {
  const page = await openFixture(t);
  await page.locator('[data-project-toggle="a"]').click();
  const sessionA = sessions(page, "a").locator('[data-session-id="session-a"]');

  await sessionA.click({ button: "right" });
  await page.locator("#session-archive-action").click();
  await page.waitForFunction(() => !document.querySelector('[data-session-id="session-a"]'));
  assert.equal(await page.evaluate(() => window.fixture.sessions.find((item) => item.id === "session-a").status), "archived");

  await page.locator("#settings-trigger").click();
  await page.locator('#settings-nav [data-category="archived_sessions"]').click();
  const group = page.locator('[data-archived-project="a"]');
  await group.locator('[data-restore-session="session-a"]').waitFor();
  assert.match(await group.textContent(), /Project A/);
  assert.match(await group.textContent(), /Session A/);

  await group.locator('[data-restore-session="session-a"]').click();
  await page.waitForFunction(() => !document.querySelector('[data-restore-session="session-a"]'));
  assert.equal(await page.evaluate(() => window.fixture.sessions.find((item) => item.id === "session-a").status), "active");
  assert.equal(await sessions(page, "a").locator('[data-session-id="session-a"]').count(), 1);
});

test("keeps available Project archives restorable when another Project is unavailable", async (t) => {
  const page = await openFixture(t, () => {
    window.fixture.sessions.find((item) => item.id === "session-a").status = "archived";
    window.fixture.sessionFailures.add("b");
  });

  await page.locator("#settings-trigger").click();
  await page.locator('#settings-nav [data-category="archived_sessions"]').click();
  const available = page.locator('[data-archived-project="a"]');
  const unavailable = page.locator('[data-archived-project="b"]');
  await available.locator('[data-restore-session="session-a"]').waitFor();
  await unavailable.locator('[role="alert"]').waitFor();

  await available.locator('[data-restore-session="session-a"]').click();
  await page.waitForFunction(() => window.fixture.sessions.find((item) => item.id === "session-a").status === "active");
  assert.equal(await page.locator('[data-restore-session="session-a"]').count(), 0);

  await page.evaluate(() => window.fixture.sessionFailures.delete("b"));
  await unavailable.getByRole("button", { name: "Retry" }).click();
  await unavailable.waitFor({ state: "detached" });
});

test("ignores an archived Session snapshot captured before Restore completes", async (t) => {
  const page = await openFixture(t, () => {
    window.fixture.sessions.find((item) => item.id === "session-a").status = "archived";
  });

  await page.locator("#settings-trigger").click();
  await page.locator('#settings-nav [data-category="archived_sessions"]').click();
  const restore = page.locator('[data-restore-session="session-a"]');
  await restore.waitFor();
  await page.locator("#settings-cancel").click();

  const completedReads = await page.evaluate(() => {
    window.fixture.delayedSessionReadProject = "a";
    return window.fixture.sessionReadReturns.filter((projectId) => projectId === "a").length;
  });
  await page.locator("#settings-trigger").click();
  await page.waitForFunction(() => window.fixture.sessionReadPending);
  await restore.click();
  await page.waitForFunction(() => window.fixture.sessions.find((item) => item.id === "session-a").status === "active");
  assert.equal(await restore.count(), 0);

  await page.evaluate(() => window.fixture.releaseSessionRead());
  await page.waitForFunction((before) => window.fixture.sessionReadReturns.filter((projectId) => projectId === "a").length > before, completedReads);
  assert.equal(await restore.count(), 0);
  assert.equal(await page.locator("#settings-state").textContent(), "0 archived Sessions.");
  assert.equal(await page.evaluate(() => window.fixture.sessions.find((item) => item.id === "session-a").status), "active");
});

test("refreshes and reselects the visible Session after an external archive event", async (t) => {
  const page = await openFixture(t, () => {
    const current = window.fixture.sessions.find((item) => item.id === "session-a");
    window.fixture.sessions.push({ ...current, id: "session-a-older", title: "Older Session A", updatedAt: 0 });
  });
  await page.locator('[data-project-toggle="a"]').click();
  const reads = await page.evaluate(() => window.fixture.projectReads.length);

  await page.evaluate(() => {
    const fixture = window.fixture;
    const current = fixture.sessions.find((item) => item.id === "session-a");
    current.status = "archived";
    fixture.emit([{ type: "event", event: {
      api_version: 1,
      cursor: 11,
      kind: "session.archived",
      entity_id: current.id,
      body: { id: current.id, project_id: current.projectId, status: current.status },
      created_at: 11,
    } }]);
  });

  await page.waitForFunction((before) => window.fixture.projectReads.length > before, reads);
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Older Session A");
  assert.equal(await sessions(page, "a").locator('[data-session-id="session-a"]').count(), 0);
});

test("failed Project navigation preserves the visible Session and its send target", async (t) => {
  const page = await openFixture(t);
  await page.evaluate(() => { window.fixture.viewFailure = true; });
  await select(page, "b");
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  await page.locator("#message-input").fill("Still for A");
  await page.locator("#send-button").click();
  assert.equal(await page.evaluate(() => window.fixture.sent.at(-1).projectId), "a");
});
