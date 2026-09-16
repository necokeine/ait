import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

test("creates Crons from Session heads or Message IDs and opens each manual occurrence Session", async (t) => {
  const page = await openFixture(t);
  await page.locator("#crons-nav").click();
  await page.locator("#cron-create").click();
  await page.locator("#cron-name").fill("Daily summary");
  await page.locator("#cron-schedule").fill("0 9 * * *");
  await page.evaluate(() => {
    window.fixture.sessions.find((session) => session.id === "session-a").currentMessageId = "message-a-advanced";
  });
  await page.locator("#cron-save").click();
  await page.locator(".cron-row").waitFor();
  assert.deepEqual(await page.evaluate(() => window.fixture.cronCreates[0]), {
    name: "Daily summary", projectId: "a", baseMessageId: "message-a-advanced", agentId: "agent",
    schedule: "0 9 * * *", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  });

  await page.locator("#cron-create").click();
  await page.locator("#cron-name").fill("Exact node");
  await page.locator("#cron-target-kind").selectOption("message");
  await page.locator("#cron-message").fill("root-a");
  await page.locator("#cron-save").click();
  await page.locator(".cron-row").nth(1).waitFor();
  assert.equal(await page.evaluate(() => window.fixture.cronCreates[1].baseMessageId), "root-a");

  await page.locator(".cron-row").first().locator("[data-cron-run]").click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent.startsWith("Daily summary ·"));
  assert.equal(await page.locator("#sessions-nav").getAttribute("aria-current"), "page");
  assert.equal(await page.evaluate(() => window.fixture.cronRuns.length), 1);
});

test("fails closed when the selected Session leaves its Project before save", async (t) => {
  const page = await openFixture(t);
  await page.locator("#crons-nav").click();
  await page.locator("#cron-create").click();
  await page.locator("#cron-name").fill("Stale target");
  await page.evaluate(() => {
    window.fixture.sessions.find((session) => session.id === "session-a").projectId = "b";
  });
  await page.locator("#cron-save").click();

  await page.locator("#cron-error").filter({ hasText: "Selected Session is no longer available in this Project." }).waitFor();
  assert.equal(await page.evaluate(() => window.fixture.cronCreates.length), 0);
});
