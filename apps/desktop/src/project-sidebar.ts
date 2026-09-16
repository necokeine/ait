import type { DesktopSession } from "./types.js";

/** Sidebar summaries stay separate from the selected Project's conversation data. */
export class ProjectSidebar {
  readonly expanded = new Set<string>();
  readonly sessions = new Map<string, DesktopSession[]>();
  readonly errors = new Map<string, string>();
  private readonly generations = new Map<string, number>();

  replace(projectId: string, sessions: DesktopSession[]): void {
    if (sessions.some((session) => session.projectId !== projectId)) {
      throw new Error("Session list belongs to another Project.");
    }
    this.generations.set(projectId, (this.generations.get(projectId) ?? 0) + 1);
    this.sessions.set(projectId, sessions);
    this.errors.delete(projectId);
  }

  async refresh(projectId: string, read: (id: string) => Promise<DesktopSession[]>): Promise<void> {
    const generation = (this.generations.get(projectId) ?? 0) + 1;
    this.generations.set(projectId, generation);
    try {
      const sessions = await read(projectId);
      if (this.generations.get(projectId) === generation) this.replace(projectId, sessions);
    } catch (error) {
      if (this.generations.get(projectId) === generation) {
        this.errors.set(projectId, error instanceof Error ? error.message : String(error));
      }
    }
  }
}
