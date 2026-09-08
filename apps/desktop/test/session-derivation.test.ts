import assert from "node:assert/strict";
import test from "node:test";

import { submitSessionDerivation } from "../src/session-derivation.js";

test("continues an eligible leaf in the unlocked current Session", async () => {
  let forks = 0;
  const result = await submitSessionDerivation({
    currentSessionId: "current",
    newSessionId: "fork",
    reuseCurrentSession: true,
    submitCurrent: async () => ({ id: "current-run" }),
    submitFork: async () => { forks += 1; return { id: "fork-run" }; },
  });

  assert.deepEqual(result, {
    run: { id: "current-run" },
    selectedSessionId: "current",
    reusedCurrentSession: true,
  });
  assert.equal(forks, 0);
});

test("falls back to a new Session when the current Session is locked", async () => {
  const busy = Object.assign(new Error("session is busy"), { code: "SESSION_BUSY" });
  const result = await submitSessionDerivation({
    currentSessionId: "current",
    newSessionId: "fork",
    reuseCurrentSession: true,
    submitCurrent: async () => { throw busy; },
    submitFork: async () => ({ id: "fork-run" }),
  });

  assert.deepEqual(result, {
    run: { id: "fork-run" },
    selectedSessionId: "fork",
    reusedCurrentSession: false,
  });
});

test("does not hide non-lock failures behind a fork", async () => {
  const failure = Object.assign(new Error("provider rejected input"), { code: "INVALID_MESSAGE_ROLE" });
  let forks = 0;

  await assert.rejects(submitSessionDerivation({
    currentSessionId: "current",
    newSessionId: "fork",
    reuseCurrentSession: true,
    submitCurrent: async () => { throw failure; },
    submitFork: async () => { forks += 1; return { id: "fork-run" }; },
  }), failure);
  assert.equal(forks, 0);
});

test("uses a normal fork when the selected Message is not the current leaf", async () => {
  let currentSubmissions = 0;
  const result = await submitSessionDerivation({
    currentSessionId: "current",
    newSessionId: "fork",
    reuseCurrentSession: false,
    submitCurrent: async () => { currentSubmissions += 1; return { id: "current-run" }; },
    submitFork: async () => ({ id: "fork-run" }),
  });

  assert.equal(result.selectedSessionId, "fork");
  assert.equal(result.reusedCurrentSession, false);
  assert.equal(currentSubmissions, 0);
});
