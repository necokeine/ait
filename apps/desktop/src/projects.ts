import { legacyBuiltInCodexAgentId } from "./agents.js";
import type { AgentSummary, DesktopProject, DesktopSession, DesktopSnapshot } from "./types.js";

export interface ProjectGroup {
  project: DesktopProject;
  sessions: DesktopSession[];
}

export function projectNameFromWorkdir(workdir: string): string {
  const withoutTrailingSeparators = workdir.replace(/[\\/]+$/, "");
  return withoutTrailingSeparators.split(/[\\/]/).at(-1) ?? "";
}

export function agentDisplayName(agent: AgentSummary): string {
  return agent.id === legacyBuiltInCodexAgentId && agent.mode === "echo" ? "Echo" : agent.name;
}

export function agentLabel(agent: AgentSummary): string {
  return agent.id === legacyBuiltInCodexAgentId && agent.mode === "echo"
    ? "Echo · echo"
    : `${agent.name} · ${agent.model}`;
}

export function groupProjects(snapshot: DesktopSnapshot): ProjectGroup[] {
  return snapshot.projects.map((project) => ({
    project,
    sessions: snapshot.sessions
      .filter((session) => session.projectId === project.id)
      .toSorted((left, right) => right.updatedAt - left.updatedAt),
  }));
}
