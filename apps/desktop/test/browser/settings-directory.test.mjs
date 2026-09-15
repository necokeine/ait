import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installSettingsFixture() {
  const key = "projects.default_workdir";
  const defaults = { "interface.theme": "dark", "permissions.sandbox": "workspace_write", [key]: "/fixture/Documents" };
  const f = window.fixture;
  f.settingsValues = JSON.parse(sessionStorage.getItem("settings-values") || "null") || { ...defaults };
  f.settingsRevision = 7;
  f.savedSettings = [];
  f.directoryRequests = [];
  f.nextDirectory = null;
  f.directoryFailure = false;
  f.directoryPending = false;
  f.settingsFailure = false;
  const response = () => ({
    schema: { revision: 3, definitions: [{
      id: key, category: "projects", label: "Default work directory",
      description: "Directory offered when creating a Project.", kind: { type: "path" },
      defaultValue: defaults[key], restartRequired: false,
    }] },
    values: structuredClone(f.settingsValues), revision: f.settingsRevision,
  });
  window.ait.settings = async () => response();
  window.ait.saveSettings = async (revision, values) => {
    if (f.settingsFailure || revision !== f.settingsRevision) throw new Error("Settings changed in another client");
    f.savedSettings.push({ revision, values: structuredClone(values) });
    f.settingsValues = structuredClone(values);
    f.settingsRevision++;
    sessionStorage.setItem("settings-values", JSON.stringify(values));
    return response();
  };
  window.ait.resetSettings = async () => {
    f.settingsValues = { ...defaults };
    f.settingsRevision++;
    sessionStorage.setItem("settings-values", JSON.stringify(f.settingsValues));
    return response();
  };
  window.ait.chooseProjectDirectory = async (defaultPath) => {
    f.directoryRequests.push(defaultPath);
    if (f.directoryFailure) throw new Error("Directory picker unavailable");
    if (f.directoryPending) return new Promise((resolve) => { f.resolveDirectory = resolve; });
    return f.nextDirectory;
  };
}

async function openSettings(page) {
  await page.locator("#settings-trigger").click();
  await page.locator('[data-category="projects"]').click();
  return page.getByLabel("Default work directory", { exact: true });
}

test("default directory is a keyboard-accessible picker, and cancel preserves its value", async (t) => {
  const page = await openFixture(t, installSettingsFixture);
  const control = await openSettings(page);
  assert.equal(await control.getAttribute("title"), "/fixture/Documents");
  assert.equal(await control.evaluate((element) => element.tagName), "BUTTON");
  assert.equal(await page.locator('input[data-setting-id="projects.default_workdir"]').count(), 0);
  await control.focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => window.fixture.directoryRequests.length === 1);
  assert.deepEqual(await page.evaluate(() => window.fixture.directoryRequests), ["/fixture/Documents"]);
  assert.equal(await control.getAttribute("title"), "/fixture/Documents");
  assert.deepEqual(await page.evaluate(() => window.fixture.savedSettings), []);
  assert.equal(await control.evaluate((element) => element === document.activeElement), true);
});

test("choosing a folder updates only the draft; saving preserves it across reload and reset restores Documents", async (t) => {
  const page = await openFixture(t, installSettingsFixture);
  const selected = '/fixture/中文 & <project> "notes"';
  const control = await openSettings(page);
  await page.evaluate((path) => { window.fixture.nextDirectory = path; }, selected);
  await control.click();
  await page.waitForFunction((path) => document.querySelector(".setting-path")?.title === path, selected);
  assert.equal(await control.locator("span").first().textContent(), selected);
  assert.equal(await control.evaluate((element) => element === document.activeElement), true);
  assert.deepEqual(await page.evaluate(() => window.fixture.savedSettings), []);
  await page.locator("#settings-save").click();
  await page.waitForFunction(() => window.fixture.savedSettings.length === 1);
  assert.deepEqual(await page.evaluate(() => window.fixture.savedSettings[0]), {
    revision: 7, values: { "interface.theme": "dark", "permissions.sandbox": "workspace_write", "projects.default_workdir": selected },
  });
  await page.reload();
  await page.locator("#app:not(.is-loading)").waitFor();
  await openSettings(page);
  assert.equal(await control.getAttribute("title"), selected);
  await control.click();
  assert.deepEqual(await page.evaluate(() => window.fixture.directoryRequests), [selected]);
  await page.locator("#settings-reset").click();
  await page.waitForFunction(() => document.querySelector(".setting-path")?.title === "/fixture/Documents");
});

test("cancelling Settings discards a chosen directory", async (t) => {
  const page = await openFixture(t, installSettingsFixture);
  const control = await openSettings(page);
  await page.evaluate(() => { window.fixture.nextDirectory = "/fixture/Unsaved"; });
  await control.click();
  await page.locator("#settings-cancel").click();
  await openSettings(page);
  assert.equal(await control.getAttribute("title"), "/fixture/Documents");
  assert.deepEqual(await page.evaluate(() => window.fixture.savedSettings), []);
});

test("picker failure preserves the draft and allows retry", async (t) => {
  const page = await openFixture(t, installSettingsFixture);
  const control = await openSettings(page);
  await page.evaluate(() => { window.fixture.directoryFailure = true; });
  await control.click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await control.getAttribute("title"), "/fixture/Documents");
  assert.equal(await control.isDisabled(), false);
  await page.evaluate(() => { window.fixture.directoryFailure = false; window.fixture.nextDirectory = "/fixture/Retry"; });
  await control.click();
  await page.waitForFunction(() => document.querySelector(".setting-path")?.title === "/fixture/Retry");
});

for (const action of ["cancel", "reset"]) {
  test(`a late picker result cannot change Settings after ${action}`, async (t) => {
    const page = await openFixture(t, installSettingsFixture);
    const control = await openSettings(page);
    await page.evaluate(() => { window.fixture.directoryPending = true; });
    await control.click();
    assert.equal(await control.isDisabled(), true);
    await page.locator(`#settings-${action}`).click();
    if (action === "cancel") await openSettings(page);
    await page.evaluate(() => window.fixture.resolveDirectory("/fixture/Stale"));
    await page.waitForFunction(() => !document.querySelector(".setting-path")?.disabled);
    assert.equal(await control.getAttribute("title"), "/fixture/Documents");
    assert.deepEqual(await page.evaluate(() => window.fixture.savedSettings), []);
  });
}

test("a rejected save keeps the selected draft and confirmed setting separate", async (t) => {
  const page = await openFixture(t, installSettingsFixture);
  const control = await openSettings(page);
  await page.evaluate(() => { window.fixture.nextDirectory = "/fixture/Unsaved"; window.fixture.settingsFailure = true; });
  await control.click();
  await page.locator("#settings-save").click();
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await control.getAttribute("title"), "/fixture/Unsaved");
  assert.equal(await page.evaluate(() => window.fixture.settingsValues["projects.default_workdir"]), "/fixture/Documents");
  await page.locator("#settings-cancel").click();
  await openSettings(page);
  assert.equal(await control.getAttribute("title"), "/fixture/Documents");
});
