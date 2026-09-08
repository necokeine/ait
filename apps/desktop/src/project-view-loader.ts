export type ProjectViewReader<View> = (projectId?: string) => Promise<View>;

export interface ProjectViewMutation {
  readonly generation: number;
  readonly projectId: string;
}

/**
 * Owns the target of every bounded Project view read and mutation result.
 *
 * Selecting a Project changes the target synchronously, before the backend read
 * starts. A later read invalidates every earlier response, including refreshes
 * that were already queued for the previous Project. Mutations reserve the same
 * generation and can commit their returned view only while that intent is current.
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

  beginMutation(projectId: string): ProjectViewMutation {
    this.targetProjectId = projectId;
    return { generation: ++this.generation, projectId };
  }

  commitMutation(mutation: ProjectViewMutation, view: View): boolean {
    if (mutation.generation !== this.generation || mutation.projectId !== this.targetProjectId) return false;
    this.loadedProjectId = mutation.projectId;
    this.loadedView = view;
    return true;
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
