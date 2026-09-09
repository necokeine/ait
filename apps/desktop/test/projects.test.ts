import assert from "node:assert/strict";
import test from "node:test";

import {
  agentDisplayName,
  agentLabel,
  availableProjectDefaultAgentId,
  groupProjects,
  projectNameFromWorkdir,
} from "../src/projects.js";
import type { AgentSummary, DesktopProject, DesktopSession, DesktopState } from "../src/types.js";

const codex: AgentSummary = {
  id: "codex-local",
  name: "Codex",
  model: "gpt-5.6-sol",
  mode: "codex",
  enabled: true,
  config: { provider_id: "builtin-codex", model: "gpt-5.6-sol", reasoning_effort: null },
  ownerSessionId: null,
};

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
  name: id,
  title: id,
  description: "",
  titleGenerationStarted: false,
  currentMessageId: `${id}-message`,
  agentId: "codex-local",
  version: 1,
  active: false,
  activeRunId: null,
  updatedAt,
});

test("keeps every Project visible and groups Sessions beneath their owner", () => {
  const view: DesktopState = {
    projects: [project("project-a"), project("project-b")],
    agents: [],
    providers: [],
    sessions: [
      session("a-older", "project-a", 1),
      session("b-only", "project-b", 2),
      session("a-newer", "project-a", 3),
    ],
    messages: [],
    runs: [],
    runProgress: [],
  };

  const groups = groupProjects(view);

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
  const custom = { ...codex, id: "session-agent", name: "", ownerSessionId: "session-a" };

  assert.equal(agentDisplayName(codex), "Codex");
  assert.equal(agentLabel(codex), "Codex · gpt-5.6-sol");
  assert.equal(agentDisplayName(custom), "Custom");
  assert.equal(agentLabel(custom), "Custom · gpt-5.6-sol");
});

test("uses only an enabled named Project default for a new Session", () => {
  const configured = project("project-a");
  const alternate = { ...codex, id: "alternate" };

  assert.equal(availableProjectDefaultAgentId(configured, [alternate, codex]), codex.id);
  assert.equal(availableProjectDefaultAgentId({ ...configured, defaultAgentId: null }, [codex]), undefined);
  assert.equal(availableProjectDefaultAgentId(configured, [{ ...codex, enabled: false }]), undefined);
  assert.equal(availableProjectDefaultAgentId(configured, [{ ...codex, ownerSessionId: "session-a" }]), undefined);
});
