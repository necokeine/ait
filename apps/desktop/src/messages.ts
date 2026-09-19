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
  const data = objectValue(message.data);
  const native = objectValue(data.native_message);
  if (message.kind === "tool_result") {
    const result = objectValue(native.tool_result ?? data.tool_result);
    return [{
      type: "tool_result", call_id: stringValue(result.call_id) ?? message.id,
      status: stringValue(result.status) ?? "completed",
      output: result.output === null ? null : typeof result.output === "string" ? result.output
        : result.output !== undefined ? JSON.stringify(result.output) : message.text ?? JSON.stringify(message.data ?? {}),
      error: stringValue(result.error) ?? null,
    }];
  }
  if (Array.isArray(native.sub_messages) && native.sub_messages.length > 0) {
    const parts: MessagePart[] = native.sub_messages.flatMap((value): MessagePart[] => {
      const part = objectValue(value);
      if (part.type === "text" && typeof part.text === "string") return [{ type: "text", text: part.text }];
      if (part.type === "tool_use") return [{
        type: "tool_use", call_id: String(part.call_id ?? ""), tool_name: String(part.tool_name ?? "tool"),
        arguments: typeof part.arguments === "string" ? part.arguments : JSON.stringify(part.arguments ?? {}),
      }];
      if (part.type === "file_ref") return [{ type: "file", name: String(part.name ?? "Attachment"), media_type: String(part.media_type ?? "") }];
      if (part.type === "structured_data") return [{ type: "structured", media_type: String(part.media_type ?? "application/json"), value: String(part.value ?? "") }];
      if (part.type === "provider_item") return providerItemParts(part);
      return [];
    });
    if (parts.length > 0) return parts;
  }
  const codex = objectValue(data.codex);
  const operations = operationParts(codex.operations);
  const outputItems = Array.isArray(codex.output_items) ? codex.output_items : [];
  if (outputItems.length > 0) {
    const operationsById = new Map(operations.map((operation) => [operation.id, operation]));
    const projected: MessagePart[] = [];
    for (const value of outputItems) {
      const item = objectValue(value);
      const id = stringValue(item.id);
      if (item.type === "message" && id) {
        const text = stringValue(item.text);
        if (text) projected.push({
          type: "codex_message",
          id,
          phase: stringValue(item.phase) ?? "",
          text,
        });
      } else if (item.type === "operation" && id) {
        const operation = operationsById.get(id);
        if (operation) projected.push(operation);
      }
    }
    let messageIndexes = projected
      .map((part, index) => part.type === "codex_message" ? index : -1)
      .filter((index) => index >= 0);
    if (messageIndexes.length === 0 && message.text !== null) {
      projected.push({ type: "codex_message", id: message.id, phase: "final_answer", text: message.text });
      messageIndexes = [projected.length - 1];
    }
    if (!messageIndexes.some((index) => projected[index]?.type === "codex_message"
      && projected[index].phase === "final_answer")) {
      const finalIndex = messageIndexes.at(-1);
      const finalMessage = finalIndex === undefined ? undefined : projected[finalIndex];
      if (finalIndex !== undefined && finalMessage?.type === "codex_message") {
        projected[finalIndex] = { ...finalMessage, phase: "final_answer" };
      }
    }
    if (projected.length > 0) return projected;
  }

  if (operations.length > 0 && message.text !== null) {
    return [
      ...operations,
      { type: "codex_message", id: message.id, phase: "final_answer", text: message.text },
    ];
  }

  const parts: MessagePart[] = [];
  if (message.text !== null) parts.push({ type: "text", text: message.text });
  parts.push(...operations);
  if (parts.length > 0) return parts;
  const toolUse = objectValue(data.tool_use);
  if (Object.keys(toolUse).length > 0) return [{
    type: "tool_use", call_id: String(toolUse.call_id ?? ""), tool_name: String(toolUse.tool_name ?? "tool"),
    arguments: JSON.stringify(toolUse.arguments ?? {}),
  }];
  return [{ type: "structured", media_type: "application/json", value: JSON.stringify(message.data ?? {}) }];
}

function providerItemParts(part: Record<string, unknown>): MessagePart[] {
  const item = objectValue(part.payload);
  const kind = stringValue(part.item_type) ?? stringValue(item.type) ?? "unknown";
  const id = stringValue(part.external_item_id) ?? stringValue(item.id) ?? "native-item";
  if (kind === "agentMessage" && typeof item.text === "string") {
    return [{ type: "codex_message", id, phase: stringValue(item.phase) ?? "final_answer", text: item.text }];
  }
  if (kind === "plan" && typeof item.text === "string") {
    return [{ type: "operation", id, kind, status: "completed", title: "Plan", paths: [], detail: item.text }];
  }
  if (kind === "reasoning") {
    const summary = Array.isArray(item.summary) ? item.summary.filter((text): text is string => typeof text === "string").join("\n") : "";
    return [{ type: "operation", id, kind, status: "completed", title: "Reasoning", paths: [], ...(summary ? { detail: summary } : {}) }];
  }
  const titles: Record<string, string> = {
    commandExecution: "Command", fileChange: "File changes", mcpToolCall: "MCP tool",
    dynamicToolCall: "Tool", collabAgentToolCall: "Agent", webSearch: "Web search",
    imageView: "Image", imageGeneration: "Image generation", contextCompaction: "Context compaction",
    enteredReviewMode: "Review started", exitedReviewMode: "Review completed",
  };
  const title = Object.hasOwn(titles, kind) ? titles[kind] : undefined;
  if (title) {
    const paths = Array.isArray(item.changes) ? item.changes.map((change) => stringValue(objectValue(change).path)).filter((path): path is string => !!path) : [];
    const summary = stringValue(item.command) ?? stringValue(item.query) ?? stringValue(item.tool) ?? stringValue(item.prompt);
    const detail = stringValue(item.aggregatedOutput);
    return [{ type: "operation", id, kind, status: stringValue(item.status) ?? "completed", title, paths: paths.slice(0, 32),
      ...(summary ? { summary } : {}), ...(detail ? { detail } : {}) }];
  }
  // Unknown provider variants stay individually inspectable without dumping the Message envelope.
  return [{ type: "structured", media_type: "application/json", value: JSON.stringify({ type: kind, id, payload: item }) }];
}

function operationParts(value: unknown): Extract<MessagePart, { type: "operation" }>[] {
  const parts: Extract<MessagePart, { type: "operation" }>[] = [];
  const operations = Array.isArray(value) ? value : [];
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
  return parts;
}

function objectValue(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function messageAuthor(message: DesktopMessage, agents: AgentSummary[]): string {
  if (message.kind === "tool_result") return "Tool";
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
