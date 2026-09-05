import assert from "node:assert/strict";
import test from "node:test";
import { flattenMessageTree, pathToMessage } from "../src/tree.js";
import type { DesktopMessage, DesktopSession } from "../src/types.js";

const message = (id: string, parentMessageId: string | null, createdAt: number): DesktopMessage => ({
  id,
  parentMessageId,
  projectId: "p1",
  role: id === "root" ? "system" : "user",
  kind: "standard",
  parts: [{ type: "text", text: id }],
  createdAt,
});

const messages = [
  message("root", null, 0),
  message("a", "root", 1),
  message("b", "a", 2),
  message("branch", "a", 3),
];
const session: DesktopSession = {
  id: "s1",
  projectId: "p1",
  title: "Main",
  currentMessageId: "b",
  agentId: "agent",
  version: 1,
  active: false,
  updatedAt: 0,
};

test("builds stable pre-order rows and marks both relevant paths", () => {
  const flat = flattenMessageTree(messages, session, "branch", new Set());
  assert.deepEqual(flat.map((node) => node.message.id), ["root", "a", "b", "branch"]);
  assert.deepEqual(flat.map((node) => node.depth), [0, 1, 2, 2]);
  assert.equal(flat.find((node) => node.message.id === "b")?.onCurrentBranch, true);
  assert.equal(flat.find((node) => node.message.id === "branch")?.selected, true);
  assert.equal(flat.find((node) => node.message.id === "a")?.ancestorOfSelection, true);
});

test("collapse removes descendants without corrupting other roots", () => {
  const flat = flattenMessageTree(
    [...messages, message("second-root", null, 4)],
    session,
    "branch",
    new Set(["a"]),
  );
  assert.deepEqual(flat.map((node) => node.message.id), ["root", "a", "second-root"]);
});

test("projects root-to-head paths without copying history", () => {
  assert.deepEqual(pathToMessage(messages, "branch").map(({ id }) => id), ["root", "a", "branch"]);
});

test("handles a large deep tree iteratively", () => {
  const large: DesktopMessage[] = [];
  for (let index = 0; index < 10_000; index += 1) {
    large.push(message(String(index), index === 0 ? null : String(index - 1), index));
  }
  const flat = flattenMessageTree(large, undefined, "9999", new Set());
  assert.equal(flat.length, 10_000);
  assert.equal(flat.at(-1)?.depth, 9_999);
});
