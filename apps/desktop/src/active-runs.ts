import { sessionDisplayTitle } from "./session-titles.js";
import type { ActiveRunsCatalog, ActiveRunSummary } from "./types.js";

interface RunRecord {
  id: string;
  project_id: string;
  session_id: string | null;
  agent_id: string;
  config: { model: string };
  status: string;
  phase?: string | null;
  trigger: string;
  native_approvals?: Array<{ status: string }>;
  tool_approvals?: Array<{ status: string }>;
}

interface SessionRecord {
  id: string;
  project_id: string;
  name?: string;
  title?: string | null;
}

const activeStatuses = new Set(["queued", "running", "waiting_approval", "retry_wait", "settling", "cancelling"]);

/** Mirrors the nonterminal application LifecycleStatus vocabulary. */
export function isActiveRunStatus(status: string): boolean {
  return activeStatuses.has(status);
}

/** Main-process aggregation uses scoped reads and sends only activity summaries to the renderer. */
export async function loadActiveRuns(
  get: (path: string, kind: string) => Promise<unknown>,
): Promise<ActiveRunsCatalog> {
  const projects = await get("/v1/project/list", "projects") as Array<{ id: string; name: string }>;
  const catalog: ActiveRunsCatalog = { runs: [], unavailableProjects: [] };
  let nextProject = 0;
  // Bound fan-out even for workspaces with many registered Projects.
  await Promise.all(Array.from({ length: Math.min(4, projects.length) }, async () => {
    while (nextProject < projects.length) {
      const project = projects[nextProject++]!;
      const query = `?project_id=${encodeURIComponent(project.id)}`;
      try {
        const runs = await get(`/v1/run/list${query}`, "runs") as RunRecord[];
        assertProject(runs, project.id);
        const active = runs.filter((run) => isActiveRunStatus(run.status));
        let sessions: SessionRecord[] = [];
        if (active.some((run) => run.session_id)) {
          try {
            sessions = await get(`/v1/session/list${query}`, "sessions") as SessionRecord[];
            assertProject(sessions, project.id);
          } catch {
            sessions = [];
            catalog.unavailableProjects.push({
              projectId: project.id, projectName: project.name,
              message: "Session names could not be loaded. Runs are still shown.",
            });
          }
        }
        const sessionsById = new Map(sessions.map((session) => [session.id, session]));
        catalog.runs.push(...active.map((run): ActiveRunSummary => ({
          id: run.id,
          projectId: project.id,
          projectName: project.name,
          sessionId: run.session_id,
          sessionTitle: run.session_id
            ? sessionDisplayTitle(sessionsById.get(run.session_id) ?? { id: run.session_id })
            : null,
          agentId: run.agent_id,
          model: run.config.model,
          status: run.status,
          phase: run.phase ?? null,
          trigger: run.trigger,
          pendingApprovals: [...(run.native_approvals ?? []), ...(run.tool_approvals ?? [])].filter((approval) => approval.status === "pending").length,
        })));
      } catch {
        catalog.unavailableProjects.push({
          projectId: project.id, projectName: project.name,
          message: "Runs could not be loaded for this Project.",
        });
      }
    }
  }));
  catalog.runs.sort((left, right) => left.projectName.localeCompare(right.projectName)
    || (left.sessionTitle ?? "").localeCompare(right.sessionTitle ?? "")
    || left.id.localeCompare(right.id));
  catalog.unavailableProjects.sort((left, right) => left.projectName.localeCompare(right.projectName));
  return catalog;
}

function assertProject(records: Array<{ project_id: string }>, projectId: string): void {
  if (records.some((record) => record.project_id !== projectId)) {
    throw new Error("Ait daemon returned data for an unexpected Project.");
  }
}
