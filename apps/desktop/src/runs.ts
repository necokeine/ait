interface RunFailure {
  code?: string;
  message: string;
}

export interface RecoveryNotice {
  projectId: string;
  projectName: string;
  sessionId?: string;
  sessionTitle?: string;
  runId: string;
  code?: string;
  message: string;
}

interface StartupWorkspaceSnapshot {
  projects: Array<{ id: string; name: string }>;
  sessions: Array<{ id: string; project_id: string; name?: string; title?: string | null }>;
  runs: Array<{
    id: string; project_id: string; session_id?: string | null; status?: string;
    error?: { code?: string; message?: string } | null;
  }>;
}

/** Projects startup-recovered Runs into persistent, location-aware UI notices. */
export function startupRecoveryNotices(workspace: StartupWorkspaceSnapshot): RecoveryNotice[] {
  return workspace.runs.flatMap((run) => {
    if (run.status !== "interrupted") return [];
    const project = workspace.projects.find((candidate) => candidate.id === run.project_id);
    const session = run.session_id
      ? workspace.sessions.find((candidate) => candidate.id === run.session_id)
      : undefined;
    const code = typeof run.error?.code === "string" ? run.error.code : undefined;
    return [{
      projectId: run.project_id,
      projectName: project?.name ?? `Project ${run.project_id.slice(0, 8)}`,
      ...(session ? {
        sessionId: session.id,
        sessionTitle: session.name?.trim() || session.title?.trim() || `Session ${session.id.slice(0, 8)}`,
      } : {}),
      runId: run.id,
      ...(code ? { code } : {}),
      message: run.error?.message ?? "Recovery stopped because workspace effects need review.",
    }];
  });
}

export function runFailure(value: unknown): RunFailure | undefined {
  const run = record(value);
  if (!isTerminalFailure(run.status)) return undefined;
  const failure = record(run.error);
  const code = typeof failure.code === "string" ? failure.code : undefined;
  return {
    ...(code ? { code } : {}),
    message: typeof failure.message === "string"
      ? failure.message
      : `Codex run ended with status ${String(run.status)}.`,
  };
}

function isTerminalFailure(status: unknown): boolean {
  return status === "failed" || status === "cancelled" || status === "limit_exceeded"
    || status === "interrupted";
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
