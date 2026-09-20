import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installConfigFixture() {
  const f = window.fixture;
  f.providers = [
    { id: "codex", name: "Fixture Provider", kind: "codex", models: [
      { id: "fixture-model", name: "Reasoning model", reasoning_efforts: ["off", "low", "high", "max"] },
      { id: "other-model", name: "Other model", reasoning_efforts: ["minimal", "high"] },
      { id: "plain-model", name: "Plain model", reasoning_efforts: [] },
    ] },
    { id: "other-provider", name: "Other Provider", kind: "openai", models: [
      { id: "other-model", name: "Other provider model", reasoning_efforts: ["low"] },
    ] },
  ];
  f.configChanges = [];
  const catalog = () => ({ protocolVersion: 1, revision: 1, providers: structuredClone(f.providers),
    agents: f.agents.map((agent) => ({ ...structuredClone(agent),
      supportedReasoningEfforts: f.providers.find((p) => p.id === agent.config.provider_id)
        .models.find((m) => m.id === agent.config.model).reasoning_efforts,
    })),
  });
  window.ait.agents = async () => catalog();
  window.ait.setSessionConfig = async (input) => {
    f.configChanges.push(input);
    if (f.configDelay) await new Promise((resolve) => { f.releaseConfig = resolve; });
    if (f.configFailure) throw new Error("Configuration save failed");
    const session = f.sessions.find((s) => s.id === input.sessionId);
    const custom = { ...f.agents[0], id: "custom", ownerSessionId: session.id, config: input.config, model: input.config.model };
    f.agents = [...f.agents.filter((a) => a.id !== custom.id), custom];
    session.agentId = custom.id;
    return { project: f.view(input.projectId), agents: catalog() };
  };
  window.ait.setSessionAgent = async ({ projectId, sessionId, agentId }) => {
    f.sessions.find((s) => s.id === sessionId).agentId = agentId;
    return f.view(projectId);
  };
}

async function selectConfig(page, label, value) {
  await page.locator("#composer-config-panel").getByLabel(new RegExp(`^${label}\\b`)).selectOption(value);
  await page.waitForFunction(() => !document.querySelector("#composer-model").disabled);
}

async function assertPanelPosition(page) {
  const panel = await page.locator("#composer-config-panel").boundingBox();
  const trigger = await page.locator("#composer-config-trigger").boundingBox();
  const viewport = page.viewportSize();
  assert.ok(panel.x >= 0 && panel.x + panel.width <= viewport.width);
  assert.ok(panel.y >= 0 && panel.y + panel.height <= trigger.y);
}

test("the model popover saves reasoning levels and provider default in the Session configuration", async (t) => {
  const page = await openFixture(t, installConfigFixture);
  assert.equal(await page.locator("#composer-config-effort").textContent(), "· Default");
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).isVisible(), false);
  await page.locator("#composer-config-trigger").click();
  assert.equal(await page.locator("#composer-config-panel").getByLabel("Reasoning effort").isVisible(), true);
  assert.deepEqual(await page.locator("#composer-reasoning option").allTextContents(), ["Provider default", "Off", "Low", "High", "Max"]);
  for (const [value, label] of [["high", "High"], ["off", "Off"], ["", "Default"]]) {
    await selectConfig(page, "Reasoning effort", value);
    assert.equal(await page.locator("#composer-config-effort").textContent(), `· ${label}`);
    assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), value);
    assert.deepEqual(await page.evaluate(() => window.fixture.configChanges.at(-1)), {
      projectId: "a", sessionId: "session-a",
      config: { provider_id: "codex", model: "fixture-model", reasoning_effort: value || null },
    });
  }
  assert.equal(await page.evaluate(() => window.fixture.agents[0].config.reasoning_effort), null);
  await page.locator("#composer-config-close").click();
  await page.locator("#composer-config-trigger").click();
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), "");
});

test("model, provider and saved Agent changes update supported levels and reposition the popover", async (t) => {
  const page = await openFixture(t, installConfigFixture);
  await page.locator("#composer-config-trigger").click();
  await selectConfig(page, "Reasoning effort", "high");
  await selectConfig(page, "Model", "other-model");
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), "high");
  assert.deepEqual(await page.locator("#composer-reasoning option").allTextContents(), ["Provider default", "Minimal", "High"]);
  await selectConfig(page, "Model", "plain-model");
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).isVisible(), false);
  assert.equal(await page.locator("#composer-config-effort").isVisible(), false);
  assert.equal(await page.evaluate(() => window.fixture.configChanges.at(-1).config.reasoning_effort), null);
  await selectConfig(page, "Model", "fixture-model");
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).isVisible(), true);
  await assertPanelPosition(page);
  await selectConfig(page, "Reasoning effort", "high");
  await selectConfig(page, "Provider", "other-provider");
  assert.deepEqual(await page.locator("#composer-reasoning option").allTextContents(), ["Provider default", "Low"]);
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), "");
  await selectConfig(page, "Reasoning effort", "low");
  assert.deepEqual(await page.evaluate(() => window.fixture.configChanges.at(-1).config), {
    provider_id: "other-provider", model: "other-model", reasoning_effort: "low",
  });
  await selectConfig(page, "Saved Agent", "agent");
  assert.equal(await page.getByLabel(/^Provider\b/).inputValue(), "codex");
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), "");
  await assertPanelPosition(page);
});

test("pending saves disable configuration, failures restore saved reasoning, and running Sessions lock the entry", async (t) => {
  const page = await openFixture(t, installConfigFixture);
  await page.locator("#composer-config-trigger").click();
  await page.evaluate(() => { window.fixture.configDelay = true; window.fixture.configFailure = true; });
  await page.getByLabel("Reasoning effort", { exact: true }).selectOption("high");
  for (const id of ["trigger", "agent", "model", "provider", "reasoning"]) {
    assert.equal(await page.locator(id === "trigger" ? "#composer-config-trigger" : `#composer-${id}`).isDisabled(), true);
  }
  await page.evaluate(() => window.fixture.releaseConfig());
  await page.locator("#toast.is-error").waitFor();
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).inputValue(), "");
  assert.equal(await page.locator("#composer-config-effort").textContent(), "· Default");
  assert.equal(await page.locator("#composer-config-trigger").isEnabled(), true);
  await page.keyboard.press("Escape");
  await page.locator('[data-project-id="b"]').click();
  await page.waitForFunction(() => document.querySelector("#session-title").textContent === "Session B");
  assert.equal(await page.locator("#composer-config-trigger").isDisabled(), true);
  assert.equal(await page.getByLabel("Reasoning effort", { exact: true }).isVisible(), false);
  assert.equal(await page.locator("#composer-reasoning").isDisabled(), true);
});

for (const theme of ["light", "dark"]) {
  test(`combined configuration is keyboard accessible and fits the ${theme} desktop`, async (t) => {
    const page = await openFixture(t, installConfigFixture, { viewport: { width: 1280, height: 800 }, colorScheme: theme });
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
      document.documentElement.style.colorScheme = theme;
    }, theme);
    await page.locator("#composer-config-trigger").focus();
    await page.keyboard.press("Enter");
    for (const id of ["agent", "provider", "model", "reasoning"]) {
      assert.equal(await page.evaluate(() => document.activeElement.id), `composer-${id}`);
      if (id !== "reasoning") await page.keyboard.press("Tab");
    }
    await selectConfig(page, "Reasoning effort", "high");
    await assertPanelPosition(page);
    if (process.env.AIT_COMPOSER_SCREENSHOTS) {
      await mkdir(process.env.AIT_COMPOSER_SCREENSHOTS, { recursive: true });
      await page.screenshot({ path: join(process.env.AIT_COMPOSER_SCREENSHOTS, `composer-${theme}.png`) });
    }
    await page.keyboard.press("Escape");
    await page.locator("#composer-config-panel").waitFor({ state: "hidden" });
    assert.equal(await page.locator("#composer-config-trigger").getAttribute("aria-expanded"), "false");
    assert.equal(await page.locator("#composer-config-effort").isVisible(), true);
  });
}
