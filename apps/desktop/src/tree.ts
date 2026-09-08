import type { DesktopMessage, DesktopSession } from "./types.js";

export interface TimelineBranch {
  message: DesktopMessage;
  active: boolean;
}

export interface TimelineNode {
  message: DesktopMessage;
  onCurrentBranch: boolean;
  children: TimelineBranch[];
}

export function messageText(message: DesktopMessage): string {
  const final = message.parts.find((part) =>
    part.type === "codex_message" && part.phase === "final_answer");
  if (final?.type === "codex_message") return final.text;
  for (const part of message.parts) {
    if (part.type === "text") return part.text;
    if (part.type === "codex_message") return part.text;
    if (part.type === "tool_use") return `${part.tool_name} ${part.arguments}`;
    if (part.type === "file") return part.name;
    if (part.type === "redacted") return "Redacted message";
  }
  return message.kind === "tool_result" ? "Tool result" : "Structured message";
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
      onCurrentBranch: currentBranch.has(message.id),
      children: successors.map((successor) => ({
        message: successor,
        active: successor.id === activeSuccessorId,
      })),
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

  const session = sessionForBranch(messages, sessions, branchRootId);
  if (session) return session.currentMessageId;

  const children = collectChildren(messages, byId);
  let deepest = byId.get(branchRootId)!;
  let deepestDepth = 0;
  const stack = [{ message: deepest, depth: 0 }];
  const seen = new Set<string>();
  while (stack.length > 0) {
    const current = stack.pop()!;
    if (seen.has(current.message.id)) continue;
    seen.add(current.message.id);
    if (current.depth > deepestDepth
      || current.depth === deepestDepth && compareMessages(deepest, current.message) < 0) {
      deepest = current.message;
      deepestDepth = current.depth;
    }
    for (const child of children.get(current.message.id) ?? []) {
      stack.push({ message: child, depth: current.depth + 1 });
    }
  }
  return deepest.id;
}

export function sessionForBranch(
  messages: DesktopMessage[],
  sessions: DesktopSession[],
  branchRootId: string,
): DesktopSession | undefined {
  const byId = new Map(messages.map((message) => [message.id, message]));
  return sessions.flatMap((session) => {
    const depth = descendantDepth(byId, session.currentMessageId, branchRootId);
    return depth === undefined ? [] : [{ session, depth }];
  }).toSorted((left, right) =>
    right.depth - left.depth
      || right.session.updatedAt - left.session.updatedAt
      || left.session.id.localeCompare(right.session.id))[0]?.session;
}

export function directMessageChildren(
  messages: DesktopMessage[],
  parentId: string,
): DesktopMessage[] {
  return messages
    .filter((message) => message.parentMessageId === parentId)
    .toSorted(compareMessages);
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
    siblings.sort(compareMessages);
  }
  return children;
}

function descendantDepth(
  byId: ReadonlyMap<string, DesktopMessage>,
  candidateId: string,
  ancestorId: string,
): number | undefined {
  const seen = new Set<string>();
  let cursor = byId.get(candidateId);
  let depth = 0;
  while (cursor && !seen.has(cursor.id)) {
    if (cursor.id === ancestorId) return depth;
    seen.add(cursor.id);
    cursor = cursor.parentMessageId ? byId.get(cursor.parentMessageId) : undefined;
    depth += 1;
  }
  return undefined;
}

function compareMessages(left: DesktopMessage, right: DesktopMessage): number {
  return left.createdAt - right.createdAt || left.id.localeCompare(right.id);
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
