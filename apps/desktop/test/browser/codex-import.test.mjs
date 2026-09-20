import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installImportFixture() {
  const f = window.fixture;
  const readAgents = window.ait.agents;
  window.ait.agents = async () => {
    const catalog = await readAgents();
    catalog.providers.push({ ...catalog.providers[0], id: "second", name: "Second Codex" });
    catalog.agents.push({ ...catalog.agents[0], id: "second-agent", name: "Second Agent", config: { ...catalog.agents[0].config, provider_id: "second" } });
    catalog.agents.push({ ...catalog.agents[0], id: "private", ownerSessionId: "session-a" });
    catalog.agents.push({ ...catalog.agents[0], id: "disabled", enabled: false });
    return catalog;
  };
  f.codexLists = []; f.codexSyncs = []; f.codexFailure = false;
  f.codexThreads = [
    { threadId: "new", title: "Native <script> session", preview: "A native conversation", cwd: "/fixture/a",
      updatedAt: 1_800_000_000_000, archived: false, sessionId: null, agentId: null, activeRunId: null },
    { threadId: "bound", title: "Existing Codex session", preview: "Already imported", cwd: "/fixture/a",
      updatedAt: 1_800_000_000_000, archived: true, sessionId: "session-a", agentId: "agent", activeRunId: null },
  ];
  window.ait.codexThreads = async (input) => {
    f.codexLists.push(input);
    if (f.codexListDelay) await new Promise((resolve) => { f.releaseCodexList = resolve; });
    if (f.codexListFailure) throw new Error("Codex unavailable");
    return structuredClone(f.codexThreads);
  };
  window.ait.syncCodexThread = async (input) => {
    f.codexSyncs.push(input);
    if (f.codexSyncDelay) await new Promise((resolve) => { f.releaseCodexSync = resolve; });
    if (f.codexFailure && input.threadId === "bound") throw new Error("Sync failed; retry");
    const sessionId = input.threadId === "bound" ? "session-a" : `imported-${input.projectId}`;
    if (!f.sessions.some((session) => session.id === sessionId)) f.sessions.push({ ...f.sessions[0],
      id: sessionId, projectId: input.projectId, title: "Imported Codex", agentId: input.agentId });
    return { projectId: input.projectId, sessionId };
  };
}

async function openImport(page, projectId = "a") {
  await page.locator(`[data-project-settings-id="${projectId}"]`).click();
  await page.locator("#project-codex-action").click();
  await page.locator('#codex-thread-list[aria-busy="false"]').waitFor();
}

test("Project menu discovers on demand, then imports selected history and preserves bound Agents", async (t) => {
  const page = await openFixture(t, installImportFixture);
  assert.deepEqual(await page.evaluate(() => window.fixture.codexLists), []);
  await openImport(page);
  assert.deepEqual(await page.evaluate(() => window.fixture.codexLists), [{ projectId: "a", providerId: "codex" }]);
  assert.equal(await page.locator('[data-codex-thread="new"] strong').textContent(), "Native <script> session");
  assert.equal(await page.locator("#codex-import-submit").isDisabled(), true);
  await page.locator("#codex-agent").selectOption("alternate");
  await page.locator("#codex-select-all").check();
  if (process.env.AIT_CODEX_IMPORT_SCREENSHOT) await page.screenshot({ path: process.env.AIT_CODEX_IMPORT_SCREENSHOT });
  await page.locator("#codex-import-submit").click();
  await page.waitForFunction(() => document.querySelector("#codex-import-status").textContent === "0 selected · 2 completed");
  assert.deepEqual(await page.evaluate(() => window.fixture.codexSyncs), [
    { projectId: "a", providerId: "codex", threadId: "new", agentId: "alternate" },
    { projectId: "a", providerId: "codex", threadId: "bound", agentId: "agent" },
  ]);
  assert.deepEqual(await page.evaluate(() => window.fixture.sent), []);
  assert.deepEqual(await page.evaluate(() => window.fixture.forks), []);
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
});

test("partial failure retries only failed rows and refreshes an offscreen Project", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await page.evaluate(() => { window.fixture.codexFailure = true; });
  await openImport(page, "b");
  await page.locator("#codex-select-all").check();
  await page.locator("#codex-import-submit").click();
  await page.locator('[data-codex-thread="bound"] [role="alert"]').waitFor();
  assert.equal(await page.locator('[data-thread-select="new"]').isDisabled(), true);
  await page.evaluate(() => { window.fixture.codexFailure = false; });
  await page.locator("#codex-import-submit").click();
  await page.waitForFunction(() => document.querySelector("#codex-import-status").textContent === "0 selected · 2 completed");
  assert.deepEqual(await page.evaluate(() => window.fixture.codexSyncs.map((r) => r.threadId)), ["new", "bound", "bound"]);
  assert.ok((await page.evaluate(() => window.fixture.sessionReads)).includes("b"));
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  assert.deepEqual(await page.evaluate(() => window.fixture.projectReads), ["a"]);
});

test("discovery failures retry and empty results do not allow submission", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await page.evaluate(() => { window.fixture.codexListFailure = true; });
  await openImport(page);
  assert.match(await page.locator("#codex-thread-list").textContent(), /Codex unavailable/);
  await page.evaluate(() => { window.fixture.codexListFailure = false; window.fixture.codexThreads = []; });
  await page.locator("#codex-refresh").click();
  await page.getByText(/No matching Codex sessions/).waitFor();
  assert.equal(await page.locator("#codex-import-submit").isDisabled(), true);
});

test("late discovery after closing cannot overwrite a new Project dialog", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await page.evaluate(() => { window.fixture.codexListDelay = true; });
  await page.locator('[data-project-settings-id="a"]').click();
  await page.locator("#project-codex-action").click();
  await page.waitForFunction(() => typeof window.fixture.releaseCodexList === "function");
  await page.locator("#codex-import-close").click();
  await page.evaluate(() => { window.fixture.codexListDelay = false; });
  await openImport(page, "b");
  await page.evaluate(() => { window.fixture.codexThreads = []; window.fixture.releaseCodexList(); });
  await page.waitForFunction(() => document.querySelectorAll("[data-codex-thread]").length === 2);
  assert.match(await page.locator("#codex-import-dialog .eyebrow").textContent(), /Project B/);
  await page.locator('[data-thread-select="new"]').check();
  await page.locator("#codex-import-submit").click();
  await page.waitForFunction(() => window.fixture.codexSyncs.length === 1);
  assert.equal(await page.evaluate(() => window.fixture.codexSyncs[0].projectId), "b");
});

test("closing an in-flight batch stops subsequent imports and keeps completion in its original Project", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await openImport(page);
  await page.evaluate(() => { window.fixture.codexSyncDelay = true; });
  await page.locator("#codex-select-all").check();
  await page.locator("#codex-import-submit").click();
  await page.waitForFunction(() => typeof window.fixture.releaseCodexSync === "function");
  assert.equal(await page.locator("#codex-import-submit").isDisabled(), true);
  await page.locator("#codex-import-close").click();
  await page.locator('[data-project-id="b"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  await openImport(page, "b");
  await page.evaluate(() => { window.fixture.releaseCodexSync(); });
  await page.waitForFunction(() => window.fixture.sessionReads.includes("a"));
  assert.equal(await page.evaluate(() => window.fixture.codexSyncs.length), 1);
  assert.equal(await page.locator("#session-title").textContent(), "Session B");
  assert.equal(await page.locator("#codex-import-status").textContent(), "0 selected");
});

test("active Runs and unavailable bound Agents cannot be selected", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await page.evaluate(() => {
    window.fixture.codexThreads[0].sessionId = "active";
    window.fixture.codexThreads[0].activeRunId = "run";
    window.fixture.codexThreads[1].agentId = "missing";
  });
  await openImport(page);
  assert.equal(await page.locator('[data-thread-select="new"]').isDisabled(), true);
  assert.equal(await page.locator('[data-thread-select="bound"]').isDisabled(), true);
  assert.equal(await page.locator("#codex-select-all").isDisabled(), true);
  assert.match(await page.locator("#codex-thread-list").textContent(), /An Ait Run is active/);
});

test("changing Provider invalidates stale lists and selects only its enabled global Agents", async (t) => {
  const page = await openFixture(t, installImportFixture);
  await page.evaluate(() => { window.fixture.codexListDelay = true; });
  await page.locator('[data-project-settings-id="a"]').click();
  await page.locator("#project-codex-action").click();
  await page.waitForFunction(() => typeof window.fixture.releaseCodexList === "function");
  assert.deepEqual(await page.locator("#codex-agent option").evaluateAll((options) => options.map((option) => option.value)), ["agent", "alternate"]);
  await page.evaluate(() => { window.fixture.codexListDelay = false; });
  await page.locator("#codex-provider").selectOption("second");
  await page.locator('#codex-thread-list[aria-busy="false"]').waitFor();
  assert.equal(await page.locator("#codex-agent").inputValue(), "second-agent");
  assert.deepEqual(await page.evaluate(() => window.fixture.codexLists), [
    { projectId: "a", providerId: "codex" }, { projectId: "a", providerId: "second" },
  ]);
  await page.evaluate(() => { window.fixture.codexThreads = []; window.fixture.releaseCodexList(); });
  await page.locator('[data-thread-select="new"]').check();
  await page.locator("#codex-import-submit").click();
  await page.waitForFunction(() => window.fixture.codexSyncs.length === 1);
  assert.deepEqual(await page.evaluate(() => window.fixture.codexSyncs[0]), {
    projectId: "a", providerId: "second", threadId: "new", agentId: "second-agent",
  });
});

test("Project menu and import dialog are keyboard accessible and restore focus", async (t) => {
  const page = await openFixture(t, installImportFixture);
  const trigger = page.locator('[data-project-settings-id="a"]');
  await trigger.focus();
  await page.keyboard.press("Enter");
  assert.equal(await page.locator("#project-open-action").evaluate((el) => el === document.activeElement), true);
  await page.keyboard.press("ArrowDown");
  assert.equal(await page.locator("#project-codex-action").evaluate((el) => el === document.activeElement), true);
  await page.keyboard.press("ArrowDown");
  assert.equal(await page.locator("#project-settings-action").evaluate((el) => el === document.activeElement), true);
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await page.locator('#codex-thread-list[aria-busy="false"]').waitFor();
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.locator('[data-thread-select="bound"]').evaluate((el) => el === document.activeElement), true);
  await page.keyboard.press("Tab");
  assert.equal(await page.locator("#codex-import-close").evaluate((el) => el === document.activeElement), true);
  await page.keyboard.press("Escape");
  assert.equal(await trigger.evaluate((el) => el === document.activeElement), true);
});
