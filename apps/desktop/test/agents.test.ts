import assert from "node:assert/strict";
import test from "node:test";
import { projectAgent } from "../src/agents.js";
import { providerChoices } from "../src/agent-settings.js";
import { desktopDaemonRuntime, desktopProviderCatalog } from "../src/desktop-runtime.js";
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
test("development Mock providers remain selectable when the backend advertises them", () => {
  const mock: AgentProvider = {
    id: "builtin-mock", name: "Mock (Development)", kind: "mock", url: null,
    has_secret: false, models: [{ id: "mock-local", name: "Mock Local", reasoning_efforts: [] }],
  };
  const mockAgent: AgentView = {
    ...agent, id: "mock-agent", config: { provider_id: mock.id, model: "mock-local", reasoning_effort: null },
  };
  const development = desktopProviderCatalog([mock], [mockAgent], true);
  const production = desktopProviderCatalog([mock], [mockAgent], false);
  assert.deepEqual(providerChoices(development.providers), [mock]);
  assert.deepEqual(development.agents, [mockAgent]);
  assert.deepEqual(providerChoices(production.providers), []);
  assert.deepEqual(production.agents, []);
});
test("development and packaged desktop daemons use isolated ports and databases", () => {
  const development = desktopDaemonRuntime(false);
  const production = desktopDaemonRuntime(true);
  assert.equal(development.allowDevelopmentMock, true);
  assert.equal(production.allowDevelopmentMock, false);
  assert.notEqual(development.endpoint, production.endpoint);
  assert.notEqual(development.databaseFilename, production.databaseFilename);
});
test("unknown models do not receive invented reasoning capabilities", () => {
  const summary = projectAgent({ ...agent, config: { ...agent.config, model: "unknown" } }, [provider]);
  assert.equal(summary.model, "unknown");
  assert.deepEqual(summary.supportedReasoningEfforts, []);
});
