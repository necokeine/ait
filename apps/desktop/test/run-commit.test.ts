import assert from "node:assert/strict";
import test from "node:test";
import { renderRunCommit } from "../src/run-commit.js";
import type { DesktopRun } from "../src/types.js";

const run = (status: DesktopRun["status"], gitCommit?: DesktopRun["gitCommit"]): DesktopRun => ({
  id: "run-1", status, gitCommit,
} as DesktopRun);

test("Git failure offers an independent retry only after model completion", () => {
  const receipt = { status: "failed", commitId: "fixed-commit", reason: "Git index is locked" } as const;
  const html = renderRunCommit(run("completed", receipt));
  assert.match(html, /data-retry-commit="run-1"/);
  assert.match(html, /fixed-commit/);
  for (const status of ["running", "settling", "cancelled", "failed"] as const) {
    assert.doesNotMatch(renderRunCommit(run(status, receipt)), /data-retry-commit/);
  }
  assert.doesNotMatch(renderRunCommit(run("completed", receipt), false), /data-retry-commit/);
});

test("Git receipts remain separate and escape untrusted error text", () => {
  assert.equal(renderRunCommit(run("completed")), "");
  for (const status of ["pending", "prepared", "committed", "skipped"] as const) {
    const html = renderRunCommit(run("completed", { status, reason: '<script>"error"</script>' }));
    assert.match(html, /aria-label="Git commit"/);
    assert.doesNotMatch(html, /<script>|data-retry-commit/);
  }
});
