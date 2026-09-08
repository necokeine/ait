import assert from "node:assert/strict";
import test from "node:test";
import { approvalAction, approvalScope, isApprovalEvent, renderPendingApprovals } from "../src/approval-ui.js";
import type { DesktopRun } from "../src/types.js";

const pendingRun: DesktopRun = {
  id: "run-a",
  sessionId: "session-a",
  baseMessageId: "message-a",
  lastMessageId: null,
  status: "running",
  permissionProfile: { sandbox: "read_only", approval: "on_request" },
  nativeApprovals: [{
    id: "approval-a",
    runId: "run-a",
    protocolRequestId: 42,
    method: "item/permissions/requestApproval",
    kind: "permissions",
    threadId: "thread-a",
    turnId: "turn-a",
    itemId: "item-a",
    requestedPermissions: { fileSystem: { write: ["/workspace"] } },
    status: "pending",
    createdAt: 1,
  }],
};

test("renders a correlated permission request with only explicit decisions", () => {
  const html = renderPendingApprovals(pendingRun);
  assert.match(html, /thread-a/);
  assert.match(html, /turn-a/);
  assert.match(html, /Request[\s\S]*42/);
  assert.match(html, /data-approval-scope="turn"/);
  assert.doesNotMatch(html, /data-approval-scope="one_shot"/);
  assert.match(html, /data-approval-scope="session"/);
  assert.match(html, /data-approval-action="deny"/);
  assert.match(html, /data-approval-action="cancel"/);
  assert.doesNotMatch(html, /secret/i);
});

test("ordinary command approval stays one-shot rather than turn-scoped", () => {
  const commandRun = structuredClone(pendingRun);
  commandRun.nativeApprovals[0]!.kind = "command_execution";
  commandRun.nativeApprovals[0]!.requestedPermissions = undefined;
  const html = renderPendingApprovals(commandRun);
  assert.match(html, /data-approval-scope="one_shot"/);
  assert.doesNotMatch(html, /data-approval-scope="turn"/);
});

test("rejects invented actions and scopes before IPC", () => {
  assert.throws(() => approvalAction("allow"));
  assert.throws(() => approvalScope("forever", "approve"));
  assert.throws(() => approvalScope("session", "deny"));
  assert.equal(approvalScope(undefined, "deny"), undefined);
});

test("approval lifecycle events force a durable view resync", () => {
  assert.equal(isApprovalEvent("run.approval_requested"), true);
  assert.equal(isApprovalEvent("run.approval_resolved"), true);
  assert.equal(isApprovalEvent("run.approval_expired"), true);
  assert.equal(isApprovalEvent("run.progress"), false);
});
