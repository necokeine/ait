import type {
  ControlEvent,
  DesktopRun,
  DesktopView,
  MessagePart,
  RunProgress,
  RunProgressItem,
} from "./types.js";

const terminalRunStatuses = new Set(["completed", "failed", "cancelled", "limit_exceeded", "interrupted"]);

export function isTerminalRunEvent(event: ControlEvent): boolean {
  if (event.kind !== "run.updated" && event.kind !== "run.cancelled") return false;
  const status = text(record(event.body).status);
  return status !== undefined && terminalRunStatuses.has(status);
}

export function terminalRunForSession(
  view: Pick<DesktopView, "sessions" | "runs">,
  sessionId: string,
): DesktopRun | undefined {
  const session = view.sessions.find((candidate) => candidate.id === sessionId);
  if (!session || session.activeRunId) return undefined;
  const run = view.runs.findLast((candidate) => candidate.sessionId === sessionId);
  return run
    && terminalRunStatuses.has(run.status)
    && run.status !== "completed"
    && run.lastMessageId === null
    ? run
    : undefined;
}

export function progressFromCheckpoint(value: unknown): RunProgress | undefined {
  const checkpoint = record(value);
  const runId = text(checkpoint.run_id);
  const projectId = text(checkpoint.project_id);
  const seq = integer(checkpoint.seq);
  if (!runId || !projectId || seq === undefined) return undefined;
  const items = (Array.isArray(checkpoint.items) ? checkpoint.items : [])
    .map(checkpointItem)
    .filter((item): item is RunProgressItem => item !== undefined);
  return {
    runId,
    projectId,
    sessionId: nullableText(checkpoint.session_id),
    seq,
    status: text(checkpoint.status) ?? "running",
    items,
    warnings: warnings(checkpoint.warnings),
    updatedAt: integer(checkpoint.updated_at) ?? 0,
  };
}

export function applyProgressEvent(
  current: RunProgress | undefined,
  value: unknown,
): RunProgress | undefined {
  const event = record(value);
  const runId = text(event.run_id);
  const projectId = text(event.project_id);
  const seq = integer(event.seq);
  if (!runId || !projectId || seq === undefined || current && seq <= current.seq) return current;
  const next: RunProgress = current ? structuredClone(current) : {
    runId,
    projectId,
    sessionId: nullableText(event.session_id),
    seq: 0,
    status: "running",
    items: [],
    warnings: [],
    updatedAt: 0,
  };
  next.seq = seq;
  next.updatedAt = Date.now();
  const itemId = text(event.item_id);
  switch (event.type) {
    case "message_started":
    case "message_completed": {
      if (!itemId) break;
      const part: RunProgressItem = {
        type: "codex_message",
        id: itemId,
        phase: text(event.phase) ?? "",
        text: typeof event.text === "string" ? event.text : "",
      };
      replaceItem(next.items, itemId, part);
      break;
    }
    case "text_delta": {
      if (!itemId || typeof event.delta !== "string") break;
      const index = itemIndex(next.items, itemId);
      const existing = index >= 0 ? next.items[index] : undefined;
      if (existing?.type === "codex_message") existing.text += event.delta;
      else next.items.push({ type: "codex_message", id: itemId, phase: "", text: event.delta });
      break;
    }
    case "operation_started":
    case "operation_completed": {
      const operation = operationPart(event.operation);
      if (operation) replaceItem(next.items, operation.id, operation);
      break;
    }
    case "warning": {
      const message = text(event.message);
      if (message) {
        next.warnings.push({
          message,
          retrying: event.retrying === true,
          ...(text(event.code) ? { code: text(event.code)! } : {}),
        });
        next.warnings = next.warnings.slice(-8);
      }
      break;
    }
    case "turn_status":
      next.status = text(event.status) ?? next.status;
      break;
  }
  return next;
}

function checkpointItem(value: unknown): RunProgressItem | undefined {
  const item = record(value);
  if (item.type === "message") {
    const id = text(item.id);
    if (!id) return undefined;
    return {
      type: "codex_message",
      id,
      phase: text(item.phase) ?? "",
      text: typeof item.text === "string" ? item.text : "",
    };
  }
  return item.type === "operation" ? operationPart(item.operation) : undefined;
}

function operationPart(value: unknown): Extract<MessagePart, { type: "operation" }> | undefined {
  const operation = record(value);
  const id = text(operation.id);
  const title = text(operation.title);
  if (!id || !title) return undefined;
  const summary = text(operation.summary);
  const detail = text(operation.detail);
  return {
    type: "operation",
    id,
    kind: text(operation.kind) ?? "operation",
    status: text(operation.status) ?? "in_progress",
    title,
    paths: Array.isArray(operation.paths)
      ? operation.paths.filter((path): path is string => typeof path === "string").slice(0, 32)
      : [],
    ...(summary ? { summary } : {}),
    ...(detail ? { detail } : {}),
  };
}

function replaceItem(items: RunProgressItem[], id: string, replacement: RunProgressItem): void {
  const index = itemIndex(items, id);
  if (index >= 0) items[index] = replacement;
  else items.push(replacement);
}

function itemIndex(items: RunProgressItem[], id: string): number {
  return items.findIndex((item) => item.id === id);
}

function warnings(value: unknown): RunProgress["warnings"] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    const warning = record(entry);
    const message = text(warning.message);
    if (!message) return [];
    const code = text(warning.code);
    return [{ message, retrying: warning.retrying === true, ...(code ? { code } : {}) }];
  }).slice(-8);
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function nullableText(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function integer(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}
