import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installThemeFixture() {
  const f = window.fixture;
  const key = "interface.theme";
  f.theme = sessionStorage.getItem(key) || "system";
  f.settingsRevision = 1;
  const response = () => ({
    schema: { revision: 1, definitions: [{
      id: key, category: "interface", label: "Theme", description: "Application color scheme",
      kind: { type: "select", options: ["system", "light", "dark"] }, defaultValue: "system", restartRequired: false,
    }] },
    values: { [key]: f.theme, "permissions.sandbox": "workspace_write" }, revision: f.settingsRevision,
  });
  window.ait.settings = async () => response();
  window.ait.saveSettings = async (revision, values) => {
    if (revision !== f.settingsRevision) throw new Error("Stale settings revision");
    f.theme = values[key];
    f.settingsRevision++;
    sessionStorage.setItem(key, f.theme);
    return response();
  };
  const view = f.view;
  f.view = (projectId) => {
    const result = view(projectId);
    result.messages[1].parts[0].text = "Check that this conversation follows the selected theme.";
    result.messages.push({
      id: `answer-${projectId}`, projectId, parentMessageId: `message-${projectId}`,
      role: "assistant", kind: "standard", createdAt: 3,
      parts: [{ type: "text", text: "The reply, code, and message tree should remain readable.\n\n```rust\nlet theme = \"application preference\";\n```" }],
    });
    result.sessions.forEach((session) => { session.currentMessageId = `answer-${projectId}`; });
    result.runProgress = result.runs.filter((run) => run.status === "running").map((run) => ({
      runId: run.id, projectId, sessionId: run.sessionId, seq: 1, status: "running", warnings: [], updatedAt: 3,
      items: ["inProgress", "in_progress"].map((status) => ({
        type: "operation", id: `operation-${status}`, kind: "commandExecution", status,
        title: `Checking theme contrast (${status})`, paths: [],
      })),
    }));
    return result;
  };
}

const palette = {
  light: { bg: "rgb(246, 247, 243)", panel: "rgb(239, 240, 235)", title: "rgba(246, 247, 243, 0.92)",
    text: "rgb(30, 33, 27)", raised: "rgb(251, 252, 248)", border: "rgb(199, 205, 190)",
    focus: "rgb(101, 150, 44)", disabledBg: "rgb(225, 229, 218)", disabledText: "rgb(104, 111, 97)",
    sendText: "rgb(246, 251, 240)", shadow: "rgba(37, 41, 31, 0.08)" },
  dark: { bg: "rgb(16, 17, 15)", panel: "rgb(18, 19, 16)", title: "rgba(16, 17, 15, 0.92)",
    text: "rgb(236, 238, 232)", raised: "rgb(26, 28, 24)", border: "rgb(57, 61, 51)",
    focus: "rgb(87, 99, 69)", disabledBg: "rgb(48, 52, 44)", disabledText: "rgb(105, 110, 100)",
    sendText: "rgb(20, 32, 11)", shadow: "rgba(0, 0, 0, 0.2)" },
};

async function styles(page, selector) {
  return page.locator(selector).first().evaluate((element) => {
    const s = getComputedStyle(element);
    return { background: s.backgroundColor, color: s.color, border: s.borderTopColor,
      shadow: s.boxShadow, colorScheme: s.colorScheme };
  });
}

async function surfaceSnapshot(page) {
  return page.locator("body, body *").evaluateAll((elements) => elements.map((element) => {
    const style = getComputedStyle(element);
    return ["color", "backgroundColor", "backgroundImage", "borderColor", "boxShadow", "outlineColor"]
      .map((property) => style[property]);
  }));
}

async function assertReadableText(page, selector, backgroundSelector = selector) {
  const { color } = await styles(page, selector);
  const { background } = await styles(page, backgroundSelector);
  const luminance = (rgb) => rgb.match(/[\d.]+/g).slice(0, 3).map(Number).reduce((sum, value, i) => {
    const c = value / 255;
    return sum + (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][i];
  }, 0);
  const light = Math.max(luminance(color), luminance(background));
  const dark = Math.min(luminance(color), luminance(background));
  const ratio = (light + 0.05) / (dark + 0.05);
  assert.ok(ratio >= 4.5, `${selector} text contrast ${ratio.toFixed(2)}:1`);
  return ratio;
}

async function chooseTheme(page, theme) {
  await page.locator("#settings-trigger").click();
  await page.locator('[data-category="interface"]').click();
  await page.getByLabel("Theme", { exact: true }).selectOption(theme);
  await page.locator("#settings-save").click();
  await page.waitForFunction((value) => document.documentElement.dataset.theme === value, theme);
  await assertReadableText(page, "#settings-save");
  await page.locator("#settings-close").click();
}

async function assertTheme(page, theme) {
  const p = palette[theme];
  assert.equal((await styles(page, "body")).background, p.bg);
  assert.equal((await styles(page, "body")).color, p.text);
  assert.equal((await styles(page, ".titlebar")).background, p.title);
  for (const selector of [".sidebar", ".tree-panel"]) {
    const s = await styles(page, selector);
    assert.equal(s.background, p.panel, selector);
    assert.equal(s.color, p.text, selector);
  }
  assert.equal((await styles(page, ".conversation-pane")).background, p.bg);
  assert.equal((await styles(page, ".message-content")).color, p.text);
  assert.equal((await styles(page, ".code-block pre")).color, p.text);
  assert.equal((await styles(page, "#message-input")).color, p.text);
  assert.equal((await styles(page, "#composer")).background, p.raised);
  assert.ok((await styles(page, "#composer")).shadow.includes(p.shadow));
}

async function screenshot(page, name) {
  if (!process.env.AIT_THEME_SCREENSHOTS) return;
  await mkdir(process.env.AIT_THEME_SCREENSHOTS, { recursive: true });
  await page.locator("#toast.is-hidden").waitFor({ state: "attached", timeout: 6_000 });
  await page.screenshot({ path: join(process.env.AIT_THEME_SCREENSHOTS, `${name}.png`) });
}

for (const viewport of [{ width: 1440, height: 900 }, { width: 1280, height: 800 }]) {
  for (const system of ["light", "dark"]) {
    for (const theme of ["light", "dark"]) {
      test(`${viewport.width}: system ${system}, explicit ${theme} controls all surfaces and input states`, async (t) => {
        const page = await openFixture(t, installThemeFixture, { viewport, colorScheme: system });
        await assertTheme(page, system);
        await chooseTheme(page, theme);
        await assertTheme(page, theme);
        assert.equal((await styles(page, "body")).colorScheme, theme);
        await page.locator('[data-project-toggle="a"]').click();
        const p = palette[theme];
        const name = `${viewport.width}-system-${system}-app-${theme}`;
        const send = page.locator("#send-button");
        assert.equal(await send.isDisabled(), true);
        assert.equal((await styles(page, "#send-button")).background, p.disabledBg);
        assert.equal((await styles(page, "#send-button")).color, p.disabledText);
        await page.waitForFunction((color) => getComputedStyle(document.querySelector("#composer")).borderTopColor === color, p.border);
        await screenshot(page, `${name}-empty`);
        await page.locator("#message-input").fill("Continue with the selected theme.");
        await page.waitForFunction((color) => getComputedStyle(document.querySelector("#composer")).borderTopColor === color, p.focus);
        assert.equal(await send.isDisabled(), false);
        assert.equal((await styles(page, "#send-button")).color, p.sendText);
        await assertReadableText(page, "#send-button");
        await screenshot(page, `${name}-focused`);
        await page.locator("#composer-config-trigger").click();
        await page.locator("#composer-config-panel:popover-open").waitFor();
        assert.equal((await styles(page, "#composer-config-panel")).background, p.raised);
        assert.equal((await styles(page, "#composer-agent")).colorScheme, theme);
        await page.keyboard.press("Escape");
        await page.reload();
        await page.locator("#app:not(.is-loading)").waitFor();
        assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), theme);
        await assertTheme(page, theme);
        await page.waitForFunction((color) => getComputedStyle(document.querySelector("#composer")).borderTopColor === color, p.border);
        const snapshot = await surfaceSnapshot(page);
        await page.emulateMedia({ colorScheme: system === "dark" ? "light" : "dark" });
        await assertTheme(page, theme);
        assert.deepEqual(await surfaceSnapshot(page), snapshot, "system changes must not alter any explicit-theme surface");
      });
    }
  }
}

for (const system of ["light", "dark"]) {
  for (const theme of ["light", "dark"]) {
    test(`running operation status: system ${system}, explicit ${theme} remains readable`, async (t) => {
      const page = await openFixture(t, installThemeFixture, { colorScheme: system, viewport: { width: 1440, height: 900 } });
      await chooseTheme(page, theme);
      await page.locator('[data-project-id="b"]').click();
      await page.locator(".live-run .message-disclosure > summary").click();
      const aggregate = page.locator(".live-run .activity-summary .operation-status");
      assert.equal(await aggregate.innerText(), "2 running");
      await page.locator(".live-run .activity-summary").click();
      assert.equal(await page.locator(".live-run .activity-item-content .operation-status").count(), 2);
      for (const status of ["inprogress", "in_progress"]) {
        const selector = `.live-run .activity-item-content .operation-status.status-${status}`;
        assert.equal(await page.locator(selector).isVisible(), true);
        const ratio = await assertReadableText(page, selector, ".conversation-pane");
        t.diagnostic(`${status}: ${ratio.toFixed(2)}:1`);
        if (theme === "dark") assert.equal((await styles(page, selector)).color, "rgb(213, 173, 86)");
      }
      await screenshot(page, `running-system-${system}-app-${theme}`);
    });
  }
}

test("saved System preference follows live system changes after explicit themes and reload", async (t) => {
  const page = await openFixture(t, installThemeFixture);
  for (const theme of ["light", "dark", "system"]) await chooseTheme(page, theme);
  for (const system of ["light", "dark", "light"]) {
    await page.emulateMedia({ colorScheme: system });
    await assertTheme(page, system);
  }
  await page.reload();
  await page.locator("#app:not(.is-loading)").waitFor();
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "system");
  await assertTheme(page, "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await assertTheme(page, "dark");
});
