import assert from "node:assert/strict";
import test from "node:test";
import { renderCronRows, resolveCronBaseMessage } from "../src/crons-page.js";
import type { AgentSummary, DesktopCron, DesktopProject, DesktopSession } from "../src/types.js";

const session = {
  id: "session", projectId: "project", workdir: "/project/.ait/session", name: "", title: "Session",
  description: "", titleGenerationStarted: false, status: "active", currentMessageId: "message-from-session",
  agentId: "agent", version: 1, active: false, activeRunId: null, updatedAt: 0,
} satisfies DesktopSession;

test("resolves a Cron target from either a Session head or an explicit Message ID", () => {
  assert.equal(resolveCronBaseMessage("session", "ignored", session), "message-from-session");
  assert.equal(resolveCronBaseMessage("message", "  direct-message  "), "direct-message");
  assert.throws(() => resolveCronBaseMessage("session", ""), /Choose a Session/);
  assert.throws(() => resolveCronBaseMessage("message", "  "), /Enter a Message ID/);
});

test("renders escaped Cron identity, target and enablement controls", () => {
  const cron = {
    id: 'cron"', name: "<Daily>", projectId: "project", baseMessageId: "message-1234567890",
    agentId: "agent", schedule: "0 9 * * *", timezone: "Asia/Shanghai", enabled: true,
  } satisfies DesktopCron;
  const project = {
    id: "project", name: "<Project>", workdir: "/project", description: "", baseCommit: "a".repeat(40),
    defaultAgentId: "agent",
  } satisfies DesktopProject;
  const agent = {
    id: "agent", name: "<Agent>", model: "model", mode: "codex", enabled: true,
    config: { provider_id: "provider", model: "model", reasoning_effort: null }, ownerSessionId: null,
  } satisfies AgentSummary;

  const html = renderCronRows([cron], [project], [agent]);
  assert.ok(!html.includes("<Daily>"));
  assert.match(html, /&lt;Daily&gt;/);
  assert.match(html, /Asia\/Shanghai/);
  assert.match(html, /Message message-/);
  assert.match(html, /data-cron-toggle="cron&quot;"/);
  assert.match(html, /data-cron-run=/);
  assert.match(html, />Enabled</);
});
