import assert from "node:assert/strict";
import { before, after, test } from "node:test";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { installRendererFixture } from "./renderer-fixture.mjs";

let browser;
let server;
let url;
before(async () => {
  const root = new URL("../../dist/", import.meta.url);
  // This fixture serves only the built renderer on an ephemeral loopback port.
  server = createServer(async (req, res) => {
    const name = req.url === "/" ? "index.html" : req.url.slice(1);
    if (!/^[\w.-]+$/.test(name)) return res.writeHead(404).end();
    try {
      const data = await readFile(fileURLToPath(new URL(name, root)));
      const type = name.endsWith(".js") ? "text/javascript" : name.endsWith(".css") ? "text/css" : name.endsWith(".html") ? "text/html" : "image/png";
      res.setHeader("Content-Type", type);
      res.end(data);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  url = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({ headless: true,
    ...(process.env.AIT_TEST_CHROMIUM ? { executablePath: process.env.AIT_TEST_CHROMIUM } : {}),
  });
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function openFixture(t) {
  const context = await browser.newContext({ viewport: { width: 1480, height: 920 }, colorScheme: "dark" });
  t.after(() => context.close());
  const page = await context.newPage();
  page.setDefaultTimeout(3_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.addInitScript(installRendererFixture);
  await page.goto(url);
  await page.locator("#app:not(.is-loading)").waitFor();
  return page;
}

async function openRuns(page) {
  await page.locator("#runs-nav").click();
  await page.locator('[data-run-id="run-b"]').waitFor();
}

async function sendToVisibleSession(page) {
  await page.locator("#message-input").fill("Send to the visible Session");
  const visibleTitle = await page.locator("#session-title").textContent();
  await page.locator("#send-button").click();
  const sent = await page.evaluate(() => window.fixture.sent.at(-1));
  const expected = visibleTitle === "Session A" ? { projectId: "a", sessionId: "session-a" }
    : visibleTitle === "Session B" ? { projectId: "b", sessionId: "session-b" }
      : { projectId: "a", sessionId: "derived" };
  assert.equal(sent?.projectId, expected.projectId, `Visible ${visibleTitle} must match the send Project`);
  assert.equal(sent?.sessionId, expected.sessionId, `Visible ${visibleTitle} must match the send Session`);
}

async function deriveSession(page) {
  const source = await page.locator("#session-title").textContent() === "Session A" ? "a" : "b";
  await page.locator(`#tree-list [data-message-id="message-${source}"]`).click({ button: "right" });
  await page.locator("#message-start-session-action").click();
  await page.locator("#message-input").fill("Derive a new Session");
  await page.locator("#send-button").click();
  await page.waitForFunction(() => document.querySelector("#message-input").placeholder === "Creating the new Session…");
}

test("catalog failure after a Run finishes cannot change the send target behind the displayed Session", async (t) => {
  const page = await openFixture(t);
  await openRuns(page);
  await page.evaluate(() => { window.fixture.finish("run-b", "completed", false); window.fixture.catalogFailure = true; });
  await page.locator('[data-run-id="run-b"]').click();
  await page.locator("#toast.is-error").waitFor();
  if (!await page.locator("#sessions-page").isVisible()) await page.locator("#sessions-nav").click();
  await sendToVisibleSession(page);
});

test("a delayed catalog cannot expose a Session whose send target has already changed", async (t) => {
  const page = await openFixture(t);
  await openRuns(page);
  await page.evaluate(() => { window.fixture.finish("run-b", "completed", false); window.fixture.catalogDelay = true; });
  await page.locator('[data-run-id="run-b"]').click();
  await page.waitForFunction(() => window.fixture.projectReads.includes("b"));
  await page.locator("#sessions-nav").click();
  await sendToVisibleSession(page);
  await page.evaluate(() => { window.fixture.catalogDelay = false; window.fixture.releaseCatalog(); });
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
});

test("Project view failure preserves the original Session and send target", async (t) => {
  const page = await openFixture(t);
  await openRuns(page);
  await page.evaluate(() => { window.fixture.viewFailure = true; });
  await page.locator('[data-run-id="run-b"]').click();
  await page.locator("#toast.is-error").waitFor();
  await page.evaluate(() => { window.fixture.viewFailure = false; });
  await page.locator("#sessions-nav").click();
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  await sendToVisibleSession(page);
});

test("a Run completing during delayed navigation opens with consistent idle Session data", async (t) => {
  const page = await openFixture(t);
  await openRuns(page);
  await page.evaluate(() => { window.fixture.catalogDelay = true; });
  await page.locator('[data-run-id="run-b"]').click();
  await page.waitForFunction(() => window.fixture.projectReads.includes("b"));
  assert.equal(await page.locator("#session-title").textContent(), "Session A");
  await page.evaluate(() => {
    window.fixture.finish("run-b");
    window.fixture.catalogDelay = false;
    window.fixture.releaseCatalog();
  });
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B" && !document.querySelector("#message-input").disabled);
  await sendToVisibleSession(page);
});

for (const branchStatus of ["completed", "failed"]) {
  for (const otherStatus of ["completed", "failed"]) {
    test(`a pending derivation cannot lock another Project when Runs finish ${branchStatus}/${otherStatus}`, async (t) => {
      const page = await openFixture(t);
      await deriveSession(page);
      await openRuns(page);
      await page.locator('[data-run-id="run-b"]').click();
      await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
      await page.evaluate(({ branchStatus, otherStatus }) => {
        window.fixture.finish("run-derived", branchStatus);
        window.fixture.finish("run-b", otherStatus);
      }, { branchStatus, otherStatus });
      await page.waitForFunction(() => !document.querySelector("#message-input").disabled);
      await sendToVisibleSession(page);
      await page.locator('[data-project-id="a"]').click();
      await page.waitForFunction(() => document.querySelector("#session-title").textContent !== "Session B");
      await page.waitForFunction(() => !document.querySelector("#message-input").disabled);
    });
  }
}

for (const branchStatus of ["completed", "failed"]) {
  test(`B is usable while A still derives, and A later ${branchStatus}`, async (t) => {
    const page = await openFixture(t);
    await deriveSession(page);
    await openRuns(page);
    await page.locator('[data-run-id="run-b"]').click();
    await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
    await page.evaluate(() => window.fixture.finish("run-b"));
    await page.waitForFunction(() => !document.querySelector("#message-input").disabled);
    await sendToVisibleSession(page);
    assert.equal(await page.evaluate(() => window.fixture.runs.find((run) => run.id === "run-derived").status), "running");
    await page.locator('[data-project-id="a"]').click();
    await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session A");
    assert.equal(await page.locator("#message-input").isDisabled(), true, "the original source still tracks its pending derivation");
    await page.evaluate((status) => window.fixture.finish("run-derived", status), branchStatus);
    await page.waitForFunction(() => !document.querySelector("#message-input").disabled);
    assert.equal(await page.locator("#session-title").textContent(), branchStatus === "completed" ? "Derived Session" : "Session A");
  });

  test(`returning to A recovers its ${branchStatus} derivation even if the terminal event was missed`, async (t) => {
    const page = await openFixture(t);
    await deriveSession(page);
    await openRuns(page);
    await page.locator('[data-run-id="run-b"]').click();
    await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
    await page.evaluate((status) => window.fixture.finish("run-derived", status, false), branchStatus);
    await page.locator('[data-project-id="a"]').click();
    await page.waitForFunction(() => document.querySelector("#session-title").textContent !== "Session B" && !document.querySelector("#message-input").disabled);
  });
}
