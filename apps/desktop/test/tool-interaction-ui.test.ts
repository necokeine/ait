import assert from "node:assert/strict";
import test from "node:test";
import { interactionResponse, renderToolInteractions } from "../src/tool-interaction-ui.js";
import type { DesktopRun, ToolInteraction } from "../src/types.js";

function run(interaction: ToolInteraction): DesktopRun {
  return {
    id: "run-a",
    sessionId: "session-a",
    baseMessageId: "message-a",
    lastMessageId: null,
    status: "running",
    permissionProfile: { sandbox: "read_only", approval: "on_request" },
    nativeApprovals: [],
    toolInteractions: [interaction],
  };
}

test("renders escaped questions and explicit response actions", () => {
  const html = renderToolInteractions(run({
    id: "interaction-a",
    toolName: "ask_user_question",
    request: {
      questions: [{
        id: "choice",
        header: "Choose <mode>",
        question: "Which & why?",
        options: [{ label: "Safe", description: "No <script>" }, { label: "Fast" }],
      }],
    },
    status: "pending",
    expiresAt: Date.now() + 10_000,
    createdAt: 1,
  }));
  assert.match(html, /data-interaction-action="submit"/);
  assert.match(html, /data-interaction-action="cancel"/);
  assert.match(html, /Choose &lt;mode&gt;/);
  assert.doesNotMatch(html, /<script>/);
});

test("plan review renders only approve, deny, and cancellation decisions", () => {
  const html = renderToolInteractions(run({
    id: "plan-a",
    toolName: "exit_plan_mode",
    request: { plan: "# Ship safely\n\nDo <work>." },
    status: "pending",
    expiresAt: Date.now() + 10_000,
    createdAt: 1,
  }));
  assert.match(html, /data-interaction-action="approve"/);
  assert.match(html, /data-interaction-action="deny"/);
  assert.match(html, /data-interaction-action="cancel"/);
  assert.match(html, /Do &lt;work&gt;/);
});

test("collects keyed single and multi-select answers", () => {
  const fields = [
    {
      dataset: { questionId: "mode", multiSelect: "false" },
      querySelectorAll: () => [{ type: "radio", checked: true, value: "Safe" }],
    },
    {
      dataset: { questionId: "checks", multiSelect: "true" },
      querySelectorAll: () => [
        { type: "checkbox", checked: true, value: "Tests" },
        { type: "checkbox", checked: false, value: "Deploy" },
      ],
    },
  ];
  const card = { querySelectorAll: () => fields } as unknown as HTMLElement;
  assert.deepEqual(interactionResponse(card), { mode: "Safe", checks: ["Tests"] });
});
