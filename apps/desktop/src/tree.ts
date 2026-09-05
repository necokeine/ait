import { agentDisplayName } from "./projects.js";
import type { AgentSummary, DesktopMessage, DesktopSession } from "./types.js";

export interface TimelineBranch {
  message: DesktopMessage;
  active: boolean;
}

export interface TimelineNode {
  message: DesktopMessage;
  selected: boolean;
  onCurrentBranch: boolean;
  branches: TimelineBranch[];
}

export function messageText(message: DesktopMessage): string {
  for (const part of message.parts) {
    if (part.type === "text") return part.text;
    if (part.type === "tool_use") return `${part.tool_name} ${part.arguments}`;
    if (part.type === "file") return part.name;
    if (part.type === "redacted") return "Redacted message";
  }
  return message.kind === "tool_result" ? "Tool result" : "Structured message";
}

export function messageAuthor(message: DesktopMessage, agents: AgentSummary[]): string {
  if (message.role === "user") return "You";
  if (message.role === "system") return "System";
  const agent = agents.find((candidate) => candidate.id === message.agentId);
  return agent ? agentDisplayName(agent) : message.agentId ?? "Agent";
}

export function pathToMessage(messages: DesktopMessage[], headId: string): DesktopMessage[] {
  const byId = new Map(messages.map((message) => [message.id, message]));
  const path: DesktopMessage[] = [];
  const seen = new Set<string>();
  let cursor = byId.get(headId);
  while (cursor && !seen.has(cursor.id)) {
    seen.add(cursor.id);
    path.push(cursor);
    cursor = cursor.parentMessageId ? byId.get(cursor.parentMessageId) : undefined;
  }
  return path.reverse();
}

export function buildMessageTimeline(
  messages: DesktopMessage[],
  currentSession: DesktopSession | undefined,
  viewedHeadId: string | undefined,
  selectedId: string | undefined,
): TimelineNode[] {
  const byId = new Map(messages.map((message) => [message.id, message]));
  const children = collectChildren(messages, byId);
  const currentBranch = collectAncestors(byId, currentSession?.currentMessageId);
  const requestedHead = viewedHeadId && byId.has(viewedHeadId)
    ? viewedHeadId
    : currentSession?.currentMessageId;
  const path = requestedHead ? pathToMessage(messages, requestedHead) : [];

  return path.map((message, index) => {
    const successors = children.get(message.id) ?? [];
    const activeSuccessorId = path[index + 1]?.id;
    return {
      message,
      selected: message.id === selectedId,
      onCurrentBranch: currentBranch.has(message.id),
      branches: successors.length > 1
        ? successors.map((successor) => ({
            message: successor,
            active: successor.id === activeSuccessorId,
          }))
        : [],
    };
  });
}

export function resolveBranchHead(
  messages: DesktopMessage[],
  sessions: DesktopSession[],
  branchRootId: string,
): string | undefined {
  const byId = new Map(messages.map((message) => [message.id, message]));
  if (!byId.has(branchRootId)) return undefined;

  const session = sessions
    .filter((candidate) => isDescendantOf(byId, candidate.currentMessageId, branchRootId))
    .toSorted((left, right) => right.updatedAt - left.updatedAt || left.id.localeCompare(right.id))[0];
  if (session) return session.currentMessageId;

  const children = collectChildren(messages, byId);
  let cursor = byId.get(branchRootId);
  const seen = new Set<string>();
  while (cursor && !seen.has(cursor.id)) {
    seen.add(cursor.id);
    const successor = (children.get(cursor.id) ?? []).at(-1);
    if (!successor) return cursor.id;
    cursor = successor;
  }
  return branchRootId;
}

export function sessionForMessage(
  messages: DesktopMessage[],
  sessions: DesktopSession[],
  messageId: string,
  preferredSessionId: string | undefined,
): DesktopSession | undefined {
  const byId = new Map(messages.map((message) => [message.id, message]));
  const candidates = sessions.filter((session) =>
    isDescendantOf(byId, session.currentMessageId, messageId));
  return candidates.find((session) => session.id === preferredSessionId)
    ?? candidates.toSorted((left, right) =>
      right.updatedAt - left.updatedAt || left.id.localeCompare(right.id))[0];
}

function collectChildren(
  messages: DesktopMessage[],
  byId: ReadonlyMap<string, DesktopMessage>,
): Map<string, DesktopMessage[]> {
  const children = new Map<string, DesktopMessage[]>();
  for (const message of messages) {
    if (!message.parentMessageId || !byId.has(message.parentMessageId)) continue;
    const siblings = children.get(message.parentMessageId) ?? [];
    siblings.push(message);
    children.set(message.parentMessageId, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((left, right) => left.createdAt - right.createdAt || left.id.localeCompare(right.id));
  }
  return children;
}

function isDescendantOf(
  byId: ReadonlyMap<string, DesktopMessage>,
  candidateId: string,
  ancestorId: string,
): boolean {
  const seen = new Set<string>();
  let cursor = byId.get(candidateId);
  while (cursor && !seen.has(cursor.id)) {
    if (cursor.id === ancestorId) return true;
    seen.add(cursor.id);
    cursor = cursor.parentMessageId ? byId.get(cursor.parentMessageId) : undefined;
  }
  return false;
}

function collectAncestors(
  byId: ReadonlyMap<string, DesktopMessage>,
  id: string | undefined,
): Set<string> {
  const result = new Set<string>();
  let cursor = id ? byId.get(id) : undefined;
  while (cursor && !result.has(cursor.id)) {
    result.add(cursor.id);
    cursor = cursor.parentMessageId ? byId.get(cursor.parentMessageId) : undefined;
  }
  return result;
}
