const markdownLink = /!?\[([^\]]*)\]\([^)]*\)/g;
const instructionTag = /<\/?(?:system|developer|user|assistant|instructions?|prompt|context)[^>]*>/gi;
const htmlTag = /<[^>]+>/g;
const lineMarker = /^\s*(?:#{1,6}\s+|>+\s*|[-+*]\s+|\d+[.)]\s+|\[[ xX]\]\s+)/gm;
const leadingCommand = /^\s*\/[a-z][\w-]*\s+/i;

export interface PendingSessionTitle {
  runId: string;
  sessionId: string;
  prompt: string;
}

interface TitleSessionState {
  id: string;
  name: string;
  titleGenerationStarted: boolean;
}

interface TitleRunState {
  id: string;
  status: string;
}

/** Tracks accepted first Runs independently of terminal-event ordering. */
export class PendingSessionTitles {
  private readonly requests = new Map<string, PendingSessionTitle>();

  register(runId: string, sessionId: string, prompt: string): void {
    this.requests.set(runId, { runId, sessionId, prompt });
  }

  takeReady(sessions: TitleSessionState[], runs: TitleRunState[]): PendingSessionTitle[] {
    const ready: PendingSessionTitle[] = [];
    for (const [runId, request] of this.requests) {
      const session = sessions.find((candidate) => candidate.id === request.sessionId);
      const run = runs.find((candidate) => candidate.id === runId);
      if (!session || session.titleGenerationStarted || session.name.trim()) {
        this.requests.delete(runId);
      } else if (run?.status === "completed") {
        this.requests.delete(runId);
        ready.push(request);
      } else if (run && ["failed", "cancelled", "limit_exceeded"].includes(run.status)) {
        this.requests.delete(runId);
      }
    }
    return ready;
  }

  get size(): number {
    return this.requests.size;
  }
}

/** Removes presentation/instruction syntax while retaining the user's actual words. */
export function sanitizeSessionPrompt(prompt: string, maxCharacters = 2_000): string {
  const plain = prompt
    .replace(markdownLink, "$1")
    .replace(instructionTag, " ")
    .replace(htmlTag, " ")
    .replace(/^\s*```[^\n]*$/gm, " ")
    .replace(lineMarker, "")
    .replace(leadingCommand, "")
    .replace(/[*_~`]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return Array.from(plain).slice(0, Math.max(0, maxCharacters)).join("").trim();
}

/** Immediate sidebar title shown while the dedicated title turn runs. */
export function temporarySessionTitle(prompt: string): string {
  return sanitizeSessionPrompt(prompt, 60);
}

/** Member names override generated titles; unnamed Sessions retain the legacy fallback. */
export function sessionDisplayTitle(session: { id: string; name?: string; title?: string | null }): string {
  const name = session.name?.trim();
  if (name) return name;
  const generated = session.title?.trim();
  return generated || `Session ${session.id.slice(0, 8)}`;
}
