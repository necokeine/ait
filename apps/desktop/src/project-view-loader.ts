export type ProjectViewReader<View> = (projectId?: string) => Promise<View>;

export interface ProjectViewMutation {
  readonly intentGeneration: number;
  readonly projectId: string;
}

/**
 * Owns the target of every bounded Project view read and mutation result.
 *
 * Selecting a Project changes the target synchronously, before the backend read
 * starts. A later read invalidates every earlier read response, including refreshes
 * that were already queued for the previous Project. Mutations use a separate
 * intent generation so same-target refreshes do not invalidate them, and they do
 * not change the target until their returned view commits successfully.
 */
export class ProjectViewLoader<View> {
  private intentGeneration = 0;
  private requestGeneration = 0;
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
    this.intentGeneration += 1;
    this.requestGeneration += 1;
    this.targetProjectId = projectId;
    this.loadedProjectId = projectId;
    this.loadedView = view;
  }

  beginMutation(projectId: string): ProjectViewMutation {
    return { intentGeneration: ++this.intentGeneration, projectId };
  }

  commitMutation(mutation: ProjectViewMutation, view: View): boolean {
    if (mutation.intentGeneration !== this.intentGeneration) return false;
    this.intentGeneration += 1;
    this.requestGeneration += 1;
    this.targetProjectId = mutation.projectId;
    this.loadedProjectId = mutation.projectId;
    this.loadedView = view;
    return true;
  }

  discardMutation(mutation: ProjectViewMutation): boolean {
    if (mutation.intentGeneration !== this.intentGeneration) return false;
    this.intentGeneration += 1;
    return true;
  }

  select(projectId: string): Promise<boolean> {
    this.intentGeneration += 1;
    this.targetProjectId = projectId;
    return this.load(projectId);
  }

  refresh(): Promise<boolean> {
    return this.load(this.targetProjectId);
  }

  private async load(projectId: string | undefined): Promise<boolean> {
    const generation = ++this.requestGeneration;
    const view = await this.read(projectId);
    if (generation !== this.requestGeneration || projectId !== this.targetProjectId) return false;
    this.loadedProjectId = projectId;
    this.loadedView = view;
    return true;
  }
}
