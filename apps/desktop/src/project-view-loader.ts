export type ProjectViewReader<View> = (projectId?: string) => Promise<View>;

/**
 * Owns the target of every bounded Project view read.
 *
 * Selecting a Project changes the target synchronously, before the backend read
 * starts. A later read invalidates every earlier response, including refreshes
 * that were already queued for the previous Project.
 */
export class ProjectViewLoader<View> {
  private generation = 0;
  private targetProjectId: string | undefined;
  private loadedProjectId: string | undefined;
  private loadedView: View | undefined;

  constructor(private readonly read: ProjectViewReader<View>) {}

  get selectedProjectId(): string | undefined {
    return this.targetProjectId;
  }

  get projectId(): string | undefined {
    return this.loadedProjectId;
  }

  get view(): View | undefined {
    return this.loadedView;
  }

  replace(projectId: string | undefined, view: View): void {
    this.generation += 1;
    this.targetProjectId = projectId;
    this.loadedProjectId = projectId;
    this.loadedView = view;
  }

  select(projectId: string): Promise<boolean> {
    this.targetProjectId = projectId;
    return this.load(projectId);
  }

  refresh(): Promise<boolean> {
    return this.load(this.targetProjectId);
  }

  private async load(projectId: string | undefined): Promise<boolean> {
    const generation = ++this.generation;
    const view = await this.read(projectId);
    if (generation !== this.generation || projectId !== this.targetProjectId) return false;
    this.loadedProjectId = projectId;
    this.loadedView = view;
    return true;
  }
}
