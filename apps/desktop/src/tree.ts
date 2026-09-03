import type { DesktopMessage, DesktopSession } from "./types.js";

export interface FlatTreeNode {
  message: DesktopMessage;
  depth: number;
  childCount: number;
  expanded: boolean;
  selected: boolean;
  onCurrentBranch: boolean;
  ancestorOfSelection: boolean;
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

export function flattenMessageTree(
  messages: DesktopMessage[],
  currentSession: DesktopSession | undefined,
  selectedId: string | undefined,
  collapsed: ReadonlySet<string>,
): FlatTreeNode[] {
  const byId = new Map(messages.map((message) => [message.id, message]));
  const children = new Map<string | null, DesktopMessage[]>();
  for (const message of messages) {
    const parent = message.parentMessageId && byId.has(message.parentMessageId)
      ? message.parentMessageId
      : null;
    const siblings = children.get(parent) ?? [];
    siblings.push(message);
    children.set(parent, siblings);
  }
  for (const siblings of children.values()) {
    siblings.sort((left, right) => left.createdAt - right.createdAt || left.id.localeCompare(right.id));
  }

  const ancestorsOfSelection = collectAncestors(byId, selectedId);
  const currentBranch = collectAncestors(byId, currentSession?.currentMessageId);
  const flat: FlatTreeNode[] = [];
  const roots = children.get(null) ?? [];
  const stack = roots.toReversed().map((message) => ({ message, depth: 0 }));
  const seen = new Set<string>();

  while (stack.length > 0) {
    const entry = stack.pop();
    if (!entry || seen.has(entry.message.id)) continue;
    seen.add(entry.message.id);
    const descendants = children.get(entry.message.id) ?? [];
    const expanded = !collapsed.has(entry.message.id);
    flat.push({
      message: entry.message,
      depth: entry.depth,
      childCount: descendants.length,
      expanded,
      selected: entry.message.id === selectedId,
      onCurrentBranch: currentBranch.has(entry.message.id),
      ancestorOfSelection: ancestorsOfSelection.has(entry.message.id),
    });
    if (expanded) {
      for (const child of descendants.toReversed()) {
        stack.push({ message: child, depth: entry.depth + 1 });
      }
    }
  }
  return flat;
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
