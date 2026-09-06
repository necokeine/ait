import type { AgentSummary, AgentView, AgentProvider } from "./types.js";

export const builtInCodexAgentId = "codex-app-server";
export const legacyBuiltInCodexAgentId = "codex-local";
export const builtInCodexModel = "gpt-5.6-sol";

export function projectAgent(agent: AgentView, providers: AgentProvider[]): AgentSummary {
  const provider = providers.find((item) => item.id === agent.config.provider_id);
  const model = provider?.models.find((item) => item.id === agent.config.model);
  return {
    id: agent.id, name: agent.name, enabled: agent.enabled,
    config: agent.config, ownerSessionId: agent.owner_session_id,
    model: agent.config.model, mode: provider?.kind ?? "unavailable",
    supportedReasoningEfforts: model?.reasoning_efforts ?? [],
  };
}
