import type { AgentSummary, DesktopProject, DesktopSession, DesktopState } from "./types.js";

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

export function groupProjects(view: DesktopState): ProjectGroup[] {
  return view.projects.map((project) => ({
    project,
    sessions: view.sessions
      .filter((session) => session.projectId === project.id)
      .toSorted((left, right) => right.updatedAt - left.updatedAt),
  }));
}

export interface ProjectCreationInput {
  name: string;
  workdir?: string;
  agentId: string;
  repoUrl?: string;
}

export function projectCreationInput(name: string, workdir: string, agentId: string): ProjectCreationInput {
  const resolvedName = name.trim() || projectNameFromWorkdir(workdir);
  if (!resolvedName || !agentId) throw new Error("Enter a project name or choose a directory, and select a backend.");
  return { name: resolvedName, ...(workdir ? { workdir } : {}), agentId };
}

export async function registerDesktopProject(
  post: (path: string, kind: string, body: unknown) => Promise<unknown>,
  id: string,
  input: ProjectCreationInput,
): Promise<void> {
  await post("/v1/project/register", "project", {
    id, name: input.name, workdir: input.workdir, repo_url: input.repoUrl,
  });
  await post("/v1/project/set-default-agent", "project", {
    project_id: id, agent_id: input.agentId,
  });
}
