import assert from "node:assert/strict";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installNotices() {
  const f = window.fixture;
  f.noticeCount = 1;
  const view = f.view;
  f.view = (projectId) => ({ ...view(projectId), recoveryNotices: Array.from({ length: f.noticeCount }, (_, index) => ({
    projectId, sessionId: `session-${projectId}`, sessionTitle: `Session ${projectId.toUpperCase()}`,
    runId: `interrupted-${index}`, code: index ? "RUN_RECOVERY_FAILED" : "RUN_CANCELLED",
    message: index ? "Workspace changes were preserved for review. " + "long-path/".repeat(30) : "native Turn did not complete successfully",
  })) });
}

async function assertLayout(page, withNotices) {
  const boxes = await page.evaluate(() => {
    const box = (selector) => {
      const node = document.querySelector(selector);
      const r = node.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height,
        clientHeight: node.clientHeight, scrollHeight: node.scrollHeight };
    };
    return { notices: box("#recovery-notices"), header: box(".conversation-header"),
      messages: box("#conversation-scroll"), composer: box(".composer-wrap"), pane: box(".conversation-pane") };
  });
  if (withNotices) assert.ok(boxes.notices.bottom <= boxes.header.top, JSON.stringify(boxes));
  else assert.equal(boxes.header.top, boxes.pane.top);
  assert.equal(boxes.header.height, 63);
  assert.ok(boxes.header.bottom <= boxes.messages.top);
  assert.ok(boxes.messages.height > 100);
  assert.ok(boxes.messages.bottom <= boxes.composer.top);
  assert.ok(boxes.composer.bottom <= boxes.pane.bottom);
  return boxes;
}

test("interrupted notice stays above the header and opens its Session", async (t) => {
  const page = await openFixture(t, installNotices);
  const notice = page.locator(".recovery-notice");
  await notice.waitFor();
  assert.equal(await notice.locator("strong").textContent(), "Run interrupted");
  await assertLayout(page, true);
  await notice.click();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  await assertLayout(page, true);
  await page.screenshot({ path: "/tmp/ait-recovery-notice-fixed.png" });
});

test("many notices scroll without covering messages or the composer; hidden notices use no space", async (t) => {
  const page = await openFixture(t, installNotices, { viewport: { width: 1000, height: 700 } });
  await page.evaluate(() => { window.fixture.noticeCount = 12; });
  await page.locator('[data-project-id="b"]').click();
  await page.locator(".recovery-notice").nth(11).waitFor({ state: "attached" });
  assert.equal(await page.locator(".recovery-notice strong").nth(1).textContent(), "Workspace recovery needs review");
  const boxes = await assertLayout(page, true);
  assert.ok(boxes.notices.scrollHeight > boxes.notices.clientHeight);
  await page.evaluate(() => { window.fixture.noticeCount = 0; });
  await page.locator('[data-project-id="a"]').click();
  await page.locator("#recovery-notices").waitFor({ state: "hidden" });
  await assertLayout(page, false);
});
