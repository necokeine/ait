import assert from "node:assert/strict";
import test from "node:test";

import { agentDisplayName, agentLabel, groupProjects, projectNameFromWorkdir } from "../src/projects.js";
import type { DesktopProject, DesktopSession, DesktopSnapshot } from "../src/types.js";

const project = (id: string): DesktopProject => ({
  id,
  name: id,
  workdir: `/${id}`,
  description: "",
  defaultAgentId: "codex-local",
});

const session = (id: string, projectId: string, updatedAt: number): DesktopSession => ({
  id,
  projectId,
  title: id,
  currentMessageId: `${id}-message`,
  agentId: "codex-local",
  version: 1,
  active: false,
  updatedAt,
});

test("keeps every Project visible and groups Sessions beneath their owner", () => {
  const snapshot: DesktopSnapshot = {
    protocolVersion: 1,
    revision: 1,
    projects: [project("project-a"), project("project-b")],
    agents: [],
    sessions: [
      session("a-older", "project-a", 1),
      session("b-only", "project-b", 2),
      session("a-newer", "project-a", 3),
    ],
    messages: [],
  };

  const groups = groupProjects(snapshot);

  assert.deepEqual(groups.map(({ project: item }) => item.id), ["project-a", "project-b"]);
  assert.deepEqual(groups[0]?.sessions.map(({ id }) => id), ["a-newer", "a-older"]);
  assert.deepEqual(groups[1]?.sessions.map(({ id }) => id), ["b-only"]);
});

test("derives the default Project name from the selected directory", () => {
  assert.equal(projectNameFromWorkdir("/Users/member/code/ait"), "ait");
  assert.equal(projectNameFromWorkdir("/Users/member/code/ait/"), "ait");
  assert.equal(projectNameFromWorkdir("C:\\Users\\member\\code\\ait"), "ait");
  assert.equal(projectNameFromWorkdir("C:\\Users\\member\\code\\ait\\"), "ait");
});

test("labels legacy echo Agents without masquerading as Codex", () => {
  const echo = { id: "codex-local", name: "Codex", model: "gpt-5.6-codex", mode: "echo", enabled: true };
  const codex = { id: "codex-app-server", name: "Codex", model: "gpt-5.6-codex", mode: "codex", enabled: true };

  assert.equal(agentDisplayName(echo), "Echo");
  assert.equal(agentLabel(echo), "Echo · echo");
  assert.equal(agentLabel(codex), "Codex · gpt-5.6-codex");
});
