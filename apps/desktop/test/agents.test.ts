import assert from "node:assert/strict";
import test from "node:test";
import { projectAgent } from "../src/agents.js";
import { providerChoices } from "../src/agent-settings.js";
import type { AgentProvider, AgentView } from "../src/types.js";
const provider: AgentProvider = { id: "provider", name: "Provider", kind: "openai", url: null, has_secret: true, models: [{ id: "model", name: "Model", reasoning_efforts: ["minimal", "high"] }] };
const agent: AgentView = { id: "agent", name: "", enabled: true, revision: 2, owner_session_id: "session", config: { provider_id: "provider", model: "model", reasoning_effort: "high" } };

test("projects saved configuration and provider capabilities without rewriting model IDs", () => {
  const summary = projectAgent(agent, [provider]);
  assert.equal(summary.model, "model");
  assert.equal(summary.ownerSessionId, "session");
  assert.equal(summary.config.reasoning_effort, "high");
  assert.deepEqual(summary.supportedReasoningEfforts, ["minimal", "high"]);
  assert.deepEqual(providerChoices([provider]), [provider]);
});
test("unknown models do not receive invented reasoning capabilities", () => {
  const summary = projectAgent({ ...agent, config: { ...agent.config, model: "unknown" } }, [provider]);
  assert.equal(summary.model, "unknown");
  assert.deepEqual(summary.supportedReasoningEfforts, []);
});
