import assert from "node:assert/strict";
import test from "node:test";

import { runFailure } from "../src/runs.js";

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
