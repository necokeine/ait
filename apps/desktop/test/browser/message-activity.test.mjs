import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { openFixture } from "./browser-harness.mjs";

function installActivityFixture() {
  const f = window.fixture;
  const view = f.view;
  f.view = (projectId) => {
    const result = view(projectId);
    result.messages[1].parts = [{ type: "text", text: "帮我打包一个apk出来。" }];
    result.messages[1].createdAt = 1_789_288_965_000;
    result.messages.push({
      id: `answer-${projectId}`, projectId, parentMessageId: `message-${projectId}`,
      role: "assistant", kind: "standard", createdAt: 1_789_289_025_000, agentId: "agent",
      parts: [
        { type: "codex_message", id: "before", phase: "commentary", text: "我先检查项目的构建配置和 Android 环境，然后打包 APK 并确认产物位置。" },
        { type: "operation", id: "check", kind: "commandExecution", title: "Command", status: "completed", summary: "pwd && rg --files -g '*gradle*'", detail: "app/build.gradle.kts\ngradle/wrapper/gradle-wrapper.properties", paths: [] },
        { type: "operation", id: "failed", kind: "commandExecution", title: "Command", status: "failed", summary: "flutter --version", detail: "<script>doNotExecute()</script>\nFlutter is unavailable.", paths: [] },
        { type: "codex_message", id: "after", phase: "commentary", text: "项目当前是原生 Kotlin Android 应用。我会打包可直接安装的 Debug APK；已找到 Android Studio 自带的 JDK 和所需 SDK。" },
        { type: "operation", id: "build", kind: "commandExecution", title: "Command", status: "completed", summary: "./gradlew assembleDebug", detail: "BUILD SUCCESSFUL", paths: ["app/build/outputs/apk/debug/app-debug.apk"] },
        { type: "codex_message", id: "final", phase: "final_answer", text: "APK 已打包：[app-debug.apk](app/build/outputs/apk/debug/app-debug.apk)\n\n- Debug 版，约 10 MB，支持 Android 8.0 及以上。\n- 签名校验通过，23 项单元测试全部通过。" },
      ],
    });
    result.sessions.forEach((session) => { session.currentMessageId = `answer-${projectId}`; });
    return result;
  };
  window.ait.settings = async () => ({ schema: { revision: 1, definitions: [] }, values: {
    "interface.theme": "system", "permissions.sandbox": "workspace_write",
  }, revision: 1 });
}

async function repaint(page, preserveOpen) {
  await page.evaluate(async (preserve) => {
    const { renderConversationMessages, replaceConversationContent } = await import("/message-renderer.js");
    const view = window.fixture.view("a");
    replaceConversationContent(document.querySelector("#conversation"), renderConversationMessages(view.messages, window.fixture.agents), preserve);
  }, preserveOpen);
}

for (const theme of ["light", "dark"]) {
  test(`${theme}: native activity shows prose and compact command groups with inspectable failures`, async (t) => {
    const page = await openFixture(t, installActivityFixture, { colorScheme: theme, viewport: { width: 1280, height: 960 } });
    const activity = page.locator("#conversation .message-disclosure");
    assert.equal(await activity.count(), 1);
    assert.equal(await activity.getAttribute("open"), null);
    assert.equal(await activity.locator(":scope > summary .status-failed").innerText(), "1 failed");
    assert.equal(await page.locator("[data-codex-final-answer]").isVisible(), true);
    assert.equal(await page.locator('[data-codex-item-id="before"]').isVisible(), false);
    await activity.locator(":scope > summary").focus();
    await page.keyboard.press("Enter");
    assert.equal(await page.locator('[data-codex-item-id="before"]').isVisible(), true);
    assert.equal(await activity.locator(".message-heading, .message-event-kind, .message-event-count").count(), 0);
    assert.deepEqual(await activity.locator(".activity-label").allTextContents(), ["Ran 2 commands", "Ran a command"]);
    assert.equal(await activity.locator(".activity-item-content").first().isVisible(), false);
    assert.equal(await activity.locator(".activity-summary .status-failed").innerText(), "1 failed");
    assert.equal(await activity.locator('[data-message-id="answer-a"]').count(), 4);

    const screenshotDirectory = process.env.AIT_TEST_SCREENSHOTS;
    if (screenshotDirectory) {
      await mkdir(screenshotDirectory, { recursive: true });
      await page.evaluate(() => document.activeElement?.blur());
      await page.locator("#conversation").screenshot({ path: join(screenshotDirectory, `activity-${theme}.png`) });
    }

    const commands = activity.locator(".activity-item").first();
    await commands.locator(":scope > summary").focus();
    await page.keyboard.press("Space");
    assert.equal(await commands.locator(".activity-item-content").isVisible(), true);
    assert.ok((await commands.innerText()).includes("flutter --version"));
    assert.ok((await commands.innerText()).includes("<script>doNotExecute()</script>"));
    assert.equal(await commands.locator("script").count(), 0);
    const key = await commands.getAttribute("data-disclosure-id");
    await repaint(page, true);
    assert.equal(await commands.locator(".activity-item-content").isVisible(), true);
    assert.equal(await page.evaluate(() => document.activeElement.closest("details")?.dataset.disclosureId), key);
    await page.keyboard.press("Enter");
    await repaint(page, true);
    assert.equal(await commands.getAttribute("open"), null);
    assert.notEqual(await activity.getAttribute("open"), null);
    await repaint(page, false);
    assert.equal(await activity.getAttribute("open"), null);
    assert.equal(await page.locator("[data-codex-final-answer]").isVisible(), true);
    assert.equal(await page.evaluate(() => document.querySelector("#conversation").scrollWidth > document.querySelector("#conversation").clientWidth), false);
  });
}

test("streaming commands retain expanded details as the group grows and reports failure", async (t) => {
  const page = await openFixture(t);
  await page.evaluate(async () => {
    const { renderRunProgress, replaceConversationContent } = await import("/message-renderer.js");
    window.activityProgress = { runId: "stream", projectId: "a", sessionId: "session-a", seq: 1, status: "running", updatedAt: 1, warnings: [], items: [
      { type: "operation", id: "first", kind: "command", title: "Ran command", status: "inProgress", summary: "cargo check", detail: "Checking workspace", paths: [] },
    ] };
    window.paintActivity = () => replaceConversationContent(document.querySelector("#conversation"), renderRunProgress(window.activityProgress, "Codex", true), true);
    window.paintActivity();
  });
  const activity = page.locator(".live-run .message-disclosure");
  await activity.locator(":scope > summary").click();
  const command = activity.locator(".activity-item");
  await command.locator(":scope > summary").click();
  const key = await command.getAttribute("data-disclosure-id");
  await page.evaluate(() => {
    window.activityProgress.items[0].status = "completed";
    window.activityProgress.items.push({ type: "operation", id: "second", kind: "command", title: "Ran command", status: "in_progress", summary: "cargo test", detail: "Running tests", paths: [] });
    window.paintActivity();
  });
  assert.equal(await command.getAttribute("data-disclosure-id"), key);
  assert.notEqual(await command.getAttribute("open"), null);
  assert.equal(await command.locator(".activity-label").innerText(), "Running 2 commands");
  assert.equal(await command.locator(".activity-summary .operation-status").innerText(), "1 running");
  await page.evaluate(() => {
    window.activityProgress.items[1].status = "failed";
    window.activityProgress.items.push({ type: "codex_message", id: "answer", phase: "final_answer", text: "One test needs a fix." });
    window.paintActivity();
  });
  assert.equal(await command.locator(".activity-label").innerText(), "Ran 2 commands");
  assert.equal(await command.locator(".activity-summary .operation-status").innerText(), "1 failed");
  assert.equal(await page.locator("[data-codex-final-answer]").isVisible(), true);
  assert.equal(await command.locator(".activity-item-content").isVisible(), true);
});
