import assert from "node:assert/strict";
import test from "node:test";

import { normalizedBuiltInAgent } from "../src/agents.js";

test("projects the persisted legacy built-in Codex model as the supported model", () => {
  assert.deepEqual(normalizedBuiltInAgent({
    id: "codex-app-server",
    name: "Codex",
    model: "gpt-5.6-codex",
    mode: "codex",
    enabled: true,
  }), {
    id: "codex-app-server",
    name: "Codex",
    model: "gpt-5.6-sol",
    mode: "codex",
    enabled: true,
    supportedReasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    defaultReasoningEffort: "low",
  });
});

test("projects the built-in Codex model's advertised reasoning profile", () => {
  const agent = normalizedBuiltInAgent({
    id: "codex-app-server",
    name: "Codex",
    model: "gpt-5.6-sol",
    mode: "codex",
    enabled: true,
  });

  assert.deepEqual(agent.supportedReasoningEfforts, ["low", "medium", "high", "xhigh", "max", "ultra"]);
  assert.equal(agent.defaultReasoningEffort, "low");
});

test("does not rewrite user-defined Agents", () => {
  const custom = {
    id: "custom-codex",
    name: "Custom Codex",
    model: "gpt-5.6-codex",
    mode: "codex",
    enabled: true,
  };
  assert.equal(normalizedBuiltInAgent(custom), custom);
});
