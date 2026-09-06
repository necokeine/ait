import assert from "node:assert/strict";
import test from "node:test";

import { agentDisplayName, agentLabel, groupProjects, projectNameFromWorkdir } from "../src/projects.js";
import type { DesktopProject, DesktopSession, DesktopSnapshot } from "../src/types.js";

const project = (id: string): DesktopProject => ({
  id,
  name: id,
  workdir: `/${id}`,
  description: "",
  baseCommit: "a".repeat(40),
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

test("labels named presets and Session-owned Agents", () => {
  const codex = { id: "codex-app-server", name: "Codex", model: "gpt-5.6-sol", mode: "codex", enabled: true };
  const custom = { ...codex, id: "session-agent", name: "", ownerSessionId: "session-a" };

  assert.equal(agentDisplayName(codex), "Codex");
  assert.equal(agentLabel(codex), "Codex · gpt-5.6-sol");
  assert.equal(agentDisplayName(custom), "Custom");
  assert.equal(agentLabel(custom), "Custom · gpt-5.6-sol");
});
