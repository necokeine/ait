import type {
  AgentCatalog,
  DesktopProject,
  DesktopState,
  ProjectCatalog,
  ProjectView,
} from "./types.js";

export function emptyProjectView(): ProjectView {
  return {
    protocolVersion: 1,
    revision: 0,
    projectId: "",
    sessions: [],
    messages: [],
    runs: [],
    runProgress: [],
    recoveryNotices: [],
  };
}

export function resolveInitialProjectId(
  projects: DesktopProject[],
  rememberedProjectId: string | undefined,
): string | undefined {
  return projects.some((project) => project.id === rememberedProjectId)
    ? rememberedProjectId
    : projects[0]?.id;
}

export function composeDesktopState(
  projects: ProjectCatalog | undefined,
  agents: AgentCatalog | undefined,
  project: ProjectView | undefined,
): DesktopState {
  const current = project ?? emptyProjectView();
  return {
    projects: projects?.projects ?? [],
    agents: agents?.agents ?? [],
    providers: agents?.providers ?? [],
    sessions: current.sessions,
    messages: current.messages,
    runs: current.runs,
    runProgress: current.runProgress,
    recoveryNotices: current.recoveryNotices,
  };
}

export function projectReadPaths(projectId: string): readonly [string, string, string, string] {
  const encoded = encodeURIComponent(projectId);
  return [
    `/v1/session/list?project_id=${encoded}`,
    `/v1/message/list?project_id=${encoded}`,
    `/v1/run/list?project_id=${encoded}`,
    `/v1/run/progress?project_id=${encoded}`,
  ];
}

export function eventBelongsToProject(value: unknown, projectId: string | undefined): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return true;
  const eventProjectId = (value as Record<string, unknown>).project_id;
  return typeof eventProjectId !== "string" || eventProjectId === projectId;
}

export interface RefreshedDesktopSlices {
  projectAccepted: boolean;
  projects?: ProjectCatalog;
  agents?: AgentCatalog;
}

/** Refreshes only the selected Project unless an event-stream resync invalidates global catalogs. */
export async function refreshVisibleSlices(
  refreshProject: () => Promise<boolean>,
  catalogs: {
    projects(): Promise<ProjectCatalog>;
    agents(): Promise<AgentCatalog>;
  },
  includeCatalogs: boolean,
): Promise<RefreshedDesktopSlices> {
  if (!includeCatalogs) return { projectAccepted: await refreshProject() };
  const [projectAccepted, projects, agents] = await Promise.all([
    refreshProject(),
    catalogs.projects(),
    catalogs.agents(),
  ]);
  return { projectAccepted, projects, agents };
}
