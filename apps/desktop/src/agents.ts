import type { AgentSummary } from "./types.js";

export const builtInCodexAgentId = "codex-app-server";
export const legacyBuiltInCodexAgentId = "codex-local";
export const builtInCodexModel = "gpt-5.6-sol";

const legacyBuiltInCodexModel = "gpt-5.6-codex";

export function normalizedBuiltInAgent(agent: AgentSummary): AgentSummary {
  if (
    agent.id === builtInCodexAgentId
    && agent.mode === "codex"
    && agent.model === legacyBuiltInCodexModel
  ) {
    return { ...agent, model: builtInCodexModel };
  }
  return agent;
}
