interface RunFailure {
  code?: string;
  message: string;
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
  return status === "failed" || status === "cancelled" || status === "limit_exceeded";
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
