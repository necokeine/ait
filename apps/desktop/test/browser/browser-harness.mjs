import assert from "node:assert/strict";
import { before, after } from "node:test";
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

export async function openFixture(t, configure) {
  const context = await browser.newContext({ viewport: { width: 1480, height: 920 }, colorScheme: "dark" });
  t.after(() => context.close());
  const page = await context.newPage();
  page.setDefaultTimeout(3_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.addInitScript({ content: `(${installRendererFixture.toString()})();\n${configure ? `(${configure.toString()})();` : ""}` });
  await page.goto(url);
  await page.locator("#app:not(.is-loading)").waitFor();
  return page;
}
