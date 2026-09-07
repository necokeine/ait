import assert from "node:assert/strict";
import test from "node:test";

import { renderRunProgress, renderRunTerminal } from "../src/message-renderer.js";
import { applyProgressEvent, progressFromCheckpoint } from "../src/run-progress.js";

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
    items: [{ type: "message", id: "answer", phase: "final_answer", text: "Hel", completed: false }],
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
  assert.equal(next?.items[0]?.type === "codex_message" && next.items[0].completed, false);
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
  assert.equal(progress?.items[0]?.type === "codex_message" && progress.items[0].completed, true);
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

test("keeps live commentary in process instead of presenting it as a final answer", () => {
  const progress = progressFromCheckpoint({
    ...base,
    seq: 1,
    status: "running",
    updated_at: 10,
    warnings: [],
    items: [{ type: "message", id: "commentary", phase: "commentary", text: "Still checking", completed: false }],
  });
  const html = renderRunProgress(progress, "Codex", true);
  assert.ok(html.includes('<details class="codex-process" open>'));
  assert.ok(!html.includes("codex-final-answer"));
});

test("renders retained partial output, error, and guarded workspace recovery separately", () => {
  const progress = progressFromCheckpoint({
    ...base,
    seq: 2,
    status: "failed",
    updated_at: 10,
    warnings: [],
    items: [
      { type: "message", id: "commentary", phase: "commentary", text: "File written", completed: true },
      { type: "message", id: "answer", phase: "final_answer", text: "Unfinished", completed: false },
    ],
  });
  assert.ok(progress);
  const html = renderRunTerminal("failed", "Stream ended.", "Codex", {
    progress,
    worktree: {
      head: "a".repeat(40), dirty: true, fingerprint: "f".repeat(64), truncated: false,
      changes: [{ status: "??", path: "src/new file.rs" }],
    },
  }, "run-a", "project-a");
  assert.ok(html.includes("Run failed"));
  assert.ok(html.includes("Partial output before termination"));
  assert.ok(html.includes("Confirmed · commentary"));
  assert.ok(html.includes("Unfinished · final_answer"));
  assert.ok(!html.includes("codex-final-answer"));
  assert.ok(html.includes("Workspace changes kept"));
  assert.ok(html.includes('data-file-path="src/new file.rs"'));
  assert.ok(html.includes('data-run-continue="run-a"'));
  assert.ok(html.includes("Continue with these changes"));
});

test("renders terminal inspection failures as explicit unknown state", () => {
  const html = renderRunTerminal("failed", "Provider stopped.", "Codex", {
    progressError: { code: "run_recovery_failed", message: "checkpoint unavailable" },
    worktreeError: { code: "project_git_head_unavailable", message: "path raced" },
  }, "run-a", "project-a");
  assert.ok(html.includes("Some terminal state could not be inspected"));
  assert.ok(html.includes("Progress archive unknown: checkpoint unavailable"));
  assert.ok(html.includes("Workspace state unknown: path raced"));
  assert.ok(!html.includes("Continue with these changes"));
});
