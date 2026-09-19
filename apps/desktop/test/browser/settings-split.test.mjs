import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installSplitSettingsFixture() {
  const f = window.fixture;
  f.settingsRevision = 4;
  f.savedSettings = [];
  f.settingsValues = {
    "agents.default_agent": "agent",
    "agents.small_agent": "agent",
    "agents.max_steps": 128,
    "agents.parallel_tools": 4,
    "network.proxy": "http://retired.invalid",
    "interface.theme": "dark",
    "permissions.sandbox": "workspace_write",
  };
  const definitions = [
    { id: "agents.default_agent", category: "agents", label: "Default Agent", description: "Default", kind: { type: "agent_reference" }, defaultValue: "", restartRequired: false },
    { id: "agents.small_agent", category: "agents", label: "Small Agent", description: "Small", kind: { type: "agent_reference" }, defaultValue: "", restartRequired: false },
    { id: "agents.max_steps", category: "runtime", label: "Maximum steps", description: "Step limit", kind: { type: "number", min: 1, max: 10_000 }, defaultValue: 128, restartRequired: false },
    { id: "agents.parallel_tools", category: "runtime", label: "Parallel tools", description: "Tool limit", kind: { type: "number", min: 1, max: 32 }, defaultValue: 4, restartRequired: false },
    { id: "network.proxy", category: "network", label: "HTTP proxy", description: "Retired", kind: { type: "text" }, defaultValue: "", restartRequired: true },
    { id: "interface.theme", category: "interface", label: "Theme", description: "Theme", kind: { type: "select", options: ["dark"] }, defaultValue: "dark", restartRequired: false },
  ];
  const response = () => ({
    schema: { revision: 5, definitions: structuredClone(definitions) },
    values: structuredClone(f.settingsValues),
    revision: f.settingsRevision,
  });
  window.ait.settings = async () => response();
  window.ait.saveSettings = async (revision, values) => {
    if (revision !== f.settingsRevision) throw new Error("Settings changed in another client");
    f.savedSettings.push({ revision, values: structuredClone(values) });
    f.settingsValues = structuredClone(values);
    f.settingsRevision++;
    return response();
  };
  f.providers = [
    { id: "codex", name: "Codex", kind: "codex", url: null, has_secret: false, models: [{ id: "fixture-model", name: "fixture-model", reasoning_efforts: [] }] },
    { id: "other", name: "Other Provider", kind: "codex", url: null, has_secret: false, models: [{ id: "other-model", name: "other-model", reasoning_efforts: [] }] },
  ];
  f.providerSaves = [];
  f.completedProviderSaves = 0;
  f.delayProviderSave = false;
  f.releaseProviderSave = () => {};
  const agentCatalog = () => ({
    protocolVersion: 1,
    revision: 1,
    agents: structuredClone(f.agents),
    providers: structuredClone(f.providers),
  });
  window.ait.agents = async () => agentCatalog();
  window.ait.discoverProviderModels = async ({ provider }) => structuredClone(provider.models);
  window.ait.saveProvider = async (input) => {
    f.providerSaves.push(structuredClone(input));
    if (f.delayProviderSave) await new Promise((resolve) => { f.releaseProviderSave = resolve; });
    const index = f.providers.findIndex((provider) => provider.id === input.provider.id);
    f.providers[index] = { ...structuredClone(input.provider), has_secret: f.providers[index].has_secret };
    f.completedProviderSaves++;
    return agentCatalog();
  };
  f.sessionReadFailures = new Set();
  f.delayedSessionReads = new Set();
  f.releaseSessionReads = {};
  window.ait.projectSessions = async (projectId, status = "active") => {
    f.sessionReads.push(projectId);
    const snapshot = structuredClone(f.sessions.filter((session) => session.projectId === projectId && session.status === status));
    if (f.delayedSessionReads.has(projectId)) {
      await new Promise((resolve) => { f.releaseSessionReads[projectId] = resolve; });
    }
    if (f.sessionReadFailures.has(projectId)) throw new Error("Sessions unavailable");
    return snapshot;
  };
  const activeA = f.sessions.find((session) => session.projectId === "a");
  const activeB = f.sessions.find((session) => session.projectId === "b");
  f.sessions.push(
    { ...structuredClone(activeA), id: "archived-a", title: "Archived Alpha", description: "Alpha history", status: "archived", active: false, activeRunId: null, updatedAt: 30 },
    { ...structuredClone(activeB), id: "archived-b", title: "Archived Beta", description: "Beta history", status: "archived", active: false, activeRunId: null, updatedAt: 20 },
  );
}

test("Agent roles are edited on Agents and rendered as badges", async (t) => {
  const page = await openFixture(t, installSplitSettingsFixture);
  await page.locator("#agents-nav").click();

  assert.equal(await page.locator("#default-agent-role").inputValue(), "agent");
  assert.equal(await page.locator("#small-agent-role").inputValue(), "agent");
  const fixtureAgent = page.locator(".named-agent-row").filter({ hasText: "Fixture Agent" });
  assert.deepEqual(await fixtureAgent.locator(".catalog-badge").allTextContents(), ["Default Agent", "Small Agent"]);

  await page.locator("#default-agent-role").selectOption("alternate");
  await page.waitForFunction(() => window.fixture.savedSettings.length === 1);
  assert.equal(await page.evaluate(() => window.fixture.savedSettings[0].values["agents.default_agent"]), "alternate");
  assert.deepEqual(await fixtureAgent.locator(".catalog-badge").allTextContents(), ["Small Agent"]);
  assert.deepEqual(
    await page.locator(".named-agent-row").filter({ hasText: "Alternate Agent" }).locator(".catalog-badge").allTextContents(),
    ["Default Agent"],
  );
});

test("Provider configuration uses its own popup and Settings only shows owned categories", async (t) => {
  const page = await openFixture(t, installSplitSettingsFixture);
  await page.locator("#agents-nav").click();
  await page.getByRole("button", { name: "Configure Codex provider" }).click();
  await page.locator("#provider-dialog:not(.is-hidden)").waitFor();
  assert.equal(await page.locator("#settings-dialog").isVisible(), false);
  assert.equal(await page.locator("#provider-dialog-body h3").textContent(), "Configure provider");
  await page.locator("#provider-dialog-close").click();

  await page.locator("#settings-trigger").click();
  assert.deepEqual(await page.locator("#settings-nav button").allTextContents(), ["runtime", "interface", "Archived sessions"]);
  assert.match(await page.locator("#settings-fields").textContent(), /Maximum steps/);
  assert.match(await page.locator("#settings-fields").textContent(), /Parallel tools/);
  assert.doesNotMatch(await page.locator("#settings-dialog").textContent(), /HTTP proxy|Agent providers/);
});

test("a late Provider save updates the catalog without closing a newer Provider draft", async (t) => {
  const page = await openFixture(t, installSplitSettingsFixture);
  await page.locator("#agents-nav").click();
  await page.getByRole("button", { name: "Configure Codex provider" }).click();
  await page.locator("#provider-name").fill("Saved A");
  await page.locator("#provider-next").click();
  await page.locator("#provider-save").waitFor();
  await page.evaluate(() => { window.fixture.delayProviderSave = true; });
  await page.locator("#provider-save").click();
  await page.waitForFunction(() => window.fixture.providerSaves.length === 1);

  await page.locator("#provider-dialog-close").click();
  await page.getByRole("button", { name: "Configure Other Provider provider" }).click();
  await page.locator("#provider-name").fill("Unsaved B draft");
  await page.evaluate(() => window.fixture.releaseProviderSave());
  await page.waitForFunction(() => window.fixture.completedProviderSaves === 1);

  await page.locator("#provider-dialog:not(.is-hidden)").waitFor();
  assert.equal(await page.locator("#provider-name").inputValue(), "Unsaved B draft");
  assert.equal(await page.evaluate(() => window.fixture.providers[0].name), "Saved A");
});

test("Archived Sessions are loaded for every Project and grouped in Settings", async (t) => {
  const page = await openFixture(t, installSplitSettingsFixture);
  await page.locator("#settings-trigger").click();
  await page.locator('[data-category="archived_sessions"]').click();
  await page.locator("#settings-fields").getByText("Archived Alpha", { exact: true }).waitFor();

  assert.deepEqual(await page.locator(".archived-project h4").allTextContents(), ["Project A", "Project B"]);
  assert.deepEqual(await page.locator(".archived-session-row strong").allTextContents(), ["Archived Alpha", "Archived Beta"]);
  assert.deepEqual((await page.evaluate(() => window.fixture.sessionReads)).toSorted(), ["a", "b"]);
});

test("a stale archive refresh cannot restore a Session that was restored while it loaded", async (t) => {
  const page = await openFixture(t, installSplitSettingsFixture);
  await page.evaluate(() => { window.fixture.sessionReadFailures.add("b"); });
  await page.locator("#settings-trigger").click();
  await page.locator('[data-category="archived_sessions"]').click();
  await page.locator("#settings-fields").getByText("Archived Alpha", { exact: true }).waitFor();
  await page.locator('[data-archived-project="b"] [role="alert"]').waitFor();

  await page.evaluate(() => { window.fixture.delayedSessionReads.add("a"); });
  await page.locator('[data-archived-project="b"] [data-archived-retry]').click();
  await page.waitForFunction(() => Boolean(window.fixture.releaseSessionReads.a));
  await page.getByRole("button", { name: "Restore" }).click();
  await page.waitForFunction(() => window.fixture.sessions.find((session) => session.id === "archived-a").status === "active");
  await page.evaluate(() => window.fixture.releaseSessionReads.a());
  await page.waitForFunction(() => document.querySelector("#settings-state")?.textContent !== "Refreshing archived Sessions…");

  assert.equal(await page.locator("#settings-fields").getByText("Archived Alpha", { exact: true }).count(), 0);
  assert.equal(await page.locator("#settings-state").textContent(), "0 archived Sessions");
  assert.match(await page.locator('[data-archived-project="b"] [role="alert"]').textContent(), /Sessions unavailable/);
  assert.equal(await page.locator('[data-archived-project="b"] [data-archived-retry]').count(), 1);
});
