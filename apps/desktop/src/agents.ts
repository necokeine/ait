import type { AgentSummary, ReasoningEffort } from "./types.js";

export const builtInCodexAgentId = "codex-app-server";
export const legacyBuiltInCodexAgentId = "codex-local";
export const builtInCodexModel = "gpt-5.6-sol";

const legacyBuiltInCodexModel = "gpt-5.6-codex";
const builtInReasoningEfforts: ReasoningEffort[] = ["low", "medium", "high", "xhigh", "max", "ultra"];

export function normalizedBuiltInAgent(agent: AgentSummary): AgentSummary {
  if (
    agent.id === builtInCodexAgentId
    && agent.mode === "codex"
    && (agent.model === legacyBuiltInCodexModel || agent.model === builtInCodexModel)
  ) {
    return {
      ...agent,
      model: builtInCodexModel,
      supportedReasoningEfforts: [...builtInReasoningEfforts],
      defaultReasoningEffort: "low",
    };
  }
  return agent;
}
