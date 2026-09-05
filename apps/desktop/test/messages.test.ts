import assert from "node:assert/strict";
import test from "node:test";
import { messageAgentIds, messageAuthor } from "../src/messages.js";
import type { AgentSummary, DesktopMessage } from "../src/types.js";

const agents: AgentSummary[] = [
  { id: "codex", name: "Codex", model: "gpt-5.6-sol", mode: "codex", enabled: true },
  { id: "reviewer", name: "Reviewer", model: "review", mode: "manual", enabled: true },
];
const message = (role: DesktopMessage["role"], agentId?: string | null): DesktopMessage => ({
  id: `${role}-${agentId ?? "none"}`,
  parentMessageId: null,
  projectId: "project",
  role,
  kind: "standard",
  parts: [{ type: "text", text: "hello" }],
  createdAt: 0,
  agentId,
});

test("labels assistant messages with their actual producing Agent", () => {
  assert.equal(messageAuthor(message("assistant", "codex"), agents), "Codex");
  assert.equal(messageAuthor(message("assistant", "reviewer"), agents), "Reviewer");
});

test("uses role-aware fallbacks when producer identity is unavailable", () => {
  assert.equal(messageAuthor(message("assistant", "missing"), agents), "Agent");
  assert.equal(messageAuthor(message("user"), agents), "You");
  assert.equal(messageAuthor(message("system"), agents), "System");
});

test("attributes every assistant output in a Run to its producing Agent", () => {
  const ids = messageAgentIds([
    { id: "input", parent_message_id: null, role: "user" },
    { id: "tool-call", parent_message_id: "input", role: "assistant" },
    { id: "tool-result", parent_message_id: "tool-call", role: "user" },
    { id: "reply", parent_message_id: "tool-result", role: "assistant" },
    { id: "other-reply", parent_message_id: "input", role: "assistant" },
  ], [
    { agent_id: "codex", base_message_id: "input", last_message_id: "reply" },
    { agent_id: "reviewer", base_message_id: "input", last_message_id: "other-reply" },
  ]);

  assert.equal(ids.get("tool-call"), "codex");
  assert.equal(ids.get("reply"), "codex");
  assert.equal(ids.get("other-reply"), "reviewer");
  assert.equal(ids.has("tool-result"), false);
});
