import type { AgentSummary, DesktopMessage, MessagePart } from "./types.js";

export interface WorkspaceMessage {
  id: string;
  project_id: string;
  parent_message_id: string | null;
  role: DesktopMessage["role"];
  kind: DesktopMessage["kind"];
  text: string | null;
  created_at?: number;
  git_commit?: string | null;
  data?: unknown;
}

export function projectMessage(message: WorkspaceMessage, agentId: string | null): DesktopMessage {
  return {
    id: message.id, projectId: message.project_id, parentMessageId: message.parent_message_id,
    role: message.role, kind: message.kind, parts: messageParts(message),
    createdAt: message.created_at ?? 0, agentId,
    ...(message.git_commit ? { gitCommit: message.git_commit } : {}),
  };
}

function messageParts(message: WorkspaceMessage): MessagePart[] {
  const parts: MessagePart[] = [];
  if (message.text !== null) parts.push({ type: "text", text: message.text });
  const data = objectValue(message.data);
  const codex = objectValue(data.codex);
  const operations = Array.isArray(codex.operations) ? codex.operations : [];
  for (const value of operations.slice(0, 200)) {
    const operation = objectValue(value);
    const title = stringValue(operation.title);
    if (!title) continue;
    const summary = stringValue(operation.summary);
    const detail = stringValue(operation.detail);
    parts.push({
      type: "operation",
      id: stringValue(operation.id) ?? "operation",
      kind: stringValue(operation.kind) ?? "operation",
      status: stringValue(operation.status) ?? "completed",
      title,
      paths: Array.isArray(operation.paths)
        ? operation.paths.filter((path): path is string => typeof path === "string").slice(0, 32)
        : [],
      ...(summary ? { summary } : {}),
      ...(detail ? { detail } : {}),
    });
  }
  if (parts.length > 0) return parts;
  const toolUse = objectValue(data.tool_use);
  if (Object.keys(toolUse).length > 0) return [{
    type: "tool_use", call_id: String(toolUse.call_id ?? ""), tool_name: String(toolUse.tool_name ?? "tool"),
    arguments: JSON.stringify(toolUse.arguments ?? {}),
  }];
  return [{ type: "structured", media_type: "application/json", value: JSON.stringify(message.data ?? {}) }];
}

function objectValue(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

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
