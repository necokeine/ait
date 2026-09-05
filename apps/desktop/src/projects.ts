import type { DesktopProject, DesktopSession, DesktopSnapshot } from "./types.js";

export interface ProjectGroup {
  project: DesktopProject;
  sessions: DesktopSession[];
}

export function projectNameFromWorkdir(workdir: string): string {
  const withoutTrailingSeparators = workdir.replace(/[\\/]+$/, "");
  return withoutTrailingSeparators.split(/[\\/]/).at(-1) ?? "";
}

export function groupProjects(snapshot: DesktopSnapshot): ProjectGroup[] {
  return snapshot.projects.map((project) => ({
    project,
    sessions: snapshot.sessions
      .filter((session) => session.projectId === project.id)
      .toSorted((left, right) => right.updatedAt - left.updatedAt),
  }));
}
