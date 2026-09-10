import assert from "node:assert/strict";
import test from "node:test";

import { pendingBranchResolution, runFailure, startupRecoveryNotices } from "../src/runs.js";

test("surfaces a failed Codex run returned by send-message", () => {
  assert.deepEqual(runFailure({
    status: "failed",
    error: {
      code: "PROVIDER_FAILED",
      message: "The configured Codex model is unavailable.",
    },
  }), {
    code: "PROVIDER_FAILED",
    message: "The configured Codex model is unavailable.",
  });
});

test("accepts completed and still-running runs", () => {
  assert.equal(runFailure({ status: "completed" }), undefined);
  assert.equal(runFailure({ status: "running" }), undefined);
});

test("surfaces interrupted Runs that need workspace review", () => {
  assert.deepEqual(runFailure({
    status: "interrupted",
    error: {
      code: "RUN_RECOVERY_FAILED",
      message: "Workspace changes were preserved for review.",
    },
  }), {
    code: "RUN_RECOVERY_FAILED",
    message: "Workspace changes were preserved for review.",
  });
});

test("projects startup-interrupted Runs with Project and Session locations", () => {
  const view = {
    projects: [{ id: "project-1", name: "Ait" }, { id: "project-2", name: "Docs" }],
    sessions: [{ id: "session-1", project_id: "project-1", name: "Recovery work", title: null }],
    runs: [
      {
        id: "run-interrupted", project_id: "project-1", session_id: "session-1",
        status: "interrupted",
        error: { code: "RUN_RECOVERY_FAILED", message: "Git index changed after checkpoint." },
      },
      {
        id: "run-completed", project_id: "project-2", session_id: null,
        status: "completed", error: null,
      },
    ],
  };
  assert.deepEqual(startupRecoveryNotices(view), [{
    projectId: "project-1",
    sessionId: "session-1",
    sessionTitle: "Recovery work",
    runId: "run-interrupted",
    code: "RUN_RECOVERY_FAILED",
    message: "Git index changed after checkpoint.",
  }]);
});

test("keeps a new branch pending until its Run is completed and its Session exists", () => {
  const pending = { sourceMessageId: "message-a", sessionId: "session-new", runId: "run-new" };
  assert.deepEqual(pendingBranchResolution(pending, {
    sessions: [{ id: "session-new" }],
    runs: [{ id: "run-new", status: "settling" }],
  }), { kind: "pending" });
  assert.deepEqual(pendingBranchResolution(pending, {
    sessions: [],
    runs: [{ id: "run-new", status: "completed" }],
  }), { kind: "pending" });
  assert.deepEqual(pendingBranchResolution(pending, {
    sessions: [{ id: "session-new" }],
    runs: [{ id: "run-new", status: "completed" }],
  }), { kind: "ready", sessionId: "session-new" });
});

test("keeps the source Session selected when new branch generation fails", () => {
  const pending = { sourceMessageId: "message-a", sessionId: "session-new", runId: "run-new" };
  assert.deepEqual(pendingBranchResolution(pending, {
    sessions: [{ id: "session-new" }],
    runs: [{ id: "run-new", status: "failed", error: { message: "Provider unavailable." } }],
  }), { kind: "failed", message: "Provider unavailable." });
});
