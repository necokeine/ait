import assert from "node:assert/strict";
import test from "node:test";
import { buildMessageTimeline, directMessageChildren, pathToMessage, resolveBranchHead, sessionForBranch } from "../src/tree.js";
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
  message("branch-tail", "branch", 4),
];
const session = (id: string, currentMessageId: string, updatedAt = 0): DesktopSession => ({
  id,
  projectId: "p1",
  workdir: `/p1/.ait/${id}`,
  title: id,
  currentMessageId,
  agentId: "agent",
  version: 1,
  active: false,
  activeRunId: null,
  updatedAt,
});

test("projects a linear branch as one unindented timeline", () => {
  const timeline = buildMessageTimeline(messages, session("main", "b"), undefined);

  assert.deepEqual(timeline.map((node) => node.message.id), ["root", "a", "b"]);
  assert.equal(timeline.every((node) => !("depth" in node)), true);
  assert.equal(timeline.every((node) => node.onCurrentBranch), true);
});

test("exposes every direct child, including nodes on a linear path", () => {
  const timeline = buildMessageTimeline(messages, session("main", "b"), undefined);
  const fork = timeline.find((node) => node.message.id === "a");

  assert.deepEqual(fork?.children.map((branch) => branch.message.id), ["b", "branch"]);
  assert.deepEqual(fork?.children.map((branch) => branch.active), [true, false]);
  assert.deepEqual(timeline.find((node) => node.message.id === "root")?.children.map(({ message }) => message.id), ["a"]);
  assert.deepEqual(directMessageChildren(messages, "a").map(({ id }) => id), ["b", "branch"]);
});

test("switches to a child path and follows its Session head", () => {
  const sessions = [session("main", "b", 1), session("alternative", "branch-tail", 2)];
  const head = resolveBranchHead(messages, sessions, "branch");
  const timeline = buildMessageTimeline(messages, sessions[0], head);

  assert.equal(head, "branch-tail");
  assert.deepEqual(timeline.map((node) => node.message.id), ["root", "a", "branch", "branch-tail"]);
  assert.deepEqual(
    timeline.find((node) => node.message.id === "a")?.children.map((branch) => branch.active),
    [false, true],
  );
  assert.equal(timeline.find((node) => node.message.id === "branch")?.onCurrentBranch, false);
});

test("falls back to the deepest descendant when a branch has no Session head", () => {
  assert.equal(resolveBranchHead(messages, [], "branch"), "branch-tail");
  assert.equal(resolveBranchHead([...messages, message("newest-shallow", "root", 20)], [], "root"), "branch-tail");
});

test("chooses the Session with the longest descendant chain before recency", () => {
  const sessions = [session("main", "b", 100), session("alternative", "branch-tail", 2)];

  assert.equal(sessionForBranch(messages, sessions, "branch")?.id, "alternative");
  assert.equal(sessionForBranch(messages, sessions, "a")?.id, "alternative");
  assert.equal(sessionForBranch(messages, sessions, "b")?.id, "main");
});

test("projects root-to-head paths without copying history", () => {
  assert.deepEqual(pathToMessage(messages, "branch-tail").map(({ id }) => id), ["root", "a", "branch", "branch-tail"]);
});

test("handles a large deep timeline iteratively", () => {
  const large: DesktopMessage[] = [];
  for (let index = 0; index < 10_000; index += 1) {
    large.push(message(String(index), index === 0 ? null : String(index - 1), index));
  }
  const timeline = buildMessageTimeline(large, undefined, "9999");
  assert.equal(timeline.length, 10_000);
  assert.equal(timeline.at(-1)?.message.id, "9999");
});
