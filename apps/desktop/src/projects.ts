import type { AgentSummary, DesktopProject, DesktopSession, DesktopView } from "./types.js";

export interface ProjectGroup {
  project: DesktopProject;
  sessions: DesktopSession[];
}

export function projectNameFromWorkdir(workdir: string): string {
  const withoutTrailingSeparators = workdir.replace(/[\\/]+$/, "");
  return withoutTrailingSeparators.split(/[\\/]/).at(-1) ?? "";
}

export function agentDisplayName(agent: AgentSummary): string {
  return agent.ownerSessionId ? "Custom" : agent.name;
}

export function agentLabel(agent: AgentSummary): string {
  return `${agentDisplayName(agent)} · ${agent.model}`;
}

export function availableProjectDefaultAgentId(
  project: DesktopProject,
  agents: AgentSummary[],
): string | undefined {
  if (!project.defaultAgentId) return undefined;
  const agent = agents.find((candidate) => candidate.id === project.defaultAgentId);
  return agent?.enabled && !agent.ownerSessionId ? agent.id : undefined;
}

export function groupProjects(view: DesktopView): ProjectGroup[] {
  return view.projects.map((project) => ({
    project,
    sessions: view.sessions
      .filter((session) => session.projectId === project.id)
      .toSorted((left, right) => right.updatedAt - left.updatedAt),
  }));
}
