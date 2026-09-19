import assert from "node:assert/strict";
import test from "node:test";
import { messageAgentIds, messageAuthor, projectMessage } from "../src/messages.js";
import type { AgentSummary, DesktopMessage } from "../src/types.js";

const agents: AgentSummary[] = [
  { id: "codex", name: "Codex", model: "gpt-5.6-sol", mode: "codex", enabled: true },
  { id: "reviewer", name: "Reviewer", model: "review", mode: "manual", enabled: true },
];

test("native provider items display ordered content and individual unknown fallbacks", () => {
  const items = [
    { type: "reasoning", summary: ["Check API"] },
    { type: "plan", text: "Read then validate" },
    { type: "commandExecution", command: "cargo test", status: "completed", aggregatedOutput: "passed" },
    { type: "fileChange", changes: [{path: "src/main.rs"}], status: "completed" },
    { type: "agentMessage", text: "Done", phase: "final_answer" },
    { type: "futureItem", value: "inspect me" },
  ];
  const projected = projectMessage({id: "native", project_id: "project", parent_message_id: "root", role: "assistant", kind: "standard", text: null,
    data: {native_message: {metadata: {privateEnvelope: "do not display"}, sub_messages: items.map((payload, ordinal) => ({
      type: "provider_item", provider_kind: "codex", item_type: payload.type, external_item_id: `item-${ordinal}`, payload,
    }))}}}, "codex");
  assert.deepEqual(projected.parts.map((part) => part.type), ["operation", "operation", "operation", "operation", "codex_message", "structured"]);
  assert.equal(projected.parts[4].type === "codex_message" && projected.parts[4].text, "Done");
  assert.equal(projected.parts[2].type === "operation" && projected.parts[2].summary, "cargo test");
  assert.deepEqual(projected.parts[3].type === "operation" && projected.parts[3].paths, ["src/main.rs"]);
  const unknown = projected.parts[5];
  assert.equal(unknown.type === "structured" && JSON.parse(unknown.value).payload.value, "inspect me");
  assert.equal(JSON.stringify(projected.parts).includes("privateEnvelope"), false);
});
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
  assert.equal(messageAuthor(message("assistant", "missing"), agents), "Assistant");
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
