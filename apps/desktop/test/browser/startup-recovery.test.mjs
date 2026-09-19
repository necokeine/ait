import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installRecoveryFixture() {
  const f = window.fixture;
  f.resetCalls = 0;
  f.resetResult = false;
  f.resetError = "";
  f.recovered = sessionStorage.getItem("startup-recovered") === "yes";
  const projects = window.ait.projects;
  window.ait.projects = async () => {
    if (!f.recovered) throw new Error("LEGACY_RECOVERY_REQUIRED: old database <fixture>");
    return projects();
  };
  window.ait.startupRecovery = async () => ({ databasePath: '/fixture/用户/<old> "db".sqlite3' });
  window.ait.resetStartupDatabase = async () => {
    f.resetCalls++;
    if (f.resetPending) await new Promise((resolve) => { f.finishReset = resolve; });
    if (f.resetError) throw new Error(f.resetError);
    if (f.resetResult) sessionStorage.setItem("startup-recovered", "yes");
    return f.resetResult;
  };
  window.ait.retryStartup = async () => { sessionStorage.setItem("startup-recovered", "yes"); };
}

test("legacy startup offers scoped deletion; cancellation leaves the page actionable", async (t) => {
  const page = await openFixture(t, installRecoveryFixture);
  const reset = page.getByRole("button", { name: "Delete old database…", exact: true });
  await reset.waitFor();
  assert.match(await page.locator(".startup-recovery").textContent(), /No backup will be created/);
  assert.equal(await page.locator(".startup-database-path").textContent(), '/fixture/用户/<old> "db".sqlite3');
  assert.equal(await page.locator(".startup-database-path old").count(), 0);
  assert.deepEqual(await page.evaluate(() => window.fixture.resetCalls), 0);
  assert.equal(await page.locator("#session-title").textContent(), "Startup recovery");
  await page.keyboard.press("Meta+k");
  assert.equal(await page.locator("#command-dialog").isVisible(), false);
  assert.equal(await page.locator("#sidebar").evaluate((element) => element.inert), true);
  await reset.click();
  await page.waitForFunction(() => window.fixture.resetCalls === 1 && !document.querySelector("#startup-reset-database").disabled);
  assert.equal(await page.locator("#startup-recovery-error").isVisible(), false);
  await page.screenshot({ path: "/tmp/ait-startup-recovery.png" });
});

test("pending deletion blocks repeated actions; failure can be retried and success restarts", async (t) => {
  const page = await openFixture(t, installRecoveryFixture);
  const reset = page.locator("#startup-reset-database");
  await page.evaluate(() => { window.fixture.resetPending = true; window.fixture.resetError = "Database is read-only"; });
  await reset.click();
  await page.waitForFunction(() => window.fixture.resetCalls === 1);
  assert.equal(await reset.isDisabled(), true);
  assert.equal(await page.locator("#startup-retry").isDisabled(), true);
  await page.evaluate(() => window.fixture.finishReset());
  await page.getByRole("alert").filter({ hasText: "Database is read-only" }).waitFor();
  assert.equal(await reset.isDisabled(), false);
  await page.evaluate(() => { window.fixture.resetPending = false; window.fixture.resetError = ""; window.fixture.resetResult = true; });
  await reset.click();
  await page.locator("#core-status.is-ready").waitFor();
  assert.equal(await page.locator("#startup-reset-database").count(), 0);
});

test("generic startup errors expose retry without offering database deletion", async (t) => {
  const page = await openFixture(t, () => {
    const f = window.fixture;
    f.catalogFailure = sessionStorage.getItem("retried") !== "yes";
    window.ait.startupRecovery = async () => null;
    window.ait.retryStartup = async () => { sessionStorage.setItem("retried", "yes"); };
  });
  await page.locator("#startup-retry").waitFor();
  assert.equal(await page.locator("#startup-reset-database").count(), 0);
  assert.doesNotMatch(await page.locator("#conversation").textContent(), /build:daemon/);
  await page.locator("#startup-retry").click();
  await page.locator("#core-status.is-ready").waitFor();
});
