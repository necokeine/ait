export interface SubmittedRun {
  id: string;
}

export interface SessionDerivationResult {
  run: SubmittedRun;
  selectedSessionId: string;
  reusedCurrentSession: boolean;
}

interface SessionDerivationRequest {
  currentSessionId: string;
  newSessionId: string;
  reuseCurrentSession: boolean;
  submitCurrent(): Promise<SubmittedRun>;
  submitFork(): Promise<SubmittedRun>;
}

/**
 * Tries an eligible leaf derivation against the current Session first. The
 * daemon's admission result is authoritative: a concurrent lock turns the
 * request into a normal fork without relying on a stale desktop snapshot.
 */
export async function submitSessionDerivation(
  request: SessionDerivationRequest,
): Promise<SessionDerivationResult> {
  if (request.reuseCurrentSession) {
    try {
      return {
        run: await request.submitCurrent(),
        selectedSessionId: request.currentSessionId,
        reusedCurrentSession: true,
      };
    } catch (error) {
      if (errorCode(error) !== "SESSION_BUSY") throw error;
    }
  }

  return {
    run: await request.submitFork(),
    selectedSessionId: request.newSessionId,
    reusedCurrentSession: false,
  };
}

function errorCode(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) return undefined;
  return typeof error.code === "string" ? error.code : undefined;
}
