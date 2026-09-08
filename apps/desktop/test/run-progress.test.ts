import assert from "node:assert/strict";
import test from "node:test";

import { renderRunProgress, renderRunTerminal } from "../src/message-renderer.js";
import {
  applyProgressEvent,
  isTerminalRunEvent,
  progressFromCheckpoint,
  terminalRunForSession,
} from "../src/run-progress.js";
import type { DesktopView } from "../src/types.js";

const base = {
  version: 1,
  run_id: "run-a",
  project_id: "project-a",
  session_id: "session-a",
};

test("restores a checkpoint then appends only newer Run-local sequences", () => {
  const checkpoint = progressFromCheckpoint({
    ...base,
    seq: 4,
    status: "running",
    updated_at: 10,
    warnings: [],
    items: [{ type: "message", id: "answer", phase: "final_answer", text: "Hel" }],
  });
  assert.ok(checkpoint);
  const duplicate = applyProgressEvent(checkpoint, {
    ...base, seq: 4, type: "text_delta", item_id: "answer", delta: "duplicate",
  });
  assert.equal(duplicate?.items[0]?.type === "codex_message" && duplicate.items[0].text, "Hel");
  const next = applyProgressEvent(duplicate, {
    ...base, seq: 5, type: "text_delta", item_id: "answer", delta: "lo",
  });
  assert.equal(next?.items[0]?.type === "codex_message" && next.items[0].text, "Hello");
});

test("keeps item order while tool state and authoritative text are replaced", () => {
  let progress = applyProgressEvent(undefined, {
    ...base, seq: 1, type: "message_started", item_id: "commentary", phase: "commentary", text: "",
  });
  progress = applyProgressEvent(progress, {
    ...base, seq: 2, type: "operation_started", item_id: "tool",
    operation: { id: "tool", kind: "read", status: "inProgress", title: "Read file", paths: ["src/main.rs"] },
  });
  progress = applyProgressEvent(progress, {
    ...base, seq: 3, type: "message_completed", item_id: "commentary", phase: "commentary", text: "Inspected it.",
  });
  progress = applyProgressEvent(progress, {
    ...base, seq: 4, type: "operation_completed", item_id: "tool",
    operation: { id: "tool", kind: "read", status: "completed", title: "Read file", paths: ["src/main.rs"] },
  });
  assert.deepEqual(progress?.items.map((item) => item.id), ["commentary", "tool"]);
  assert.equal(progress?.items[0]?.type === "codex_message" && progress.items[0].text, "Inspected it.");
  assert.equal(progress?.items[1]?.type === "operation" && progress.items[1].status, "completed");
});

test("renders live process, partial final answer, and disconnected state separately", () => {
  const progress = progressFromCheckpoint({
    ...base,
    seq: 2,
    status: "running",
    updated_at: 10,
    warnings: [],
    items: [
      { type: "operation", operation: { id: "tool", kind: "read", status: "inProgress", title: "Read file", paths: [] } },
      { type: "message", id: "answer", phase: "final_answer", text: "Partial answer" },
    ],
  });
  const html = renderRunProgress(progress, "Codex", false);
  assert.ok(html.includes('<details class="codex-process" open>'));
  assert.ok(html.includes("Read file"));
  assert.ok(html.includes("Partial answer"));
  assert.ok(html.includes("Connection interrupted"));
  assert.ok(!html.includes("Run failed"));
});

test("keeps commentary-only live output in Process until an explicit final phase arrives", () => {
  const progress = progressFromCheckpoint({
    ...base,
    seq: 1,
    status: "running",
    updated_at: 10,
    warnings: [],
    items: [{ type: "message", id: "commentary", phase: "commentary", text: "Still inspecting." }],
  });
  const html = renderRunProgress(progress, "Codex", true);
  assert.ok(html.includes('class="codex-process"'));
  assert.ok(html.includes("Still inspecting."));
  assert.ok(!html.includes("data-codex-final-answer"));
});

test("renders failure and cancellation as terminal states rather than connection loss", () => {
  const failed = renderRunTerminal("failed", "Provider stopped.", "Codex");
  assert.ok(failed.includes("Run failed"));
  assert.ok(failed.includes("Provider stopped."));
  assert.ok(!failed.includes("Connection interrupted"));
  const cancelled = renderRunTerminal("cancelled", undefined, "Codex");
  assert.ok(cancelled.includes("Run cancelled"));
});

test("a cancellation event refreshes an active view into its cancelled terminal card", () => {
  const session = {
    id: "session-a", projectId: "project-a", name: "", title: "Session", description: "",
    titleGenerationStarted: false, currentMessageId: "message-a", agentId: "agent-a", version: 1,
    active: true, activeRunId: "run-a", updatedAt: 0,
  };
  const active: Pick<DesktopView, "sessions" | "runs"> = {
    sessions: [session],
    runs: [{
      id: "run-a", sessionId: "session-a", baseMessageId: "message-a",
      lastMessageId: null, status: "running",
    }],
  };
  const cancelledEvent = {
    api_version: 1, cursor: 12, kind: "run.updated", entity_id: "run-a", created_at: 1,
    body: { id: "run-a", status: "cancelled" },
  };

  assert.equal(terminalRunForSession(active, "session-a"), undefined);
  assert.equal(isTerminalRunEvent(cancelledEvent), true);

  const authoritative: Pick<DesktopView, "sessions" | "runs"> = {
    sessions: [{ ...session, active: false, activeRunId: null, version: 2 }],
    runs: [{
      id: "run-a", sessionId: "session-a", baseMessageId: "message-a",
      lastMessageId: null, status: "cancelled", error: { message: "run was cancelled" },
    }],
  };
  const terminal = terminalRunForSession(authoritative, "session-a");
  assert.equal(authoritative.sessions[0]?.activeRunId, null);
  assert.equal(terminal?.status, "cancelled");
  assert.ok(renderRunTerminal(terminal!.status, terminal!.error?.message, "Codex").includes("Run cancelled"));

  assert.equal(isTerminalRunEvent({ ...cancelledEvent, kind: "run.cancelled" }), true);
});
