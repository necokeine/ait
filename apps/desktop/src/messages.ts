import type { AgentSummary, DesktopMessage } from "./types.js";

export function messageAuthor(message: DesktopMessage, agents: AgentSummary[]): string {
  if (message.role === "user") return "You";
  if (message.role === "system") return "System";
  return agents.find((agent) => agent.id === message.agentId)?.name.trim() || "Assistant";
}

export function messageAgentIds(
  messages: Array<{ id: string; parent_message_id: string | null; role: string }>,
  runs: Array<{ agent_id: string; base_message_id: string; last_message_id: string | null }>,
): Map<string, string> {
  const messagesById = new Map(messages.map((message) => [message.id, message]));
  const result = new Map<string, string>();
  for (const run of runs) {
    const seen = new Set<string>();
    let message = run.last_message_id ? messagesById.get(run.last_message_id) : undefined;
    while (message && message.id !== run.base_message_id && !seen.has(message.id)) {
      seen.add(message.id);
      if (message.role === "assistant") result.set(message.id, run.agent_id);
      message = message.parent_message_id ? messagesById.get(message.parent_message_id) : undefined;
    }
  }
  return result;
}
